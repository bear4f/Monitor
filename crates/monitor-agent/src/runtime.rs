use std::{
    error::Error,
    fmt, thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    cli::RunConfig,
    client::{ClientError, FailureKind, MonitorClient},
    collector::{CollectorError, SystemCollector},
    ping::PingEngine,
};
use monitor_common::{AgentConfigPayload, AgentReport, PingReport};

const CONFIG_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub struct RuntimeError(String);

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for RuntimeError {}

impl From<CollectorError> for RuntimeError {
    fn from(error: CollectorError) -> Self {
        Self(error.to_string())
    }
}

pub fn run(run_config: RunConfig) -> Result<(), RuntimeError> {
    let mut collector = SystemCollector::new()?;
    let client = MonitorClient::new(&run_config);
    let mut startup_backoff = Backoff::new();
    let mut startup_waiting = false;
    let mut config = loop {
        match client.get_config() {
            Ok(config) => {
                if startup_waiting {
                    eprintln!("monitor-agent: server connection recovered");
                }
                break config;
            }
            Err(error) if error.kind() == FailureKind::Transient => {
                if !startup_waiting {
                    eprintln!("monitor-agent: server unavailable; retrying");
                    startup_waiting = true;
                }
                thread::sleep(startup_backoff.next_delay());
            }
            Err(error) => return Err(fatal_client_error(error)),
        }
    };

    collector.initialize_baselines()?;
    let mut ping_engine = PingEngine::new();
    let mut pending_ping: Option<Vec<PingReport>> = None;
    let now = Instant::now();
    let (mut next_report, mut next_ping) = initial_deadlines(now, &config);
    let mut next_config_refresh = now + CONFIG_REFRESH_INTERVAL;
    let mut report_backoff = Backoff::new();
    let mut config_backoff = Backoff::new();
    let mut report_waiting = false;
    let mut collector_waiting = false;

    loop {
        let now = Instant::now();
        if now >= next_config_refresh {
            match client.get_config() {
                Ok(next_config) => {
                    let report_interval_changed =
                        next_config.report_interval_seconds != config.report_interval_seconds;
                    let ping_changed = ping_schedule_changed(&config, &next_config);
                    config = next_config;
                    config_backoff.reset();
                    let now = Instant::now();
                    next_config_refresh = now + CONFIG_REFRESH_INTERVAL;
                    if report_interval_changed {
                        next_report =
                            now + Duration::from_secs(config.report_interval_seconds as u64);
                    }
                    if ping_changed {
                        pending_ping = None;
                        next_ping = next_ping_deadline(now, &config);
                    }
                }
                Err(error) if error.kind() == FailureKind::Transient => {
                    eprintln!(
                        "monitor-agent: configuration refresh failed; retaining last known config"
                    );
                    next_config_refresh = Instant::now() + config_backoff.next_delay();
                }
                Err(error) => return Err(fatal_client_error(error)),
            }
        }

        let now = Instant::now();
        if next_ping.is_some_and(|deadline| now >= deadline) {
            pending_ping = Some(ping_engine.ping_round(&config.targets));
            let now = Instant::now();
            let next_deadline = advance_deadline(
                next_ping.expect("due ping deadline"),
                Duration::from_secs(config.ping_interval_seconds as u64),
                now,
            );
            next_ping = Some(next_deadline);
            next_report = schedule_report_after_ping(next_report, next_deadline, now);
        }

        let now = Instant::now();
        if now >= next_report {
            let mut report = match collector.collect_report() {
                Ok(report) => {
                    if collector_waiting {
                        eprintln!("monitor-agent: metric collection recovered");
                        collector_waiting = false;
                    }
                    report
                }
                // A runtime collection failure must not end the process, and a
                // partial or fabricated report is worse than none: skip this
                // cycle and let the node fall offline until the host recovers.
                Err(error) => {
                    if !collector_waiting {
                        eprintln!(
                            "monitor-agent: metric collection failed; skipping report: {error}"
                        );
                        collector_waiting = true;
                    }
                    next_report = advance_deadline(
                        next_report,
                        Duration::from_secs(config.report_interval_seconds as u64),
                        Instant::now(),
                    );
                    continue;
                }
            };
            let included_ping = attach_pending_ping(&mut report, &mut pending_ping);
            match client.post_report(&report) {
                Ok(()) => {
                    if report_waiting {
                        eprintln!("monitor-agent: report connection recovered");
                        report_waiting = false;
                    }
                    report_backoff.reset();
                    next_report = advance_deadline(
                        next_report,
                        Duration::from_secs(config.report_interval_seconds as u64),
                        Instant::now(),
                    );
                }
                Err(error) if error.kind() == FailureKind::Transient => {
                    if !report_waiting {
                        eprintln!("monitor-agent: report failed; retrying current state later");
                        report_waiting = true;
                    }
                    thread::sleep(report_backoff.next_delay());
                    next_report = Instant::now();
                    continue;
                }
                Err(error) if error.status_code() == Some(400) && included_ping => {
                    match reconcile_stale_config(&client, &config)? {
                        Some(next_config) => {
                            config = next_config;
                            let now = Instant::now();
                            next_config_refresh = now + CONFIG_REFRESH_INTERVAL;
                            next_ping = next_ping_deadline(now, &config);
                            next_report = now;
                            config_backoff.reset();
                        }
                        None => {
                            eprintln!(
                                "monitor-agent: configuration reconciliation failed; retrying"
                            );
                            let now = Instant::now();
                            next_config_refresh = now + config_backoff.next_delay();
                            next_report =
                                now + Duration::from_secs(config.report_interval_seconds as u64);
                        }
                    }
                }
                Err(error) => return Err(fatal_client_error(error)),
            }
        }

        let deadline = next_ping
            .map(|next_ping| next_report.min(next_config_refresh).min(next_ping))
            .unwrap_or_else(|| next_report.min(next_config_refresh));
        thread::sleep(deadline.saturating_duration_since(Instant::now()));
    }
}

fn initial_deadlines(now: Instant, config: &AgentConfigPayload) -> (Instant, Option<Instant>) {
    (
        now + Duration::from_secs(config.report_interval_seconds as u64),
        next_ping_deadline(now, config),
    )
}

fn next_ping_deadline(now: Instant, config: &AgentConfigPayload) -> Option<Instant> {
    (!config.targets.is_empty())
        .then(|| now + Duration::from_secs(config.ping_interval_seconds as u64))
}

fn ping_schedule_changed(previous: &AgentConfigPayload, next: &AgentConfigPayload) -> bool {
    previous.ping_interval_seconds != next.ping_interval_seconds || previous.targets != next.targets
}

fn schedule_report_after_ping(next_report: Instant, next_ping: Instant, now: Instant) -> Instant {
    if next_report >= next_ping {
        now
    } else {
        next_report
    }
}

fn attach_pending_ping(
    report: &mut AgentReport,
    pending_ping: &mut Option<Vec<PingReport>>,
) -> bool {
    let Some(pings) = pending_ping.take() else {
        return false;
    };
    let included_ping = !pings.is_empty();
    report.pings = pings;
    included_ping
}

fn reconcile_stale_config(
    client: &MonitorClient,
    previous: &AgentConfigPayload,
) -> Result<Option<AgentConfigPayload>, RuntimeError> {
    match client.get_config() {
        Ok(next) if previous.targets != next.targets => Ok(Some(next)),
        Ok(_) => Err(RuntimeError(
            "agent protocol error: report rejected with unchanged ping configuration".to_owned(),
        )),
        Err(error) if error.kind() == FailureKind::Transient => Ok(None),
        Err(error) => Err(fatal_client_error(error)),
    }
}

fn fatal_client_error(error: ClientError) -> RuntimeError {
    match error.kind() {
        FailureKind::Authentication => RuntimeError("agent authentication rejected".to_owned()),
        FailureKind::Protocol => RuntimeError(format!("agent protocol error: {error}")),
        FailureKind::Transient => RuntimeError(format!("transient network error: {error}")),
    }
}

fn advance_deadline(previous: Instant, interval: Duration, now: Instant) -> Instant {
    let scheduled = previous + interval;
    if scheduled <= now {
        now + interval
    } else {
        scheduled
    }
}

struct Backoff {
    next_seconds: u64,
    random: u64,
}

impl Backoff {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        Self::with_seed(nanos ^ u64::from(std::process::id()))
    }

    fn with_seed(seed: u64) -> Self {
        Self {
            next_seconds: 2,
            random: seed.max(1),
        }
    }

    fn reset(&mut self) {
        self.next_seconds = 2;
    }

    fn next_delay(&mut self) -> Duration {
        let base_millis = self.next_seconds * 1_000;
        self.random ^= self.random << 13;
        self.random ^= self.random >> 7;
        self.random ^= self.random << 17;
        let spread = base_millis / 10;
        let offset = if spread == 0 {
            0
        } else {
            self.random % (spread * 2 + 1)
        };
        let jittered = base_millis - spread + offset;
        self.next_seconds = (self.next_seconds * 2).min(60);
        Duration::from_millis(jittered.min(60_000))
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::mpsc,
    };

    use monitor_common::AgentPingTarget;

    use super::*;
    use crate::cli::{Action, parse};

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn config(report_interval: i64, ping_interval: i64) -> AgentConfigPayload {
        AgentConfigPayload {
            protocol_version: 1,
            report_interval_seconds: report_interval,
            ping_interval_seconds: ping_interval,
            targets: vec![AgentPingTarget {
                id: 1,
                name: "target".to_owned(),
                host: "127.0.0.1".to_owned(),
                ip_family: 4,
            }],
        }
    }

    fn empty_report() -> AgentReport {
        serde_json::from_str(
            r#"{
                "protocol_version":1,"agent_version":"0.1.0","boot_id":"boot",
                "hostname":"node","os":{"name":"Debian","version":"13",
                "kernel":"6.12","architecture":"x86_64","virtualization":"kvm"},
                "cpu":{"model":"CPU","cores":1,"usage":0.0,"load_1":0.0,
                "load_5":0.0,"load_15":0.0},"memory":{"total":1,"used":0,
                "swap_total":0,"swap_used":0},"disk":{"total":1,"used":0},
                "network":{"rx_bytes":1,"tx_bytes":1,"rx_rate":0,"tx_rate":0},
                "uptime_seconds":1,"process_count":1,"pings":[]
            }"#,
        )
        .expect("valid report fixture")
    }

    #[test]
    fn backoff_is_bounded_and_resets() {
        let mut backoff = Backoff::with_seed(1);
        for (index, expected_base) in [2_u64, 4, 8, 16, 32, 60, 60].into_iter().enumerate() {
            let delay = backoff.next_delay();
            let lower = Duration::from_millis(expected_base * 900);
            let upper = Duration::from_millis((expected_base * 1_100).min(60_000));
            assert!(delay >= lower && delay <= upper, "delay {index}: {delay:?}");
        }
        backoff.reset();
        assert!(backoff.next_delay() < Duration::from_secs(3));
    }

    #[test]
    fn cadence_skips_missed_intervals_without_catch_up_burst() {
        let start = Instant::now();
        let now = start + Duration::from_secs(10);
        assert_eq!(
            advance_deadline(start, Duration::from_secs(2), now),
            now + Duration::from_secs(2)
        );
        assert_eq!(
            advance_deadline(start, Duration::from_secs(20), now),
            start + Duration::from_secs(20)
        );
    }

    #[test]
    fn a_skipped_collection_reschedules_into_the_future_without_spinning() {
        // A collector failure skips the cycle and reuses `advance_deadline`; if
        // that ever returned a past deadline the agent would busy-loop instead
        // of sleeping, which is the failure mode the skip path must not create.
        let now = Instant::now();
        for interval_seconds in [2_u64, 15, 60] {
            let interval = Duration::from_secs(interval_seconds);
            for missed in [0_u64, 1, 5, 1_000] {
                let overdue = now - Duration::from_secs(missed * interval_seconds);
                assert!(
                    advance_deadline(overdue, interval, now) > now,
                    "interval {interval_seconds}s, {missed} missed cycles"
                );
            }
        }
    }

    #[test]
    fn initial_report_waits_for_a_real_sampling_window() {
        let now = Instant::now();
        let (next_report, next_ping) = initial_deadlines(now, &config(2, 10));
        assert_eq!(next_report, now + Duration::from_secs(2));
        assert_eq!(next_ping, Some(now + Duration::from_secs(10)));
        let mut no_targets = config(60, 10);
        no_targets.targets.clear();
        assert_eq!(initial_deadlines(now, &no_targets).1, None);
    }

    #[test]
    fn one_ping_round_is_attached_to_at_most_one_report() {
        let mut pending = Some(vec![PingReport {
            target_id: 1,
            success: true,
            latency_ms: Some(1.5),
        }]);
        let mut first = empty_report();
        assert!(attach_pending_ping(&mut first, &mut pending));
        assert_eq!(first.pings.len(), 1);
        let mut second = empty_report();
        assert!(!attach_pending_ping(&mut second, &mut pending));
        assert!(second.pings.is_empty());
    }

    #[test]
    fn slow_report_interval_is_advanced_before_the_next_ping_round() {
        let now = Instant::now();
        let next_report = now + Duration::from_secs(50);
        let next_ping = now + Duration::from_secs(10);
        assert_eq!(schedule_report_after_ping(next_report, next_ping, now), now);
        assert_eq!(
            schedule_report_after_ping(now + Duration::from_secs(5), next_ping, now),
            now + Duration::from_secs(5)
        );
    }

    #[test]
    fn target_or_ping_interval_change_resets_ping_schedule() {
        let original = config(2, 10);
        let mut changed = original.clone();
        changed.targets[0].id = 2;
        assert!(ping_schedule_changed(&original, &changed));
        changed = original.clone();
        changed.ping_interval_seconds = 20;
        assert!(ping_schedule_changed(&original, &changed));
        changed = original.clone();
        changed.report_interval_seconds = 3;
        assert!(!ping_schedule_changed(&original, &changed));
    }

    #[test]
    fn stale_ping_config_reconciles_and_the_next_resource_report_continues() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
        let address = listener.local_addr().expect("fixture address");
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept fixture client");
            let mut requests = Vec::new();
            let config_body = concat!(
                "{\"protocol_version\":1,\"report_interval_seconds\":2,",
                "\"ping_interval_seconds\":10,\"targets\":[]}"
            );
            for response in [
                "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: keep-alive\r\n\r\n"
                    .to_owned(),
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{config_body}",
                    config_body.len()
                ),
                "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_owned(),
            ] {
                requests.push(read_http_request(&mut stream));
                stream
                    .write_all(response.as_bytes())
                    .expect("write response");
                stream.flush().expect("flush response");
            }
            sender.send(requests).expect("send fixture requests");
        });
        let Action::Run(run_config) = parse(
            [
                "monitor-agent",
                "--server",
                &format!("http://{address}"),
                "--token",
                TOKEN,
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .expect("parse fixture config") else {
            panic!("expected run action");
        };
        let client = MonitorClient::new(&run_config);
        let mut stale_report = empty_report();
        stale_report.pings.push(PingReport {
            target_id: 1,
            success: false,
            latency_ms: None,
        });
        let error = client
            .post_report(&stale_report)
            .expect_err("stale target must be rejected");
        assert_eq!(error.status_code(), Some(400));
        let refreshed = reconcile_stale_config(&client, &config(2, 10))
            .expect("reconciliation")
            .expect("changed configuration");
        assert!(refreshed.targets.is_empty());
        client
            .post_report(&empty_report())
            .expect("resource-only report continues");
        let requests = receiver.recv().expect("captured requests");
        assert!(requests[0].contains("\"pings\":[{"));
        assert!(requests[1].starts_with("GET /api/agent/config HTTP/1.1"));
        assert!(requests[2].contains("\"pings\":[]"));
    }

    fn read_http_request(stream: &mut TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 1_024];
        loop {
            let count = stream.read(&mut buffer).expect("read request");
            assert!(count > 0);
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(header_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                let body_start = header_end + 4;
                let headers = String::from_utf8_lossy(&bytes[..body_start]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while bytes.len() < body_start + content_length {
                    let count = stream.read(&mut buffer).expect("read request body");
                    assert!(count > 0);
                    bytes.extend_from_slice(&buffer[..count]);
                }
                return String::from_utf8(bytes).expect("UTF-8 fixture request");
            }
        }
    }
}
