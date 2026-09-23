pub mod auth;
pub mod handlers;
pub mod routes;
pub mod server;

use std::sync::Arc;
use tokio::sync::RwLock;
use zega::Zega;

/// One canonical database behind an exclusion gate.
///
/// Reads share the gate and fan out across Tokio workers. Writes take the
/// exclusive gate so graph data and its indexes are never observed halfway
/// through an update. MVCC snapshots can remove the read/write blocking later.
#[derive(Clone)]
pub struct AppState {
    pub zega: Arc<RwLock<Zega>>,
    pub token_hash: [u8; 32],
}

impl AppState {
    pub fn new(zega: Zega, token: &str) -> Self {
        Self {
            zega: Arc::new(RwLock::new(zega)),
            token_hash: auth::hash_token(token),
        }
    }
}
