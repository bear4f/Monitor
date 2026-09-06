pub mod app;
pub mod config;
pub mod database;

use std::io;

use app::AppState;
use axum::Router;
use config::Config;
use database::{Database, hydrate_startup};

#[derive(Debug)]
pub enum ServerError {
    Database(database::DatabaseError),
    Io {
        operation: &'static str,
        source: io::Error,
    },
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "database startup failed: {error}"),
            Self::Io { operation, source } => write!(formatter, "{operation}: {source}"),
        }
    }
}

impl std::error::Error for ServerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            Self::Io { source, .. } => Some(source),
        }
    }
}

impl From<database::DatabaseError> for ServerError {
    fn from(value: database::DatabaseError) -> Self {
        Self::Database(value)
    }
}

pub async fn run(config: Config) -> Result<(), ServerError> {
    let database = Database::open(&config.database_path)?;
    let hydration = hydrate_startup(&database).await?;
    let state = AppState::new(database, hydration);

    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .map_err(|source| ServerError::Io {
            operation: "failed to bind HTTP listener",
            source,
        })?;

    tracing::info!(listen = %config.listen, db = %config.database_path.display(), "monitor-server started");

    let router = Router::<AppState>::new().with_state(state);
    axum::serve(listener, router)
        .await
        .map_err(|source| ServerError::Io {
            operation: "HTTP server failed",
            source,
        })
}
