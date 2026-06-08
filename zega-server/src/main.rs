use std::{env, io};
use tokio::net::TcpListener;
use zega_core::Zega;
use zega_server::{server, AppState};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data = env::var("ZEGA_DATA").map_err(|_| "ZEGA_DATA is required")?;
    let token = env::var("ZEGA_SERVER_TOKEN").map_err(|_| "ZEGA_SERVER_TOKEN is required")?;
    let addr = env::var("ZEGA_SERVER_ADDR").unwrap_or_else(|_| "127.0.0.1:7700".to_string());
    let workers = env::var("ZEGA_SERVER_WORKERS")
        .ok()
        .map(|value| value.parse::<usize>())
        .transpose()?
        .unwrap_or_else(num_cpus::get)
        .max(1);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let zega = Zega::open(&data).build().map_err(io::Error::other)?;
        let listener = TcpListener::bind(&addr).await?;
        server::serve(listener, AppState::new(zega, &token)).await
    })?;
    Ok(())
}
