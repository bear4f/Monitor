mod migrations;
mod models;
mod persistence;

use std::{
    path::{Path, PathBuf},
    sync::mpsc as std_mpsc,
    thread,
    time::Duration,
};

pub use migrations::CURRENT_SCHEMA_VERSION;
pub use models::{NodeMetaRow, SettingsRow, SqlitePragmas, StartupHydration, TrafficRecoveryRow};
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
            Self::UnsupportedSchemaVersion { .. }
            | Self::MissingSettings
            | Self::WorkerStopped
            | Self::WorkerResponseDropped => None,
        }
    }
}

#[derive(Clone)]
pub struct Database {
    sender: mpsc::Sender<Command>,
}

enum Command {
    LoadSettings(oneshot::Sender<Result<SettingsRow, DatabaseError>>),
    UpsertSettings(SettingsRow, oneshot::Sender<Result<(), DatabaseError>>),
    LoadNodeMetadata(oneshot::Sender<Result<Vec<NodeMetaRow>, DatabaseError>>),
    LoadTrafficRecovery(oneshot::Sender<Result<Vec<TrafficRecoveryRow>, DatabaseError>>),
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

    pub async fn upsert_settings(&self, settings: SettingsRow) -> Result<(), DatabaseError> {
        self.request(|response| Command::UpsertSettings(settings, response))
            .await
    }

    pub async fn load_node_metadata(&self) -> Result<Vec<NodeMetaRow>, DatabaseError> {
        self.request(Command::LoadNodeMetadata).await
    }

    pub async fn load_traffic_recovery(&self) -> Result<Vec<TrafficRecoveryRow>, DatabaseError> {
        self.request(Command::LoadTrafficRecovery).await
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
    let traffic_recovery = database.load_traffic_recovery().await?;

    Ok(StartupHydration {
        settings,
        nodes,
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
            Command::LoadSettings(response) => {
                let _ = response.send(persistence::load_settings(&connection));
            }
            Command::UpsertSettings(settings, response) => {
                let _ = response.send(persistence::upsert_settings(&mut connection, &settings));
            }
            Command::LoadNodeMetadata(response) => {
                let _ = response.send(persistence::load_node_metadata(&connection));
            }
            Command::LoadTrafficRecovery(response) => {
                let _ = response.send(persistence::load_traffic_recovery(&connection));
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
