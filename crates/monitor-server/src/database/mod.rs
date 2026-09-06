mod migrations;
mod models;
mod persistence;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::mpsc as std_mpsc,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub use migrations::CURRENT_SCHEMA_VERSION;
pub use models::{
    AdminNodeRow, DeletedNodeRow, EnabledPingTargetRow, NewNodeRow, NodeLastStateRow, NodeMetaRow,
    NodePatchRow, NodeTokenRow, NodeUpdateResult, RotatedNodeTokenRow, SessionRow, SettingsRow,
    SqlitePragmas, StartupHydration, TrafficCheckpointRow, TrafficCycleCheckpointRow,
    TrafficDayCheckpointRow, TrafficRecoveryRow, UpdateNodeResult,
};
use rusqlite::Connection;
use tokio::sync::{mpsc, oneshot};

const DATABASE_QUEUE_CAPACITY: usize = 64;

#[derive(Debug)]
pub enum DatabaseError {
    Open {
        path: PathBuf,
        source: rusqlite::Error,
    },
    Sql {
        operation: &'static str,
        source: rusqlite::Error,
    },
    Migration {
        version: i64,
        source: rusqlite::Error,
    },
    UnsupportedSchemaVersion {
        installed: i64,
        supported: i64,
    },
    MissingSettings,
    WorkerSpawn(std::io::Error),
    WorkerStopped,
    WorkerResponseDropped,
    TooManyEnabledPingTargets {
        count: usize,
    },
    Clock(std::time::SystemTimeError),
    Time {
        operation: &'static str,
        source: jiff::Error,
    },
}

impl std::fmt::Display for DatabaseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open { path, source } => {
                write!(formatter, "open {}: {source}", path.display())
            }
            Self::Sql { operation, source } => write!(formatter, "{operation}: {source}"),
            Self::Migration { version, source } => {
                write!(formatter, "migration {version} failed: {source}")
            }
            Self::UnsupportedSchemaVersion {
                installed,
                supported,
            } => write!(
                formatter,
                "database schema version {installed} is newer than supported version {supported}"
            ),
            Self::MissingSettings => formatter.write_str("settings row id=1 is missing"),
            Self::WorkerSpawn(source) => write!(formatter, "start database worker: {source}"),
            Self::WorkerStopped => formatter.write_str("database worker stopped"),
            Self::WorkerResponseDropped => {
                formatter.write_str("database worker dropped its response")
            }
            Self::TooManyEnabledPingTargets { count } => {
                write!(
                    formatter,
                    "database has {count} enabled ping targets; maximum is 6"
                )
            }
            Self::Clock(source) => write!(formatter, "read system clock: {source}"),
            Self::Time { operation, source } => write!(formatter, "{operation}: {source}"),
        }
    }
}

impl std::error::Error for DatabaseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Open { source, .. }
            | Self::Sql { source, .. }
            | Self::Migration { source, .. } => Some(source),
            Self::WorkerSpawn(source) => Some(source),
            Self::Clock(source) => Some(source),
            Self::Time { source, .. } => Some(source),
            Self::UnsupportedSchemaVersion { .. }
            | Self::MissingSettings
            | Self::WorkerStopped
            | Self::WorkerResponseDropped
            | Self::TooManyEnabledPingTargets { .. } => None,
        }
    }
}

#[derive(Clone)]
pub struct Database {
    sender: mpsc::Sender<Command>,
}

enum Command {
    LoadAdminPasswordHash(oneshot::Sender<Result<Option<String>, DatabaseError>>),
    SetAdminPassword {
        password_hash: String,
        updated_at: i64,
        response: oneshot::Sender<Result<(), DatabaseError>>,
    },
    ChangeAdminPassword {
        expected_password_hash: String,
        new_password_hash: String,
        updated_at: i64,
        response: oneshot::Sender<Result<bool, DatabaseError>>,
    },
    CreateSession {
        expected_password_hash: String,
        token_hash: [u8; 32],
        created_at: i64,
        expires_at: i64,
        response: oneshot::Sender<Result<bool, DatabaseError>>,
    },
    FindSession {
        token_hash: [u8; 32],
        response: oneshot::Sender<Result<Option<SessionRow>, DatabaseError>>,
    },
    DeleteSession {
        token_hash: [u8; 32],
        response: oneshot::Sender<Result<bool, DatabaseError>>,
    },
    DeleteAllSessions(oneshot::Sender<Result<usize, DatabaseError>>),
    DeleteExpiredSessions {
        now: i64,
        response: oneshot::Sender<Result<usize, DatabaseError>>,
    },
    LoadNodeTokens(oneshot::Sender<Result<Vec<NodeTokenRow>, DatabaseError>>),
    LoadNodeLastStates(oneshot::Sender<Result<Vec<NodeLastStateRow>, DatabaseError>>),
    LoadEnabledPingTargets(oneshot::Sender<Result<Vec<EnabledPingTargetRow>, DatabaseError>>),
    ListAdminNodes {
        day_start_utc: i64,
        now: i64,
        response: oneshot::Sender<Result<Vec<AdminNodeRow>, DatabaseError>>,
    },
    CreateNode {
        node: NewNodeRow,
        token_hash: [u8; 32],
        now: i64,
        response: oneshot::Sender<Result<NodeMetaRow, DatabaseError>>,
    },
    UpdateNode {
        public_id: String,
        patch: NodePatchRow,
        now: i64,
        response: oneshot::Sender<Result<UpdateNodeResult, DatabaseError>>,
    },
    DeleteNode {
        public_id: String,
        now: i64,
        response: oneshot::Sender<Result<Option<DeletedNodeRow>, DatabaseError>>,
    },
    RotateNodeToken {
        public_id: String,
        new_token_hash: [u8; 32],
        now: i64,
        response: oneshot::Sender<Result<Option<RotatedNodeTokenRow>, DatabaseError>>,
    },
    LoadSettings(oneshot::Sender<Result<SettingsRow, DatabaseError>>),
    UpsertSettings(SettingsRow, oneshot::Sender<Result<(), DatabaseError>>),
    LoadNodeMetadata(oneshot::Sender<Result<Vec<NodeMetaRow>, DatabaseError>>),
    LoadTrafficRecovery {
        day_start_utc: i64,
        now: i64,
        response: oneshot::Sender<Result<Vec<TrafficRecoveryRow>, DatabaseError>>,
    },
    PersistTrafficBatch {
        rows: Vec<TrafficCheckpointRow>,
        persisted_at: i64,
        response: oneshot::Sender<Result<(), DatabaseError>>,
    },
    ReadPragmas(oneshot::Sender<Result<SqlitePragmas, DatabaseError>>),
    ReadSchemaVersion(oneshot::Sender<Result<i64, DatabaseError>>),
    Shutdown(oneshot::Sender<()>),
}

impl Database {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DatabaseError> {
        let path = path.as_ref().to_path_buf();
        let (sender, receiver) = mpsc::channel(DATABASE_QUEUE_CAPACITY);
        let (startup_sender, startup_receiver) = std_mpsc::sync_channel(1);

        thread::Builder::new()
            .name("monitor-sqlite".into())
            .spawn({
                let path = path.clone();
                move || database_worker(path, receiver, startup_sender)
            })
            .map_err(DatabaseError::WorkerSpawn)?;

        startup_receiver
            .recv()
            .map_err(|_| DatabaseError::WorkerStopped)??;

        Ok(Self { sender })
    }

    pub async fn load_settings(&self) -> Result<SettingsRow, DatabaseError> {
        self.request(Command::LoadSettings).await
    }

    pub async fn load_admin_password_hash(&self) -> Result<Option<String>, DatabaseError> {
        self.request(Command::LoadAdminPasswordHash).await
    }

    pub async fn set_admin_password(
        &self,
        password_hash: String,
        updated_at: i64,
    ) -> Result<(), DatabaseError> {
        self.request(|response| Command::SetAdminPassword {
            password_hash,
            updated_at,
            response,
        })
        .await
    }

    pub async fn change_admin_password(
        &self,
        expected_password_hash: String,
        new_password_hash: String,
        updated_at: i64,
    ) -> Result<bool, DatabaseError> {
        self.request(|response| Command::ChangeAdminPassword {
            expected_password_hash,
            new_password_hash,
            updated_at,
            response,
        })
        .await
    }

    pub async fn create_session(
        &self,
        expected_password_hash: String,
        token_hash: [u8; 32],
        created_at: i64,
        expires_at: i64,
    ) -> Result<bool, DatabaseError> {
        self.request(|response| Command::CreateSession {
            expected_password_hash,
            token_hash,
            created_at,
            expires_at,
            response,
        })
        .await
    }

    pub async fn find_session(
        &self,
        token_hash: [u8; 32],
    ) -> Result<Option<SessionRow>, DatabaseError> {
        self.request(|response| Command::FindSession {
            token_hash,
            response,
        })
        .await
    }

    pub async fn delete_session(&self, token_hash: [u8; 32]) -> Result<bool, DatabaseError> {
        self.request(|response| Command::DeleteSession {
            token_hash,
            response,
        })
        .await
    }

    pub async fn delete_all_sessions(&self) -> Result<usize, DatabaseError> {
        self.request(Command::DeleteAllSessions).await
    }

    pub async fn delete_expired_sessions(&self, now: i64) -> Result<usize, DatabaseError> {
        self.request(|response| Command::DeleteExpiredSessions { now, response })
            .await
    }

    pub async fn load_node_tokens(&self) -> Result<Vec<NodeTokenRow>, DatabaseError> {
        self.request(Command::LoadNodeTokens).await
    }

    pub async fn load_enabled_ping_targets(
        &self,
    ) -> Result<Vec<EnabledPingTargetRow>, DatabaseError> {
        self.request(Command::LoadEnabledPingTargets).await
    }

    pub async fn list_admin_nodes(
        &self,
        day_start_utc: i64,
        now: i64,
    ) -> Result<Vec<AdminNodeRow>, DatabaseError> {
        self.request(|response| Command::ListAdminNodes {
            day_start_utc,
            now,
            response,
        })
        .await
    }

    pub async fn create_node(
        &self,
        node: NewNodeRow,
        token_hash: [u8; 32],
        now: i64,
    ) -> Result<NodeMetaRow, DatabaseError> {
        self.request(|response| Command::CreateNode {
            node,
            token_hash,
            now,
            response,
        })
        .await
    }

    pub async fn update_node(
        &self,
        public_id: String,
        patch: NodePatchRow,
        now: i64,
    ) -> Result<UpdateNodeResult, DatabaseError> {
        self.request(|response| Command::UpdateNode {
            public_id,
            patch,
            now,
            response,
        })
        .await
    }

    pub async fn delete_node(
        &self,
        public_id: String,
        now: i64,
    ) -> Result<Option<DeletedNodeRow>, DatabaseError> {
        self.request(|response| Command::DeleteNode {
            public_id,
            now,
            response,
        })
        .await
    }

    pub async fn rotate_node_token(
        &self,
        public_id: String,
        new_token_hash: [u8; 32],
        now: i64,
    ) -> Result<Option<RotatedNodeTokenRow>, DatabaseError> {
        self.request(|response| Command::RotateNodeToken {
            public_id,
            new_token_hash,
            now,
            response,
        })
        .await
    }

    pub async fn upsert_settings(&self, settings: SettingsRow) -> Result<(), DatabaseError> {
        self.request(|response| Command::UpsertSettings(settings, response))
            .await
    }

    pub async fn load_node_metadata(&self) -> Result<Vec<NodeMetaRow>, DatabaseError> {
        self.request(Command::LoadNodeMetadata).await
    }

    pub async fn load_node_last_states(&self) -> Result<Vec<NodeLastStateRow>, DatabaseError> {
        self.request(Command::LoadNodeLastStates).await
    }

    pub async fn load_traffic_recovery(
        &self,
        day_start_utc: i64,
        now: i64,
    ) -> Result<Vec<TrafficRecoveryRow>, DatabaseError> {
        self.request(|response| Command::LoadTrafficRecovery {
            day_start_utc,
            now,
            response,
        })
        .await
    }

    pub async fn persist_traffic_batch(
        &self,
        rows: Vec<TrafficCheckpointRow>,
        persisted_at: i64,
    ) -> Result<(), DatabaseError> {
        self.request(|response| Command::PersistTrafficBatch {
            rows,
            persisted_at,
            response,
        })
        .await
    }

    pub async fn read_pragmas(&self) -> Result<SqlitePragmas, DatabaseError> {
        self.request(Command::ReadPragmas).await
    }

    pub async fn schema_version(&self) -> Result<i64, DatabaseError> {
        self.request(Command::ReadSchemaVersion).await
    }

    pub async fn shutdown(self) -> Result<(), DatabaseError> {
        let (response_sender, response_receiver) = oneshot::channel();
        self.sender
            .send(Command::Shutdown(response_sender))
            .await
            .map_err(|_| DatabaseError::WorkerStopped)?;
        response_receiver
            .await
            .map_err(|_| DatabaseError::WorkerResponseDropped)
    }

    async fn request<T>(
        &self,
        command: impl FnOnce(oneshot::Sender<Result<T, DatabaseError>>) -> Command,
    ) -> Result<T, DatabaseError> {
        let (response_sender, response_receiver) = oneshot::channel();
        self.sender
            .send(command(response_sender))
            .await
            .map_err(|_| DatabaseError::WorkerStopped)?;
        response_receiver
            .await
            .map_err(|_| DatabaseError::WorkerResponseDropped)?
    }
}

pub async fn hydrate_startup(database: &Database) -> Result<StartupHydration, DatabaseError> {
    let settings = database.load_settings().await?;
    let nodes = database.load_node_metadata().await?;
    let node_tokens = database.load_node_tokens().await?;
    let node_last_states = database.load_node_last_states().await?;
    let enabled_ping_targets = database.load_enabled_ping_targets().await?;
    if enabled_ping_targets.len() > 6 {
        return Err(DatabaseError::TooManyEnabledPingTargets {
            count: enabled_ping_targets.len(),
        });
    }
    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(DatabaseError::Clock)?
            .as_secs(),
    )
    .map_err(|_| DatabaseError::Time {
        operation: "convert startup timestamp",
        source: jiff::Error::from_args(format_args!("timestamp does not fit in i64")),
    })?;
    let day_start_utc =
        crate::time::day_start_utc(now, &settings.site_timezone).map_err(|source| {
            DatabaseError::Time {
                operation: "calculate startup natural day",
                source,
            }
        })?;
    let mut traffic_recovery = database.load_traffic_recovery(day_start_utc, now).await?;
    let reset_days: HashMap<i64, i64> = nodes
        .iter()
        .map(|node| (node.id, node.traffic_reset_day))
        .collect();
    for row in &mut traffic_recovery {
        let reset_day =
            reset_days
                .get(&row.node_id)
                .copied()
                .ok_or_else(|| DatabaseError::Sql {
                    operation: "match startup traffic state to node metadata",
                    source: rusqlite::Error::QueryReturnedNoRows,
                })?;
        let cycle = crate::time::billing_cycle(now, &settings.site_timezone, reset_day).map_err(
            |source| DatabaseError::Time {
                operation: "calculate startup billing cycle",
                source,
            },
        )?;
        if row.cycle_start_utc != cycle.start_utc || row.cycle_end_utc != cycle.end_utc {
            row.cycle_start_utc = cycle.start_utc;
            row.cycle_end_utc = cycle.end_utc;
            row.cycle_rx_bytes = 0;
            row.cycle_tx_bytes = 0;
        }
    }

    Ok(StartupHydration {
        settings,
        nodes,
        node_tokens,
        node_last_states,
        enabled_ping_targets,
        traffic_recovery,
    })
}

fn database_worker(
    path: PathBuf,
    mut receiver: mpsc::Receiver<Command>,
    startup_sender: std_mpsc::SyncSender<Result<(), DatabaseError>>,
) {
    let mut connection = match open_ready_connection(&path) {
        Ok(connection) => {
            if startup_sender.send(Ok(())).is_err() {
                return;
            }
            connection
        }
        Err(error) => {
            let _ = startup_sender.send(Err(error));
            return;
        }
    };

    while let Some(command) = receiver.blocking_recv() {
        match command {
            Command::LoadAdminPasswordHash(response) => {
                let _ = response.send(persistence::load_admin_password_hash(&connection));
            }
            Command::SetAdminPassword {
                password_hash,
                updated_at,
                response,
            } => {
                let _ = response.send(persistence::set_admin_password(
                    &mut connection,
                    &password_hash,
                    updated_at,
                ));
            }
            Command::ChangeAdminPassword {
                expected_password_hash,
                new_password_hash,
                updated_at,
                response,
            } => {
                let _ = response.send(persistence::change_admin_password(
                    &mut connection,
                    &expected_password_hash,
                    &new_password_hash,
                    updated_at,
                ));
            }
            Command::CreateSession {
                expected_password_hash,
                token_hash,
                created_at,
                expires_at,
                response,
            } => {
                let _ = response.send(persistence::create_session(
                    &connection,
                    &expected_password_hash,
                    &token_hash,
                    created_at,
                    expires_at,
                ));
            }
            Command::FindSession {
                token_hash,
                response,
            } => {
                let _ = response.send(persistence::find_session(&connection, &token_hash));
            }
            Command::DeleteSession {
                token_hash,
                response,
            } => {
                let _ = response.send(persistence::delete_session(&connection, &token_hash));
            }
            Command::DeleteAllSessions(response) => {
                let _ = response.send(persistence::delete_all_sessions(&connection));
            }
            Command::DeleteExpiredSessions { now, response } => {
                let _ = response.send(persistence::delete_expired_sessions(&connection, now));
            }
            Command::LoadNodeTokens(response) => {
                let _ = response.send(persistence::load_node_tokens(&connection));
            }
            Command::LoadEnabledPingTargets(response) => {
                let _ = response.send(persistence::load_enabled_ping_targets(&connection));
            }
            Command::ListAdminNodes {
                day_start_utc,
                now,
                response,
            } => {
                let _ = response.send(persistence::list_admin_nodes(
                    &connection,
                    day_start_utc,
                    now,
                ));
            }
            Command::CreateNode {
                node,
                token_hash,
                now,
                response,
            } => {
                let _ = response.send(persistence::create_node(
                    &mut connection,
                    &node,
                    &token_hash,
                    now,
                ));
            }
            Command::UpdateNode {
                public_id,
                patch,
                now,
                response,
            } => {
                let _ = response.send(persistence::update_node(
                    &mut connection,
                    &public_id,
                    &patch,
                    now,
                ));
            }
            Command::DeleteNode {
                public_id,
                now,
                response,
            } => {
                let _ = response.send(persistence::delete_node(&mut connection, &public_id, now));
            }
            Command::RotateNodeToken {
                public_id,
                new_token_hash,
                now,
                response,
            } => {
                let _ = response.send(persistence::rotate_node_token(
                    &mut connection,
                    &public_id,
                    &new_token_hash,
                    now,
                ));
            }
            Command::LoadSettings(response) => {
                let _ = response.send(persistence::load_settings(&connection));
            }
            Command::UpsertSettings(settings, response) => {
                let _ = response.send(persistence::upsert_settings(&mut connection, &settings));
            }
            Command::LoadNodeMetadata(response) => {
                let _ = response.send(persistence::load_node_metadata(&connection));
            }
            Command::LoadNodeLastStates(response) => {
                let _ = response.send(persistence::load_node_last_states(&connection));
            }
            Command::LoadTrafficRecovery {
                day_start_utc,
                now,
                response,
            } => {
                let _ = response.send(persistence::load_traffic_recovery(
                    &connection,
                    day_start_utc,
                    now,
                ));
            }
            Command::PersistTrafficBatch {
                rows,
                persisted_at,
                response,
            } => {
                let _ = response.send(persistence::persist_traffic_batch(
                    &mut connection,
                    &rows,
                    persisted_at,
                ));
            }
            Command::ReadPragmas(response) => {
                let _ = response.send(persistence::read_pragmas(&connection));
            }
            Command::ReadSchemaVersion(response) => {
                let _ = response.send(migrations::schema_version(&connection));
            }
            Command::Shutdown(response) => {
                drop(connection);
                let _ = response.send(());
                return;
            }
        }
    }
}

fn open_ready_connection(path: &Path) -> Result<Connection, DatabaseError> {
    let mut connection = Connection::open(path).map_err(|source| DatabaseError::Open {
        path: path.to_path_buf(),
        source,
    })?;

    configure_connection(&connection)?;
    migrations::apply_migrations(&mut connection)?;
    persistence::ensure_default_settings(&connection)?;
    Ok(connection)
}

fn configure_connection(connection: &Connection) -> Result<(), DatabaseError> {
    connection
        .busy_timeout(Duration::from_millis(5_000))
        .map_err(|source| DatabaseError::Sql {
            operation: "set SQLite busy timeout",
            source,
        })?;
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(|source| DatabaseError::Sql {
            operation: "enable SQLite foreign keys",
            source,
        })?;

    let schema_object_count: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .map_err(|source| DatabaseError::Sql {
            operation: "inspect database before migration",
            source,
        })?;
    if schema_object_count == 0 {
        connection
            .pragma_update(None, "page_size", 4_096)
            .map_err(|source| DatabaseError::Sql {
                operation: "set initial SQLite page size",
                source,
            })?;
    }

    for (name, value) in [
        ("journal_mode", "WAL"),
        ("synchronous", "NORMAL"),
        ("temp_store", "MEMORY"),
        ("cache_size", "-2048"),
        ("wal_autocheckpoint", "1000"),
    ] {
        connection
            .pragma_update(None, name, value)
            .map_err(|source| DatabaseError::Sql {
                operation: "configure SQLite connection",
                source,
            })?;
    }

    Ok(())
}

#[cfg(test)]
mod tests;
