use crate::{handlers, AppState};
use axum::{routing::get, routing::post, Router};

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(handlers::health))
        .route("/cql", post(handlers::cql))
        .route("/kv", post(handlers::kv))
        .with_state(state)
}
