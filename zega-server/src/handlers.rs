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

/// Chunks in flight between the engine and the socket, each one write of the
/// exporter's 64 KiB buffer: what bounds a download's memory.
const CHUNKS_IN_FLIGHT: usize = 4;

type Chunk = Result<Bytes, io::Error>;

/// Hands each write to the response body, blocking while the client is
/// behind. A client that went away fails the write, which ends the export.
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

/// Reads a request body the async side forwards chunk by chunk.
struct BodyReader {
    chunks: tokio::sync::mpsc::Receiver<Chunk>,
    current: Bytes,
}

impl io::Read for BodyReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.current.is_empty() {
            match self.chunks.blocking_recv() {
                None => return Ok(0),
                Some(chunk) => self.current = chunk?,
            }
        }
        let n = buf.len().min(self.current.len());
        buf[..n].copy_from_slice(&self.current.split_to(n));
        Ok(n)
    }
}

/// `GET /graph`: the whole graph as a `.graph` file (docs/graph-format.md),
/// streamed as it is written. `Accept: application/json` gets the JSON view
/// the explorer draws instead.
pub async fn graph(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !authorized(&headers, &state) {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let wants_json = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("application/json"));
    if wants_json {
        return execute(state, Zega::graph_json).await;
    }
    let (send, mut receive) = tokio::sync::mpsc::channel::<Chunk>(CHUNKS_IN_FLIGHT);
    tokio::task::spawn_blocking(move || {
        let result = match state.zega.lock() {
            Ok(db) => db
                .export(&mut BodyWriter(send.clone()))
                .map_err(|error| io::Error::other(error.to_string())),
            Err(_) => Err(io::Error::other("database lock poisoned")),
        };
        // Headers are already sent: failing the body is how the client
        // learns the file is incomplete (and a .graph file without its
        // final section never imports).
        if let Err(error) = result {
            let _ = send.blocking_send(Err(error));
        }
    });
    let body = futures_util::stream::poll_fn(move |context| receive.poll_recv(context));
    Response::builder()
        .header(header::CONTENT_TYPE, zega::graph_file::MEDIA_TYPE)
        .header(header::CONTENT_DISPOSITION, "attachment; filename=\"graph.graph\"")
        .body(Body::from_stream(body))
        .unwrap_or_else(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "response failed"))
}

/// `PUT /graph`: replace the whole graph with the `.graph` file in the body.
/// The body is decoded as it arrives; nothing changes unless all of it is a
/// valid file.
pub async fn import_graph(State(state): State<AppState>, headers: HeaderMap, body: Body) -> Response {
    if !authorized(&headers, &state) {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let (send, receive) = tokio::sync::mpsc::channel::<Chunk>(CHUNKS_IN_FLIGHT);
    let import = tokio::task::spawn_blocking(move || {
        let db = state
            .zega
            .lock()
            .map_err(|_| ZegaError::Execution("database lock poisoned".into()))?;
        db.import(BodyReader {
            chunks: receive,
            current: Bytes::new(),
        })
    });
    let mut body = body.into_data_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|error| io::Error::other(error.to_string()));
        let failed = chunk.is_err();
        // A send fails once the importer has stopped reading (it already
        // knows the file is bad); its error is the one to report.
        if send.send(chunk).await.is_err() || failed {
            break;
        }
    }
    drop(send);
    match import.await {
        Ok(Ok(summary)) => Json(json!({"ok": true, "result": summary})).into_response(),
        Ok(Err(cause)) => error(StatusCode::BAD_REQUEST, cause.to_string()),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "database worker failed"),
    }
}

pub async fn clear(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !authorized(&headers, &state) {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    execute(state, |db| {
        let graph = db.graph_json()?;
        for node in graph["nodes"].as_array().into_iter().flatten() {
            if let Some(id) = node["id"].as_u64() {
                db.delete_node(id)?;
            }
        }
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
