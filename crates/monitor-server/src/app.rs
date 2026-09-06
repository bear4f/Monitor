use std::{collections::HashMap, sync::Arc};

use tokio::sync::RwLock;

use crate::database::{
    Database, DatabaseError, NodeMetaRow, SettingsRow, StartupHydration, TrafficRecoveryRow,
};

pub type SettingsCache = Arc<RwLock<SettingsRow>>;
pub type NodeMetaCache = Arc<RwLock<HashMap<i64, NodeMetaRow>>>;

#[derive(Clone)]
pub struct AppState {
    pub database: Database,
    pub settings: SettingsCache,
    pub node_metadata: NodeMetaCache,
    pub traffic_recovery: Arc<[TrafficRecoveryRow]>,
}

impl AppState {
    pub fn new(database: Database, hydration: StartupHydration) -> Self {
        let node_metadata = hydration
            .nodes
            .into_iter()
            .map(|node| (node.id, node))
            .collect();

        Self {
            database,
            settings: Arc::new(RwLock::new(hydration.settings)),
            node_metadata: Arc::new(RwLock::new(node_metadata)),
            traffic_recovery: hydration.traffic_recovery.into(),
        }
    }

    pub async fn persist_settings(&self, settings: SettingsRow) -> Result<(), DatabaseError> {
        // Persistent mutations commit first; cache changes only after SQLite succeeds.
        self.database.upsert_settings(settings.clone()).await?;
        *self.settings.write().await = settings;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use super::*;
    use crate::database::hydrate_startup;

    #[tokio::test]
    async fn failed_settings_write_does_not_change_cache() {
        let path =
            std::env::temp_dir().join(format!("monitor-app-state-{}.db", std::process::id()));
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
