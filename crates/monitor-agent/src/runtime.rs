use std::{
    error::Error,
    fmt, thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    cli::RunConfig,
    client::{ClientError, FailureKind, MonitorClient},
    collector::{CollectorError, SystemCollector},
};

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
    let mut next_report = Instant::now();
    let mut next_config_refresh = Instant::now() + CONFIG_REFRESH_INTERVAL;
    let mut report_backoff = Backoff::new();
    let mut config_backoff = Backoff::new();
    let mut report_waiting = false;

    loop {
        let now = Instant::now();
        if now >= next_config_refresh {
            match client.get_config() {
                Ok(next_config) => {
                    let interval_changed =
                        next_config.report_interval_seconds != config.report_interval_seconds;
                    config = next_config;
                    config_backoff.reset();
                    next_config_refresh = Instant::now() + CONFIG_REFRESH_INTERVAL;
                    if interval_changed {
                        next_report = Instant::now()
                            + Duration::from_secs(config.report_interval_seconds as u64);
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
        if now >= next_report {
            let report = collector.collect_report()?;
            debug_assert!(report.pings.is_empty());
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
                Err(error) => return Err(fatal_client_error(error)),
            }
        }

        let deadline = next_report.min(next_config_refresh);
        thread::sleep(deadline.saturating_duration_since(Instant::now()));
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
    use super::*;

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
}
