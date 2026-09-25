use wasm_bindgen::prelude::*;
use zega::Zega;

/// The canonical byte-faithful JSON layout (APS 12).
#[wasm_bindgen]
pub fn format_json(source: &str) -> String {
    zega::fmt::format_json(source)
}

/// Format a ZQL file or editor pane using the canonical Rust formatter.
#[wasm_bindgen]
pub fn format(source: &str) -> Result<String, JsValue> {
    zega::fmt::format_zql(source).map_err(to_js_error)
}

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

    /// Run a v2 schema-language query. `schema` is the text of `schema.zql`.
    /// `source` is one read or one `mutation`.
    /// Run a `.zql` file of `schema`, `unique`, `mutation`, and `query` blocks.
    pub fn apply(&self, source: String) -> Result<String, JsValue> {
        let value = self.inner.apply_zql(&source).map_err(to_js_error)?;
        serde_json::to_string(&value).map_err(to_js_error)
    }

    pub fn run(&self, schema: String, source: String) -> Result<String, JsValue> {
        let value = self.inner.run_lang(&schema, &source).map_err(to_js_error)?;
        serde_json::to_string(&value).map_err(to_js_error)
    }

    /// Raw source texts keyed by the locations written in ZQL. JS only transports
    /// bytes; JSON/CSV parsing, binding and insertion stay in the engine.
    pub fn run_with_sources(&self, schema: String, source: String, sources: String) -> Result<String, JsValue> {
        let sources = serde_json::from_str(&sources).map_err(to_js_error)?;
        let value = self.inner.run_lang_with_sources(&schema, &source, &sources).map_err(to_js_error)?;
        serde_json::to_string(&value).map_err(to_js_error)
    }

    pub fn apply_with_sources(&self, source: String, sources: String) -> Result<String, JsValue> {
        let sources = serde_json::from_str(&sources).map_err(to_js_error)?;
        let value = self.inner.apply_zql_with_sources(&source, &sources).map_err(to_js_error)?;
        serde_json::to_string(&value).map_err(to_js_error)
    }

    pub fn load_locations(&self, source: String, document: bool) -> Result<String, JsValue> {
        let entry = if document { zega::ZqlEntryPoint::File } else { zega::ZqlEntryPoint::Statement };
        let locations = zega::zql_load_locations(entry, &source).map_err(to_js_error)?;
        serde_json::to_string(&locations).map_err(to_js_error)
    }

    /// Preview metadata and cells come from the same Rust parser as insertion.
    pub fn preview_import(&self, text: String) -> Result<String, JsValue> {
        let format = if text.trim_start().starts_with(['[', '{']) { zega::LoadFormat::Json } else { zega::LoadFormat::Csv };
        let rows = zega::parse_import(format, &text).map_err(to_js_error)?;
        let headers: std::collections::BTreeSet<_> = rows.iter().flat_map(|row| row.keys().cloned()).collect();
        let headers: Vec<_> = headers.into_iter().collect();
        let cells: Vec<Vec<String>> = rows.iter().map(|row| headers.iter().map(|header| match row.get(header) {
            None | Some(serde_json::Value::Null) => String::new(),
            Some(serde_json::Value::String(text)) => text.clone(),
            Some(value) => value.to_string(),
        }).collect()).collect();
        serde_json::to_string(&serde_json::json!({ "headers": headers, "rows": cells,
            "kind": if format == zega::LoadFormat::Json { "json" } else { "csv" }, "value": rows }))
            .map_err(to_js_error)
    }

    /// Return checked schema types and the explicit display configuration as JSON.
    pub fn schema(&self, source: String) -> Result<String, JsValue> {
        let schema = self.inner.schema(&source).map_err(to_js_error)?;
        serde_json::to_string(&schema).map_err(to_js_error)
    }

    /// PCA and full-vector explanations, attached to a query result by the host.
    pub fn vector_view(&self, schema: String, result: String, kind: String, selected: Option<u32>, k: usize, threshold: f64) -> Result<String, JsValue> {
        let result = serde_json::from_str(&result).map_err(to_js_error)?;
        let kind = match kind.as_str() { "vector2d" => zega::ViewKind::Vector2d, "vector3d" => zega::ViewKind::Vector3d, _ => return Err(JsValue::from_str("expected vector2d or vector3d")) };
        let value = self.inner.vector_view(&schema, &result, kind, selected.map(u64::from), k, threshold).map_err(to_js_error)?;
        serde_json::to_string(&value).map_err(to_js_error)
    }

    /// Return a diagnostic report with rendered text and editor source spans.
    pub fn check(&self, schema: String, source: String) -> String {
        serde_json::to_string(&zega::diagnose(&schema, &source))
            .unwrap_or_else(|_| "[]".into())
    }

    /// Rows a ZQL filter has been tested on since this database was created.
    /// An `index { }` block lowers it; results never change.
    pub fn rows_examined(&self) -> Result<f64, JsValue> {
        self.inner.rows_examined().map(|rows| rows as f64).map_err(to_js_error)
    }

    /// Nodes a ZQL `*path` search has expanded since this database opened.
    pub fn nodes_expanded(&self) -> Result<f64, JsValue> {
        self.inner.nodes_expanded().map(|nodes| nodes as f64).map_err(to_js_error)
    }

    pub fn delete_node(&self, id: f64) -> Result<(), JsValue> {
        self.inner.delete_node(id as u64).map_err(to_js_error)
    }

    pub fn delete_relationship(&self, id: f64) -> Result<(), JsValue> {
        self.inner
            .delete_relationship(id as u64)
            .map_err(to_js_error)
    }

    /// `field` is the relationship name on the source node's type.
    pub fn connect(
        &self,
        schema: String,
        from_id: f64,
        field: String,
        to_id: f64,
    ) -> Result<(), JsValue> {
        self.inner
            .connect_schema(&schema, from_id as u64, &field, to_id as u64)
            .map_err(to_js_error)
    }

    /// Every stored node and relationship, for the graph canvas.
    pub fn graph(&self) -> Result<String, JsValue> {
        let value = self.inner.graph_json().map_err(to_js_error)?;
        serde_json::to_string(&value).map_err(to_js_error)
    }

    /// The whole graph as a `.graph` file (docs/graph-format.md): a
    /// `Uint8Array` to download, upload or `new Blob([bytes])`. `schema` is
    /// ZQL schema text to carry along; `meta` is a JSON object of string
    /// manifest metadata such as `{"licence": "CC0-1.0"}`.
    #[wasm_bindgen(js_name = exportGraph)]
    pub fn export_graph(&self, schema: Option<String>, meta: Option<String>) -> Result<Vec<u8>, JsValue> {
        let meta = match meta {
            Some(meta) => serde_json::from_str(&meta).map_err(to_js_error)?,
            None => Default::default(),
        };
        let options = zega::graph_file::ExportOptions { schema, meta };
        let mut bytes = Vec::new();
        self.inner.export_with(&mut bytes, &options).map_err(to_js_error)?;
        Ok(bytes)
    }

    /// Replace the whole graph with a `.graph` file's bytes. All or nothing:
    /// a damaged file throws and changes nothing. Returns what the file
    /// carried besides the graph (counts, schema text, metadata) as JSON.
    #[wasm_bindgen(js_name = importGraph)]
    pub fn import_graph(&self, bytes: &[u8]) -> Result<String, JsValue> {
        let summary = self.inner.import(bytes).map_err(to_js_error)?;
        serde_json::to_string(&summary).map_err(to_js_error)
    }

    /// Serialize the whole graph database to a base64 string, so the
    /// browser build can persist it across reloads.
    ///
    /// Deprecated for anything that leaves this browser: the bytes are the
    /// engine's internal snapshot, with no version contract. Use
    /// `exportGraph`. Kept, unchanged, because the explorer's saved
    /// sessions (localStorage) are in this encoding.
    pub fn export_base64(&self) -> Result<String, JsValue> {
        use base64::Engine;
        let bytes = self.inner.snapshot_bytes().map_err(to_js_error)?;
        Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    /// Restore a database previously produced by `export_base64`, replacing
    /// current state. Deprecated like `export_base64`; use `importGraph`.
    pub fn import_base64(&self, data: String) -> Result<(), JsValue> {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(to_js_error)?;
        self.inner.restore_bytes(&bytes).map_err(to_js_error)
    }
}

fn to_js_error(error: impl ToString) -> JsValue {
    JsValue::from_str(&error.to_string())
}
