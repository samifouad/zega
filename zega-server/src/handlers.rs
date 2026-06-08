use crate::{auth, classification, AppState};
use axum::{
    extract::{rejection::JsonRejection, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use zega_core::Value;

#[derive(Serialize)]
struct ErrorBody {
    ok: bool,
    error: String,
}

fn error(status: StatusCode, message: impl Into<String>) -> Response {
    (
        status,
        Json(ErrorBody {
            ok: false,
            error: message.into(),
        }),
    )
        .into_response()
}

fn is_authorized(headers: &HeaderMap, state: &AppState) -> bool {
    auth::authorized(headers, &state.token_hash)
}

pub async fn health(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !is_authorized(&headers, &state) {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    Json(json!({"ok": true})).into_response()
}

#[derive(Deserialize)]
pub struct CqlRequest {
    query: String,
    #[serde(default)]
    params: HashMap<String, Value>,
}

pub async fn cql(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Result<Json<CqlRequest>, JsonRejection>,
) -> Response {
    if !is_authorized(&headers, &state) {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => return error(StatusCode::BAD_REQUEST, rejection.body_text()),
    };
    let is_write = match classification::cql_is_write(&request.query) {
        Ok(value) => value,
        Err(message) => return error(StatusCode::BAD_REQUEST, message),
    };
    let result = if is_write {
        state
            .zega
            .write()
            .await
            .query(&request.query, request.params)
    } else {
        state
            .zega
            .read()
            .await
            .query(&request.query, request.params)
    };
    match result {
        Ok(rows) => {
            let rows: Vec<_> = rows.into_iter().map(|row| row.fields).collect();
            Json(json!({"ok": true, "count": rows.len(), "rows": rows})).into_response()
        }
        Err(cause) => error(StatusCode::BAD_REQUEST, cause.to_string()),
    }
}

#[derive(Deserialize)]
pub struct KvRequest {
    op: String,
    key: String,
    value: Option<Value>,
    ttl: Option<u64>,
    start: Option<usize>,
    stop: Option<usize>,
}

pub async fn kv(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Result<Json<KvRequest>, JsonRejection>,
) -> Response {
    if !is_authorized(&headers, &state) {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => return error(StatusCode::BAD_REQUEST, rejection.body_text()),
    };
    let result = if classification::kv_is_write(&request.op) {
        let zega = state.zega.write().await;
        execute_kv(&zega, request)
    } else {
        let zega = state.zega.read().await;
        execute_kv(&zega, request)
    };
    match result {
        Ok(value) => Json(json!({"ok": true, "result": value})).into_response(),
        Err(message) => error(StatusCode::BAD_REQUEST, message),
    }
}

fn execute_kv(zega: &zega_core::Zega, request: KvRequest) -> Result<JsonValue, String> {
    let value = match request.op.as_str() {
        "get" => json!(zega.kv_get(&request.key)),
        "set" => {
            zega.kv_set(request.key, required(request.value, "value")?, request.ttl)
                .map_err(|error| error.to_string())?;
            json!(true)
        }
        "del" => json!(zega
            .kv_del(&request.key)
            .map_err(|error| error.to_string())?),
        "incr" => json!(zega
            .kv_incr(&request.key)
            .map_err(|error| error.to_string())?),
        "exists" => json!(zega.kv_exists(&request.key)),
        "ttl" => json!(zega.kv_ttl(&request.key)),
        "expire" => json!(zega
            .kv_expire(&request.key, required(request.ttl, "ttl")?)
            .map_err(|error| error.to_string())?),
        "lpush" => json!(zega
            .kv_lpush(&request.key, required(request.value, "value")?)
            .map_err(|error| error.to_string())?),
        "lrange" => json!(zega
            .kv_lrange(
                &request.key,
                required(request.start, "start")?,
                required(request.stop, "stop")?
            )
            .map_err(|error| error.to_string())?),
        "ltrim" => json!(zega
            .kv_ltrim(
                &request.key,
                required(request.start, "start")?,
                required(request.stop, "stop")?
            )
            .map_err(|error| error.to_string())?),
        _ => return Err(format!("unsupported KV operation: {}", request.op)),
    };
    Ok(value)
}

fn required<T>(value: Option<T>, name: &str) -> Result<T, String> {
    value.ok_or_else(|| format!("missing required field: {name}"))
}
