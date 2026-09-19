use std::collections::HashMap;
use wasm_bindgen::prelude::*;
use zega_core::{Value, Zega};

#[wasm_bindgen]
pub struct ZegaWasm {
    inner: Zega,
}

#[wasm_bindgen]
impl ZegaWasm {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Result<ZegaWasm, JsValue> {
        let inner = Zega::in_memory().build().map_err(to_js_error)?;
        Ok(ZegaWasm { inner })
    }

    pub fn query(&self, zql: String, params_json: String) -> Result<String, JsValue> {
        let params = parse_params(&params_json)?;
        let rows = self.inner.query(&zql, params).map_err(to_js_error)?;
        let json_rows: Vec<serde_json::Value> = rows
            .into_iter()
            .map(|row| {
                let fields = row
                    .fields
                    .into_iter()
                    .map(|(key, value)| (key, value_to_json(value)))
                    .collect();
                serde_json::Value::Object(fields)
            })
            .collect();
        serde_json::to_string(&json_rows).map_err(to_js_error)
    }

    pub fn kv_get(&self, key: String) -> Result<String, JsValue> {
        let value = self.inner.kv_get(&key).unwrap_or(Value::Null);
        serde_json::to_string(&value_to_json(value)).map_err(to_js_error)
    }

    pub fn kv_set(
        &self,
        key: String,
        value_json: String,
        ttl_secs: Option<u64>,
    ) -> Result<(), JsValue> {
        let json = serde_json::from_str(&value_json).map_err(to_js_error)?;
        let value = json_to_value(json);
        self.inner.kv_set(key, value, ttl_secs).map_err(to_js_error)
    }

    pub fn kv_del(&self, key: String) -> Result<bool, JsValue> {
        self.inner.kv_del(&key).map_err(to_js_error)
    }

    /// Serialize the whole database (graph + KV) to a base64 string, so the
    /// browser build can persist it across reloads.
    pub fn export_base64(&self) -> Result<String, JsValue> {
        use base64::Engine;
        let bytes = self.inner.snapshot_bytes().map_err(to_js_error)?;
        Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    /// Restore a database previously produced by `export_base64`, replacing
    /// current state.
    pub fn import_base64(&self, data: String) -> Result<(), JsValue> {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(to_js_error)?;
        self.inner.restore_bytes(&bytes).map_err(to_js_error)
    }
}

fn parse_params(params_json: &str) -> Result<HashMap<String, Value>, JsValue> {
    if params_json.trim().is_empty() {
        return Ok(HashMap::new());
    }

    let json: serde_json::Value = serde_json::from_str(params_json).map_err(to_js_error)?;
    let object = json
        .as_object()
        .ok_or_else(|| JsValue::from_str("params_json must be a JSON object"))?;
    Ok(object
        .iter()
        .map(|(key, value)| (key.clone(), json_to_value(value.clone())))
        .collect())
}

fn json_to_value(value: serde_json::Value) -> Value {
    match value {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(v) => Value::Bool(v),
        serde_json::Value::Number(v) => {
            if let Some(int) = v.as_i64() {
                Value::Int(int)
            } else if let Some(float) = v.as_f64() {
                Value::from_f64(float)
            } else {
                Value::Null
            }
        }
        serde_json::Value::String(v) => Value::String(v),
        serde_json::Value::Array(values) => {
            Value::List(values.into_iter().map(json_to_value).collect())
        }
        serde_json::Value::Object(values) => Value::Map(
            values
                .into_iter()
                .map(|(key, value)| (key, json_to_value(value)))
                .collect(),
        ),
    }
}

fn value_to_json(value: Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Bool(v) => serde_json::Value::Bool(v),
        Value::Int(v) => serde_json::Value::Number(v.into()),
        Value::Float(bits) => serde_json::Number::from_f64(f64::from_bits(bits))
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::String(v) => serde_json::Value::String(v),
        Value::List(values) => {
            serde_json::Value::Array(values.into_iter().map(value_to_json).collect())
        }
        Value::Map(values) => serde_json::Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, value_to_json(value)))
                .collect(),
        ),
    }
}

fn to_js_error(error: impl ToString) -> JsValue {
    JsValue::from_str(&error.to_string())
}
