use std::{error::Error, fmt, time::Duration};

use monitor_common::{AgentConfigPayloadV2, AgentReport};

use crate::cli::RunConfig;

const CONFIG_BODY_LIMIT: u64 = 32 * 1_024;
const JS_SAFE_INTEGER_MAX: i64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    Authentication,
    Protocol,
    Transient,
}

#[derive(Debug)]
pub struct ClientError {
    kind: FailureKind,
    status_code: Option<u16>,
    message: String,
}

impl ClientError {
    pub fn kind(&self) -> FailureKind {
        self.kind
    }

    pub fn status_code(&self) -> Option<u16> {
        self.status_code
    }

    fn authentication(status_code: Option<u16>) -> Self {
        Self {
            kind: FailureKind::Authentication,
            status_code,
            message: "agent authentication rejected".to_owned(),
        }
    }

    fn protocol(status_code: Option<u16>, message: impl Into<String>) -> Self {
        Self {
            kind: FailureKind::Protocol,
            status_code,
            message: message.into(),
        }
    }

    fn transient(message: impl Into<String>) -> Self {
        Self {
            kind: FailureKind::Transient,
            status_code: None,
            message: message.into(),
        }
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ClientError {}

pub struct MonitorClient {
    agent: ureq::Agent,
    authorization: String,
    config_url: String,
    report_url: String,
}

impl MonitorClient {
    pub fn new(config: &RunConfig) -> Self {
        let transport = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .max_redirects(0)
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            agent: transport,
            authorization: format!("Bearer {}", config.token.expose()),
            config_url: format!("{}/api/agent/config", config.server),
            report_url: format!("{}/api/agent/report", config.server),
        }
    }

    pub fn get_config(&self) -> Result<AgentConfigPayloadV2, ClientError> {
        let mut response = self
            .agent
            .get(&self.config_url)
            .header("Authorization", &self.authorization)
            .header("Accept", "application/json")
            // Asking for config protocol 2. A v0.1.2 Server does not know this
            // header, ignores it, and answers protocol 1 -- which this Agent
            // accepts, so a new Agent keeps working against an old Server.
            .header(
                monitor_common::CONFIG_VERSION_HEADER,
                &monitor_common::CONFIG_VERSION_V2.to_string(),
            )
            .call()
            .map_err(classify_transport)?;
        let status = response.status().as_u16();
        if status != 200 {
            classify_status(status)?;
            return Err(ClientError::protocol(
                Some(status),
                format!("unexpected config response status {status}"),
            ));
        }
        let body = response
            .body_mut()
            .with_config()
            .limit(CONFIG_BODY_LIMIT)
            .read_to_string()
            .map_err(|error| ClientError::transient(format!("failed to read config: {error}")))?;
        let config = AgentConfigPayloadV2::from_json(&body).map_err(|error| {
            ClientError::protocol(None, format!("invalid agent config: {error:?}"))
        })?;
        validate_config(&config)?;
        Ok(config)
    }

    pub fn post_report(&self, report: &AgentReport) -> Result<(), ClientError> {
        let body = serde_json::to_vec(report).map_err(|error| {
            ClientError::protocol(None, format!("failed to encode report: {error}"))
        })?;
        let response = self
            .agent
            .post(&self.report_url)
            .header("Authorization", &self.authorization)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .send(&body)
            .map_err(classify_transport)?;
        let status = response.status().as_u16();
        if status == 204 {
            Ok(())
        } else {
            classify_status(status)?;
            Err(ClientError::protocol(
                Some(status),
                format!("unexpected report response status {status}"),
            ))
        }
    }
}

fn classify_status(status: u16) -> Result<(), ClientError> {
    match status {
        200..=299 => Ok(()),
        401 => Err(ClientError::authentication(Some(status))),
        500..=599 => Err(ClientError {
            kind: FailureKind::Transient,
            status_code: Some(status),
            message: format!("server temporarily unavailable ({status})"),
        }),
        400 | 413 | 415 => Err(ClientError::protocol(
            Some(status),
            format!("agent protocol rejected ({status})"),
        )),
        _ if (400..=499).contains(&status) => Err(ClientError::protocol(
            Some(status),
            format!("unexpected client response status {status}"),
        )),
        _ => Err(ClientError::protocol(
            Some(status),
            format!("unexpected server response status {status}"),
        )),
    }
}

fn classify_transport(error: ureq::Error) -> ClientError {
    match error {
        ureq::Error::StatusCode(status) => {
            classify_status(status).expect_err("status error must classify as an error")
        }
        other => ClientError::transient(format!("network request failed: {other}")),
    }
}

fn validate_config(config: &AgentConfigPayloadV2) -> Result<(), ClientError> {
    if !matches!(
        config.protocol_version,
        monitor_common::CONFIG_VERSION_V1 | monitor_common::CONFIG_VERSION_V2
    ) || !(2..=60).contains(&config.report_interval_seconds)
        || !(10..=300).contains(&config.ping_interval_seconds)
        || config.targets.len() > 6
    {
        return Err(ClientError::protocol(None, "invalid agent config values"));
    }
    let mut ids = Vec::with_capacity(config.targets.len());
    for target in &config.targets {
        if !(1..=JS_SAFE_INTEGER_MAX).contains(&target.id)
            || ids.contains(&target.id)
            || !matches!(target.ip_family, 4 | 6)
            || target.name.trim().is_empty()
            || target.name.chars().count() > 64
            || target.host.is_empty()
            || target.host.len() > 253
            || target
                .host
                .bytes()
                .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
            || !target.endpoint_is_valid()
        {
            return Err(ClientError::protocol(None, "invalid agent target config"));
        }
        ids.push(target.id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::mpsc,
        thread,
    };

    use monitor_common::{
        CpuReport, DiskReport, MemoryReport, NetworkReport, OsReport, PingReport,
    };

    use super::*;
    use crate::cli::{Action, parse};

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn client(server: &str) -> MonitorClient {
        let Action::Run(config) = parse(
            ["monitor-agent", "--server", server, "--token", TOKEN]
                .into_iter()
                .map(str::to_owned),
        )
        .expect("valid client CLI") else {
            panic!("expected run config");
        };
        MonitorClient::new(&config)
    }

    fn report() -> AgentReport {
        AgentReport {
            protocol_version: 1,
            agent_version: "0.1.0".to_owned(),
            boot_id: "boot".to_owned(),
            hostname: "node".to_owned(),
            os: OsReport {
                name: "Debian".to_owned(),
                version: "13".to_owned(),
                kernel: "6.12".to_owned(),
                architecture: "x86_64".to_owned(),
                virtualization: "kvm".to_owned(),
            },
            cpu: CpuReport {
                model: "CPU".to_owned(),
                cores: 1,
                usage: 0.0,
                load_1: 0.0,
                load_5: 0.0,
                load_15: 0.0,
            },
            memory: MemoryReport {
                total: 1,
                used: 0,
                swap_total: 0,
                swap_used: 0,
            },
            disk: DiskReport { total: 1, used: 0 },
            network: NetworkReport {
                rx_bytes: 1,
                tx_bytes: 2,
                rx_rate: 0,
                tx_rate: 0,
            },
            uptime_seconds: 1,
            process_count: 1,
            pings: Vec::<PingReport>::new(),
        }
    }

    fn read_request(stream: &mut TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let count = stream.read(&mut buffer).expect("read HTTP request");
            assert!(count > 0, "request ended before headers");
            bytes.extend_from_slice(&buffer[..count]);
            if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let header_end = bytes
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("header terminator")
            + 4;
        let headers = String::from_utf8(bytes[..header_end].to_vec()).expect("ASCII headers");
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(str::trim)
                    .and_then(|value| value.parse::<usize>().ok())
            })
            .unwrap_or(0);
        while bytes.len() < header_end + content_length {
            let count = stream.read(&mut buffer).expect("read HTTP body");
            assert!(count > 0, "request ended before body");
            bytes.extend_from_slice(&buffer[..count]);
        }
        String::from_utf8(bytes).expect("UTF-8 request")
    }

    fn server<F>(requests: usize, respond: F) -> (String, mpsc::Receiver<Vec<String>>)
    where
        F: Fn(usize, &str) -> String + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        let address = listener.local_addr().expect("fixture address");
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept fixture client");
            let mut captured = Vec::new();
            for index in 0..requests {
                let request = read_request(&mut stream);
                let response = respond(index, &request);
                captured.push(request);
                stream
                    .write_all(response.as_bytes())
                    .expect("write response");
                stream.flush().expect("flush response");
            }
            sender.send(captured).expect("send captured requests");
        });
        (format!("http://{address}"), receiver)
    }

    #[test]
    fn config_and_report_use_frozen_paths_headers_and_one_agent_pool() {
        let config_body = r#"{"protocol_version":1,"report_interval_seconds":2,"ping_interval_seconds":15,"targets":[]}"#;
        let (server, captured) = server(2, move |index, _| {
            if index == 0 {
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{config_body}",
                    config_body.len()
                )
            } else {
                "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_owned()
            }
        });
        let client = client(&server);
        assert_eq!(client.get_config().expect("get config").protocol_version, 1);
        client.post_report(&report()).expect("post report");
        let requests = captured.recv().expect("captured HTTP requests");
        assert!(requests[0].starts_with("GET /api/agent/config HTTP/1.1"));
        assert!(requests[1].starts_with("POST /api/agent/report HTTP/1.1"));
        for request in &requests {
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains(&format!("authorization: bearer {TOKEN}"))
            );
        }
        assert!(
            requests[1]
                .to_ascii_lowercase()
                .contains("content-type: application/json")
        );
        assert!(requests[1].contains("\"pings\":[]"));
        // The config request negotiates protocol 2; the report wire protocol is
        // frozen at 1 and must not be told anything about config versions.
        assert!(
            requests[0]
                .to_ascii_lowercase()
                .contains("x-monitor-config-version: 2")
        );
        assert!(
            !requests[1]
                .to_ascii_lowercase()
                .contains("x-monitor-config-version")
        );
    }

    #[test]
    fn accepts_config_protocol_one_and_two_and_rejects_malformed_version_two() {
        // An old Server answers protocol 1: ICMP-only targets, no probe fields.
        let v1 = r#"{"protocol_version":1,"report_interval_seconds":2,"ping_interval_seconds":15,"targets":[{"id":7,"name":"Cloudflare","host":"1.1.1.1","ip_family":4}]}"#;
        let v2 = r#"{"protocol_version":2,"report_interval_seconds":2,"ping_interval_seconds":15,"targets":[{"id":7,"name":"Cloudflare","host":"1.1.1.1","ip_family":4,"probe_kind":"icmp","port":null},{"id":8,"name":"HTTPS","host":"1.1.1.1","ip_family":4,"probe_kind":"tcp","port":443}]}"#;
        // A TCP target without a port is refused rather than repaired into ICMP.
        let broken = r#"{"protocol_version":2,"report_interval_seconds":2,"ping_interval_seconds":15,"targets":[{"id":9,"name":"broken","host":"1.1.1.1","ip_family":4,"probe_kind":"tcp","port":null}]}"#;
        let (server, _captured) = server(3, move |index, _| {
            let (body, connection) = match index {
                0 => (v1, "keep-alive"),
                1 => (v2, "keep-alive"),
                _ => (broken, "close"),
            };
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: {connection}\r\n\r\n{body}",
                body.len()
            )
        });
        let client = client(&server);

        let legacy = client.get_config().expect("protocol 1 config");
        assert_eq!(legacy.protocol_version, 1);
        assert_eq!(legacy.targets.len(), 1);
        assert_eq!(
            legacy.targets[0].probe_kind,
            monitor_common::ProbeKind::Icmp
        );
        assert_eq!(legacy.targets[0].port, None);

        let current = client.get_config().expect("protocol 2 config");
        assert_eq!(current.protocol_version, 2);
        assert_eq!(
            current
                .targets
                .iter()
                .map(|target| (target.id, target.probe_kind, target.port))
                .collect::<Vec<_>>(),
            vec![
                (7, monitor_common::ProbeKind::Icmp, None),
                (8, monitor_common::ProbeKind::Tcp, Some(443)),
            ]
        );

        let error = client
            .get_config()
            .expect_err("malformed protocol 2 config");
        assert_eq!(error.kind(), FailureKind::Protocol);
    }

    #[test]
    fn classifies_authentication_protocol_and_transient_statuses() {
        for (status, expected) in [
            (401, FailureKind::Authentication),
            (400, FailureKind::Protocol),
            (413, FailureKind::Protocol),
            (415, FailureKind::Protocol),
            (404, FailureKind::Protocol),
            (503, FailureKind::Transient),
        ] {
            let error = classify_status(status).expect_err("error status");
            assert_eq!(error.kind(), expected);
            assert_eq!(error.status_code(), Some(status));
        }
    }

    #[test]
    fn http_401_is_fatal_authentication_and_503_is_transient() {
        for (status, reason, expected) in [
            (401, "Unauthorized", FailureKind::Authentication),
            (503, "Service Unavailable", FailureKind::Transient),
        ] {
            let (server, captured) = server(1, move |_, _| {
                format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
            });
            let error = client(&server)
                .get_config()
                .expect_err("error response must be classified");
            assert_eq!(error.kind(), expected);
            assert_eq!(captured.recv().expect("captured request").len(), 1);
        }
    }

    #[test]
    fn connection_failure_is_transient() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("reserve local address");
        let address = listener.local_addr().expect("reserved address");
        drop(listener);
        let error = client(&format!("http://{address}"))
            .get_config()
            .expect_err("closed listener must fail");
        assert_eq!(error.kind(), FailureKind::Transient);
    }

    #[test]
    fn strict_config_validation_rejects_bad_ranges_and_targets() {
        let mut config = AgentConfigPayloadV2 {
            protocol_version: 1,
            report_interval_seconds: 2,
            ping_interval_seconds: 15,
            targets: vec![monitor_common::AgentPingTargetV2 {
                id: 1,
                name: "v4".to_owned(),
                host: "203.0.113.1".to_owned(),
                ip_family: 4,
                probe_kind: monitor_common::ProbeKind::Icmp,
                port: None,
            }],
        };
        assert!(validate_config(&config).is_ok());
        config.targets.push(config.targets[0].clone());
        assert_eq!(
            validate_config(&config).expect_err("duplicate ID").kind(),
            FailureKind::Protocol
        );
        config.targets.pop();
        config.report_interval_seconds = 1;
        assert!(validate_config(&config).is_err());
        config.report_interval_seconds = 2;
        // Second gate after parsing: a probe kind and endpoint that cannot be
        // probed is refused instead of silently downgraded.
        config.protocol_version = monitor_common::CONFIG_VERSION_V2;
        config.targets[0].probe_kind = monitor_common::ProbeKind::Tcp;
        assert_eq!(
            validate_config(&config)
                .expect_err("tcp target without a port")
                .kind(),
            FailureKind::Protocol
        );
        config.targets[0].port = Some(0);
        assert!(validate_config(&config).is_err());
        config.targets[0].port = Some(443);
        assert!(validate_config(&config).is_ok());
    }
}
