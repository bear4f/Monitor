use std::time::Duration;

use crate::{
    app::AppState,
    auth::unix_timestamp,
    database::{MaintenanceCleanupResult, WalCheckpointResult},
    history, time,
};

const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(60 * 60);
const CLEANUP_BATCH_ROWS: usize = 5_000;
const MAX_CLEANUP_BATCHES: usize = 16;

#[derive(Debug, Default)]
pub struct MaintenanceReport {
    pub history_rows: usize,
    pub cleanup: MaintenanceCleanupResult,
    pub wal_checkpoint: Option<WalCheckpointResult>,
}

#[derive(Debug)]
pub struct MaintenanceError(Vec<String>);

impl std::fmt::Display for MaintenanceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0.join("; "))
    }
}

impl std::error::Error for MaintenanceError {}

async fn run_at(state: &AppState, now: i64) -> Result<MaintenanceReport, MaintenanceError> {
    let mut report = MaintenanceReport::default();
    let mut errors = Vec::new();

    match history::cleanup_at(state, now).await {
        Ok(deleted) => report.history_rows = deleted,
        Err(error) => errors.push(format!("history cleanup failed: {error}")),
    }

    let timezone = state.settings.read().await.site_timezone.clone();
    match time::previous_day_start_utc(now, &timezone) {
        Ok(previous_day_start_utc) => {
            for batch_index in 0..MAX_CLEANUP_BATCHES {
                match state
                    .database
                    .cleanup_maintenance_batch(now, previous_day_start_utc, CLEANUP_BATCH_ROWS)
                    .await
                {
                    Ok(cleanup) => {
                        report.cleanup.expired_sessions += cleanup.expired_sessions;
                        report.cleanup.traffic_daily += cleanup.traffic_daily;
                        report.cleanup.traffic_cycles += cleanup.traffic_cycles;
                        if cleanup.total() < CLEANUP_BATCH_ROWS {
                            break;
                        }
                        if batch_index + 1 == MAX_CLEANUP_BATCHES {
                            tracing::warn!(
                                batch_rows = CLEANUP_BATCH_ROWS,
                                max_batches = MAX_CLEANUP_BATCHES,
                                "database housekeeping reached hourly work bound; remaining rows deferred"
                            );
                        }
                    }
                    Err(error) => {
                        errors.push(format!("database housekeeping failed: {error}"));
                        break;
                    }
                }
            }
        }
        Err(error) => errors.push(format!("calculate previous site day failed: {error}")),
    }

    match state.database.passive_wal_checkpoint().await {
        Ok(checkpoint) => report.wal_checkpoint = Some(checkpoint),
        Err(error) => errors.push(format!("passive WAL checkpoint failed: {error}")),
    }

    if errors.is_empty() {
        Ok(report)
    } else {
        Err(MaintenanceError(errors))
    }
}

pub(crate) async fn run_worker(state: AppState) {
    let mut interval = tokio::time::interval(MAINTENANCE_INTERVAL);
    interval.tick().await;
    loop {
        interval.tick().await;
        let now = match unix_timestamp() {
            Ok(now) => now,
            Err(error) => {
                tracing::error!(error = %error, "maintenance clock read failed");
                if let Err(error) = state.database.passive_wal_checkpoint().await {
                    tracing::error!(error = %error, "hourly passive WAL checkpoint failed");
                }
                continue;
            }
        };
        if let Err(error) = run_at(&state, now).await {
            tracing::error!(error = %error, "hourly maintenance completed with errors");
        }
    }
}
