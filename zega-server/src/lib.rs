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
/// --max-import-bytes` says otherwise: 64 MiB, the Pro graph cap. Decoding
/// needs 12-22x a file's size in memory, so this also bounds an import.
pub const DEFAULT_MAX_IMPORT_BYTES: u64 = 64 << 20;

/// How long a `.graph` transfer may make no progress (an upload sends
/// nothing, a download reads nothing) before the server gives up on the
/// client and frees its staging file.
pub const DEFAULT_TRANSFER_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// How many `.graph` transfers (`GET` and `PUT /graph`) may hold a staging
/// file at once; more get `503`. Bounds the disk and threads they use.
pub const DEFAULT_TRANSFER_SLOTS: usize = 16;

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
    pub transfer_idle_timeout: Duration,
    pub transfers: Arc<tokio::sync::Semaphore>,
}

impl AppState {
    pub fn new(zega: Zega, token: Option<&str>) -> Self {
        Self {
            zega: Arc::new(Mutex::new(zega)),
            token_hash: token.map(auth::hash_token),
            max_import_bytes: DEFAULT_MAX_IMPORT_BYTES,
            transfer_idle_timeout: DEFAULT_TRANSFER_IDLE_TIMEOUT,
            transfers: Arc::new(tokio::sync::Semaphore::new(DEFAULT_TRANSFER_SLOTS)),
        }
    }

    /// Change the `PUT /graph` size cap and the `.graph` transfer idle timeout.
    pub fn with_import_limits(mut self, max_bytes: u64, idle_timeout: Duration) -> Self {
        self.max_import_bytes = max_bytes;
        self.transfer_idle_timeout = idle_timeout;
        self
    }

    /// Change how many `.graph` transfers may run at once.
    pub fn with_transfer_slots(mut self, slots: usize) -> Self {
        self.transfers = Arc::new(tokio::sync::Semaphore::new(slots));
        self
    }
}
