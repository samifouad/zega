use crate::{handlers, AppState};
use axum::{
    extract::DefaultBodyLimit,
    routing::{delete, get, post},
    Router,
};

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(handlers::health))
        .route("/zql", post(handlers::zql))
        .route("/graph", get(handlers::graph).delete(handlers::clear))
        .route("/graph/nodes/:id", delete(handlers::delete_node))
        .route(
            "/graph/relationships/:id",
            delete(handlers::delete_relationship),
        )
        .route("/graph/relationships", post(handlers::connect))
        // Raw source text needs room for JSON escaping. Per-source limits are
        // still enforced by the engine before parsing and insertion.
        .layer(DefaultBodyLimit::max(16_000_000))
        .with_state(state)
}
