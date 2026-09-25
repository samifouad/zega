pub mod auth;
pub mod handlers;
pub mod routes;
pub mod server;

use std::sync::{Arc, Mutex};
use std::time::Duration;
use zega::Zega;

/// How long one ZQL statement may run on a server before it is stopped with a
/// `query_time_limit` error (APS 13: "2 second limit" per query). `zega start
/// --query-time-limit` changes it for a self-hosted server.
pub const DEFAULT_QUERY_TIME_LIMIT: Duration = Duration::from_secs(2);

/// The largest `.graph` upload `PUT /graph` takes unless `zega start
/// --max-import-bytes` says otherwise.
pub const DEFAULT_MAX_IMPORT_BYTES: u64 = 1 << 30;

/// How long `PUT /graph` waits for the next piece of an upload before it
/// gives up on the client.
pub const DEFAULT_IMPORT_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// One database and one gate for each complete HTTP operation. Blocking engine
/// work runs on Tokio's blocking pool, never on its request/health workers.
/// A `.graph` transfer holds the gate only for the engine's part of it: the
/// network side runs against a staging file, so a slow client never blocks
/// anyone else.
#[derive(Clone)]
pub struct AppState {
    pub zega: Arc<Mutex<Zega>>,
    pub token_hash: Option<[u8; 32]>,
    pub max_import_bytes: u64,
    pub import_idle_timeout: Duration,
}

impl AppState {
    pub fn new(zega: Zega, token: Option<&str>) -> Self {
        Self {
            zega: Arc::new(Mutex::new(zega)),
            token_hash: token.map(auth::hash_token),
            max_import_bytes: DEFAULT_MAX_IMPORT_BYTES,
            import_idle_timeout: DEFAULT_IMPORT_IDLE_TIMEOUT,
        }
    }

    /// Change the `PUT /graph` size cap and idle timeout.
    pub fn with_import_limits(mut self, max_bytes: u64, idle_timeout: Duration) -> Self {
        self.max_import_bytes = max_bytes;
        self.import_idle_timeout = idle_timeout;
        self
    }
}
