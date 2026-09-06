mod auth;

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
        .with_state(state)
}
