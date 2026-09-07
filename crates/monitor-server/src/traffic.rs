use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
};

use crate::{
    database::{
        TrafficCheckpointRow, TrafficCycleCheckpointRow, TrafficDayCheckpointRow,
        TrafficRecoveryRow,
    },
    snapshot::NodeSnapshot,
    time::BillingCycle,
};

pub const JS_SAFE_INTEGER_MAX: i64 = 9_007_199_254_740_991;

pub(crate) fn browser_safe_counter(value: i64) -> i64 {
    value.clamp(0, JS_SAFE_INTEGER_MAX)
}

#[derive(Clone, Default)]
pub struct TrafficState {
    inner: Arc<Mutex<HashMap<i64, NodeTrafficState>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeTrafficState {
    pub rx_total_bytes: i64,
    pub tx_total_bytes: i64,
    pub last_rx_counter_bytes: Option<i64>,
    pub last_tx_counter_bytes: Option<i64>,
    pub last_boot_id: Option<String>,
    pub day_start_utc: i64,
    pub today_rx_bytes: i64,
    pub today_tx_bytes: i64,
    pub cycle_start_utc: i64,
    pub cycle_end_utc: i64,
    pub cycle_rx_bytes: i64,
    pub cycle_tx_bytes: i64,
    pub generation: u64,
    pub persisted_generation: u64,
    // A boundary handoff is bounded to one prior bucket, never per-report history.
    pub previous_day: Option<TrafficDayCheckpointRow>,
    pub previous_cycle: Option<TrafficCycleCheckpointRow>,
}

#[derive(Debug, Clone, Copy)]
pub struct TrafficSample<'a> {
    pub rx_counter_bytes: i64,
    pub tx_counter_bytes: i64,
    pub boot_id: &'a str,
    pub day_start_utc: i64,
    pub billing_cycle: BillingCycle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrafficUpdate {
    pub wake_checkpoint: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicTrafficState {
    pub rx_total_bytes: i64,
    pub tx_total_bytes: i64,
    pub today_rx_bytes: i64,
    pub today_tx_bytes: i64,
    pub day_start_utc: i64,
    pub cycle_start_utc: i64,
    pub cycle_end_utc: i64,
    pub cycle_rx_bytes: i64,
    pub cycle_tx_bytes: i64,
}

#[derive(Debug, Clone, Copy)]
struct TrafficCheckpointAck {
    node_id: i64,
    captured_generation: u64,
    previous_day: Option<TrafficDayCheckpointRow>,
    previous_cycle: Option<TrafficCycleCheckpointRow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrafficError {
    IntegerOverflow,
    GenerationOverflow,
}

impl TrafficState {
    pub fn from_recovery(rows: Vec<TrafficRecoveryRow>) -> Self {
        let states = rows
            .into_iter()
            .map(|row| {
                (
                    row.node_id,
                    NodeTrafficState {
                        rx_total_bytes: row.rx_total_bytes,
                        tx_total_bytes: row.tx_total_bytes,
                        last_rx_counter_bytes: row.last_rx_counter_bytes,
                        last_tx_counter_bytes: row.last_tx_counter_bytes,
                        last_boot_id: row.last_boot_id,
                        day_start_utc: row.day_start_utc,
                        today_rx_bytes: row.today_rx_bytes,
                        today_tx_bytes: row.today_tx_bytes,
                        cycle_start_utc: row.cycle_start_utc,
                        cycle_end_utc: row.cycle_end_utc,
                        cycle_rx_bytes: row.cycle_rx_bytes,
                        cycle_tx_bytes: row.cycle_tx_bytes,
                        generation: 0,
                        persisted_generation: 0,
                        previous_day: None,
                        previous_cycle: None,
                    },
                )
            })
            .collect();
        Self {
            inner: Arc::new(Mutex::new(states)),
        }
    }

    pub fn update(
        &self,
        node_id: i64,
        sample: TrafficSample<'_>,
    ) -> Result<TrafficUpdate, TrafficError> {
        let mut states = self.lock();
        let current = states.entry(node_id).or_insert_with(|| NodeTrafficState {
            rx_total_bytes: 0,
            tx_total_bytes: 0,
            last_rx_counter_bytes: None,
            last_tx_counter_bytes: None,
            last_boot_id: None,
            day_start_utc: sample.day_start_utc,
            today_rx_bytes: 0,
            today_tx_bytes: 0,
            cycle_start_utc: sample.billing_cycle.start_utc,
            cycle_end_utc: sample.billing_cycle.end_utc,
            cycle_rx_bytes: 0,
            cycle_tx_bytes: 0,
            generation: 0,
            persisted_generation: 0,
            previous_day: None,
            previous_cycle: None,
        });
        let mut next = current.clone();
        let first_baseline = next.last_boot_id.is_none();
        let boot_changed = next
            .last_boot_id
            .as_deref()
            .is_some_and(|boot_id| boot_id != sample.boot_id);
        let rx_reset = !boot_changed
            && next
                .last_rx_counter_bytes
                .is_some_and(|last| sample.rx_counter_bytes < last);
        let tx_reset = !boot_changed
            && next
                .last_tx_counter_bytes
                .is_some_and(|last| sample.tx_counter_bytes < last);
        let rx_delta = counter_delta(
            next.last_rx_counter_bytes,
            boot_changed,
            sample.rx_counter_bytes,
        );
        let tx_delta = counter_delta(
            next.last_tx_counter_bytes,
            boot_changed,
            sample.tx_counter_bytes,
        );
        let day_changed = next.day_start_utc != sample.day_start_utc;
        let cycle_changed = next.cycle_start_utc != sample.billing_cycle.start_utc
            || next.cycle_end_utc != sample.billing_cycle.end_utc;

        next.rx_total_bytes = checked_i64_add(next.rx_total_bytes, rx_delta)?;
        next.tx_total_bytes = checked_i64_add(next.tx_total_bytes, tx_delta)?;
        if day_changed {
            next.previous_day = Some(TrafficDayCheckpointRow {
                day_start_utc: next.day_start_utc,
                rx_bytes: next.today_rx_bytes,
                tx_bytes: next.today_tx_bytes,
            });
            next.day_start_utc = sample.day_start_utc;
            next.today_rx_bytes = 0;
            next.today_tx_bytes = 0;
        }
        next.today_rx_bytes = checked_i64_add(next.today_rx_bytes, rx_delta)?;
        next.today_tx_bytes = checked_i64_add(next.today_tx_bytes, tx_delta)?;
        if cycle_changed {
            next.previous_cycle = Some(TrafficCycleCheckpointRow {
                cycle_start_utc: next.cycle_start_utc,
                cycle_end_utc: next.cycle_end_utc,
                rx_bytes: next.cycle_rx_bytes,
                tx_bytes: next.cycle_tx_bytes,
            });
            next.cycle_start_utc = sample.billing_cycle.start_utc;
            next.cycle_end_utc = sample.billing_cycle.end_utc;
            next.cycle_rx_bytes = 0;
            next.cycle_tx_bytes = 0;
        }
        next.cycle_rx_bytes = checked_i64_add(next.cycle_rx_bytes, rx_delta)?;
        next.cycle_tx_bytes = checked_i64_add(next.cycle_tx_bytes, tx_delta)?;
        next.last_rx_counter_bytes = Some(sample.rx_counter_bytes);
        next.last_tx_counter_bytes = Some(sample.tx_counter_bytes);
        next.last_boot_id = Some(sample.boot_id.to_owned());
        next.generation = next
            .generation
            .checked_add(1)
            .ok_or(TrafficError::GenerationOverflow)?;

        *current = next;
        Ok(TrafficUpdate {
            wake_checkpoint: first_baseline
                || boot_changed
                || rx_reset
                || tx_reset
                || day_changed
                || cycle_changed,
        })
    }

    pub fn capture_dirty(
        &self,
        snapshots: &HashMap<i64, NodeSnapshot>,
    ) -> Vec<TrafficCheckpointRow> {
        self.lock()
            .iter()
            .filter(|(_, state)| state.generation > state.persisted_generation)
            .filter_map(|(node_id, state)| {
                let snapshot = snapshots.get(node_id)?.clone();
                Some(TrafficCheckpointRow {
                    node_id: *node_id,
                    captured_generation: state.generation,
                    rx_total_bytes: state.rx_total_bytes,
                    tx_total_bytes: state.tx_total_bytes,
                    last_rx_counter_bytes: state.last_rx_counter_bytes?,
                    last_tx_counter_bytes: state.last_tx_counter_bytes?,
                    last_boot_id: state.last_boot_id.clone()?,
                    day_start_utc: state.day_start_utc,
                    today_rx_bytes: state.today_rx_bytes,
                    today_tx_bytes: state.today_tx_bytes,
                    cycle_start_utc: state.cycle_start_utc,
                    cycle_end_utc: state.cycle_end_utc,
                    cycle_rx_bytes: state.cycle_rx_bytes,
                    cycle_tx_bytes: state.cycle_tx_bytes,
                    previous_day: state.previous_day,
                    previous_cycle: state.previous_cycle,
                    snapshot,
                })
            })
            .collect()
    }

    pub fn read_current(&self) -> HashMap<i64, PublicTrafficState> {
        self.lock()
            .iter()
            .map(|(node_id, state)| {
                (
                    *node_id,
                    PublicTrafficState {
                        rx_total_bytes: state.rx_total_bytes,
                        tx_total_bytes: state.tx_total_bytes,
                        today_rx_bytes: state.today_rx_bytes,
                        today_tx_bytes: state.today_tx_bytes,
                        day_start_utc: state.day_start_utc,
                        cycle_start_utc: state.cycle_start_utc,
                        cycle_end_utc: state.cycle_end_utc,
                        cycle_rx_bytes: state.cycle_rx_bytes,
                        cycle_tx_bytes: state.cycle_tx_bytes,
                    },
                )
            })
            .collect()
    }

    fn mark_persisted(&self, acknowledgements: &[TrafficCheckpointAck]) {
        let mut states = self.lock();
        for acknowledgement in acknowledgements {
            if let Some(state) = states.get_mut(&acknowledgement.node_id) {
                state.persisted_generation = state
                    .persisted_generation
                    .max(acknowledgement.captured_generation.min(state.generation));
                if state.previous_day == acknowledgement.previous_day {
                    state.previous_day = None;
                }
                if state.previous_cycle == acknowledgement.previous_cycle {
                    state.previous_cycle = None;
                }
            }
        }
    }

    pub fn remove(&self, node_id: i64) {
        self.lock().remove(&node_id);
    }

    #[cfg(test)]
    pub fn get(&self, node_id: i64) -> Option<NodeTrafficState> {
        self.lock().get(&node_id).cloned()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<i64, NodeTrafficState>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

pub async fn run_checkpoint_worker(state: crate::app::AppState) {
    let period = std::time::Duration::from_secs(60);
    let mut interval = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    loop {
        tokio::select! {
            _ = interval.tick() => {}
            () = state.traffic_flush.notified() => {}
        }
        if let Err(error) = checkpoint_once(&state).await {
            tracing::error!(error = %error, "traffic checkpoint failed; dirty state retained");
        }
    }
}

pub async fn checkpoint_once(
    state: &crate::app::AppState,
) -> Result<usize, crate::database::DatabaseError> {
    let rows = {
        let _lifecycle_guard = state.node_lifecycle_gate.write().await;
        let snapshots = state.snapshots.read().await;
        state.traffic.capture_dirty(&snapshots)
    };
    if rows.is_empty() {
        return Ok(0);
    }
    let now = crate::auth::unix_timestamp().map_err(|_| crate::database::DatabaseError::Time {
        operation: "read traffic checkpoint clock",
        source: jiff::Error::from_args(format_args!("system clock is outside the supported range")),
    })?;
    let persisted_at = rows.iter().fold(now, |timestamp, row| {
        timestamp.max(row.snapshot.last_seen_at)
    });
    let acknowledgements: Vec<_> = rows
        .iter()
        .map(|row| TrafficCheckpointAck {
            node_id: row.node_id,
            captured_generation: row.captured_generation,
            previous_day: row.previous_day,
            previous_cycle: row.previous_cycle,
        })
        .collect();
    let count = rows.len();
    state
        .database
        .persist_traffic_batch(rows, persisted_at)
        .await?;
    state.traffic.mark_persisted(&acknowledgements);
    Ok(count)
}

fn counter_delta(last: Option<i64>, boot_changed: bool, current: i64) -> i64 {
    match last {
        None => 0,
        Some(_) if boot_changed => current,
        Some(previous) if current >= previous => current - previous,
        Some(_) => current,
    }
}

fn checked_i64_add(value: i64, delta: i64) -> Result<i64, TrafficError> {
    value
        .checked_add(delta)
        .ok_or(TrafficError::IntegerOverflow)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        net::{IpAddr, Ipv4Addr},
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use rusqlite::Connection;

    use super::*;
    use crate::{
        app::AppState,
        database::{Database, NewNodeRow, NodePatchRow, UpdateNodeResult, hydrate_startup},
        time,
    };

    static TEST_ID: AtomicU64 = AtomicU64::new(0);

    fn state() -> TrafficState {
        TrafficState::from_recovery(Vec::new())
    }

    fn sample<'a>(rx: i64, tx: i64, boot: &'a str) -> TrafficSample<'a> {
        TrafficSample {
            rx_counter_bytes: rx,
            tx_counter_bytes: tx,
            boot_id: boot,
            day_start_utc: 1_000,
            billing_cycle: BillingCycle {
                start_utc: 500,
                end_utc: 2_000,
            },
        }
    }

    fn sample_at<'a>(
        rx: i64,
        tx: i64,
        boot: &'a str,
        timestamp: i64,
        timezone: &str,
    ) -> TrafficSample<'a> {
        TrafficSample {
            rx_counter_bytes: rx,
            tx_counter_bytes: tx,
            boot_id: boot,
            day_start_utc: time::day_start_utc(timestamp, timezone).expect("day start"),
            billing_cycle: time::billing_cycle(timestamp, timezone, 1).expect("billing cycle"),
        }
    }

    #[test]
    fn first_report_only_establishes_baseline() {
        let traffic = state();
        traffic
            .update(1, sample(900_000, 800_000, "boot-a"))
            .expect("update");
        let current = traffic.get(1).expect("node state");
        assert_eq!((current.rx_total_bytes, current.tx_total_bytes), (0, 0));
        assert_eq!(current.last_rx_counter_bytes, Some(900_000));
    }

    #[test]
    fn same_boot_boot_change_and_independent_resets_use_expected_deltas() {
        let traffic = state();
        traffic
            .update(1, sample(1_000, 2_000, "boot-a"))
            .expect("baseline");
        traffic
            .update(1, sample(1_300, 2_700, "boot-a"))
            .expect("normal delta");
        assert_eq!(
            (
                traffic.get(1).unwrap().rx_total_bytes,
                traffic.get(1).unwrap().tx_total_bytes
            ),
            (300, 700)
        );

        traffic
            .update(1, sample(100, 3_000, "boot-a"))
            .expect("RX reset");
        assert_eq!(
            (
                traffic.get(1).unwrap().rx_total_bytes,
                traffic.get(1).unwrap().tx_total_bytes
            ),
            (400, 1_000)
        );

        traffic
            .update(1, sample(200, 50, "boot-b"))
            .expect("boot change");
        assert_eq!(
            (
                traffic.get(1).unwrap().rx_total_bytes,
                traffic.get(1).unwrap().tx_total_bytes
            ),
            (600, 1_050)
        );
    }

    #[test]
    fn raw_counters_past_the_js_safe_range_still_produce_exact_deltas() {
        // Raw counters are i64 all the way to SQLite, but everything the browser
        // reads is accumulated from deltas and must stay JS-safe.
        let traffic = state();
        let baseline = JS_SAFE_INTEGER_MAX + 100;
        traffic
            .update(1, sample(baseline, baseline, "boot-a"))
            .expect("baseline past the JS safe range");
        let established = traffic.get(1).unwrap();
        assert_eq!(established.last_rx_counter_bytes, Some(baseline));
        assert_eq!(
            (established.rx_total_bytes, established.tx_total_bytes),
            (0, 0),
            "the first sample must never invent traffic"
        );

        traffic
            .update(1, sample(baseline + 1_000, baseline + 250, "boot-a"))
            .expect("same-boot monotonic delta past the JS safe range");
        let current = traffic.get(1).unwrap();
        assert_eq!(
            (current.rx_total_bytes, current.tx_total_bytes),
            (1_000, 250)
        );
        assert_eq!(
            (current.today_rx_bytes, current.cycle_rx_bytes),
            (1_000, 1_000)
        );
        assert_eq!(current.last_rx_counter_bytes, Some(baseline + 1_000));
        for accumulated in [
            current.rx_total_bytes,
            current.tx_total_bytes,
            current.today_rx_bytes,
            current.today_tx_bytes,
            current.cycle_rx_bytes,
            current.cycle_tx_bytes,
        ] {
            assert!(accumulated <= JS_SAFE_INTEGER_MAX);
        }

        // A reboot resets the interface counter; the delta is the new counter,
        // not the difference against the huge previous baseline.
        traffic
            .update(1, sample(4_096, 2_048, "boot-b"))
            .expect("boot change after a huge counter");
        let rebooted = traffic.get(1).unwrap();
        assert_eq!(
            (rebooted.rx_total_bytes, rebooted.tx_total_bytes),
            (1_000 + 4_096, 250 + 2_048)
        );
    }

    #[test]
    fn i64_overflow_leaves_entire_state_unchanged() {
        let traffic = TrafficState::from_recovery(vec![TrafficRecoveryRow {
            node_id: 1,
            rx_total_bytes: i64::MAX,
            tx_total_bytes: 20,
            last_rx_counter_bytes: Some(100),
            last_tx_counter_bytes: Some(100),
            last_boot_id: Some("boot-a".into()),
            day_start_utc: 1_000,
            today_rx_bytes: 10,
            today_tx_bytes: 10,
            cycle_start_utc: 500,
            cycle_end_utc: 2_000,
            cycle_rx_bytes: 10,
            cycle_tx_bytes: 10,
        }]);
        let before = traffic.get(1).unwrap();
        assert_eq!(
            traffic.update(1, sample(101, 200, "boot-a")),
            Err(TrafficError::IntegerOverflow)
        );
        assert_eq!(traffic.get(1).unwrap(), before);
    }

    #[test]
    fn internal_totals_continue_past_the_browser_safe_boundary() {
        let traffic = state();
        let near_limit = JS_SAFE_INTEGER_MAX - 100;
        traffic
            .update(1, sample(100, 100, "boot-a"))
            .expect("baseline");
        traffic
            .update(1, sample(near_limit, near_limit, "boot-a"))
            .expect("reach just below the browser boundary");
        traffic
            .update(1, sample(near_limit + 1_000, near_limit + 1_000, "boot-a"))
            .expect("cross the browser boundary");

        let current = traffic.get(1).expect("node state");
        assert_eq!(current.rx_total_bytes, near_limit + 1_000 - 100);
        assert_eq!(current.tx_total_bytes, near_limit + 1_000 - 100);
        assert!(current.rx_total_bytes > JS_SAFE_INTEGER_MAX);
        assert!(current.tx_total_bytes > JS_SAFE_INTEGER_MAX);
    }

    #[test]
    fn day_and_cycle_changes_put_only_new_delta_in_new_buckets() {
        let traffic = state();
        traffic
            .update(1, sample(100, 200, "boot-a"))
            .expect("baseline");
        traffic
            .update(1, sample(150, 260, "boot-a"))
            .expect("old bucket delta");
        let mut next = sample(180, 300, "boot-a");
        next.day_start_utc = 2_000;
        next.billing_cycle = BillingCycle {
            start_utc: 2_000,
            end_utc: 3_000,
        };
        traffic.update(1, next).expect("new bucket delta");
        let current = traffic.get(1).unwrap();
        assert_eq!((current.rx_total_bytes, current.tx_total_bytes), (80, 100));
        assert_eq!((current.today_rx_bytes, current.today_tx_bytes), (30, 40));
        assert_eq!((current.cycle_rx_bytes, current.cycle_tx_bytes), (30, 40));
    }

    #[test]
    fn deltas_follow_shanghai_and_new_york_natural_day_boundaries() {
        let shanghai = state();
        shanghai
            .update(
                1,
                sample_at(
                    100,
                    100,
                    "boot",
                    second("2026-09-06T15:59:59Z"),
                    "Asia/Shanghai",
                ),
            )
            .expect("Shanghai baseline");
        shanghai
            .update(
                1,
                sample_at(
                    140,
                    160,
                    "boot",
                    second("2026-09-06T16:00:00Z"),
                    "Asia/Shanghai",
                ),
            )
            .expect("Shanghai next day");
        assert_eq!(
            (
                shanghai.get(1).unwrap().today_rx_bytes,
                shanghai.get(1).unwrap().today_tx_bytes
            ),
            (40, 60)
        );

        let new_york = state();
        new_york
            .update(
                1,
                sample_at(
                    200,
                    300,
                    "boot",
                    second("2026-03-08T04:59:59Z"),
                    "America/New_York",
                ),
            )
            .expect("New York baseline");
        new_york
            .update(
                1,
                sample_at(
                    225,
                    335,
                    "boot",
                    second("2026-03-08T05:00:00Z"),
                    "America/New_York",
                ),
            )
            .expect("New York DST day");
        assert_eq!(
            (
                new_york.get(1).unwrap().today_rx_bytes,
                new_york.get(1).unwrap().today_tx_bytes
            ),
            (25, 35)
        );
    }

    #[tokio::test]
    async fn checkpoint_is_absolute_idempotent_and_persists_current_state() {
        let context = TestContext::new("checkpoint").await;
        let now = crate::auth::unix_timestamp().expect("clock");
        context.publish(now, 100, 200, "boot-a", 11, "first").await;
        assert_eq!(
            checkpoint_once(&context.state)
                .await
                .expect("baseline checkpoint"),
            1
        );
        context
            .publish(now + 1, 150, 275, "boot-a", 11, "second")
            .await;
        assert_eq!(
            checkpoint_once(&context.state)
                .await
                .expect("delta checkpoint"),
            1
        );
        assert_eq!(
            checkpoint_once(&context.state)
                .await
                .expect("clean checkpoint"),
            0
        );

        let connection = Connection::open(&context.path).expect("inspect checkpoint database");
        let totals: (i64, i64, i64, i64, String) = connection
            .query_row(
                "SELECT rx_total_bytes, tx_total_bytes, last_rx_counter_bytes,
                        last_tx_counter_bytes, last_boot_id
                 FROM traffic_totals WHERE node_id = ?1",
                [context.node_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("traffic totals");
        assert_eq!(totals, (50, 75, 150, 275, "boot-a".into()));
        let day: (i64, i64) = connection
            .query_row(
                "SELECT rx_bytes, tx_bytes FROM traffic_daily WHERE node_id = ?1",
                [context.node_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("daily traffic");
        assert_eq!(day, (50, 75));
        let cycle: (i64, i64) = connection
            .query_row(
                "SELECT rx_bytes, tx_bytes FROM traffic_cycles WHERE node_id = ?1",
                [context.node_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("cycle traffic");
        assert_eq!(cycle, (50, 75));
        let last_state: (String, i64, i64) = connection
            .query_row(
                "SELECT hostname, cpu_usage_bp, load_1_milli
                 FROM node_last_state WHERE node_id = ?1",
                [context.node_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("node last state");
        assert_eq!(last_state, ("second".into(), 125, 500));
        let first_seen: i64 = connection
            .query_row(
                "SELECT first_seen_at FROM nodes WHERE id = ?1",
                [context.node_id],
                |row| row.get(0),
            )
            .expect("first seen");
        assert_eq!(first_seen, 11);
        drop(connection);

        context
            .publish(now + 2, 160, 285, "boot-a", 99, "third")
            .await;
        checkpoint_once(&context.state)
            .await
            .expect("third checkpoint");
        let connection = Connection::open(&context.path).expect("inspect first seen again");
        let first_seen: i64 = connection
            .query_row(
                "SELECT first_seen_at FROM nodes WHERE id = ?1",
                [context.node_id],
                |row| row.get(0),
            )
            .expect("first seen remains");
        assert_eq!(first_seen, 11);
        drop(connection);
        context.finish().await;
    }

    #[tokio::test]
    async fn boundary_checkpoint_preserves_previous_and_current_absolute_buckets() {
        let context = TestContext::new("boundary-checkpoint").await;
        let before = second("2026-08-31T15:59:00Z");
        context
            .publish(before, 100, 200, "boot", before, "before")
            .await;
        context
            .publish(before + 30, 150, 260, "boot", before, "before")
            .await;
        context
            .publish(before + 60, 180, 300, "boot", before, "after")
            .await;
        checkpoint_once(&context.state)
            .await
            .expect("checkpoint boundary buckets");

        let connection = Connection::open(&context.path).expect("inspect boundary buckets");
        let daily: Vec<(i64, i64)> = connection
            .prepare("SELECT rx_bytes, tx_bytes FROM traffic_daily WHERE node_id = ?1 ORDER BY day_start_utc")
            .expect("prepare daily query")
            .query_map([context.node_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query daily rows")
            .collect::<Result<_, _>>()
            .expect("read daily rows");
        assert_eq!(daily, vec![(50, 60), (30, 40)]);
        let cycles: Vec<(i64, i64)> = connection
            .prepare("SELECT rx_bytes, tx_bytes FROM traffic_cycles WHERE node_id = ?1 ORDER BY cycle_start_utc")
            .expect("prepare cycle query")
            .query_map([context.node_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query cycle rows")
            .collect::<Result<_, _>>()
            .expect("read cycle rows");
        assert_eq!(cycles, vec![(50, 60), (30, 40)]);
        drop(connection);
        context.finish().await;
    }

    #[tokio::test]
    async fn restart_recovers_baseline_and_replays_unflushed_interval_once() {
        let context = TestContext::new("crash-recovery").await;
        let now = crate::auth::unix_timestamp().expect("clock");
        context
            .publish(now, 1_000, 2_000, "boot-a", now, "baseline")
            .await;
        checkpoint_once(&context.state)
            .await
            .expect("persist baseline");
        context
            .publish(now + 1, 1_500, 2_600, "boot-a", now, "unflushed")
            .await;
        context
            .state
            .database
            .clone()
            .shutdown()
            .await
            .expect("simulate crash");

        let database = Database::open(&context.path).expect("restart database");
        let hydration = hydrate_startup(&database).await.expect("restart hydration");
        let restarted = AppState::new(database, hydration);
        let day = time::day_start_utc(now + 2, "Asia/Shanghai").expect("day");
        let cycle = time::billing_cycle(now + 2, "Asia/Shanghai", 1).expect("cycle");
        restarted
            .traffic
            .update(
                context.node_id,
                TrafficSample {
                    rx_counter_bytes: 1_700,
                    tx_counter_bytes: 2_900,
                    boot_id: "boot-a",
                    day_start_utc: day,
                    billing_cycle: cycle,
                },
            )
            .expect("replay unflushed counters");
        let recovered = restarted
            .traffic
            .get(context.node_id)
            .expect("recovered node");
        assert_eq!(
            (recovered.rx_total_bytes, recovered.tx_total_bytes),
            (700, 900)
        );
        restarted
            .database
            .shutdown()
            .await
            .expect("shutdown restarted database");
        remove_database_files(&context.path);
    }

    #[tokio::test]
    async fn startup_recovers_current_buckets_but_rejects_changed_cycle_key() {
        let context = TestContext::new("bucket-recovery").await;
        let now = crate::auth::unix_timestamp().expect("clock");
        context
            .publish(now, 1_000, 2_000, "boot", now, "baseline")
            .await;
        context
            .publish(now + 1, 1_125, 2_250, "boot", now, "delta")
            .await;
        checkpoint_once(&context.state)
            .await
            .expect("persist current buckets");
        context
            .state
            .database
            .clone()
            .shutdown()
            .await
            .expect("stop before recovery");

        let database = Database::open(&context.path).expect("restart database");
        let hydration = hydrate_startup(&database)
            .await
            .expect("hydrate current buckets");
        let recovered = AppState::new(database, hydration);
        let traffic = recovered
            .traffic
            .get(context.node_id)
            .expect("recovered traffic");
        assert_eq!((traffic.rx_total_bytes, traffic.tx_total_bytes), (125, 250));
        assert_eq!((traffic.today_rx_bytes, traffic.today_tx_bytes), (125, 250));
        assert_eq!((traffic.cycle_rx_bytes, traffic.cycle_tx_bytes), (125, 250));
        assert!(matches!(
            recovered
                .database
                .update_node(
                    context.public_id.clone(),
                    NodePatchRow {
                        traffic_reset_day: Some(15),
                        ..NodePatchRow::default()
                    },
                    now + 2,
                )
                .await
                .expect("change reset day"),
            UpdateNodeResult::Updated(_)
        ));
        recovered
            .database
            .clone()
            .shutdown()
            .await
            .expect("stop after reset change");

        let database = Database::open(&context.path).expect("restart with new reset day");
        let hydration = hydrate_startup(&database)
            .await
            .expect("hydrate changed cycle");
        let changed = AppState::new(database, hydration);
        let traffic = changed
            .traffic
            .get(context.node_id)
            .expect("traffic after reset change");
        assert_eq!((traffic.rx_total_bytes, traffic.tx_total_bytes), (125, 250));
        assert_eq!((traffic.today_rx_bytes, traffic.today_tx_bytes), (125, 250));
        assert_eq!((traffic.cycle_rx_bytes, traffic.cycle_tx_bytes), (0, 0));
        changed
            .database
            .shutdown()
            .await
            .expect("shutdown database");
        remove_database_files(&context.path);
    }

    #[tokio::test]
    async fn failed_checkpoint_retains_dirty_generation() {
        let context = TestContext::new("checkpoint-failure").await;
        let now = crate::auth::unix_timestamp().expect("clock");
        context.publish(now, 10, 20, "boot", now, "node").await;
        context
            .state
            .database
            .clone()
            .shutdown()
            .await
            .expect("stop database worker");
        assert!(checkpoint_once(&context.state).await.is_err());
        let traffic = context
            .state
            .traffic
            .get(context.node_id)
            .expect("traffic state");
        assert_eq!(traffic.generation, 1);
        assert_eq!(traffic.persisted_generation, 0);
        remove_database_files(&context.path);
    }

    #[tokio::test]
    async fn older_checkpoint_ack_keeps_new_generation_dirty() {
        let context = TestContext::new("generation-race").await;
        let now = crate::auth::unix_timestamp().expect("clock");
        context.publish(now, 10, 20, "boot", now, "first").await;
        let captured = {
            let snapshots = context.state.snapshots.read().await;
            context.state.traffic.capture_dirty(&snapshots)
        };
        context
            .publish(now + 1, 30, 50, "boot", now, "second")
            .await;
        let acknowledgements: Vec<_> = captured
            .iter()
            .map(|row| TrafficCheckpointAck {
                node_id: row.node_id,
                captured_generation: row.captured_generation,
                previous_day: row.previous_day,
                previous_cycle: row.previous_cycle,
            })
            .collect();
        context
            .state
            .database
            .persist_traffic_batch(captured, now + 1)
            .await
            .expect("persist captured generation");
        context.state.traffic.mark_persisted(&acknowledgements);
        let traffic = context
            .state
            .traffic
            .get(context.node_id)
            .expect("traffic state");
        assert_eq!((traffic.generation, traffic.persisted_generation), (2, 1));
        assert_eq!(
            checkpoint_once(&context.state)
                .await
                .expect("flush newest generation"),
            1
        );
        let traffic = context
            .state
            .traffic
            .get(context.node_id)
            .expect("traffic state");
        assert_eq!(traffic.generation, traffic.persisted_generation);
        context.finish().await;
    }

    #[tokio::test]
    async fn checkpoint_and_restart_preserve_full_i64_traffic_precision() {
        let context = TestContext::new("i64-roundtrip").await;
        let baseline = JS_SAFE_INTEGER_MAX + 123_456;
        let now = crate::auth::unix_timestamp().expect("clock");
        context.publish(now, 0, 0, "boot", now, "baseline").await;
        context
            .publish(now + 1, baseline, baseline, "boot", now, "large")
            .await;
        assert_eq!(
            checkpoint_once(&context.state).await.expect("checkpoint"),
            1
        );

        let connection = Connection::open(&context.path).expect("inspect checkpoint");
        let persisted: (i64, i64, i64, i64, i64, i64) = connection
            .query_row(
                "SELECT t.rx_total_bytes, t.tx_total_bytes, d.rx_bytes, d.tx_bytes,
                        c.rx_bytes, c.tx_bytes
                 FROM traffic_totals AS t
                 JOIN traffic_daily AS d ON d.node_id = t.node_id
                 JOIN traffic_cycles AS c ON c.node_id = t.node_id
                 WHERE t.node_id = ?1",
                [context.node_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .expect("full precision checkpoint");
        assert_eq!(
            persisted,
            (baseline, baseline, baseline, baseline, baseline, baseline)
        );
        drop(connection);

        context
            .state
            .database
            .clone()
            .shutdown()
            .await
            .expect("shutdown first database");
        let database = Database::open(&context.path).expect("reopen database");
        let hydration = hydrate_startup(&database)
            .await
            .expect("hydrate full precision");
        let recovered = &hydration.traffic_recovery[0];
        assert_eq!(recovered.rx_total_bytes, baseline);
        assert_eq!(recovered.tx_total_bytes, baseline);
        assert_eq!(recovered.today_rx_bytes, baseline);
        assert_eq!(recovered.today_tx_bytes, baseline);
        assert_eq!(recovered.cycle_rx_bytes, baseline);
        assert_eq!(recovered.cycle_tx_bytes, baseline);
        database
            .shutdown()
            .await
            .expect("shutdown reopened database");
        remove_database_files(&context.path);
    }

    #[tokio::test]
    async fn stale_checkpoint_after_node_delete_is_a_safe_noop() {
        let context = TestContext::new("stale-delete").await;
        let now = crate::auth::unix_timestamp().expect("clock");
        context.publish(now, 10, 20, "boot", now, "node").await;
        let captured = {
            let snapshots = context.state.snapshots.read().await;
            context.state.traffic.capture_dirty(&snapshots)
        };
        context
            .state
            .database
            .delete_node(context.public_id.clone(), now)
            .await
            .expect("delete node")
            .expect("deleted row");
        context.state.traffic.remove(context.node_id);
        context
            .state
            .database
            .persist_traffic_batch(captured, now)
            .await
            .expect("stale batch is ignored");
        let connection = Connection::open(&context.path).expect("inspect deleted node");
        for table in [
            "nodes",
            "traffic_totals",
            "traffic_daily",
            "traffic_cycles",
            "node_last_state",
        ] {
            let count: i64 = connection
                .query_row(
                    &format!(
                        "SELECT count(*) FROM {table} WHERE {} = ?1",
                        if table == "nodes" { "id" } else { "node_id" }
                    ),
                    [context.node_id],
                    |row| row.get(0),
                )
                .expect("count stale rows");
            assert_eq!(count, 0, "{table}");
        }
        drop(connection);
        context.finish().await;
    }

    fn second(value: &str) -> i64 {
        value
            .parse::<jiff::Timestamp>()
            .expect("valid timestamp")
            .as_second()
    }

    struct TestContext {
        state: AppState,
        path: PathBuf,
        node_id: i64,
        public_id: String,
    }

    impl TestContext {
        async fn new(label: &str) -> Self {
            let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "monitor-traffic-{label}-{}-{id}.db",
                std::process::id()
            ));
            remove_database_files(&path);
            let database = Database::open(&path).expect("open traffic test database");
            let public_id = format!("{id:032x}");
            let node = database
                .create_node(
                    NewNodeRow {
                        public_id: public_id.clone(),
                        name: "Traffic node".into(),
                        region_code: "US".into(),
                        traffic_limit_bytes: None,
                        traffic_reset_day: 1,
                        price_micros: None,
                        currency: None,
                        renewal_cycle: None,
                        expires_at: None,
                    },
                    [7; 32],
                    1,
                )
                .await
                .expect("create traffic test node");
            let hydration = hydrate_startup(&database)
                .await
                .expect("hydrate traffic test");
            Self {
                state: AppState::new(database, hydration),
                path,
                node_id: node.id,
                public_id,
            }
        }

        async fn publish(
            &self,
            timestamp: i64,
            rx: i64,
            tx: i64,
            boot: &str,
            first_seen_at: i64,
            hostname: &str,
        ) {
            let day_start_utc = time::day_start_utc(timestamp, "Asia/Shanghai").expect("day");
            let billing_cycle = time::billing_cycle(timestamp, "Asia/Shanghai", 1).expect("cycle");
            self.state
                .traffic
                .update(
                    self.node_id,
                    TrafficSample {
                        rx_counter_bytes: rx,
                        tx_counter_bytes: tx,
                        boot_id: boot,
                        day_start_utc,
                        billing_cycle,
                    },
                )
                .expect("traffic update");
            self.state.snapshots.write().await.insert(
                self.node_id,
                NodeSnapshot {
                    live_since_start: true,
                    first_seen_at,
                    last_seen_at: timestamp,
                    last_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
                    hostname: hostname.into(),
                    os_name: "Debian".into(),
                    os_version: "13".into(),
                    kernel: "6.12".into(),
                    architecture: "x86_64".into(),
                    virtualization: "qemu".into(),
                    agent_version: "0.1.0".into(),
                    cpu_model: "Test CPU".into(),
                    cpu_cores: 1,
                    cpu_usage: 1.25,
                    load_1: 0.5,
                    load_5: 0.25,
                    load_15: 0.125,
                    memory_total: 1_000,
                    memory_used: 500,
                    swap_total: 100,
                    swap_used: 10,
                    disk_total: 10_000,
                    disk_used: 1_000,
                    rx_counter_bytes: rx,
                    tx_counter_bytes: tx,
                    rx_rate_bytes_per_sec: 12,
                    tx_rate_bytes_per_sec: 13,
                    uptime_seconds: 100,
                    process_count: 10,
                    boot_id: boot.into(),
                },
            );
        }

        async fn finish(self) {
            self.state
                .database
                .shutdown()
                .await
                .expect("shutdown database");
            remove_database_files(&self.path);
        }
    }

    fn remove_database_files(path: &Path) {
        for suffix in ["", "-shm", "-wal"] {
            let candidate = PathBuf::from(format!("{}{}", path.display(), suffix));
            match fs::remove_file(candidate) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => panic!("remove test database: {error}"),
            }
        }
    }
}
