pub mod admin_cli;
pub mod app;
pub mod auth;
pub mod config;
pub mod database;
pub mod history;
pub mod http;
mod maintenance;
mod ping_target;
pub mod public_snapshot;
pub mod snapshot;
mod static_files;
pub mod time;
pub mod traffic;

use std::io;

use app::AppState;
use config::Config;
use database::{Database, hydrate_startup};
use tokio::task::JoinSet;

#[derive(Debug)]
pub enum ServerError {
    Database(database::DatabaseError),
    PublicSnapshot(public_snapshot::PublicSnapshotError),
    Io {
        operation: &'static str,
        source: io::Error,
    },
    Shutdown(Vec<String>),
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "database startup failed: {error}"),
            Self::PublicSnapshot(error) => {
                write!(
                    formatter,
                    "initial public snapshot generation failed: {error}"
                )
            }
            Self::Io { operation, source } => write!(formatter, "{operation}: {source}"),
            Self::Shutdown(errors) => {
                write!(formatter, "server shutdown failed: {}", errors.join("; "))
            }
        }
    }
}

impl std::error::Error for ServerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            Self::PublicSnapshot(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            Self::Shutdown(_) => None,
        }
    }
}

impl From<database::DatabaseError> for ServerError {
    fn from(value: database::DatabaseError) -> Self {
        Self::Database(value)
    }
}

impl From<public_snapshot::PublicSnapshotError> for ServerError {
    fn from(value: public_snapshot::PublicSnapshotError) -> Self {
        Self::PublicSnapshot(value)
    }
}

pub async fn run(config: Config) -> Result<(), ServerError> {
    let database = Database::open(&config.database_path)?;
    let hydration = hydrate_startup(&database).await?;
    let state = AppState::new(database, hydration);
    public_snapshot::generate_now(&state).await?;

    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .map_err(|source| ServerError::Io {
            operation: "failed to bind HTTP listener",
            source,
        })?;

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|source| ServerError::Io {
            operation: "failed to install SIGTERM handler",
            source,
        })?;
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|source| ServerError::Io {
            operation: "failed to install SIGINT handler",
            source,
        })?;

    let mut workers = JoinSet::new();
    workers.spawn(public_snapshot::run_worker(state.clone()));
    workers.spawn(traffic::run_checkpoint_worker(state.clone()));
    workers.spawn(history::run_minute_worker(state.clone()));
    workers.spawn(maintenance::run_worker(state.clone()));

    tracing::info!(listen = %config.listen, db = %config.database_path.display(), "monitor-server started");

    let router = http::router(state.clone());
    let serve_result = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        tokio::select! {
            _ = sigterm.recv() => tracing::info!("SIGTERM received; starting graceful shutdown"),
            _ = sigint.recv() => tracing::info!("SIGINT received; starting graceful shutdown"),
        }
    })
    .await;

    workers.abort_all();
    let mut worker_errors = Vec::new();
    while let Some(result) = workers.join_next().await {
        if let Err(error) = result
            && !error.is_cancelled()
        {
            worker_errors.push(format!("background worker failed: {error}"));
        }
    }

    let shutdown_result = finalize_server(state).await;
    let mut errors = worker_errors;
    if let Err(source) = serve_result {
        errors.push(format!("HTTP server failed: {source}"));
    }
    if let Err(error) = shutdown_result {
        match error {
            ServerError::Shutdown(shutdown_errors) => errors.extend(shutdown_errors),
            other => errors.push(other.to_string()),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(ServerError::Shutdown(errors))
    }
}

async fn finalize_server(state: AppState) -> Result<(), ServerError> {
    let mut errors = Vec::new();

    if let Err(error) = traffic::checkpoint_once(&state).await {
        errors.push(format!("final traffic flush failed: {error}"));
    }
    if let Err(error) = history::flush_pending(&state).await {
        errors.push(format!("final pending history flush failed: {error}"));
    }
    state.history.finalize_for_shutdown();
    if let Err(error) = history::flush_pending(&state).await {
        errors.push(format!(
            "final partial-minute history flush failed: {error}"
        ));
    }
    if let Err(error) = state.database.passive_wal_checkpoint().await {
        errors.push(format!("final passive WAL checkpoint failed: {error}"));
    }
    if let Err(error) = state.database.shutdown().await {
        errors.push(format!("database shutdown failed: {error}"));
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(ServerError::Shutdown(errors))
    }
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
        database::NewNodeRow, history::ResourceSample, snapshot::NodeSnapshot,
        traffic::TrafficSample,
    };

    static TEST_ID: AtomicU64 = AtomicU64::new(0);

    #[tokio::test]
    async fn final_shutdown_persists_traffic_last_state_and_partial_history() {
        let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("monitor-finalize-{}-{id}.db", std::process::id()));
        remove_database_files(&path);
        let database = Database::open(&path).expect("open database");
        let node = database
            .create_node(
                NewNodeRow {
                    public_id: format!("{id:032x}"),
                    name: "Shutdown node".into(),
                    region_code: "US".into(),
                    traffic_limit_bytes: None,
                    traffic_reset_day: 1,
                    price_micros: None,
                    currency: None,
                    renewal_cycle: None,
                    expires_at: None,
                },
                [9; 32],
                1,
            )
            .await
            .expect("create node");
        let hydration = hydrate_startup(&database).await.expect("hydrate startup");
        let state = AppState::new(database, hydration);
        let timestamp = 1_800_000_000;
        let day_start_utc = time::day_start_utc(timestamp, "Asia/Shanghai").expect("day start");
        let billing_cycle =
            time::billing_cycle(timestamp, "Asia/Shanghai", 1).expect("billing cycle");
        state
            .traffic
            .update(
                node.id,
                TrafficSample {
                    rx_counter_bytes: 1_000,
                    tx_counter_bytes: 2_000,
                    boot_id: "boot-a",
                    day_start_utc,
                    billing_cycle,
                },
            )
            .expect("establish baseline");
        state
            .traffic
            .update(
                node.id,
                TrafficSample {
                    rx_counter_bytes: 1_150,
                    tx_counter_bytes: 2_250,
                    boot_id: "boot-a",
                    day_start_utc,
                    billing_cycle,
                },
            )
            .expect("update traffic");
        state.snapshots.write().await.insert(
            node.id,
            NodeSnapshot {
                live_since_start: true,
                first_seen_at: timestamp,
                last_seen_at: timestamp,
                last_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
                hostname: "shutdown-node".into(),
                os_name: "Linux".into(),
                os_version: "1".into(),
                kernel: "6.0".into(),
                architecture: "x86_64".into(),
                virtualization: "qemu".into(),
                agent_version: "0.1.0".into(),
                cpu_model: "Test CPU".into(),
                cpu_cores: 1,
                cpu_usage: 12.5,
                load_1: 0.1,
                load_5: 0.2,
                load_15: 0.3,
                memory_total: 1_024,
                memory_used: 512,
                swap_total: 0,
                swap_used: 0,
                disk_total: 2_048,
                disk_used: 1_024,
                rx_counter_bytes: 1_150,
                tx_counter_bytes: 2_250,
                rx_rate_bytes_per_sec: 10,
                tx_rate_bytes_per_sec: 20,
                uptime_seconds: 60,
                process_count: 2,
                boot_id: "boot-a".into(),
            },
        );
        let history_sample = ResourceSample {
            cpu_usage: 12.5,
            load_1: 0.1,
            load_5: 0.2,
            load_15: 0.3,
            memory_used_bytes: 512,
            swap_used_bytes: 0,
            disk_used_bytes: 1_024,
            rx_rate_bytes_per_sec: 10,
            tx_rate_bytes_per_sec: 20,
        };
        state
            .history
            .record(node.id, timestamp - 60, history_sample, []);
        state.history.record(node.id, timestamp, history_sample, []);

        finalize_server(state).await.expect("finalize server");

        let connection = Connection::open(&path).expect("inspect finalized database");
        let totals: (i64, i64) = connection
            .query_row(
                "SELECT rx_total_bytes, tx_total_bytes FROM traffic_totals WHERE node_id = ?1",
                [node.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read totals");
        assert_eq!(totals, (150, 250));
        let last_states: i64 = connection
            .query_row(
                "SELECT count(*) FROM node_last_state WHERE node_id = ?1",
                [node.id],
                |row| row.get(0),
            )
            .expect("count last state");
        assert_eq!(last_states, 1);
        let history_rows: i64 = connection
            .query_row(
                "SELECT count(*) FROM node_history WHERE node_id = ?1",
                [node.id],
                |row| row.get(0),
            )
            .expect("count history");
        assert_eq!(history_rows, 2);
        drop(connection);

        Database::open(&path)
            .expect("reopen finalized database")
            .shutdown()
            .await
            .expect("close reopened database");
        remove_database_files(&path);
    }

    fn remove_database_files(path: &Path) {
        for suffix in ["", "-shm", "-wal"] {
            let file = PathBuf::from(format!("{}{}", path.display(), suffix));
            if let Err(error) = fs::remove_file(file)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                panic!("remove test database: {error}");
            }
        }
    }
}
