pub mod auth;
pub mod handlers;
pub mod routes;
pub mod server;

use std::sync::Arc;
use std::time::Duration;
use zega::Zega;

/// How long one ZQL statement may run on a server before it is stopped with a
/// `query_time_limit` error (APS 13: "2 second limit" per query). `zega start
/// --query-time-limit` changes it for a self-hosted server.
pub const DEFAULT_QUERY_TIME_LIMIT: Duration = Duration::from_secs(2);

/// One database, shared by every request. `Zega` does its own locking: each
/// statement runs under its graph lock, which is released before the statement
/// waits for its WAL entry to be durable, so requests are not serialized
/// behind each other's fsync (zega#51). Blocking engine work runs on Tokio's
/// blocking pool, never on its request/health workers.
#[derive(Clone)]
pub struct AppState {
    pub zega: Arc<Zega>,
    pub token_hash: Option<[u8; 32]>,
}

impl AppState {
    pub fn new(zega: Zega, token: Option<&str>) -> Self {
        Self {
            zega: Arc::new(zega),
            token_hash: token.map(auth::hash_token),
        }
    }
}
