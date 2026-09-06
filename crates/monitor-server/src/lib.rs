pub mod admin_cli;
pub mod app;
pub mod auth;
pub mod config;
pub mod database;
pub mod http;
pub mod public_snapshot;
pub mod snapshot;
pub mod time;
pub mod traffic;

use std::io;

use app::AppState;
use config::Config;
use database::{Database, hydrate_startup};

#[derive(Debug)]
pub enum ServerError {
    Database(database::DatabaseError),
    PublicSnapshot(public_snapshot::PublicSnapshotError),
    Io {
        operation: &'static str,
        source: io::Error,
    },
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
        }
    }
}

impl std::error::Error for ServerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            Self::PublicSnapshot(error) => Some(error),
            Self::Io { source, .. } => Some(source),
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
    tokio::spawn(public_snapshot::run_worker(state.clone()));
    tokio::spawn(traffic::run_checkpoint_worker(state.clone()));

    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .map_err(|source| ServerError::Io {
            operation: "failed to bind HTTP listener",
            source,
        })?;

    tracing::info!(listen = %config.listen, db = %config.database_path.display(), "monitor-server started");

    let router = http::router(state);
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
    .map_err(|source| ServerError::Io {
        operation: "HTTP server failed",
        source,
    })
}
