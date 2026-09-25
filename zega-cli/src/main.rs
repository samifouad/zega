mod fmt;

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
    path::{Path, PathBuf},
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
    /// Format ZQL and JSON files with the canonical layout.
    Fmt {
        /// Files or directories to format recursively (*.zql, *.json).
        // `--lang` conflicts with paths rather than `requires = "stdin"`: clap
        // counts a bool flag's implicit `false` as present, so `requires` never
        // fired and `--lang` was ignored for files (zegadb/zega#67).
        #[arg(required_unless_present = "stdin", conflicts_with_all = ["stdin", "lang"])]
        paths: Vec<PathBuf>,
        /// List files that would change and exit with code 1.
        #[arg(long)]
        check: bool,
        /// Read source from stdin and write formatted source to stdout.
        #[arg(long)]
        stdin: bool,
        /// Language for stdin (defaults to zql). Files use their extension.
        #[arg(long, value_enum)]
        lang: Option<fmt::Language>,
    },
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
        /// Stop any ZQL statement still running after this many seconds, and
        /// roll back its writes. 0 turns the limit off.
        #[arg(long, value_name = "SECONDS", default_value_t = zega_server::DEFAULT_QUERY_TIME_LIMIT.as_secs_f64())]
        query_time_limit: f64,
    },
    /// Write the database to a .graph file (docs/graph-format.md). `-` writes to stdout.
    Export {
        /// The .graph file to write. It appears only once complete.
        file: PathBuf,
        #[arg(long, default_value = "./zega-data")]
        data: PathBuf,
        /// A ZQL schema file to carry in the .graph file (types, unique, index).
        #[arg(long)]
        schema: Option<PathBuf>,
        /// Manifest metadata, repeatable: --meta licence=CC-BY-4.0 --meta source=...
        #[arg(long = "meta", value_name = "KEY=VALUE")]
        meta: Vec<String>,
    },
    /// Replace the database with the graph in a .graph file. `-` reads stdin.
    /// All or nothing: a damaged file changes nothing.
    Import {
        /// The .graph file to read.
        file: PathBuf,
        #[arg(long, default_value = "./zega-data")]
        data: PathBuf,
        /// Replace a database that already holds nodes or relationships.
        #[arg(long)]
        replace: bool,
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
    if let Command::Fmt { paths, check, stdin, lang } = cli.command {
        // 1 means `--check` found a file to reformat; 2 means zega fmt could not
        // do its job, the same code clap uses for bad arguments (zegadb/zega#66).
        match fmt::run(paths, check, stdin, lang) {
            Ok(true) => return Ok(()),
            Ok(false) => std::process::exit(1),
            Err(error) => {
                eprintln!("zega fmt: {error}");
                std::process::exit(2);
            }
        }
    }
    match cli.command {
        Command::Export { file, data, schema, meta } => {
            report("export", export(&file, &data, schema, &meta))
        }
        Command::Import { file, data, replace } => report("import", import(&file, &data, replace)),
        command => serve_command(Cli { command }),
    }
}

/// `zega export: <message>` and exit 1, rather than the debug dump `main`
/// would print: these errors are for the person at the terminal.
fn report(command: &str, result: Result<(), Box<dyn std::error::Error>>) -> ! {
    match result {
        Ok(()) => std::process::exit(0),
        Err(error) => {
            eprintln!("zega {command}: {error}");
            std::process::exit(1);
        }
    }
}

/// Take the data directory's lock, the one `zega start` and `zega explorer`
/// hold for as long as they run: two processes never share a WAL.
fn lock_data(data: &Path) -> Result<std::fs::File, Box<dyn std::error::Error>> {
    std::fs::create_dir_all(data)?;
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
    Ok(data_lock)
}

fn open_data(data: &Path) -> Result<(std::fs::File, Zega), Box<dyn std::error::Error>> {
    let lock = lock_data(data)?;
    let path = data.to_str().ok_or("data path must be UTF-8")?;
    let db = Zega::open(path).build().map_err(io::Error::other)?;
    Ok((lock, db))
}

fn export(
    file: &Path,
    data: &Path,
    schema: Option<PathBuf>,
    meta: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut options = zega::graph_file::ExportOptions {
        schema: schema.map(std::fs::read_to_string).transpose()?,
        ..Default::default()
    };
    for entry in meta {
        let (key, value) = entry
            .split_once('=')
            .ok_or_else(|| format!("--meta {entry:?} must be KEY=VALUE"))?;
        options.meta.insert(key.to_string(), value.to_string());
    }
    let (_lock, db) = open_data(data)?;
    let summary = if file == Path::new("-") {
        db.export_with(&mut io::stdout().lock(), &options)?
    } else {
        // Written beside the target and renamed into place, so a failed
        // export never leaves a partial file under the real name.
        let mut partial = file.as_os_str().to_owned();
        partial.push(".partial");
        let partial = PathBuf::from(partial);
        let written = std::fs::File::create(&partial).map_err(Into::into).and_then(|mut out| {
            let summary = db.export_with(&mut out, &options)?;
            out.sync_all()?;
            std::fs::rename(&partial, file)?;
            Ok::<_, Box<dyn std::error::Error>>(summary)
        });
        if written.is_err() {
            let _ = std::fs::remove_file(&partial);
        }
        written?
    };
    eprintln!(
        "wrote {} nodes and {} relationships to {} ({} bytes, .graph format {})",
        summary.nodes,
        summary.relationships,
        file.display(),
        summary.bytes,
        zega::graph_file::FORMAT_VERSION
    );
    Ok(())
}

fn import(file: &Path, data: &Path, replace: bool) -> Result<(), Box<dyn std::error::Error>> {
    let (_lock, db) = open_data(data)?;
    if !replace && !db.is_empty()? {
        return Err(format!(
            "{} already holds a graph; pass --replace to replace it with {}",
            data.display(),
            file.display()
        )
        .into());
    }
    let summary = if file == Path::new("-") {
        db.import(io::stdin().lock())?
    } else {
        db.import(std::fs::File::open(file)?)?
    };
    eprintln!(
        "imported {} nodes and {} relationships from {} (.graph format {}, written by {})",
        summary.nodes,
        summary.relationships,
        file.display(),
        summary.format_version,
        summary.created_by
    );
    Ok(())
}

fn serve_command(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(std::thread::available_parallelism()?.get())
        .enable_all()
        .build()?;
    runtime.block_on(run(cli))
}

async fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let (data, host, port, token_file, allow_private, explorer, time_limit) = match cli.command {
        Command::Fmt { .. } | Command::Export { .. } | Command::Import { .. } => {
            unreachable!("fmt, export and import run without a server runtime")
        }
        Command::Start {
            data,
            host,
            port,
            token_file,
            allow_private_imports,
            query_time_limit,
        } => {
            let limit = std::time::Duration::try_from_secs_f64(query_time_limit)
                .map_err(|_| "--query-time-limit must be a number of seconds, 0 or more")?;
            let limit = (!limit.is_zero()).then_some(limit);
            (data, host, port, token_file, allow_private_imports, false, limit)
        }
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
            // The explorer is one person's local database; nothing to share.
            None,
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
    let _data_lock = lock_data(&data)?;
    let path = data.to_str().ok_or("data path must be UTF-8")?;
    let mut db = Zega::open(path).allow_private_imports(allow_private);
    if let Some(limit) = time_limit {
        db = db.query_time_limit(limit);
    }
    let db = db.build().map_err(io::Error::other)?;
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
        Some("geojson") => "application/geo+json",
        Some("ttf") => "font/ttf",
        _ => "text/plain; charset=utf-8",
    };
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-cache")
        .header("x-content-type-options", "nosniff")
        .body(Body::from(file.contents()))
        .unwrap()
}
