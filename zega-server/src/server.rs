use crate::{routes, AppState};
use std::io;
use tokio::net::TcpListener;

/// Like deka's HTTP/engine fan-out, Tokio schedules requests across CPU
/// workers. Unlike deka, there is no warm V8 isolate pool or JS event loop:
/// handlers call zega-core natively under the shared/exclusive database gate.
pub async fn serve(listener: TcpListener, state: AppState) -> io::Result<()> {
    axum::serve(listener, routes::app(state)).await
}
