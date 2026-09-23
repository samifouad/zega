use serde_json::{json, Map, Value as Json};
use std::collections::HashMap;
use zega::Row;
use zega::Value;

pub type CanonicalRows = Vec<Json>;

fn typed(kind: &str, value: Json) -> Json {
    json!({"type": kind, "value": value})
}

pub fn zega_value(value: &Value) -> Json {
    match value {
        Value::String(value) => typed("string", json!(value)),
        Value::Int(value) => typed("integer", json!(value)),
        Value::Float(bits) => float(f64::from_bits(*bits)),
        Value::Bool(value) => boolean(*value),
        Value::List(values) => typed("list", Json::Array(values.iter().map(zega_value).collect())),
        Value::Map(values) => {
            let mapped = values
                .iter()
                .map(|(key, value)| (key.clone(), zega_value(value)))
                .collect();
            typed("map", Json::Object(mapped))
        }
        Value::Point(point) => typed("point", point.to_json()),
        Value::Vector(v) => typed("vector", v.to_json()),
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

#[allow(dead_code)]
pub fn compare(left: &CanonicalRows, right: &CanonicalRows) -> Result<(), String> {
    compare_with(left, right, |left, right| left == right)
}

#[allow(dead_code)]
pub fn compare_tolerant(left: &CanonicalRows, right: &CanonicalRows) -> Result<(), String> {
    const REL_EPS: f64 = 1e-9;
    const ABS_EPS: f64 = 1e-12;

    // Aggregate float accumulation order differs between Zega and Neo4j, so
    // ULP-level divergence is expected and accepted by live differential tests.
    compare_with(left, right, |left, right| {
        json_equal_tolerant(left, right, false, REL_EPS, ABS_EPS)
    })
}

fn compare_with(
    left: &CanonicalRows,
    right: &CanonicalRows,
    equal: impl Fn(&Json, &Json) -> bool,
) -> Result<(), String> {
    if left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| equal(left, right))
    {
        return Ok(());
    }

    let max = left.len().max(right.len());
    let first = (0..max).find(|index| match (left.get(*index), right.get(*index)) {
        (Some(left), Some(right)) => !equal(left, right),
        _ => true,
    });
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

fn json_equal_tolerant(
    left: &Json,
    right: &Json,
    float_value: bool,
    rel_eps: f64,
    abs_eps: f64,
) -> bool {
    match (left, right) {
        (Json::Number(left), Json::Number(right)) if float_value => {
            let (Some(left), Some(right)) = (left.as_f64(), right.as_f64()) else {
                return left == right;
            };
            (left - right).abs() <= abs_eps.max(rel_eps * left.abs().max(right.abs()))
        }
        (Json::Array(left), Json::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| json_equal_tolerant(left, right, false, rel_eps, abs_eps))
        }
        (Json::Object(left), Json::Object(right)) => {
            if left.len() != right.len() {
                return false;
            }
            let tagged_float =
                left.get("type") == Some(&json!("float")) && right.get("type") == left.get("type");
            left.iter().all(|(key, left)| {
                right.get(key).is_some_and(|right| {
                    json_equal_tolerant(
                        left,
                        right,
                        tagged_float && key == "value",
                        rel_eps,
                        abs_eps,
                    )
                })
            })
        }
        _ => left == right,
    }
}
