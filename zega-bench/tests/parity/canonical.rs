use serde_json::{json, Map, Value as Json};
use std::collections::HashMap;
use zega_core::Row;
use zega_parser::Value;

pub type CanonicalRows = Vec<Json>;

fn typed(kind: &str, value: Json) -> Json {
    json!({"type": kind, "value": value})
}

pub fn zega_value(value: &Value) -> Json {
    match value {
        Value::String(value) => typed("string", json!(value)),
        Value::Int(value) => typed("integer", json!(value)),
        Value::Float(bits) => float(f64::from_bits(*bits)),
        Value::Bool(value) => typed("boolean", json!(value)),
        Value::List(values) => typed("list", Json::Array(values.iter().map(zega_value).collect())),
        Value::Map(values) => {
            let mapped = values
                .iter()
                .map(|(key, value)| (key.clone(), zega_value(value)))
                .collect();
            typed("map", Json::Object(mapped))
        }
        Value::Null => typed("null", Json::Null),
    }
}

pub fn zega_rows(rows: Vec<Row>, ordered: bool) -> CanonicalRows {
    let rows = rows
        .into_iter()
        .map(|row| canonical_row(&row.fields))
        .collect();
    normalize(rows, ordered)
}

fn canonical_row(fields: &HashMap<String, Value>) -> Json {
    Json::Object(
        fields
            .iter()
            .map(|(key, value)| (key.clone(), zega_value(value)))
            .collect::<Map<_, _>>(),
    )
}

pub fn string(value: impl Into<String>) -> Json {
    typed("string", json!(value.into()))
}

pub fn integer(value: i64) -> Json {
    typed("integer", json!(value))
}

pub fn float(value: f64) -> Json {
    // serde_json renders non-finite floats as null, which would collapse
    // +inf / -inf / NaN into one value and hide real mismatches. Encode them
    // as distinct string sentinels so a non-finite divergence is always caught.
    let encoded = if value.is_finite() {
        json!(value)
    } else if value.is_nan() {
        json!("NaN")
    } else if value.is_sign_positive() {
        json!("+inf")
    } else {
        json!("-inf")
    };
    typed("float", encoded)
}

pub fn boolean(value: bool) -> Json {
    typed("boolean", json!(value))
}

pub fn null() -> Json {
    typed("null", Json::Null)
}

pub fn row(fields: impl IntoIterator<Item = (String, Json)>) -> Json {
    Json::Object(fields.into_iter().collect())
}

pub fn normalize(mut rows: CanonicalRows, ordered: bool) -> CanonicalRows {
    if !ordered {
        rows.sort_by_cached_key(|row| serde_json::to_string(row).expect("canonical JSON"));
    }
    rows
}

pub fn compare(left: &CanonicalRows, right: &CanonicalRows) -> Result<(), String> {
    if left == right {
        return Ok(());
    }

    let max = left.len().max(right.len());
    let first = (0..max).find(|index| left.get(*index) != right.get(*index));
    Err(format!(
        "first difference at row {}:\n  zega: {}\n  reference: {}",
        first.unwrap_or(0),
        first
            .and_then(|index| left.get(index))
            .map(Json::to_string)
            .unwrap_or_else(|| "<missing>".into()),
        first
            .and_then(|index| right.get(index))
            .map(Json::to_string)
            .unwrap_or_else(|| "<missing>".into())
    ))
}
