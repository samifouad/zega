use crate::{auth, classification, AppState};
use axum::{
    extract::{rejection::JsonRejection, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Deserializer, Serialize};
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
    params: HashMap<String, JsonValue>,
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
    let params = match request
        .params
        .into_iter()
        .map(|(key, value)| raw_to_value(value).map(|value| (key, value)))
        .collect()
    {
        Ok(params) => params,
        Err(message) => return error(StatusCode::BAD_REQUEST, message),
    };
    let is_write = match classification::cql_is_write(&request.query) {
        Ok(value) => value,
        Err(message) => return error(StatusCode::BAD_REQUEST, message),
    };
    let result = if is_write {
        state.zega.write().await.query(&request.query, params)
    } else {
        state.zega.read().await.query(&request.query, params)
    };
    match result {
        Ok(rows) => {
            let rows: Vec<_> = rows
                .into_iter()
                .map(|row| {
                    row.fields
                        .into_iter()
                        .map(|(key, value)| (key, value_to_raw(value)))
                        .collect::<HashMap<_, _>>()
                })
                .collect();
            Json(json!({"ok": true, "count": rows.len(), "rows": rows})).into_response()
        }
        Err(cause) => error(StatusCode::BAD_REQUEST, cause.to_string()),
    }
}

#[derive(Deserialize)]
pub struct KvRequest {
    op: String,
    #[serde(default)]
    key: String,
    #[serde(default)]
    value: JsonValueField,
    ttl: Option<u64>,
    #[serde(default)]
    nx: bool,
    start: Option<usize>,
    stop: Option<usize>,
    cursor: Option<usize>,
    pattern: Option<String>,
    count: Option<usize>,
}

// Unlike Option<JsonValue>, this distinguishes a missing field from `"value": null`.
#[derive(Default)]
struct JsonValueField(Option<JsonValue>);

impl<'de> Deserialize<'de> for JsonValueField {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        JsonValue::deserialize(deserializer).map(|value| Self(Some(value)))
    }
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
        "get" => zega
            .kv_get(&request.key)
            .map_or(JsonValue::Null, value_to_raw),
        "set" => {
            let value = raw_to_value(required(request.value.0, "value")?)?;
            if request.nx {
                json!(zega
                    .kv_set_nx(request.key, value, request.ttl)
                    .map_err(|error| error.to_string())?)
            } else {
                zega.kv_set(request.key, value, request.ttl)
                    .map_err(|error| error.to_string())?;
                json!(true)
            }
        }
        "del" => json!(zega
            .kv_del(&request.key)
            .map_err(|error| error.to_string())?),
        "incr" => value_to_raw(
            zega.kv_incr(&request.key)
                .map_err(|error| error.to_string())?,
        ),
        "exists" => json!(zega.kv_exists(&request.key)),
        "ttl" => json!(zega.kv_ttl(&request.key)),
        "expire" => json!(zega
            .kv_expire(&request.key, required(request.ttl, "ttl")?)
            .map_err(|error| error.to_string())?),
        "lpush" => json!(zega
            .kv_lpush(
                &request.key,
                raw_to_value(required(request.value.0, "value")?)?
            )
            .map_err(|error| error.to_string())?),
        "lrange" => zega
            .kv_lrange(
                &request.key,
                required(request.start, "start")?,
                required(request.stop, "stop")?,
            )
            .map_err(|error| error.to_string())?
            .map_or(JsonValue::Null, |values| {
                JsonValue::Array(values.into_iter().map(value_to_raw).collect())
            }),
        "ltrim" => json!(zega
            .kv_ltrim(
                &request.key,
                required(request.start, "start")?,
                required(request.stop, "stop")?
            )
            .map_err(|error| error.to_string())?),
        "rpush" => json!(zega
            .kv_rpush(
                &request.key,
                raw_to_value(required(request.value.0, "value")?)?
            )
            .map_err(|error| error.to_string())?),
        "incr_with_ttl" => value_to_raw(
            zega.kv_incr_with_ttl(&request.key, required(request.ttl, "ttl")?)
                .map_err(|error| error.to_string())?,
        ),
        "scan" => {
            let cursor = request.cursor.unwrap_or(0);
            let pattern = request.pattern.unwrap_or_default();
            let count = request.count.unwrap_or(10);
            let (next_cursor, keys) = zega.kv_scan(cursor, &pattern, count);
            json!({"cursor": next_cursor, "keys": keys})
        }
        _ => return Err(format!("unsupported KV operation: {}", request.op)),
    };
    Ok(value)
}

fn raw_to_value(value: JsonValue) -> Result<Value, String> {
    match value {
        JsonValue::Null => Ok(Value::Null),
        JsonValue::Bool(value) => Ok(Value::Bool(value)),
        JsonValue::Number(value) => {
            if let Some(value) = value.as_i64() {
                Ok(Value::Int(value))
            } else if value.is_u64() {
                Err(format!(
                    "integer is outside the supported i64 range: {value}"
                ))
            } else {
                value
                    .as_f64()
                    .map(Value::from_f64)
                    .ok_or_else(|| format!("invalid JSON number: {value}"))
            }
        }
        JsonValue::String(value) => Ok(Value::String(value)),
        JsonValue::Array(values) => values
            .into_iter()
            .map(raw_to_value)
            .collect::<Result<Vec<_>, _>>()
            .map(Value::List),
        JsonValue::Object(values) => values
            .into_iter()
            .map(|(key, value)| raw_to_value(value).map(|value| (key, value)))
            .collect::<Result<HashMap<_, _>, _>>()
            .map(Value::Map),
    }
}

fn value_to_raw(value: Value) -> JsonValue {
    match value {
        Value::Null => JsonValue::Null,
        Value::Bool(value) => JsonValue::Bool(value),
        Value::Int(value) => json!(value),
        Value::Float(bits) => json!(f64::from_bits(bits)),
        Value::String(value) => JsonValue::String(value),
        Value::List(values) => JsonValue::Array(values.into_iter().map(value_to_raw).collect()),
        Value::Map(values) => JsonValue::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, value_to_raw(value)))
                .collect(),
        ),
    }
}

fn required<T>(value: Option<T>, name: &str) -> Result<T, String> {
    value.ok_or_else(|| format!("missing required field: {name}"))
}
