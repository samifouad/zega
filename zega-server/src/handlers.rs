use crate::{auth, AppState};
use axum::{
    extract::{rejection::JsonRejection, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
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

pub async fn graph(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !authorized(&headers, &state) {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    execute(state, Zega::graph_json).await
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
