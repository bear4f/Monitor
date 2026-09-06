use std::{collections::HashMap, sync::Arc};

pub use monitor_common::AgentPingTarget;
use tokio::sync::{Mutex, Notify, RwLock, Semaphore};

use crate::auth::LoginLimiter;
use crate::database::{Database, DatabaseError, NodeMetaRow, SettingsRow, StartupHydration};
use crate::history::HistoryAccumulator;
use crate::public_snapshot::{NetworkRateRing, PublicSnapshotCache};
use crate::snapshot::SnapshotStore;
use crate::traffic::TrafficState;

pub type SettingsCache = Arc<RwLock<SettingsRow>>;
pub type NodeMetaCache = Arc<RwLock<HashMap<i64, NodeMetaRow>>>;
pub type NodeTokenCache = Arc<RwLock<HashMap<[u8; 32], i64>>>;
pub type AgentConfigCache = Arc<RwLock<AgentConfig>>;

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub report_interval_seconds: i64,
    pub ping_interval_seconds: i64,
    pub targets: Vec<AgentPingTarget>,
}

#[derive(Clone)]
pub struct AppState {
    pub database: Database,
    pub login_limiter: Arc<LoginLimiter>,
    pub(crate) auth_hash_gate: Arc<Semaphore>,
    pub settings: SettingsCache,
    pub(crate) settings_mutation_lock: Arc<Mutex<()>>,
    pub(crate) ping_target_mutation_lock: Arc<Mutex<()>>,
    pub node_metadata: NodeMetaCache,
    pub(crate) node_tokens: NodeTokenCache,
    pub(crate) node_lifecycle_gate: Arc<RwLock<()>>,
    pub(crate) snapshots: SnapshotStore,
    pub(crate) agent_config: AgentConfigCache,
    pub(crate) traffic: TrafficState,
    pub(crate) traffic_flush: Arc<Notify>,
    pub(crate) history: HistoryAccumulator,
    pub(crate) public_snapshot: PublicSnapshotCache,
    pub(crate) public_network_rates: NetworkRateRing,
    pub(crate) public_snapshot_generation_gate: Arc<Mutex<()>>,
}

impl AppState {
    pub fn new(database: Database, hydration: StartupHydration) -> Self {
        let node_metadata: HashMap<i64, NodeMetaRow> = hydration
            .nodes
            .into_iter()
            .map(|node| (node.id, node))
            .collect();
        let node_tokens = hydration
            .node_tokens
            .into_iter()
            .map(|token| (token.token_hash, token.node_id))
            .collect();
        let snapshots = hydration
            .node_last_states
            .into_iter()
            .map(|mut row| {
                // A legacy inconsistent row falls back to its last-seen time.
                row.snapshot.first_seen_at = node_metadata
                    .get(&row.node_id)
                    .and_then(|node| node.first_seen_at)
                    .unwrap_or(row.snapshot.last_seen_at);
                (row.node_id, row.snapshot)
            })
            .collect();
        let agent_config = AgentConfig {
            report_interval_seconds: hydration.settings.agent_report_interval_seconds,
            ping_interval_seconds: hydration.settings.ping_interval_seconds,
            targets: hydration
                .enabled_ping_targets
                .into_iter()
                .map(|target| AgentPingTarget {
                    id: target.id,
                    name: target.name,
                    host: target.host,
                    ip_family: target.ip_family,
                })
                .collect(),
        };

        let traffic = TrafficState::from_recovery(hydration.traffic_recovery);

        Self {
            database,
            login_limiter: Arc::new(LoginLimiter::new()),
            auth_hash_gate: Arc::new(Semaphore::new(1)),
            settings: Arc::new(RwLock::new(hydration.settings)),
            settings_mutation_lock: Arc::new(Mutex::new(())),
            ping_target_mutation_lock: Arc::new(Mutex::new(())),
            node_metadata: Arc::new(RwLock::new(node_metadata)),
            node_tokens: Arc::new(RwLock::new(node_tokens)),
            node_lifecycle_gate: Arc::new(RwLock::new(())),
            snapshots: Arc::new(RwLock::new(snapshots)),
            agent_config: Arc::new(RwLock::new(agent_config)),
            traffic,
            traffic_flush: Arc::new(Notify::new()),
            history: HistoryAccumulator::new(),
            public_snapshot: PublicSnapshotCache::new(),
            public_network_rates: NetworkRateRing::new(),
            public_snapshot_generation_gate: Arc::new(Mutex::new(())),
        }
    }

    pub async fn persist_settings(&self, settings: SettingsRow) -> Result<(), DatabaseError> {
        let _mutation_guard = self.settings_mutation_lock.lock().await;
        self.commit_settings_under_lock(settings).await
    }

    pub(crate) async fn commit_settings_under_lock(
        &self,
        settings: SettingsRow,
    ) -> Result<(), DatabaseError> {
        // The caller holds the mutation lock across commit and both cache publications.
        self.database.upsert_settings(settings.clone()).await?;
        *self.settings.write().await = settings.clone();
        let mut config = self.agent_config.write().await;
        config.report_interval_seconds = settings.agent_report_interval_seconds;
        config.ping_interval_seconds = settings.ping_interval_seconds;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use tokio::sync::Barrier;

    use super::*;
    use crate::database::hydrate_startup;

    static TEST_DATABASE_ID: AtomicU64 = AtomicU64::new(0);

    #[tokio::test]
    async fn failed_settings_write_does_not_change_cache() {
        let path = test_database_path("failed-write");
        remove_database_files(&path);

        let database = Database::open(&path).expect("open database");
        let hydration = hydrate_startup(&database).await.expect("hydrate startup");
        let state = AppState::new(database, hydration);
        let original = state.settings.read().await.clone();
        let mut invalid = original.clone();
        invalid.offline_after_seconds = invalid.agent_report_interval_seconds;

        assert!(state.persist_settings(invalid).await.is_err());
        assert_eq!(*state.settings.read().await, original);

        state.database.shutdown().await.expect("shutdown database");
        remove_database_files(&path);
    }

    #[tokio::test]
    async fn concurrent_settings_mutations_leave_database_and_cache_equal() {
        const UPDATE_COUNT: usize = 24;

        let path = test_database_path("concurrent-writes");
        let database = Database::open(&path).expect("open database");
        let hydration = hydrate_startup(&database).await.expect("hydrate startup");
        let state = AppState::new(database, hydration);
        let original = state.settings.read().await.clone();
        let barrier = Arc::new(Barrier::new(UPDATE_COUNT + 1));
        let mut mutations = Vec::with_capacity(UPDATE_COUNT);

        for index in 0..UPDATE_COUNT {
            let state = state.clone();
            let barrier = Arc::clone(&barrier);
            let mut settings = original.clone();
            settings.site_name = format!("Monitor {index}");
            settings.updated_at += index as i64 + 1;

            mutations.push(tokio::spawn(async move {
                barrier.wait().await;
                state.persist_settings(settings).await
            }));
        }

        barrier.wait().await;
        for mutation in mutations {
            mutation
                .await
                .expect("settings mutation task completed")
                .expect("settings mutation succeeded");
        }

        let persisted = state
            .database
            .load_settings()
            .await
            .expect("load persisted settings");
        let cached = state.settings.read().await.clone();
        assert_eq!(persisted, cached);

        state.database.shutdown().await.expect("shutdown database");
        remove_database_files(&path);
    }

    fn test_database_path(label: &str) -> PathBuf {
        let id = TEST_DATABASE_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "monitor-app-state-{label}-{}-{id}.db",
            std::process::id()
        ))
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
