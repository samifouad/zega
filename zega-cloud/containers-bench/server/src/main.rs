//! zega's HTTP server for the Cloudflare Containers benchmark (APS 13 spike).
//!
//! Serves `zega_server::routes::app`, the same router `zega start` serves,
//! over a data directory on the container's local disk, plus three
//! measurement routes:
//!
//! - `GET /mem` reports this process's RSS and peak RSS (`?reset=1` resets the
//!   peak first, through Linux `/proc/self/clear_refs`).
//! - `POST /snapshot` runs `Zega::snapshot` (today's engine: the whole graph
//!   is copied and encoded in memory, then written) and reports its time and
//!   peak RSS.
//! - `POST /reload` drops the database and opens the data directory again, the
//!   path a restarted process takes, and reports the time and peak RSS.
//! - `POST /wipe` drops the database, deletes the data directory and opens it
//!   empty, so the next restart starts from an empty disk (the Fly volume
//!   survives restarts; this is how that run gets an empty one).
//!
//! `/mem` also reports how long the startup open took (`openMs`: the reload
//! from disk a restarted process pays before it answers) and the memory the
//! machine actually offers (`MemTotal`/`MemAvailable` and the cgroup limit),
//! because a Fly VM's kernel takes part of its nominal size.
//!
//! On Cloudflare the container is reachable only through its Durable Object,
//! which checks the admin token, so the server runs without its own token by
//! default. On Fly it is reached directly and runs with `--token-file`.
//!
//! `--cors-origin ORIGIN` (repeatable; `http://localhost` also allows
//! `http://localhost:<port>` and `127.0.0.1`) lets those browser origins call
//! it, for explorer2. A preflight (OPTIONS) is answered before the token
//! check: it carries no Authorization header and grants nothing but the right
//! to send the real request, which still needs the token.
//! Configuration is flags; no environment variables.

use axum::{
    extract::{Query, Request, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    net::IpAddr,
    path::PathBuf,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tokio::net::TcpListener;
use zega::Zega;
use zega_server::AppState;

struct Options {
    data: PathBuf,
    host: IpAddr,
    port: u16,
    token_file: Option<PathBuf>,
    cors_origins: Vec<String>,
}

fn usage() -> ! {
    eprintln!("usage: zega-bench-server --data DIR [--host 0.0.0.0] [--port 8080] [--token-file FILE] [--cors-origin ORIGIN]...");
    std::process::exit(2)
}

fn parse() -> Options {
    let mut options = Options {
        data: PathBuf::from("/data"),
        host: IpAddr::from([0, 0, 0, 0]),
        port: 8080,
        token_file: None,
        cors_origins: Vec::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let value = args.next().unwrap_or_else(|| usage());
        match flag.as_str() {
            "--data" => options.data = PathBuf::from(value),
            "--host" => options.host = value.parse().unwrap_or_else(|_| usage()),
            "--port" => options.port = value.parse().unwrap_or_else(|_| usage()),
            "--token-file" => options.token_file = Some(PathBuf::from(value)),
            "--cors-origin" => options.cors_origins.push(value.trim_end_matches('/').to_string()),
            _ => usage(),
        }
    }
    options
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// RSS, peak RSS and anonymous RSS in bytes, from `/proc/self/status`.
fn memory() -> Value {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return json!({"rss": null, "peakRss": null, "rssAnon": null});
    };
    let field = |name: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|rest| rest.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
            .map(|kb| kb * 1024)
    };
    json!({"rss": field("VmRSS:"), "peakRss": field("VmHWM:"), "rssAnon": field("RssAnon:")})
}

/// What the machine offers: `/proc/meminfo` and, in a container, the cgroup
/// v2 limit. A Fly VM's kernel and init take part of its nominal memory.
fn machine_memory() -> Value {
    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let field = |name: &str| {
        meminfo
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|rest| rest.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
            .map(|kb| kb * 1024)
    };
    let cgroup_max = std::fs::read_to_string("/sys/fs/cgroup/memory.max")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok());
    json!({"memTotal": field("MemTotal:"), "memAvailable": field("MemAvailable:"), "cgroupMax": cgroup_max})
}

fn reset_peak() {
    let _ = std::fs::write("/proc/self/clear_refs", "5");
}

fn file_len(path: PathBuf) -> Option<u64> {
    std::fs::metadata(path).ok().map(|m| m.len())
}

#[derive(Clone)]
struct Bench {
    state: AppState,
    data: PathBuf,
    started_at_ms: u128,
    open_ms: f64,
}

impl Bench {
    fn disk(&self) -> Value {
        json!({
            "walBytes": file_len(self.data.join("wal.bin")),
            "snapshotBytes": file_len(self.data.join("snapshot.bin")),
        })
    }
}

/// With `--token-file`, the measurement routes need the same token as `/zql`.
fn allowed(bench: &Bench, headers: &HeaderMap) -> bool {
    bench
        .state
        .token_hash
        .as_ref()
        .is_none_or(|hash| zega_server::auth::authorized(headers, hash))
}

fn denied() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"ok": false, "error": "unauthorized"})),
    )
        .into_response()
}

fn failure(message: impl Into<String>) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"ok": false, "error": message.into()})),
    )
        .into_response()
}

async fn mem(
    State(bench): State<Bench>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if !allowed(&bench, &headers) {
        return denied();
    }
    let body = json!({
        "ok": true,
        "engine": "native",
        "startedAtMs": bench.started_at_ms,
        "openMs": bench.open_ms,
        "memory": memory(),
        "machine": machine_memory(),
        "disk": bench.disk(),
    });
    if query.contains_key("reset") {
        reset_peak();
    }
    Json(body).into_response()
}

/// Runs `action` on the blocking pool under the database gate, with the peak
/// RSS reset first, and reports its time and the memory before and after.
async fn measured(
    bench: Bench,
    action: impl FnOnce(&mut Zega, &Bench) -> Result<(), String> + Send + 'static,
) -> Response {
    let result = tokio::task::spawn_blocking(move || {
        let mut db = bench
            .state
            .zega
            .lock()
            .map_err(|_| "database lock poisoned".to_string())?;
        let before = memory();
        reset_peak();
        let start = Instant::now();
        action(&mut db, &bench)?;
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        Ok::<_, String>(json!({
            "ok": true,
            "ms": ms,
            "before": before,
            "after": memory(),
            "disk": bench.disk(),
        }))
    })
    .await;
    match result {
        Ok(Ok(body)) => Json(body).into_response(),
        Ok(Err(error)) => failure(error),
        Err(_) => failure("worker failed"),
    }
}

async fn snapshot(State(bench): State<Bench>, headers: HeaderMap) -> Response {
    if !allowed(&bench, &headers) {
        return denied();
    }
    measured(bench, |db, _| db.snapshot().map_err(|e| e.to_string())).await
}

async fn reload(State(bench): State<Bench>, headers: HeaderMap) -> Response {
    if !allowed(&bench, &headers) {
        return denied();
    }
    measured(bench, |db, bench| {
        // Close the current database first (its WAL writer too), so the old
        // and new graphs never coexist; then open the directory as a fresh
        // process would.
        *db = Zega::in_memory().build().map_err(|e| e.to_string())?;
        *db = open(&bench.data)?;
        Ok(())
    })
    .await
}

async fn wipe(State(bench): State<Bench>, headers: HeaderMap) -> Response {
    if !allowed(&bench, &headers) {
        return denied();
    }
    measured(bench, |db, bench| {
        *db = Zega::in_memory().build().map_err(|e| e.to_string())?;
        std::fs::remove_dir_all(&bench.data).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&bench.data).map_err(|e| e.to_string())?;
        *db = open(&bench.data)?;
        Ok(())
    })
    .await
}

/// The browser origins allowed to call this server.
#[derive(Clone)]
struct Cors {
    origins: Vec<String>,
}

impl Cors {
    /// The request's Origin if it is allowed: listed exactly, or a listed
    /// `http://localhost` / `http://127.0.0.1` with any port.
    fn allowed(&self, headers: &HeaderMap) -> Option<HeaderValue> {
        let origin = headers.get(header::ORIGIN)?;
        let text = origin.to_str().ok()?;
        let ok = self.origins.iter().any(|allowed| {
            text == allowed
                || (matches!(allowed.as_str(), "http://localhost" | "http://127.0.0.1")
                    && text
                        .strip_prefix(allowed.as_str())
                        .and_then(|rest| rest.strip_prefix(':'))
                        .is_some_and(|port| {
                            !port.is_empty() && port.len() <= 5 && port.bytes().all(|b| b.is_ascii_digit())
                        }))
        });
        ok.then(|| origin.clone())
    }
}

async fn cors(State(cors): State<Cors>, request: Request, next: Next) -> Response {
    let origin = cors.allowed(request.headers());
    let mut response = if request.method() == Method::OPTIONS {
        let status = if origin.is_some() { StatusCode::NO_CONTENT } else { StatusCode::FORBIDDEN };
        let mut response = status.into_response();
        if origin.is_some() {
            let headers = response.headers_mut();
            headers.insert(header::ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("GET, POST, DELETE"));
            headers.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("authorization, content-type"));
            // Chrome caps this at 2 h; without it every call pays a second round trip.
            headers.insert(header::ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("7200"));
        }
        response
    } else {
        next.run(request).await
    };
    let headers = response.headers_mut();
    headers.append(header::VARY, HeaderValue::from_static("Origin"));
    if let Some(origin) = origin {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    }
    response
}

fn open(data: &std::path::Path) -> Result<Zega, String> {
    let path = data.to_str().ok_or("data path must be UTF-8")?;
    Zega::open(path).build().map_err(|e| e.to_string())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let started_at_ms = unix_ms();
    let options = parse();
    // Same runtime shape as `zega start`.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(std::thread::available_parallelism()?.get())
        .enable_all()
        .build()?;
    runtime.block_on(run(options, started_at_ms))
}

async fn run(options: Options, started_at_ms: u128) -> Result<(), Box<dyn std::error::Error>> {
    let token = options
        .token_file
        .as_ref()
        .map(std::fs::read_to_string)
        .transpose()?;
    let token = token.as_deref().map(str::trim);
    if token.is_some_and(|t| t.is_empty() || t.chars().any(char::is_whitespace)) {
        return Err("token file must contain one nonempty bearer token".into());
    }
    std::fs::create_dir_all(&options.data)?;
    let open_start = Instant::now();
    let db = open(&options.data)?;
    let open_ms = open_start.elapsed().as_secs_f64() * 1000.0;
    let state = AppState::new(db, token);
    let bench = Bench {
        state: state.clone(),
        data: options.data.clone(),
        started_at_ms,
        open_ms,
    };
    let listener = TcpListener::bind((options.host, options.port)).await?;
    println!(
        "{}",
        json!({
            "event": "ready",
            "startedAtMs": started_at_ms,
            "openMs": open_ms,
            "readyAtMs": unix_ms(),
            "memory": memory(),
            "machine": machine_memory(),
            "disk": bench.disk(),
            "listen": listener.local_addr()?.to_string(),
            "corsOrigins": options.cors_origins,
        })
    );
    let measurement = Router::new()
        .route("/mem", get(mem))
        .route("/snapshot", post(snapshot))
        .route("/reload", post(reload))
        .route("/wipe", post(wipe))
        .with_state(bench);
    let mut app = zega_server::routes::app(state).merge(measurement);
    if !options.cors_origins.is_empty() {
        let cors_state = Cors { origins: options.cors_origins.clone() };
        app = app.layer(middleware::from_fn_with_state(cors_state, cors));
    }
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

/// Cloudflare stops a container with SIGTERM, and PID 1 ignores any signal it
/// has no handler for, so SIGTERM needs one (`zega start` handles only Ctrl-C).
/// Fly stops a Machine with SIGINT by default, which `ctrl_c` covers.
async fn shutdown() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("install SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, HeaderValue::from_str(value).unwrap());
        headers
    }

    #[test]
    fn cors_allows_listed_origins_and_local_ports_only() {
        let cors = Cors {
            origins: vec!["https://explorer2.zega.dev".into(), "http://localhost".into()],
        };
        for ok in ["https://explorer2.zega.dev", "http://localhost", "http://localhost:8788"] {
            assert!(cors.allowed(&origin(ok)).is_some(), "{ok}");
        }
        for no in [
            "https://explorer2.zega.dev.evil.com",
            "http://explorer2.zega.dev",
            "https://explorer.zega.dev",
            "http://localhost:",
            "http://localhost:87a8",
            "http://localhost.evil.com",
            "http://127.0.0.1:8788",
        ] {
            assert!(cors.allowed(&origin(no)).is_none(), "{no}");
        }
        assert!(cors.allowed(&HeaderMap::new()).is_none());
    }
}
