mod auth;
mod nodes;

use axum::{
    Router,
    routing::{get, patch, post},
};

use crate::app::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/me", get(auth::me))
        .route("/api/admin/password", patch(auth::change_password))
        .route("/api/admin/nodes", get(nodes::list).post(nodes::create))
        .route(
            "/api/admin/nodes/{id}",
            patch(nodes::patch).delete(nodes::delete),
        )
        .route(
            "/api/admin/nodes/{id}/rotate-token",
            post(nodes::rotate_token),
        )
        .with_state(state)
}
