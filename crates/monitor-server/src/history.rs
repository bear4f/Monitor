use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use crate::{
    app::AppState,
    auth::unix_timestamp,
    database::{DatabaseError, PingHistoryWriteRow, ResourceHistoryWriteRow},
};

const MINUTE_SECONDS: i64 = 60;
const MAX_PENDING_MINUTES: usize = 3;
const CLEANUP_BATCH_ROWS: usize = 5_000;
const MAX_CLEANUP_BATCHES: usize = 16;
const PING_RETENTION_SECONDS: i64 = 7 * 24 * 60 * 60;

#[derive(Clone, Default)]
pub struct HistoryAccumulator(Arc<Mutex<HistoryState>>);

#[derive(Default)]
struct HistoryState {
    current: Option<MinuteAccumulator>,
    pending: VecDeque<HistoryBatch>,
    closed_before: i64,
}

#[derive(Default)]
struct MinuteAccumulator {
    bucket_ts: i64,
    resources: HashMap<i64, ResourceAccumulator>,
    pings: HashMap<(i64, i64), PingAccumulator>,
}

#[derive(Debug, Clone)]
struct HistoryBatch {
    bucket_ts: i64,
    resources: Vec<ResourceHistoryWriteRow>,
    pings: Vec<PingHistoryWriteRow>,
}

#[derive(Debug, Clone, Copy)]
pub struct ResourceSample {
    pub cpu_usage: f64,
    pub load_1: f64,
    pub load_5: f64,
    pub load_15: f64,
    pub memory_used_bytes: i64,
    pub swap_used_bytes: i64,
    pub disk_used_bytes: i64,
    pub rx_rate_bytes_per_sec: i64,
    pub tx_rate_bytes_per_sec: i64,
}

/// One target's counters for the minute that is still open, copied out of the
/// accumulator. Values only -- the accumulator itself is never exposed, and the
/// minute is neither finalized nor mutated by reading it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurrentPingSample {
    pub bucket_ts: i64,
    pub target_id: i64,
    pub sample_count: i64,
    pub success_count: i64,
    pub latency_avg_ms: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
pub struct PingSample {
    pub target_id: i64,
    pub success: bool,
    pub latency_ms: Option<f64>,
}

#[derive(Default)]
struct ResourceAccumulator {
    sample_count: i64,
    cpu_usage: f64,
    load_1: f64,
    load_5: f64,
    load_15: f64,
    rx_rate: f64,
    tx_rate: f64,
    memory_used_bytes: i64,
    swap_used_bytes: i64,
    disk_used_bytes: i64,
}

#[derive(Default)]
struct PingAccumulator {
    sample_count: i64,
    success_count: i64,
    latency_mean: f64,
    latency_min: Option<f64>,
    latency_max: Option<f64>,
}

impl HistoryAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(
        &self,
        node_id: i64,
        received_at: i64,
        resource: ResourceSample,
        pings: impl IntoIterator<Item = PingSample>,
    ) {
        let bucket_ts = received_at - received_at.rem_euclid(MINUTE_SECONDS);
        let mut state = self.lock();
        if bucket_ts < state.closed_before {
            return;
        }

        match state.current.as_ref().map(|minute| minute.bucket_ts) {
            Some(current) if bucket_ts < current => return,
            Some(current) if bucket_ts > current => {
                state.finalize_current(bucket_ts);
            }
            None => {
                state.current = Some(MinuteAccumulator::new(bucket_ts));
            }
            Some(_) => {}
        }

        let minute = state.current.as_mut().expect("current minute exists");
        minute.resources.entry(node_id).or_default().add(resource);
        for ping in pings {
            minute
                .pings
                .entry((node_id, ping.target_id))
                .or_default()
                .add(ping);
        }
    }

    pub fn finalize_before(&self, timestamp: i64) {
        let bucket_ts = timestamp - timestamp.rem_euclid(MINUTE_SECONDS);
        let mut state = self.lock();
        if state
            .current
            .as_ref()
            .is_some_and(|current| current.bucket_ts < bucket_ts)
        {
            state.finalize_current(bucket_ts);
        }
        state.closed_before = state.closed_before.max(bucket_ts);
    }

    pub fn finalize_for_shutdown(&self) {
        let mut state = self.lock();
        if let Some(current) = state.current.take() {
            let bucket_ts = current.bucket_ts;
            state.push_pending(current.finalize());
            state.closed_before = state.closed_before.max(bucket_ts + MINUTE_SECONDS);
        }
    }

    /// Copies one node's ping counters for the still-open minute. The lock is
    /// held for the copy and nothing else: the returned values own no guard, so
    /// no caller can keep it across a database await or a response build.
    pub fn current_ping_samples(&self, node_id: i64) -> Vec<CurrentPingSample> {
        let state = self.lock();
        let Some(current) = state.current.as_ref() else {
            return Vec::new();
        };
        let bucket_ts = current.bucket_ts;
        let mut samples: Vec<CurrentPingSample> = current
            .pings
            .iter()
            .filter(|((id, _), _)| *id == node_id)
            .map(|((_, target_id), ping)| CurrentPingSample {
                bucket_ts,
                target_id: *target_id,
                sample_count: ping.sample_count,
                success_count: ping.success_count,
                // Same rule as a persisted minute: no successful sample, no
                // latency, so loss and latency describe one sample population.
                latency_avg_ms: (ping.success_count > 0).then_some(ping.latency_mean),
            })
            .collect();
        drop(state);
        samples.sort_unstable_by_key(|sample| sample.target_id);
        samples
    }

    pub fn remove_node(&self, node_id: i64) {
        let mut state = self.lock();
        if let Some(current) = &mut state.current {
            current.resources.remove(&node_id);
            current.pings.retain(|(id, _), _| *id != node_id);
        }
        for batch in &mut state.pending {
            batch.resources.retain(|row| row.node_id != node_id);
            batch.pings.retain(|row| row.node_id != node_id);
        }
    }

    pub fn remove_target(&self, target_id: i64) {
        let mut state = self.lock();
        if let Some(current) = &mut state.current {
            current.pings.retain(|(_, id), _| *id != target_id);
        }
        for batch in &mut state.pending {
            batch.pings.retain(|row| row.target_id != target_id);
        }
    }

    fn pending_snapshot(&self) -> Vec<HistoryBatch> {
        self.lock().pending.iter().cloned().collect()
    }

    fn acknowledge_through(&self, bucket_ts: i64) {
        self.lock()
            .pending
            .retain(|batch| batch.bucket_ts > bucket_ts);
    }

    fn lock(&self) -> MutexGuard<'_, HistoryState> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[cfg(test)]
    pub(crate) fn resource_samples(&self, node_id: i64) -> i64 {
        let state = self.lock();
        let current = state
            .current
            .as_ref()
            .and_then(|minute| minute.resources.get(&node_id))
            .map_or(0, |resource| resource.sample_count);
        current
            + state
                .pending
                .iter()
                .flat_map(|batch| &batch.resources)
                .filter(|row| row.node_id == node_id)
                .map(|row| row.sample_count)
                .sum::<i64>()
    }

    #[cfg(test)]
    pub(crate) fn ping_samples(&self, target_id: i64) -> i64 {
        let state = self.lock();
        let current = state.current.as_ref().map_or(0, |minute| {
            minute
                .pings
                .iter()
                .filter(|((_, id), _)| *id == target_id)
                .map(|(_, ping)| ping.sample_count)
                .sum()
        });
        current
            + state
                .pending
                .iter()
                .flat_map(|batch| &batch.pings)
                .filter(|row| row.target_id == target_id)
                .map(|row| row.sample_count)
                .sum::<i64>()
    }
}

impl HistoryState {
    fn finalize_current(&mut self, next_bucket: i64) {
        if let Some(current) = self.current.take() {
            self.push_pending(current.finalize());
        }
        self.closed_before = self.closed_before.max(next_bucket);
        self.current = Some(MinuteAccumulator::new(next_bucket));
    }

    fn push_pending(&mut self, batch: HistoryBatch) {
        if batch.resources.is_empty() && batch.pings.is_empty() {
            return;
        }
        if self.pending.len() == MAX_PENDING_MINUTES {
            self.pending.pop_front();
            tracing::warn!(
                limit = MAX_PENDING_MINUTES,
                "history persistence backlog full; oldest minute dropped"
            );
        }
        self.pending.push_back(batch);
    }
}

impl MinuteAccumulator {
    fn new(bucket_ts: i64) -> Self {
        Self {
            bucket_ts,
            ..Self::default()
        }
    }

    fn finalize(self) -> HistoryBatch {
        let resources = self
            .resources
            .into_iter()
            .map(|(node_id, row)| row.finish(node_id, self.bucket_ts))
            .collect();
        let pings = self
            .pings
            .into_iter()
            .map(|((node_id, target_id), row)| row.finish(node_id, target_id, self.bucket_ts))
            .collect();
        HistoryBatch {
            bucket_ts: self.bucket_ts,
            resources,
            pings,
        }
    }
}

impl ResourceAccumulator {
    fn add(&mut self, sample: ResourceSample) {
        let Some(next_count) = self.sample_count.checked_add(1) else {
            tracing::warn!("resource history sample counter overflow; sample dropped");
            return;
        };
        let denominator = next_count as f64;
        update_mean(&mut self.cpu_usage, sample.cpu_usage, denominator);
        update_mean(&mut self.load_1, sample.load_1, denominator);
        update_mean(&mut self.load_5, sample.load_5, denominator);
        update_mean(&mut self.load_15, sample.load_15, denominator);
        update_mean(
            &mut self.rx_rate,
            sample.rx_rate_bytes_per_sec as f64,
            denominator,
        );
        update_mean(
            &mut self.tx_rate,
            sample.tx_rate_bytes_per_sec as f64,
            denominator,
        );
        self.sample_count = next_count;
        self.memory_used_bytes = sample.memory_used_bytes;
        self.swap_used_bytes = sample.swap_used_bytes;
        self.disk_used_bytes = sample.disk_used_bytes;
    }

    fn finish(self, node_id: i64, bucket_ts: i64) -> ResourceHistoryWriteRow {
        ResourceHistoryWriteRow {
            node_id,
            bucket_ts,
            sample_count: self.sample_count,
            cpu_usage_bp: (self.cpu_usage * 100.0).round() as i64,
            load_1_milli: (self.load_1 * 1_000.0).round() as i64,
            load_5_milli: (self.load_5 * 1_000.0).round() as i64,
            load_15_milli: (self.load_15 * 1_000.0).round() as i64,
            memory_used_bytes: self.memory_used_bytes,
            swap_used_bytes: self.swap_used_bytes,
            disk_used_bytes: self.disk_used_bytes,
            rx_rate_bytes_per_sec: self.rx_rate.round() as i64,
            tx_rate_bytes_per_sec: self.tx_rate.round() as i64,
        }
    }
}

impl PingAccumulator {
    fn add(&mut self, sample: PingSample) {
        let Some(sample_count) = self.sample_count.checked_add(1) else {
            tracing::warn!("ping history sample counter overflow; sample dropped");
            return;
        };
        self.sample_count = sample_count;
        if !sample.success {
            return;
        }
        let Some(latency) = sample.latency_ms else {
            tracing::warn!("successful ping history sample lacks latency; sample dropped");
            self.sample_count -= 1;
            return;
        };
        let Some(success_count) = self.success_count.checked_add(1) else {
            tracing::warn!("ping history success counter overflow; sample dropped");
            self.sample_count -= 1;
            return;
        };
        update_mean(&mut self.latency_mean, latency, success_count as f64);
        self.success_count = success_count;
        self.latency_min = Some(self.latency_min.map_or(latency, |value| value.min(latency)));
        self.latency_max = Some(self.latency_max.map_or(latency, |value| value.max(latency)));
    }

    fn finish(self, node_id: i64, target_id: i64, bucket_ts: i64) -> PingHistoryWriteRow {
        let latency_avg_ms = (self.success_count > 0).then_some(self.latency_mean);
        PingHistoryWriteRow {
            node_id,
            bucket_ts,
            target_id,
            sample_count: self.sample_count,
            success_count: self.success_count,
            latency_avg_ms,
            latency_min_ms: self.latency_min,
            latency_max_ms: self.latency_max,
        }
    }
}

fn update_mean(mean: &mut f64, sample: f64, denominator: f64) {
    *mean += (sample - *mean) / denominator;
}

pub async fn flush_pending(state: &AppState) -> Result<(), DatabaseError> {
    let batches = state.history.pending_snapshot();
    let Some(last_bucket) = batches.last().map(|batch| batch.bucket_ts) else {
        return Ok(());
    };
    let resources = batches
        .iter()
        .flat_map(|batch| batch.resources.iter().cloned())
        .collect();
    let pings = batches
        .iter()
        .flat_map(|batch| batch.pings.iter().cloned())
        .collect();
    state
        .database
        .persist_history_batch(resources, pings)
        .await?;
    state.history.acknowledge_through(last_bucket);
    Ok(())
}

pub async fn run_minute_worker(state: AppState) {
    loop {
        let now = match unix_timestamp() {
            Ok(now) => now,
            Err(error) => {
                tracing::error!(error = %error, "history minute clock read failed");
                tokio::time::sleep(Duration::from_secs(MINUTE_SECONDS as u64)).await;
                continue;
            }
        };
        let seconds_to_boundary = MINUTE_SECONDS - now.rem_euclid(MINUTE_SECONDS);
        tokio::time::sleep(Duration::from_secs(seconds_to_boundary as u64)).await;
        let now = match unix_timestamp() {
            Ok(now) => now,
            Err(error) => {
                tracing::error!(error = %error, "history minute clock read failed");
                continue;
            }
        };
        state.history.finalize_before(now);
        if let Err(error) = flush_pending(&state).await {
            tracing::error!(error = %error, "history batch persistence failed; pending minutes retained");
        }
    }
}

pub async fn cleanup_at(state: &AppState, now: i64) -> Result<usize, DatabaseError> {
    let retention_days = state.settings.read().await.history_retention_days;
    let (resource_cutoff, ping_cutoff) = retention_cutoffs(now, retention_days);
    let mut deleted = 0;
    for _ in 0..MAX_CLEANUP_BATCHES {
        let batch = state
            .database
            .cleanup_history_batch(resource_cutoff, ping_cutoff, CLEANUP_BATCH_ROWS)
            .await?;
        deleted += batch;
        if batch < CLEANUP_BATCH_ROWS {
            return Ok(deleted);
        }
    }
    tracing::warn!(
        batch_rows = CLEANUP_BATCH_ROWS,
        max_batches = MAX_CLEANUP_BATCHES,
        "history cleanup reached hourly work bound; remaining rows deferred"
    );
    Ok(deleted)
}

fn retention_cutoffs(now: i64, resource_days: i64) -> (i64, i64) {
    (
        now - resource_days * 24 * 60 * 60,
        now - PING_RETENTION_SECONDS,
    )
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use super::*;
    use crate::database::{Database, NewNodeRow, hydrate_startup};

    fn resource(cpu: f64, memory: i64, disk: i64) -> ResourceSample {
        ResourceSample {
            cpu_usage: cpu,
            load_1: cpu / 10.0,
            load_5: cpu / 20.0,
            load_15: cpu / 40.0,
            memory_used_bytes: memory,
            swap_used_bytes: memory / 2,
            disk_used_bytes: disk,
            rx_rate_bytes_per_sec: memory,
            tx_rate_bytes_per_sec: disk,
        }
    }

    #[test]
    fn resource_and_ping_samples_aggregate_by_minute() {
        let history = HistoryAccumulator::new();
        history.record(
            7,
            120,
            resource(10.0, 100, 1_000),
            [PingSample {
                target_id: 3,
                success: true,
                latency_ms: Some(10.0),
            }],
        );
        history.record(
            7,
            179,
            resource(30.0, 200, 2_000),
            [
                PingSample {
                    target_id: 3,
                    success: true,
                    latency_ms: Some(30.0),
                },
                PingSample {
                    target_id: 4,
                    success: false,
                    latency_ms: None,
                },
            ],
        );
        history.record(
            7,
            179,
            resource(20.0, 250, 2_500),
            [PingSample {
                target_id: 3,
                success: false,
                latency_ms: None,
            }],
        );
        history.record(7, 180, resource(50.0, 300, 3_000), std::iter::empty());

        let batches = history.pending_snapshot();
        assert_eq!(batches.len(), 1);
        let resource = &batches[0].resources[0];
        assert_eq!(resource.sample_count, 3);
        assert_eq!(resource.cpu_usage_bp, 2_000);
        assert_eq!(resource.memory_used_bytes, 250);
        assert_eq!(resource.disk_used_bytes, 2_500);
        let ping = batches[0]
            .pings
            .iter()
            .find(|row| row.target_id == 3)
            .expect("target 3");
        assert_eq!(ping.sample_count, 3);
        assert_eq!(ping.success_count, 2);
        assert_eq!(ping.latency_avg_ms, Some(20.0));
        assert_eq!(ping.latency_min_ms, Some(10.0));
        assert_eq!(ping.latency_max_ms, Some(30.0));
        let failed = batches[0]
            .pings
            .iter()
            .find(|row| row.target_id == 4)
            .expect("target 4");
        assert_eq!(failed.success_count, 0);
        assert_eq!(failed.latency_avg_ms, None);
        assert_eq!(failed.latency_min_ms, None);
        assert_eq!(failed.latency_max_ms, None);
    }

    #[test]
    fn late_samples_are_dropped_and_pending_history_is_bounded() {
        let history = HistoryAccumulator::new();
        for minute in 1..=5 {
            history.record(
                1,
                minute * 60,
                resource(minute as f64, minute, minute),
                std::iter::empty(),
            );
        }
        history.record(1, 60, resource(99.0, 99, 99), std::iter::empty());

        let batches = history.pending_snapshot();
        assert_eq!(batches.len(), MAX_PENDING_MINUTES);
        assert_eq!(batches[0].bucket_ts, 120);
        assert_eq!(batches[2].bucket_ts, 240);
        assert!(
            batches
                .iter()
                .all(|batch| batch.resources[0].cpu_usage_bp != 9_900)
        );
    }

    #[test]
    fn timer_finalizes_a_minute_without_a_followup_report() {
        let history = HistoryAccumulator::new();
        history.record(1, 120, resource(10.0, 10, 10), std::iter::empty());
        history.finalize_before(180);
        assert_eq!(history.pending_snapshot().len(), 1);
    }

    #[test]
    fn removing_a_node_clears_current_and_pending_samples() {
        let history = HistoryAccumulator::new();
        history.record(1, 60, resource(10.0, 10, 10), std::iter::empty());
        history.record(2, 60, resource(20.0, 20, 20), std::iter::empty());
        history.record(1, 120, resource(30.0, 30, 30), std::iter::empty());
        history.remove_node(1);

        assert_eq!(history.resource_samples(1), 0);
        assert_eq!(history.resource_samples(2), 1);
        assert!(
            history
                .pending_snapshot()
                .iter()
                .flat_map(|batch| &batch.resources)
                .all(|row| row.node_id != 1)
        );
    }

    #[test]
    fn removing_a_target_clears_only_its_current_and_pending_ping_samples() {
        let history = HistoryAccumulator::new();
        let sample = |target_id| PingSample {
            target_id,
            success: false,
            latency_ms: None,
        };
        history.record(1, 60, resource(10.0, 10, 10), [sample(7), sample(8)]);
        history.record(1, 120, resource(20.0, 20, 20), [sample(7), sample(8)]);
        history.remove_target(7);

        assert_eq!(history.ping_samples(7), 0);
        assert_eq!(history.ping_samples(8), 2);
    }

    #[test]
    fn the_open_minute_is_readable_without_being_finalized_or_mutated() {
        let history = HistoryAccumulator::new();
        history.record(
            1,
            120,
            resource(10.0, 10, 10),
            [
                PingSample {
                    target_id: 7,
                    success: true,
                    latency_ms: Some(10.0),
                },
                PingSample {
                    target_id: 7,
                    success: false,
                    latency_ms: None,
                },
                PingSample {
                    target_id: 8,
                    success: false,
                    latency_ms: None,
                },
            ],
        );
        // Another node's samples never leak into this node's snapshot.
        history.record(
            2,
            120,
            resource(10.0, 10, 10),
            [PingSample {
                target_id: 7,
                success: false,
                latency_ms: None,
            }],
        );

        let first = history.current_ping_samples(1);
        assert_eq!(
            first,
            vec![
                CurrentPingSample {
                    bucket_ts: 120,
                    target_id: 7,
                    sample_count: 2,
                    success_count: 1,
                    latency_avg_ms: Some(10.0),
                },
                CurrentPingSample {
                    bucket_ts: 120,
                    target_id: 8,
                    sample_count: 1,
                    success_count: 0,
                    // No successful sample, so no latency: loss and latency
                    // describe the same population.
                    latency_avg_ms: None,
                },
            ]
        );

        // The snapshot owns no lock, so the accumulator stays usable while it is
        // alive -- a snapshot that carried the guard would deadlock here.
        history.remove_target(9);
        assert_eq!(first.len(), 2);

        // Reading finalized nothing and changed nothing.
        assert!(history.pending_snapshot().is_empty());
        assert_eq!(history.current_ping_samples(1), first);
        assert_eq!(history.ping_samples(7), 3);
        history.record(
            1,
            150,
            resource(10.0, 10, 10),
            [PingSample {
                target_id: 7,
                success: false,
                latency_ms: None,
            }],
        );
        let later = history.current_ping_samples(1);
        assert_eq!(later[0].bucket_ts, 120, "still the same open minute");
        assert_eq!((later[0].sample_count, later[0].success_count), (3, 1));
    }

    #[test]
    fn a_removed_target_disappears_from_the_open_minute_snapshot() {
        let history = HistoryAccumulator::new();
        let sample = |target_id| PingSample {
            target_id,
            success: false,
            latency_ms: None,
        };
        history.record(1, 60, resource(10.0, 10, 10), [sample(7), sample(8)]);
        history.remove_target(7);
        assert_eq!(
            history
                .current_ping_samples(1)
                .iter()
                .map(|sample| sample.target_id)
                .collect::<Vec<_>>(),
            vec![8]
        );
    }

    #[test]
    fn an_unreported_node_and_an_empty_accumulator_snapshot_nothing() {
        let history = HistoryAccumulator::new();
        assert!(history.current_ping_samples(1).is_empty());
        history.record(1, 60, resource(10.0, 10, 10), std::iter::empty());
        assert!(history.current_ping_samples(2).is_empty());
        assert!(history.current_ping_samples(1).is_empty());
    }

    #[test]
    fn retention_uses_configured_resource_days_and_fixed_seven_ping_days() {
        let now = 40 * 24 * 60 * 60;
        let (resource, ping) = retention_cutoffs(now, 30);
        assert_eq!(resource, 10 * 24 * 60 * 60);
        assert_eq!(ping, 33 * 24 * 60 * 60);
    }

    #[tokio::test]
    async fn database_failure_retains_bounded_pending_history() {
        let path =
            std::env::temp_dir().join(format!("monitor-history-failure-{}.db", std::process::id()));
        remove_database_files(&path);
        let database = Database::open(&path).expect("open database");
        database
            .create_node(
                NewNodeRow {
                    public_id: "f".repeat(32),
                    name: "Node".into(),
                    region_code: "US".into(),
                    traffic_limit_bytes: None,
                    traffic_reset_day: 1,
                    traffic_reset_mode: "monthly".to_owned(),
                    price_micros: None,
                    currency: None,
                    renewal_cycle: None,
                    expires_at: None,
                },
                [8; 32],
                1,
            )
            .await
            .expect("create node");
        let hydration = hydrate_startup(&database).await.expect("hydrate state");
        let state = AppState::new(database, hydration);
        state
            .history
            .record(1, 120, resource(10.0, 10, 10), std::iter::empty());
        state.history.finalize_before(180);
        state
            .database
            .clone()
            .shutdown()
            .await
            .expect("stop database worker");

        assert!(flush_pending(&state).await.is_err());
        assert_eq!(state.history.pending_snapshot().len(), 1);
        drop(state);
        remove_database_files(&path);
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
