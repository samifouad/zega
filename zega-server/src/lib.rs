pub mod auth;
pub mod handlers;
pub mod routes;
pub mod server;

use std::sync::{Arc, Mutex};
use zega::Zega;

/// One database and one gate for each complete HTTP operation. Blocking engine
/// work runs on Tokio's blocking pool, never on its request/health workers.
#[derive(Clone)]
pub struct AppState {
    pub zega: Arc<Mutex<Zega>>,
    pub token_hash: Option<[u8; 32]>,
}

impl AppState {
    pub fn new(zega: Zega, token: Option<&str>) -> Self {
        Self {
            zega: Arc::new(Mutex::new(zega)),
            token_hash: token.map(auth::hash_token),
        }
    }
}
