use axum::{
    body::Body,
    http::{header, StatusCode, Uri},
    response::Response,
    routing::get,
};
use clap::{Parser, Subcommand};
use include_dir::{include_dir, Dir};
use std::{
    io,
    net::{IpAddr, Ipv4Addr},
    path::PathBuf,
};
use tokio::net::TcpListener;
use zega::Zega;
use zega_server::AppState;

static EXPLORER: Dir<'_> = include_dir!("$OUT_DIR/explorer");

#[derive(Parser)]
#[command(name = "zega", version, about = "Zega graph database and explorer")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the database over HTTP with ZQL.
    Start {
        #[arg(long, default_value = "./zega-data")]
        data: PathBuf,
        #[arg(long, default_value_t = 9342)]
        port: u16,
        #[arg(long, default_value = "127.0.0.1")]
        host: IpAddr,
        /// Read the required bearer token from this file (never from an environment variable).
        #[arg(long)]
        token_file: Option<PathBuf>,
        /// Allow ZQL imports from private/loopback URLs for trusted callers.
        #[arg(long)]
        allow_private_imports: bool,
    },
    /// Serve the embedded explorer against a local database. Prints a URL; opens nothing.
    Explorer {
        #[arg(long, default_value_t = 9343)]
        port: u16,
        #[arg(long, default_value = "./zega-data")]
        data: PathBuf,
        #[arg(long)]
        allow_private_imports: bool,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(std::thread::available_parallelism()?.get())
        .enable_all()
        .build()?;
    runtime.block_on(run(cli))
}

async fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let (data, host, port, token_file, allow_private, explorer) = match cli.command {
        Command::Start {
            data,
            host,
            port,
            token_file,
            allow_private_imports,
        } => (data, host, port, token_file, allow_private_imports, false),
        Command::Explorer {
            data,
            port,
            allow_private_imports,
        } => (
            data,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            port,
            None,
            allow_private_imports,
            true,
        ),
    };
    let token = token_file.map(std::fs::read_to_string).transpose()?;
    let token = token.as_deref().map(str::trim);
    if token.is_some_and(|token| token.is_empty() || token.chars().any(char::is_whitespace)) {
        return Err("token file must contain one nonempty bearer token".into());
    }
    if token.is_none() && host != IpAddr::V4(Ipv4Addr::LOCALHOST) {
        return Err("--token-file is required when --host is not 127.0.0.1".into());
    }
    // Both commands can target the same directory; never let two CLI processes
    // append independent graph histories to one WAL.
    std::fs::create_dir_all(&data)?;
    let data_lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(data.join("zega.lock"))?;
    data_lock.try_lock().map_err(|error| {
        format!(
            "data directory {} is already in use or cannot be locked: {error}",
            data.display()
        )
    })?;
    let path = data.to_str().ok_or("data path must be UTF-8")?;
    let db = Zega::open(path)
        .allow_private_imports(allow_private)
        .build()
        .map_err(io::Error::other)?;
    let state = AppState::new(db, token);
    let listener = TcpListener::bind((host, port)).await?;
    let address = listener.local_addr()?;
    println!("http://{address}");
    if explorer {
        let app = zega_server::routes::app(state)
            .route(
                "/explorer-config.json",
                get(|| async { axum::Json(serde_config()) }),
            )
            .fallback(embedded);
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown())
            .await?;
    } else {
        axum::serve(listener, zega_server::routes::app(state))
            .with_graceful_shutdown(shutdown())
            .await?;
    }
    Ok(())
}

fn serde_config() -> std::collections::HashMap<&'static str, &'static str> {
    std::collections::HashMap::from([("backend", "native")])
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}

async fn embedded(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    let Some(file) = EXPLORER.get_file(path) else {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .unwrap();
    };
    let content_type = match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("json") => "application/json",
        _ => "text/plain; charset=utf-8",
    };
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-cache")
        .header("x-content-type-options", "nosniff")
        .body(Body::from(file.contents()))
        .unwrap()
}
