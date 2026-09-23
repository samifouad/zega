use crate::{auth, AppState};
use axum::{
    extract::{rejection::JsonRejection, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use zega::Value;

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
    let is_write = match zega::query_is_write(&request.query) {
        Ok(value) => value,
        Err(err) => return error(StatusCode::BAD_REQUEST, err.to_string()),
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
        Value::Point(point) => point.to_json(),
        Value::Map(values) => JsonValue::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, value_to_raw(value)))
                .collect(),
        ),
    }
}
