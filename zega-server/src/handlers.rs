use crate::{auth, AppState};
use axum::{
    body::{Body, Bytes},
    extract::{rejection::JsonRejection, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use futures_util::StreamExt;
use std::io;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use zega::{Zega, ZegaError};

fn error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({"ok": false, "error": message.into()}))).into_response()
}

fn authorized(headers: &HeaderMap, state: &AppState) -> bool {
    state
        .token_hash
        .as_ref()
        .is_none_or(|hash| auth::authorized(headers, hash))
}

pub async fn health(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !authorized(&headers, &state) {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    Json(json!({"ok": true})).into_response()
}

async fn execute(
    state: AppState,
    action: impl FnOnce(&Zega) -> Result<Value, ZegaError> + Send + 'static,
) -> Response {
    match tokio::task::spawn_blocking(move || {
        let db = state
            .zega
            .lock()
            .map_err(|_| ZegaError::Execution("database lock poisoned".into()))?;
        action(&db)
    })
    .await
    {
        Ok(Ok(result)) => Json(json!({"ok": true, "result": result})).into_response(),
        // Its own code, so a client can tell "this query is too slow" from
        // "this query is wrong". A 4xx, like any other refused query: a 5xx
        // or 408 invites an automatic retry of the same slow query.
        Ok(Err(cause @ ZegaError::QueryTimeLimit { .. })) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"ok": false, "error": cause.to_string(), "code": "query_time_limit"})),
        )
            .into_response(),
        Ok(Err(cause)) => error(StatusCode::BAD_REQUEST, cause.to_string()),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "database worker failed"),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZqlRequest {
    #[serde(default)]
    schema: String,
    query: String,
    #[serde(default)]
    document: bool,
    /// Optional raw text from a browser file picker; native requests omit this
    /// and use the library's own file/HTTP transport.
    sources: Option<HashMap<String, String>>,
}

pub async fn zql(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Result<Json<ZqlRequest>, JsonRejection>,
) -> Response {
    if !authorized(&headers, &state) {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => return error(StatusCode::BAD_REQUEST, rejection.body_text()),
    };
    execute(state, move |db| match (request.document, request.sources) {
        (false, None) => db.run_lang(&request.schema, &request.query),
        (false, Some(sources)) => {
            db.run_lang_with_sources(&request.schema, &request.query, &sources)
        }
        (true, None) => db.apply_zql(&request.query),
        (true, Some(sources)) => db.apply_zql_with_sources(&request.query, &sources),
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VectorViewRequest {
    schema: String,
    result: Value,
    kind: String,
    selected: Option<u64>,
    k: usize,
    threshold: f64,
}

pub async fn vector_view(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Result<Json<VectorViewRequest>, JsonRejection>,
) -> Response {
    if !authorized(&headers, &state) {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => return error(StatusCode::BAD_REQUEST, rejection.body_text()),
    };
    let kind = match request.kind.as_str() {
        "vector2d" => zega::ViewKind::Vector2d,
        "vector3d" => zega::ViewKind::Vector3d,
        _ => return error(StatusCode::BAD_REQUEST, "expected vector2d or vector3d"),
    };
    execute(state, move |db| {
        db.vector_view(
            &request.schema,
            &request.result,
            kind,
            request.selected,
            request.k,
            request.threshold,
        )
    })
    .await
}

/// Chunks in flight between a file and the socket: what bounds a
/// transfer's memory.
const CHUNKS_IN_FLIGHT: usize = 4;
const CHUNK: usize = 64 * 1024;

type Chunk = Result<Bytes, io::Error>;

/// Deletes a staging file when the transfer that owns it ends, however it
/// ends.
struct Staged(std::path::PathBuf);

impl Drop for Staged {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Hands each write to the response body, blocking while the client is
/// behind. A client that went away fails the write.
struct BodyWriter(tokio::sync::mpsc::Sender<Chunk>);

impl io::Write for BodyWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .blocking_send(Ok(Bytes::copy_from_slice(buf)))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "the client went away"))?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// How much a client wants each representation of `/graph`, from its
/// `Accept` header (RFC 9110 §12.5.1): `(json, graph)` q-values. The most
/// specific range that matches a type sets its q-value.
fn accepted(headers: &HeaderMap) -> (f32, f32) {
    let Some(accept) = headers.get(header::ACCEPT).and_then(|value| value.to_str().ok()) else {
        return (0.0, 1.0);
    };
    let mut ranges = Vec::new();
    for item in accept.split(',') {
        let mut parts = item.split(';');
        let range = parts.next().unwrap_or("").trim().to_ascii_lowercase();
        if range.is_empty() {
            continue;
        }
        let q = parts
            .filter_map(|param| param.trim().strip_prefix("q=").or_else(|| param.trim().strip_prefix("Q=")))
            .find_map(|q| q.trim().parse::<f32>().ok())
            .map_or(1.0, |q| q.clamp(0.0, 1.0));
        ranges.push((range, q));
    }
    let quality = |media: &str| {
        let (kind, _) = media.split_once('/').unwrap_or((media, ""));
        let exact = ranges.iter().find(|(range, _)| range == media);
        let family = ranges.iter().find(|(range, _)| range.strip_suffix("/*") == Some(kind));
        let any = ranges.iter().find(|(range, _)| range == "*/*");
        exact.or(family).or(any).map_or(0.0, |(_, q)| *q)
    };
    (quality("application/json"), quality(zega::graph_file::MEDIA_TYPE))
}

fn with_vary(mut response: Response) -> Response {
    response
        .headers_mut()
        .append(header::VARY, header::HeaderValue::from_static("accept"));
    response
}

/// `GET /graph`: the whole graph as a `.graph` file (docs/graph-format.md).
/// A client that prefers `application/json` gets the JSON view the explorer
/// draws instead.
///
/// On a disk database the file is written to a staging file under the gate,
/// which is released before a byte goes to the client: a slow download
/// holds disk space, never the database. In memory there is no staging
/// directory, so the export streams under the gate.
pub async fn graph(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !authorized(&headers, &state) {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let (json, graph) = accepted(&headers);
    if json <= 0.0 && graph <= 0.0 {
        return with_vary(error(
            StatusCode::NOT_ACCEPTABLE,
            format!("GET /graph serves {} or application/json", zega::graph_file::MEDIA_TYPE),
        ));
    }
    if json > graph {
        return with_vary(execute(state, Zega::graph_json).await);
    }
    let (send, mut receive) = tokio::sync::mpsc::channel::<Chunk>(CHUNKS_IN_FLIGHT);
    let Some(slot) = transfer_slot(&state) else {
        return with_vary(busy());
    };
    let gate = state.zega.clone();
    let spooled = tokio::task::spawn_blocking(move || -> Result<Option<(Staged, u64)>, ZegaError> {
        let db = gate
            .lock()
            .map_err(|_| ZegaError::Execution("database lock poisoned".into()))?;
        let Some((file, path)) = db.staging_file("export")? else {
            return Ok(None);
        };
        let staged = Staged(path);
        let mut out = io::BufWriter::new(file);
        let summary = db.export(&mut out)?;
        drop(db);
        out.into_inner().map_err(|error| error.into_error())?;
        Ok(Some((staged, summary.bytes)))
    })
    .await;
    let (staged, length) = match spooled {
        Ok(Ok(Some(spooled))) => spooled,
        Ok(Ok(None)) => {
            // In memory there is nowhere to stage: stream under the gate.
            tokio::task::spawn_blocking(move || {
                let result = match state.zega.lock() {
                    Ok(db) => db
                        .export(&mut BodyWriter(send.clone()))
                        .map_err(|error| io::Error::other(error.to_string())),
                    Err(_) => Err(io::Error::other("database lock poisoned")),
                };
                // Headers are already sent: failing the body is how the
                // client learns the file is incomplete.
                if let Err(error) = result {
                    let _ = send.blocking_send(Err(error));
                }
            });
            let body = futures_util::stream::poll_fn(move |context| receive.poll_recv(context));
            return with_vary(graph_response(Body::from_stream(body), None));
        }
        Ok(Err(cause)) => return with_vary(error(StatusCode::INTERNAL_SERVER_ERROR, cause.to_string())),
        Err(_) => return with_vary(error(StatusCode::INTERNAL_SERVER_ERROR, "database worker failed")),
    };
    let idle = state.transfer_idle_timeout;
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let _slot = slot;
        let result: io::Result<()> = async {
            let mut file = tokio::fs::File::open(&staged.0).await?;
            let mut buf = vec![0u8; CHUNK];
            loop {
                let n = file.read(&mut buf).await?;
                if n == 0 {
                    return Ok(());
                }
                let chunk = Ok(Bytes::copy_from_slice(&buf[..n]));
                match tokio::time::timeout(idle, send.send(chunk)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(_)) => return Ok(()), // the client went away
                    Err(_) => {
                        return Err(io::Error::new(io::ErrorKind::TimedOut, "the download stalled"))
                    }
                }
            }
        }
        .await;
        if let Err(error) = result {
            let _ = send.try_send(Err(error));
        }
        // The file is closed by now; `staged` deletes it.
        drop(staged);
    });
    let body = futures_util::stream::poll_fn(move |context| receive.poll_recv(context));
    with_vary(graph_response(Body::from_stream(body), Some(length)))
}

/// A transfer slot, if one is free.
fn transfer_slot(state: &AppState) -> Option<tokio::sync::OwnedSemaphorePermit> {
    state.transfers.clone().try_acquire_owned().ok()
}

/// The answer when no transfer slot is free.
fn busy() -> Response {
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "too many .graph transfers are in progress; try again shortly",
    )
}

fn graph_response(body: Body, length: Option<u64>) -> Response {
    let mut response = Response::builder()
        .header(header::CONTENT_TYPE, zega::graph_file::MEDIA_TYPE)
        .header(header::CONTENT_DISPOSITION, "attachment; filename=\"graph.graph\"");
    if let Some(length) = length {
        response = response.header(header::CONTENT_LENGTH, length);
    }
    response
        .body(body)
        .unwrap_or_else(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "response failed"))
}
fn too_large(state: &AppState) -> Response {
    error(
        StatusCode::PAYLOAD_TOO_LARGE,
        format!(
            "the upload is larger than this server's {} byte limit (zega start --max-import-bytes)",
            state.max_import_bytes
        ),
    )
}

/// `PUT /graph`: replace the whole graph with the `.graph` file in the body.
/// Nothing changes unless all of it is a valid file.
///
/// The body is spooled to a staging file without the gate, capped at
/// `max_import_bytes` (413) and given up on after `transfer_idle_timeout`
/// without data (408). Only then is the gate taken, to import the file from
/// disk. In memory the body is collected under the same limits instead.
pub async fn import_graph(State(state): State<AppState>, headers: HeaderMap, body: Body) -> Response {
    use tokio::io::AsyncWriteExt;
    if !authorized(&headers, &state) {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if declared.is_some_and(|length| length > state.max_import_bytes) {
        return too_large(&state);
    }
    let Some(_slot) = transfer_slot(&state) else {
        return busy();
    };
    let gate = state.zega.clone();
    let staging = match tokio::task::spawn_blocking(move || match gate.lock() {
        Ok(db) => db.staging_file("upload"),
        Err(_) => Err(ZegaError::Execution("database lock poisoned".into())),
    })
    .await
    {
        Ok(Ok(staging)) => staging,
        Ok(Err(cause)) => return error(StatusCode::INTERNAL_SERVER_ERROR, cause.to_string()),
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "database worker failed"),
    };
    let (mut file, staged, mut memory) = match staging {
        Some((file, path)) => (Some(tokio::fs::File::from_std(file)), Some(Staged(path)), None),
        None => (None, None, Some(Vec::new())),
    };
    let mut body = body.into_data_stream();
    let mut received: u64 = 0;
    loop {
        let chunk = match tokio::time::timeout(state.transfer_idle_timeout, body.next()).await {
            Err(_) => {
                return error(
                    StatusCode::REQUEST_TIMEOUT,
                    format!(
                        "the upload stalled: no data for {} s",
                        state.transfer_idle_timeout.as_secs_f64()
                    ),
                )
            }
            Ok(None) => break,
            Ok(Some(Err(cause))) => return error(StatusCode::BAD_REQUEST, cause.to_string()),
            Ok(Some(Ok(chunk))) => chunk,
        };
        received += chunk.len() as u64;
        if received > state.max_import_bytes {
            return too_large(&state);
        }
        let written = match (&mut file, &mut memory) {
            (Some(file), _) => file.write_all(&chunk).await,
            (None, Some(memory)) => {
                memory.extend_from_slice(&chunk);
                Ok(())
            }
            (None, None) => Ok(()),
        };
        if let Err(cause) = written {
            return error(StatusCode::INTERNAL_SERVER_ERROR, cause.to_string());
        }
    }
    if let Some(mut file) = file {
        if let Err(cause) = file.flush().await {
            return error(StatusCode::INTERNAL_SERVER_ERROR, cause.to_string());
        }
    }
    let import = tokio::task::spawn_blocking(move || {
        let db = state
            .zega
            .lock()
            .map_err(|_| ZegaError::Execution("database lock poisoned".into()))?;
        match (staged, memory) {
            (Some(staged), _) => db.import_staged(&staged.0),
            (None, Some(memory)) => db.import(&memory[..]),
            (None, None) => Err(ZegaError::Execution("nothing was uploaded".into())),
        }
    });
    match import.await {
        Ok(Ok(summary)) => Json(json!({"ok": true, "result": summary})).into_response(),
        Ok(Err(cause)) => error(StatusCode::BAD_REQUEST, cause.to_string()),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "database worker failed"),
    }
}

/// `DELETE /graph`: replace the graph with an empty one, durably and as one
/// WAL entry. What an import carried (schema text, declarations, metadata)
/// goes too, so a later export cannot claim the old licence or source.
pub async fn clear(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !authorized(&headers, &state) {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    execute(state, |db| {
        db.clear()?;
        Ok(Value::Null)
    })
    .await
}

pub async fn delete_node(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
) -> Response {
    if !authorized(&headers, &state) {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    execute(state, move |db| {
        db.delete_node(id)?;
        Ok(Value::Null)
    })
    .await
}

pub async fn delete_relationship(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
) -> Response {
    if !authorized(&headers, &state) {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    execute(state, move |db| {
        db.delete_relationship(id)?;
        Ok(Value::Null)
    })
    .await
}

#[derive(Deserialize)]
pub struct Connection {
    schema: String,
    from: u64,
    field: String,
    to: u64,
}

pub async fn connect(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Result<Json<Connection>, JsonRejection>,
) -> Response {
    if !authorized(&headers, &state) {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => return error(StatusCode::BAD_REQUEST, rejection.body_text()),
    };
    execute(state, move |db| {
        db.connect_schema(&request.schema, request.from, &request.field, request.to)?;
        Ok(Value::Null)
    })
    .await
}
