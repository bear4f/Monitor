mod agent;
mod auth;
mod history;
mod nodes;
mod patch;
mod ping_targets;
pub(crate) mod public;
mod settings;

use axum::{
    Router,
    routing::{get, patch, post},
};

use crate::{app::AppState, static_files};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/me", get(auth::me))
        .route("/api/agent/config", get(agent::config))
        .route("/api/agent/report", post(agent::report))
        .route("/api/public/snapshot", get(public::snapshot))
        .route("/api/public/nodes/{id}/history", get(history::resource))
        .route("/api/public/nodes/{id}/ping", get(history::ping))
        .route("/api/admin/password", patch(auth::change_password))
        .route("/api/admin/nodes", get(nodes::list).post(nodes::create))
        .route(
            "/api/admin/ping-targets",
            get(ping_targets::list).post(ping_targets::create),
        )
        .route(
            "/api/admin/ping-targets/{id}",
            patch(ping_targets::patch).delete(ping_targets::delete),
        )
        .route(
            "/api/admin/settings",
            get(settings::get).patch(settings::patch),
        )
        .route(
            "/api/admin/nodes/{id}",
            patch(nodes::patch).delete(nodes::delete),
        )
        .route(
            "/api/admin/nodes/{id}/rotate-token",
            post(nodes::rotate_token),
        )
        .with_state(state)
        .fallback(static_files::serve)
}
