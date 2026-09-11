use std::time::Duration;

use monitor_common::{AgentPingTargetV2, PingReport, ProbeKind};

#[cfg(any(target_os = "linux", test))]
use std::{
    net::{IpAddr, SocketAddr, ToSocketAddrs},
    time::Instant,
};

#[cfg(target_os = "linux")]
use std::{io, mem::MaybeUninit, os::fd::AsRawFd};

#[cfg(target_os = "linux")]
use socket2::{Domain, Protocol, SockAddr, Socket, Type};

pub const ROUND_TIMEOUT: Duration = Duration::from_secs(1);
#[cfg(any(target_os = "linux", test))]
const PACKET_LEN: usize = 16;
#[cfg(any(target_os = "linux", test))]
const PAYLOAD_MAGIC: [u8; 4] = *b"MNTR";

pub struct PingEngine {
    #[cfg(target_os = "linux")]
    identifier: u16,
    sequence: u16,
    #[cfg(target_os = "linux")]
    ipv4: SocketSlot,
    #[cfg(target_os = "linux")]
    ipv6: SocketSlot,
}

impl PingEngine {
    pub fn new() -> Self {
        #[cfg(target_os = "linux")]
        let identifier = (std::process::id() as u16) ^ 0x4d4e;
        Self {
            #[cfg(target_os = "linux")]
            identifier,
            sequence: 0,
            #[cfg(target_os = "linux")]
            ipv4: SocketSlot::Unopened,
            #[cfg(target_os = "linux")]
            ipv6: SocketSlot::Unopened,
        }
    }

    pub fn ping_round(&mut self, targets: &[AgentPingTargetV2]) -> Vec<PingReport> {
        #[cfg(target_os = "linux")]
        {
            self.ping_round_linux(targets)
        }

        #[cfg(not(target_os = "linux"))]
        {
            for _ in targets {
                self.next_sequence();
            }
            unavailable_results(targets)
        }
    }

    fn next_sequence(&mut self) -> u16 {
        self.sequence = self.sequence.wrapping_add(1);
        self.sequence
    }
}

impl Default for PingEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "linux")]
impl PingEngine {
    fn ping_round_linux(&mut self, targets: &[AgentPingTargetV2]) -> Vec<PingReport> {
        let deadline = Instant::now() + ROUND_TIMEOUT;
        let mut probes = Vec::with_capacity(targets.len());
        let mut tcp_probes = Vec::new();

        for (index, target) in targets.iter().enumerate() {
            let sequence = self.next_sequence();
            let Some((family, address)) = resolve_target(target) else {
                continue;
            };
            if Instant::now() >= deadline {
                break;
            }
            if target.probe_kind == ProbeKind::Tcp {
                // Every TCP connect is started non-blocking before the wait, so a
                // round of blackholed targets costs one ROUND_TIMEOUT, not N of them.
                if let Some(probe) = start_tcp_connect(index, family, address) {
                    tcp_probes.push(probe);
                }
                continue;
            }
            let packet = build_echo_request(family, self.identifier, sequence);
            let socket = match family {
                AddressFamily::V4 => self.ipv4.socket(AddressFamily::V4),
                AddressFamily::V6 => self.ipv6.socket(AddressFamily::V6),
            };
            let Some(socket) = socket else {
                continue;
            };
            let sent_at = Instant::now();
            if socket
                .send_to(&packet, &SockAddr::from(address))
                .is_ok_and(|sent| sent == packet.len())
            {
                probes.push(OutstandingProbe {
                    index,
                    family,
                    address: address.ip(),
                    identifier: self.identifier,
                    sequence,
                    sent_at,
                    latency_ms: None,
                });
            }
        }

        wait_for_probe_results(
            self.ipv4.ready(),
            self.ipv6.ready(),
            &mut probes,
            &mut tcp_probes,
            deadline,
        );
        let tcp_results: Vec<SettledProbe> = tcp_probes.iter().map(SettledProbe::from).collect();
        ordered_results(targets, &probes, &tcp_results)
    }
}

#[cfg(target_os = "linux")]
enum SocketSlot {
    Unopened,
    Ready(Socket),
    Unavailable,
}

#[cfg(target_os = "linux")]
impl SocketSlot {
    fn socket(&mut self, family: AddressFamily) -> Option<&Socket> {
        if matches!(self, Self::Unopened) {
            match open_socket(family) {
                Ok(socket) => *self = Self::Ready(socket),
                Err(error) => {
                    if matches!(error.raw_os_error(), Some(libc::EPERM) | Some(libc::EACCES)) {
                        eprintln!("monitor-agent: ICMP unavailable; CAP_NET_RAW may be required");
                    } else {
                        eprintln!("monitor-agent: ICMP socket unavailable: {error}");
                    }
                    *self = Self::Unavailable;
                }
            }
        }
        self.ready()
    }

    fn ready(&self) -> Option<&Socket> {
        match self {
            Self::Ready(socket) => Some(socket),
            Self::Unopened | Self::Unavailable => None,
        }
    }
}

#[cfg(target_os = "linux")]
fn open_socket(family: AddressFamily) -> io::Result<Socket> {
    let (domain, protocol) = match family {
        AddressFamily::V4 => (Domain::IPV4, Protocol::ICMPV4),
        AddressFamily::V6 => (Domain::IPV6, Protocol::ICMPV6),
    };
    let socket = Socket::new(domain, Type::RAW, Some(protocol))?;
    socket.set_nonblocking(true)?;
    // Linux calculates and inserts the ICMPv6 checksum for an
    // IPPROTO_ICMPV6 raw socket; IPV6_CHECKSUM must not be set here.
    Ok(socket)
}

/// A TCP probe that still owns its socket is mid-connect; settling one drops the
/// socket, so a probe never outlives the connection handshake it measures.
#[cfg(target_os = "linux")]
struct TcpProbe {
    index: usize,
    socket: Option<Socket>,
    started_at: Instant,
    latency_ms: Option<f64>,
}

#[cfg(target_os = "linux")]
fn start_tcp_connect(index: usize, family: AddressFamily, address: SocketAddr) -> Option<TcpProbe> {
    let domain = match family {
        AddressFamily::V4 => Domain::IPV4,
        AddressFamily::V6 => Domain::IPV6,
    };
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP)).ok()?;
    socket.set_nonblocking(true).ok()?;
    let started_at = Instant::now();
    match socket.connect(&SockAddr::from(address)) {
        Ok(()) => Some(TcpProbe {
            index,
            socket: None,
            started_at,
            latency_ms: finite_latency(started_at, Instant::now()),
        }),
        Err(error)
            if error.kind() == io::ErrorKind::WouldBlock
                || error.raw_os_error() == Some(libc::EINPROGRESS) =>
        {
            Some(TcpProbe {
                index,
                socket: Some(socket),
                started_at,
                latency_ms: None,
            })
        }
        // An immediate refusal or unreachable route is a failed probe, not one in flight.
        Err(_) => None,
    }
}

#[cfg(target_os = "linux")]
fn settle_tcp_probe(probe: &mut TcpProbe, revents: libc::c_short, observed_at: Instant) {
    let Some(socket) = probe.socket.as_ref() else {
        return;
    };
    let connected = revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) == 0
        && revents & libc::POLLOUT != 0
        && socket.take_error().is_ok_and(|error| error.is_none());
    if connected {
        probe.latency_ms = finite_latency(probe.started_at, observed_at);
    }
    // Connection establishment is the whole measurement: close without writing,
    // reading, or negotiating anything on the connection.
    probe.socket = None;
}

#[cfg(target_os = "linux")]
impl From<&TcpProbe> for SettledProbe {
    fn from(probe: &TcpProbe) -> Self {
        Self {
            index: probe.index,
            latency_ms: probe.latency_ms,
        }
    }
}

/// A finished probe reduced to what result assembly needs, so TCP and ICMP
/// outcomes merge through one ordering path.
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, PartialEq)]
struct SettledProbe {
    index: usize,
    latency_ms: Option<f64>,
}

#[cfg(any(target_os = "linux", test))]
fn finite_latency(started_at: Instant, observed_at: Instant) -> Option<f64> {
    let latency = observed_at
        .saturating_duration_since(started_at)
        .as_secs_f64()
        * 1_000.0;
    (latency.is_finite() && latency >= 0.0).then_some(latency)
}

#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AddressFamily {
    V4,
    V6,
}

#[cfg(any(target_os = "linux", test))]
fn resolve_target(target: &AgentPingTargetV2) -> Option<(AddressFamily, SocketAddr)> {
    let family = match target.ip_family {
        4 => AddressFamily::V4,
        6 => AddressFamily::V6,
        _ => return None,
    };
    // ip_family selects which resolved address family may be probed, for literals
    // and hostnames alike, so a v4 target never reaches a v6 address.
    let port = match target.probe_kind {
        ProbeKind::Icmp => 0,
        ProbeKind::Tcp => u16::try_from(target.port?).ok()?,
    };
    (target.host.as_str(), port)
        .to_socket_addrs()
        .ok()?
        .find(|address| {
            matches!(
                (family, address),
                (AddressFamily::V4, SocketAddr::V4(_)) | (AddressFamily::V6, SocketAddr::V6(_))
            )
        })
        .map(|address| (family, address))
}

#[cfg(any(target_os = "linux", test))]
fn build_echo_request(family: AddressFamily, identifier: u16, sequence: u16) -> [u8; PACKET_LEN] {
    let mut packet = [0_u8; PACKET_LEN];
    packet[0] = match family {
        AddressFamily::V4 => 8,
        AddressFamily::V6 => 128,
    };
    packet[4..6].copy_from_slice(&identifier.to_be_bytes());
    packet[6..8].copy_from_slice(&sequence.to_be_bytes());
    packet[8..12].copy_from_slice(&PAYLOAD_MAGIC);
    packet[12..14].copy_from_slice(&identifier.to_be_bytes());
    packet[14..16].copy_from_slice(&sequence.to_be_bytes());
    if family == AddressFamily::V4 {
        let checksum = icmp_checksum(&packet);
        packet[2..4].copy_from_slice(&checksum.to_be_bytes());
    }
    packet
}

#[cfg(any(target_os = "linux", test))]
fn icmp_checksum(bytes: &[u8]) -> u16 {
    let mut sum = 0_u32;
    let mut chunks = bytes.chunks_exact(2);
    for pair in &mut chunks {
        sum += u32::from(u16::from_be_bytes([pair[0], pair[1]]));
    }
    if let Some(byte) = chunks.remainder().first() {
        sum += u32::from(*byte) << 8;
    }
    while sum > u32::from(u16::MAX) {
        sum = (sum & u32::from(u16::MAX)) + (sum >> 16);
    }
    !(sum as u16)
}

#[cfg(any(target_os = "linux", test))]
#[derive(Debug)]
struct OutstandingProbe {
    index: usize,
    family: AddressFamily,
    address: IpAddr,
    identifier: u16,
    sequence: u16,
    sent_at: Instant,
    latency_ms: Option<f64>,
}

#[cfg(target_os = "linux")]
enum PollSlot {
    Icmp(AddressFamily),
    Tcp(usize),
}

/// One poll loop, one deadline: ICMP replies and TCP handshakes are awaited
/// together so the round length never scales with the number of targets.
#[cfg(target_os = "linux")]
fn wait_for_probe_results(
    ipv4: Option<&Socket>,
    ipv6: Option<&Socket>,
    probes: &mut [OutstandingProbe],
    tcp_probes: &mut [TcpProbe],
    deadline: Instant,
) {
    loop {
        let icmp_pending = probes.iter().any(|probe| probe.latency_ms.is_none());
        let tcp_pending = tcp_probes.iter().any(|probe| probe.socket.is_some());
        if !icmp_pending && !tcp_pending {
            break;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let mut poll_fds = Vec::with_capacity(2 + tcp_probes.len());
        let mut slots = Vec::with_capacity(2 + tcp_probes.len());
        for family in [AddressFamily::V4, AddressFamily::V6] {
            let socket = match family {
                AddressFamily::V4 => ipv4,
                AddressFamily::V6 => ipv6,
            };
            let outstanding = probes
                .iter()
                .any(|probe| probe.family == family && probe.latency_ms.is_none());
            if outstanding && let Some(socket) = socket {
                poll_fds.push(libc::pollfd {
                    fd: socket.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                });
                slots.push(PollSlot::Icmp(family));
            }
        }
        for (position, probe) in tcp_probes.iter().enumerate() {
            if let Some(socket) = probe.socket.as_ref() {
                poll_fds.push(libc::pollfd {
                    fd: socket.as_raw_fd(),
                    events: libc::POLLOUT,
                    revents: 0,
                });
                slots.push(PollSlot::Tcp(position));
            }
        }
        if poll_fds.is_empty() {
            break;
        }
        let timeout_ms = remaining.as_millis().clamp(1, i32::MAX as u128) as i32;
        // SAFETY: poll_fds owns initialized pollfd entries for valid live sockets.
        let ready = unsafe {
            libc::poll(
                poll_fds.as_mut_ptr(),
                poll_fds.len() as libc::nfds_t,
                timeout_ms,
            )
        };
        if ready == 0 {
            break;
        }
        if ready < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        for (poll_fd, slot) in poll_fds.iter().zip(slots.iter()) {
            if poll_fd.revents == 0 {
                continue;
            }
            match slot {
                PollSlot::Icmp(family) => {
                    if poll_fd.revents & libc::POLLIN == 0 {
                        continue;
                    }
                    let socket = match family {
                        AddressFamily::V4 => ipv4,
                        AddressFamily::V6 => ipv6,
                    };
                    if let Some(socket) = socket {
                        receive_available(socket, *family, probes);
                    }
                }
                PollSlot::Tcp(position) => {
                    settle_tcp_probe(&mut tcp_probes[*position], poll_fd.revents, Instant::now());
                }
            }
        }
    }
    // The round is over: abandon every handshake that did not complete in time.
    for probe in tcp_probes.iter_mut() {
        probe.socket = None;
    }
}

#[cfg(target_os = "linux")]
fn receive_available(socket: &Socket, family: AddressFamily, probes: &mut [OutstandingProbe]) {
    loop {
        let mut buffer = [MaybeUninit::<u8>::uninit(); 2_048];
        match socket.recv_from(&mut buffer) {
            Ok((length, source)) => {
                let Some(source) = source.as_socket() else {
                    continue;
                };
                // SAFETY: recv_from initialized exactly the returned prefix.
                let packet =
                    unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), length) };
                record_reply(probes, family, source.ip(), packet, Instant::now());
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

#[cfg(any(target_os = "linux", test))]
fn record_reply(
    probes: &mut [OutstandingProbe],
    family: AddressFamily,
    source: IpAddr,
    packet: &[u8],
    received_at: Instant,
) {
    let Some((identifier, sequence)) = parse_echo_reply(family, packet) else {
        return;
    };
    if let Some(probe) = probes.iter_mut().find(|probe| {
        probe.latency_ms.is_none()
            && probe.family == family
            && probe.address == source
            && probe.identifier == identifier
            && probe.sequence == sequence
    }) {
        probe.latency_ms = finite_latency(probe.sent_at, received_at);
    }
}

#[cfg(any(target_os = "linux", test))]
fn parse_echo_reply(family: AddressFamily, packet: &[u8]) -> Option<(u16, u16)> {
    let icmp = match family {
        AddressFamily::V4 => {
            let first = *packet.first()?;
            if first >> 4 != 4 {
                return None;
            }
            let header_length = usize::from(first & 0x0f).checked_mul(4)?;
            let total_length = usize::from(u16::from_be_bytes([*packet.get(2)?, *packet.get(3)?]));
            if header_length < 20
                || total_length < header_length + PACKET_LEN
                || total_length > packet.len()
            {
                return None;
            }
            &packet[header_length..total_length]
        }
        AddressFamily::V6 => packet,
    };
    let expected_type = match family {
        AddressFamily::V4 => 0,
        AddressFamily::V6 => 129,
    };
    if icmp.len() < PACKET_LEN || icmp[0] != expected_type || icmp[1] != 0 {
        return None;
    }
    if family == AddressFamily::V4 && icmp_checksum(icmp) != 0 {
        return None;
    }
    if icmp[8..12] != PAYLOAD_MAGIC {
        return None;
    }
    let identifier = u16::from_be_bytes([icmp[4], icmp[5]]);
    let sequence = u16::from_be_bytes([icmp[6], icmp[7]]);
    if icmp[12..14] != identifier.to_be_bytes() || icmp[14..16] != sequence.to_be_bytes() {
        return None;
    }
    Some((identifier, sequence))
}

#[cfg(any(target_os = "linux", test))]
fn ordered_results(
    targets: &[AgentPingTargetV2],
    probes: &[OutstandingProbe],
    tcp_probes: &[SettledProbe],
) -> Vec<PingReport> {
    targets
        .iter()
        .enumerate()
        .map(|(index, target)| {
            let latency_ms = probes
                .iter()
                .find(|probe| probe.index == index)
                .and_then(|probe| probe.latency_ms)
                .or_else(|| {
                    tcp_probes
                        .iter()
                        .find(|probe| probe.index == index)
                        .and_then(|probe| probe.latency_ms)
                });
            PingReport {
                target_id: target.id,
                success: latency_ms.is_some(),
                latency_ms,
            }
        })
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn unavailable_results(targets: &[AgentPingTargetV2]) -> Vec<PingReport> {
    targets
        .iter()
        .map(|target| PingReport {
            target_id: target.id,
            success: false,
            latency_ms: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(id: i64, host: &str, family: i64) -> AgentPingTargetV2 {
        AgentPingTargetV2 {
            id,
            name: format!("target-{id}"),
            host: host.to_owned(),
            ip_family: family,
            probe_kind: ProbeKind::Icmp,
            port: None,
        }
    }

    fn tcp_target(id: i64, host: &str, family: i64, port: Option<i64>) -> AgentPingTargetV2 {
        AgentPingTargetV2 {
            probe_kind: ProbeKind::Tcp,
            port,
            ..target(id, host, family)
        }
    }

    fn ipv4_reply(identifier: u16, sequence: u16, ihl_words: u8) -> Vec<u8> {
        let header_length = usize::from(ihl_words) * 4;
        let mut packet = vec![0_u8; header_length + PACKET_LEN];
        packet[0] = 0x40 | ihl_words;
        let packet_length = packet.len() as u16;
        packet[2..4].copy_from_slice(&packet_length.to_be_bytes());
        let icmp = &mut packet[header_length..];
        icmp[0] = 0;
        icmp[4..6].copy_from_slice(&identifier.to_be_bytes());
        icmp[6..8].copy_from_slice(&sequence.to_be_bytes());
        icmp[8..12].copy_from_slice(&PAYLOAD_MAGIC);
        icmp[12..14].copy_from_slice(&identifier.to_be_bytes());
        icmp[14..16].copy_from_slice(&sequence.to_be_bytes());
        let checksum = icmp_checksum(icmp);
        icmp[2..4].copy_from_slice(&checksum.to_be_bytes());
        packet
    }

    fn ipv6_reply(identifier: u16, sequence: u16) -> [u8; PACKET_LEN] {
        let mut packet = build_echo_request(AddressFamily::V6, identifier, sequence);
        packet[0] = 129;
        packet
    }

    #[test]
    fn checksum_handles_even_odd_and_known_fixture() {
        assert_eq!(icmp_checksum(&[0x08, 0, 0, 0, 0x12, 0x34, 0, 1]), 0xe5ca);
        assert_eq!(icmp_checksum(&[0x01, 0x02, 0x03]), 0xfbfd);
        let request = build_echo_request(AddressFamily::V4, 7, 9);
        assert_eq!(icmp_checksum(&request), 0);
    }

    #[test]
    fn echo_requests_use_frozen_types_and_identifiers() {
        let v4 = build_echo_request(AddressFamily::V4, 0x1234, 0x5678);
        assert_eq!((v4[0], v4[1]), (8, 0));
        assert_eq!(&v4[4..8], &[0x12, 0x34, 0x56, 0x78]);
        let v6 = build_echo_request(AddressFamily::V6, 0x1234, 0x5678);
        assert_eq!((v6[0], v6[1]), (128, 0));
        assert_eq!(&v6[2..4], &[0, 0]);
    }

    #[test]
    fn parses_ipv4_variable_ihl_and_ipv6_reply() {
        assert_eq!(
            parse_echo_reply(AddressFamily::V4, &ipv4_reply(4, 5, 5)),
            Some((4, 5))
        );
        assert_eq!(
            parse_echo_reply(AddressFamily::V4, &ipv4_reply(4, 5, 7)),
            Some((4, 5))
        );
        assert_eq!(
            parse_echo_reply(AddressFamily::V6, &ipv6_reply(6, 7)),
            Some((6, 7))
        );
    }

    #[test]
    fn malformed_and_non_reply_packets_are_ignored() {
        assert_eq!(parse_echo_reply(AddressFamily::V4, &[]), None);
        let mut truncated = ipv4_reply(1, 2, 5);
        truncated.truncate(12);
        assert_eq!(parse_echo_reply(AddressFamily::V4, &truncated), None);
        let mut request = ipv6_reply(1, 2);
        request[0] = 128;
        assert_eq!(parse_echo_reply(AddressFamily::V6, &request), None);
        let mut corrupt = ipv4_reply(1, 2, 5);
        *corrupt.last_mut().expect("packet") ^= 1;
        assert_eq!(parse_echo_reply(AddressFamily::V4, &corrupt), None);
    }

    #[test]
    fn reply_match_requires_family_source_identifier_and_sequence() {
        let now = Instant::now();
        let source: IpAddr = "192.0.2.1".parse().unwrap();
        let mut probes = vec![OutstandingProbe {
            index: 0,
            family: AddressFamily::V4,
            address: source,
            identifier: 10,
            sequence: 20,
            sent_at: now,
            latency_ms: None,
        }];
        for (family, address, identifier, sequence) in [
            (AddressFamily::V6, source, 10, 20),
            (AddressFamily::V4, "192.0.2.2".parse().unwrap(), 10, 20),
            (AddressFamily::V4, source, 11, 20),
            (AddressFamily::V4, source, 10, 21),
        ] {
            let packet = if family == AddressFamily::V4 {
                ipv4_reply(identifier, sequence, 5)
            } else {
                ipv6_reply(identifier, sequence).to_vec()
            };
            record_reply(&mut probes, family, address, &packet, now);
            assert_eq!(probes[0].latency_ms, None);
        }
        record_reply(
            &mut probes,
            AddressFamily::V4,
            source,
            &ipv4_reply(10, 20, 5),
            now + Duration::from_millis(3),
        );
        assert_eq!(probes[0].latency_ms, Some(3.0));
    }

    #[test]
    fn result_assembly_preserves_config_order_and_nulls_timeouts() {
        let targets = vec![
            target(1, "192.0.2.1", 4),
            target(2, "192.0.2.2", 4),
            target(3, "2001:db8::1", 6),
        ];
        let now = Instant::now();
        let probes = vec![
            OutstandingProbe {
                index: 2,
                family: AddressFamily::V6,
                address: "2001:db8::1".parse().unwrap(),
                identifier: 1,
                sequence: 3,
                sent_at: now,
                latency_ms: Some(8.0),
            },
            OutstandingProbe {
                index: 0,
                family: AddressFamily::V4,
                address: "192.0.2.1".parse().unwrap(),
                identifier: 1,
                sequence: 1,
                sent_at: now,
                latency_ms: Some(4.0),
            },
        ];
        let results = ordered_results(&targets, &probes, &[]);
        assert_eq!(
            results
                .iter()
                .map(|result| result.target_id)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(results[0].latency_ms, Some(4.0));
        assert_eq!((results[1].success, results[1].latency_ms), (false, None));
        assert_eq!(results[2].latency_ms, Some(8.0));
    }

    #[test]
    fn result_assembly_merges_icmp_and_tcp_outcomes_by_target_index() {
        let targets = vec![
            tcp_target(1, "127.0.0.1", 4, Some(443)),
            target(2, "192.0.2.2", 4),
            tcp_target(3, "127.0.0.1", 4, Some(22)),
        ];
        let probes = vec![OutstandingProbe {
            index: 1,
            family: AddressFamily::V4,
            address: "192.0.2.2".parse().unwrap(),
            identifier: 1,
            sequence: 1,
            sent_at: Instant::now(),
            latency_ms: Some(5.0),
        }];
        let tcp = vec![
            SettledProbe {
                index: 2,
                latency_ms: None,
            },
            SettledProbe {
                index: 0,
                latency_ms: Some(1.5),
            },
        ];
        let results = ordered_results(&targets, &probes, &tcp);
        assert_eq!(
            results
                .iter()
                .map(|result| (result.target_id, result.success, result.latency_ms))
                .collect::<Vec<_>>(),
            vec![(1, true, Some(1.5)), (2, true, Some(5.0)), (3, false, None),]
        );
    }

    #[test]
    fn resolver_filters_by_configured_family() {
        assert!(matches!(
            resolve_target(&target(1, "127.0.0.1", 4)),
            Some((AddressFamily::V4, SocketAddr::V4(_)))
        ));
        assert_eq!(resolve_target(&target(1, "127.0.0.1", 6)), None);
        assert!(matches!(
            resolve_target(&target(1, "::1", 6)),
            Some((AddressFamily::V6, SocketAddr::V6(_)))
        ));
    }

    #[test]
    fn resolver_applies_configured_port_only_to_tcp_targets() {
        let Some((_, address)) = resolve_target(&tcp_target(1, "127.0.0.1", 4, Some(8443))) else {
            panic!("tcp target must resolve");
        };
        assert_eq!(address.port(), 8443);
        let Some((_, address)) = resolve_target(&target(1, "127.0.0.1", 4)) else {
            panic!("icmp target must resolve");
        };
        assert_eq!(address.port(), 0);
        // Documented IPv6 literals resolve without a lookup, so a bracketed
        // admin entry survives the Server's parser as host plus port.
        let Some((family, address)) = resolve_target(&tcp_target(1, "2001:db8::1", 6, Some(443)))
        else {
            panic!("ipv6 tcp target must resolve");
        };
        assert_eq!(family, AddressFamily::V6);
        assert_eq!(address.to_string(), "[2001:db8::1]:443");
    }

    #[test]
    fn tcp_target_without_a_port_never_reaches_probe_execution() {
        assert_eq!(resolve_target(&tcp_target(1, "127.0.0.1", 4, None)), None);
        assert_eq!(
            resolve_target(&tcp_target(1, "127.0.0.1", 4, Some(70_000))),
            None
        );
        let results = PingEngine::new().ping_round(&[tcp_target(1, "127.0.0.1", 4, None)]);
        assert_eq!(
            (results.len(), results[0].success, results[0].latency_ms),
            (1, false, None)
        );
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn unavailable_raw_socket_yields_failures_without_stopping_resource_path() {
        let targets = vec![target(1, "127.0.0.1", 4), target(2, "::1", 6)];
        let results = PingEngine::new().ping_round(&targets);
        assert!(
            results
                .iter()
                .all(|result| !result.success && result.latency_ms.is_none())
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires Linux loopback and CAP_NET_RAW"]
    fn linux_raw_ipv4_loopback_smoke() {
        let result = PingEngine::new().ping_round(&[target(1, "127.0.0.1", 4)]);
        assert_eq!(result.len(), 1);
        assert!(result[0].success);
        assert!(result[0].latency_ms.is_some_and(|latency| latency >= 0.0));
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires Linux IPv6 loopback and CAP_NET_RAW"]
    fn linux_raw_ipv6_loopback_smoke() {
        let result = PingEngine::new().ping_round(&[target(1, "::1", 6)]);
        assert_eq!(result.len(), 1);
        assert!(result[0].success);
        assert!(result[0].latency_ms.is_some_and(|latency| latency >= 0.0));
    }

    #[cfg(target_os = "linux")]
    fn loopback(port: u16) -> SockAddr {
        SockAddr::from(SocketAddr::from(([127, 0, 0, 1], port)))
    }

    #[cfg(target_os = "linux")]
    fn listening_socket(backlog: i32) -> (Socket, u16) {
        let listener = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP)).unwrap();
        listener.bind(&loopback(0)).unwrap();
        listener.listen(backlog).unwrap();
        let port = listener
            .local_addr()
            .unwrap()
            .as_socket_ipv4()
            .expect("loopback address")
            .port();
        (listener, port)
    }

    #[cfg(target_os = "linux")]
    fn connect_completes(port: u16, wait: Duration) -> Option<Socket> {
        let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP)).unwrap();
        socket.set_nonblocking(true).unwrap();
        let pending = match socket.connect(&loopback(port)) {
            Ok(()) => return Some(socket),
            Err(error) => {
                error.kind() == io::ErrorKind::WouldBlock
                    || error.raw_os_error() == Some(libc::EINPROGRESS)
            }
        };
        if !pending {
            return None;
        }
        let mut poll_fd = libc::pollfd {
            fd: socket.as_raw_fd(),
            events: libc::POLLOUT,
            revents: 0,
        };
        // SAFETY: one initialized pollfd for a live socket owned by this frame.
        let ready = unsafe { libc::poll(&mut poll_fd, 1, wait.as_millis() as i32) };
        let connected = ready == 1
            && poll_fd.revents & libc::POLLOUT != 0
            && socket.take_error().is_ok_and(|error| error.is_none());
        connected.then_some(socket)
    }

    /// A listener whose accept queue is full makes the kernel drop further SYNs,
    /// so connects hang with no network involved and no timing luck.
    #[cfg(target_os = "linux")]
    fn blackholed_port() -> (Socket, Vec<Socket>, u16) {
        let (listener, port) = listening_socket(0);
        let mut held = Vec::new();
        for _ in 0..16 {
            match connect_completes(port, Duration::from_millis(150)) {
                Some(socket) => held.push(socket),
                None => return (listener, held, port),
            }
        }
        panic!("loopback listener did not stop accepting connections");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn tcp_probe_measures_connection_establishment_to_a_listening_port() {
        let (_listener, port) = listening_socket(8);
        let results =
            PingEngine::new().ping_round(&[tcp_target(1, "127.0.0.1", 4, Some(port.into()))]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].target_id, 1);
        assert!(results[0].success);
        let latency = results[0].latency_ms.expect("connect latency");
        assert!(
            latency.is_finite() && (0.0..1_000.0).contains(&latency),
            "{latency}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn tcp_probe_reports_failure_for_a_refused_port() {
        let (listener, port) = listening_socket(8);
        drop(listener);
        let started = Instant::now();
        let results =
            PingEngine::new().ping_round(&[tcp_target(1, "127.0.0.1", 4, Some(port.into()))]);
        assert_eq!(
            (
                results[0].target_id,
                results[0].success,
                results[0].latency_ms
            ),
            (1, false, None)
        );
        // A refusal is observed, not waited out.
        assert!(started.elapsed() < ROUND_TIMEOUT, "{:?}", started.elapsed());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn six_blackholed_tcp_targets_share_one_round_timeout() {
        let (_blackhole, _held, blackholed) = blackholed_port();
        let (_listener, reachable) = listening_socket(8);
        // Five hanging targets plus one reachable target last: the reachable one
        // can only answer if every connect was started before the wait began.
        let mut targets: Vec<AgentPingTargetV2> = (1..=5)
            .map(|id| tcp_target(id, "127.0.0.1", 4, Some(blackholed.into())))
            .collect();
        targets.push(tcp_target(6, "127.0.0.1", 4, Some(reachable.into())));
        let started = Instant::now();
        let results = PingEngine::new().ping_round(&targets);
        let elapsed = started.elapsed();
        assert_eq!(results.len(), 6);
        assert!(
            results[..5]
                .iter()
                .all(|result| !result.success && result.latency_ms.is_none())
        );
        // Sequential connects would spend the whole round on the first target and
        // never reach this one.
        assert!(results[5].success && results[5].latency_ms.is_some());
        // Proof the hanging connects really hung rather than failing fast...
        assert!(elapsed >= ROUND_TIMEOUT.mul_f64(0.9), "{elapsed:?}");
        // ...and that they hung concurrently instead of 6 x timeout serially.
        assert!(elapsed < ROUND_TIMEOUT * 2, "{elapsed:?}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mixed_round_keeps_per_target_results_aligned() {
        let (_listener, port) = listening_socket(8);
        let (_blackhole, _held, blackholed) = blackholed_port();
        let targets = vec![
            tcp_target(2, "127.0.0.1", 4, Some(port.into())),
            // A real ICMP send shares the round; whether it answers depends on
            // CAP_NET_RAW and the host network, so only its slot is asserted.
            target(1, "127.0.0.1", 4),
            tcp_target(3, "127.0.0.1", 4, Some(blackholed.into())),
            // Family mismatch: unprobeable, and still holds its own slot.
            target(4, "2001:db8::1", 4),
        ];
        let started = Instant::now();
        let results = PingEngine::new().ping_round(&targets);
        let elapsed = started.elapsed();
        assert_eq!(
            results
                .iter()
                .map(|result| result.target_id)
                .collect::<Vec<_>>(),
            vec![2, 1, 3, 4]
        );
        assert!(results[0].success && results[0].latency_ms.is_some());
        assert_eq!((results[2].success, results[2].latency_ms), (false, None));
        assert_eq!((results[3].success, results[3].latency_ms), (false, None));
        assert!(elapsed < ROUND_TIMEOUT * 2, "{elapsed:?}");
    }
}
