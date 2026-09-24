use crate::{
    heap,
    storage::{self, err, HostStorage, SharedTail},
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use worker::*;
use zega::{Zega, ZqlProgram};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ZqlRequest {
    #[serde(default)]
    schema: String,
    query: String,
    #[serde(default)]
    document: bool,
    #[serde(default)]
    sources: Option<HashMap<String, String>>,
}

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let url = req.url()?;
    let parts: Vec<_> = url.path().split('/').collect();
    if parts.len() != 4 || parts[1] != "g" || parts[2].is_empty() || parts[2].len() > 128 {
        return Response::from_json(&json!({"ok": false, "error": "expected POST /g/<graph>/zql"}))
            .map(|r| r.with_status(404));
    }
    if req.method() != Method::Post {
        return Response::error("POST required", 405);
    }
    if parts[3] != "zql" && !(parts[3].starts_with("_spike-") && controls(&env)) {
        return Response::error("not found", 404);
    }
    let namespace = env.durable_object("GRAPHS")?;
    namespace
        .id_from_name(parts[2])?
        .get_stub()?
        .fetch_with_request(req)
        .await
}

fn controls(env: &Env) -> bool {
    env.var("SPIKE_CONTROLS")
        .is_ok_and(|v| v.to_string() == "true")
}
fn failure(message: impl ToString) -> String {
    json!({"ok": false, "error": message.to_string()}).to_string()
}
fn success(value: Value) -> String {
    json!({"ok": true, "result": value}).to_string()
}

#[durable_object]
pub struct GraphObject {
    storage: HostStorage,
    sql: worker::SqlStorage,
    tail: SharedTail,
    db: RefCell<Option<Rc<Zega>>>,
    controls: bool,
    threshold: u64,
}

impl GraphObject {
    fn engine(&self) -> Result<Rc<Zega>> {
        self.db
            .borrow()
            .clone()
            .ok_or_else(|| err("engine not loaded"))
    }
    fn abort_response(&self, reason: &str) -> Result<Response> {
        // Abort is an uncatchable runtime exception. Unwind Rust normally first;
        // entry.mjs invokes ctx.abort only after this future has resolved.
        *self.db.borrow_mut() = None;
        let response = Response::error(reason, 503)?;
        response.headers().set("x-zega-abort", reason)?;
        Ok(response)
    }
    fn load(&self) -> Result<bool> {
        if self.db.borrow().is_some() {
            return Ok(false);
        }
        storage::initialize(&self.sql)?;
        *self.db.borrow_mut() = Some(storage::restore(&self.sql, self.tail.clone())?);
        Ok(true)
    }
    async fn sync(&self) -> Result<()> {
        JsFuture::from(self.storage.sync()?).await?;
        Ok(())
    }
    fn checkpoint(&self, fail: bool) -> Result<usize> {
        storage::snapshot(
            &self.storage,
            &self.sql,
            self.engine()?,
            self.tail.clone(),
            fail,
        )
    }
    fn response(
        &self,
        body: String,
        status: u16,
        cold: Option<f64>,
        block: Option<usize>,
        retry: bool,
    ) -> Result<Response> {
        let headers = Headers::new();
        headers.set("content-type", "application/json")?;
        headers.set("x-zega-heap-bytes", &heap::live().to_string())?;
        headers.set("x-zega-peak-heap-bytes", &heap::peak().to_string())?;
        headers.set("x-zega-wasm-bytes", &heap::capacity().to_string())?;
        headers.set(
            "x-zega-wal-seq",
            &self.tail.lock().map_err(err)?.seq.to_string(),
        )?;
        if let Some(ms) = cold {
            headers.set("x-zega-cold-start-ms", &ms.to_string())?;
        }
        if let Some(block) = block {
            headers.set("x-zega-failed-block", &(block + 1).to_string())?;
        }
        if retry {
            headers.set("x-zega-retry", "true")?;
        }
        Ok(Response::ok(body)?
            .with_status(status)
            .with_headers(headers))
    }
    fn stats(&self) -> Result<Value> {
        Ok(json!({
            "walRows": storage::number(&self.sql, "SELECT COUNT(*) FROM wal")?,
            "walBytes": storage::number(&self.sql, "SELECT COALESCE(SUM(length(entry)),0) FROM wal")?,
            "snapshotParts": storage::number(&self.sql, "SELECT COUNT(*) FROM snapshot")?,
            "snapshotBytes": storage::number(&self.sql, "SELECT COALESCE(SUM(length(bytes)),0) FROM snapshot")?,
            "maxSnapshotPart": storage::number(&self.sql, "SELECT COALESCE(MAX(length(bytes)),0) FROM snapshot")?,
            "requestRows": storage::number(&self.sql, "SELECT COUNT(*) FROM requests")?,
            "databaseBytes": self.sql.database_size(),
            "ids": self.engine()?.checkpoint_ids().map_err(err)?,
            "heapBytes": heap::live(), "peakHeapBytes": heap::peak(), "wasmBytes": heap::capacity(),
        }))
    }

    async fn execute(
        &self,
        request: ZqlRequest,
        id: Option<String>,
        fingerprint: String,
        fault: Option<String>,
        cold: Option<f64>,
    ) -> Result<Response> {
        if let Some(id) = &id {
            // A partial attempt has block rows even if it never reached the final response.
            let rows = self
                .sql
                .exec(
                    "SELECT fingerprint FROM requests WHERE id = ? LIMIT 1",
                    vec![id.as_str().into()],
                )?
                .raw()
                .next();
            if let Some(row) = rows {
                if row?[0] != worker::SqlStorageValue::String(fingerprint.clone()) {
                    return self.response(
                        failure("Request-Id was already used for a different request"),
                        409,
                        cold,
                        None,
                        false,
                    );
                }
            }
            if let Some(body) = storage::cached(&self.sql, id, -1, &fingerprint)? {
                let ok = serde_json::from_str::<Value>(&body)?["ok"] == true;
                return self.response(body, if ok { 200 } else { 400 }, cold, None, true);
            }
        }
        let program = match ZqlProgram::parse(&request.schema, &request.query, request.document) {
            Ok(p) => Rc::new(p),
            Err(e) => return self.response(failure(e), 400, cold, None, false),
        };
        if (0..program.len()).any(|i| program.is_mutation(i)) && id.is_none() {
            return self.response(
                failure("Request-Id is required for mutations"),
                400,
                cold,
                None,
                false,
            );
        }
        let sources = Rc::new(request.sources.unwrap_or_default());
        let mut last = Value::Null;
        let mut failed = None;
        let mut replayed = false;
        for block in 0..program.len() {
            if let Some(id) = &id {
                if let Some(value) = storage::cached(&self.sql, id, block as i64, &fingerprint)? {
                    last = serde_json::from_str(&value)?;
                    replayed = true;
                    continue;
                }
            }
            let db = self.engine()?;
            let p = program.clone();
            let sources = sources.clone();
            let sql = self.sql.clone();
            let retry_id = id.clone();
            let hash = fingerprint.clone();
            let fail_result = fault.as_deref() == Some("before-result");
            self.tail.lock().map_err(err)?.fail_append = fault.as_deref() == Some("append");
            // Preserve language errors separately from host/storage failures. Throwing
            // either one rolls SQLite back; only language failures become HTTP 400.
            let language_error = Rc::new(std::cell::RefCell::new(None));
            let error_slot = language_error.clone();
            let result = storage::transaction(&self.storage, move || {
                let value = p.execute(&db, block, &sources).map_err(|e| {
                    if !matches!(e, zega::ZegaError::Wal(_)) {
                        *error_slot.borrow_mut() = Some(e.to_string());
                    }
                    err(e)
                })?;
                if fail_result && p.is_mutation(block) {
                    return Err(err("injected failure before retry result INSERT"));
                }
                if let Some(id) = retry_id {
                    storage::remember(
                        &sql,
                        &id,
                        block as i64,
                        &hash,
                        &serde_json::to_string(&value)?,
                    )?;
                }
                Ok(value)
            });
            match result {
                Ok(value) => last = value,
                Err(e) => {
                    // A storage rollback after engine success must also discard the
                    // in-memory graph; reconstruction precedes any later request.
                    *self.db.borrow_mut() = None;
                    self.load()?;
                    if let Some(message) = language_error.borrow_mut().take() {
                        failed = Some((block, message));
                        break;
                    }
                    if fault.as_deref() == Some("append")
                        || fault.as_deref() == Some("before-result")
                    {
                        // Abort only after rollback: simulates losing the object mid-write.
                        return self.abort_response("spike: interrupted uncommitted block");
                    }
                    return Err(e);
                }
            }
            self.sync().await?;
            if fault.as_deref() == Some("after-commit") && program.is_mutation(block) {
                return self.abort_response("spike: committed block lost its response");
            }
            if self.tail.lock().map_err(err)?.bytes >= self.threshold {
                self.checkpoint(false)?;
                self.sync().await?;
            }
        }
        let body = match &failed {
            Some((_, message)) => failure(message),
            None => success(last),
        };
        if let Some(id) = id {
            let sql = self.sql.clone();
            let saved = body.clone();
            storage::transaction(&self.storage, move || {
                storage::remember(&sql, &id, -1, &fingerprint, &saved)?;
                storage::prune(&sql)
            })?;
        }
        self.response(
            body,
            if failed.is_some() { 400 } else { 200 },
            cold,
            failed.map(|(b, _)| b),
            replayed,
        )
    }
}

impl DurableObject for GraphObject {
    fn new(state: State, env: Env) -> Self {
        let sql = state.storage().sql();
        let raw = state._inner();
        let storage = js_sys::Reflect::get(raw.as_ref(), &"storage".into())
            .expect("Durable Object storage")
            .unchecked_into();
        Self {
            storage,
            sql,
            tail: Arc::new(Mutex::new(storage::Tail::default())),
            db: RefCell::new(None),
            controls: controls(&env),
            threshold: env
                .var("SNAPSHOT_BYTES")
                .ok()
                .and_then(|v| v.to_string().parse().ok())
                .filter(|n| *n > 0)
                .unwrap_or(8 * 1024 * 1024),
        }
    }
    async fn fetch(&self, mut req: Request) -> Result<Response> {
        let action = req
            .url()?
            .path()
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_string();
        // Consume the body before entering a synchronous engine/storage transaction.
        let text = req.text().await?;
        if text.len() > 1024 * 1024 {
            return self.response(
                failure("request exceeds 1 MiB; use smaller mutation blocks"),
                413,
                None,
                None,
                false,
            );
        }
        let started = js_sys::Date::now();
        let loaded = self.load()?;
        let cold = if loaded {
            // Workers freeze JS time during CPU work. A timer lets the clock advance;
            // this coarse measurement includes at least 1ms scheduling overhead.
            Delay::from(Duration::from_millis(1)).await;
            let elapsed = js_sys::Date::now() - started;
            console_log!(
                "{{\"event\":\"cold-start\",\"ms\":{},\"heapBytes\":{},\"wasmBytes\":{}}}",
                elapsed,
                heap::live(),
                heap::capacity()
            );
            Some(elapsed)
        } else {
            None
        };
        if action.starts_with("_spike-") {
            if !self.controls {
                return Response::error("not found", 404);
            }
            match action.as_str() {
                "_spike-evict" => {
                    self.sync().await?;
                    return self.abort_response("spike: force object cold start");
                }
                "_spike-snapshot" => {
                    self.checkpoint(false)?;
                    self.sync().await?;
                }
                "_spike-snapshot-fail" => {
                    let result = self.checkpoint(true);
                    if result.is_ok() {
                        return Err(err("snapshot fault did not fire"));
                    }
                    *self.db.borrow_mut() = None;
                    self.load()?;
                    return self.response(
                        failure("injected snapshot failure rolled back"),
                        503,
                        cold,
                        None,
                        false,
                    );
                }
                "_spike-check" => {
                    let input: Value = serde_json::from_str(&text)?;
                    let entry = match input["api"].as_str() {
                        Some("file") => zega::ZqlEntryPoint::File,
                        Some("query") => zega::ZqlEntryPoint::Query,
                        Some("statement") => zega::ZqlEntryPoint::Statement,
                        _ => {
                            return self.response(
                                failure("unknown parser API"),
                                400,
                                cold,
                                None,
                                false,
                            )
                        }
                    };
                    let error = zega::check_zql(
                        entry,
                        input["source"]
                            .as_str()
                            .ok_or_else(|| err("missing source"))?,
                    )
                    .err();
                    return self.response(success(json!({"error": error})), 200, cold, None, false);
                }
                "_spike-stats" => {}
                "_spike-graph" => {
                    return self.response(
                        success(self.engine()?.graph_json().map_err(err)?),
                        200,
                        cold,
                        None,
                        false,
                    )
                }
                _ => return Response::error("not found", 404),
            }
            return self.response(success(self.stats()?), 200, cold, None, false);
        }
        let request = if req
            .headers()
            .get("content-type")?
            .is_some_and(|v| v.starts_with("application/json"))
        {
            match serde_json::from_str(&text) {
                Ok(r) => r,
                Err(e) => return self.response(failure(e), 400, cold, None, false),
            }
        } else {
            ZqlRequest {
                schema: String::new(),
                query: text.clone(),
                document: true,
                sources: None,
            }
        };
        let id = req.headers().get("Request-Id")?;
        if id
            .as_ref()
            .is_some_and(|id| id.is_empty() || id.len() > 128)
        {
            return self.response(
                failure("Request-Id must contain 1..128 bytes"),
                400,
                cold,
                None,
                false,
            );
        }
        let fault = if self.controls {
            req.headers().get("X-Zega-Fault")?
        } else {
            None
        };
        // Include the request representation, not fault-injection headers, in identity.
        let fingerprint = format!("{:x}", Sha256::digest(text.as_bytes()));
        match self.execute(request, id, fingerprint, fault, cold).await {
            Ok(response) => Ok(response),
            Err(error) => {
                *self.db.borrow_mut() = None;
                self.response(failure(error), 503, cold, None, false)
            }
        }
    }
}
