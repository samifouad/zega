//! SPIKE: storage-backed zega in a Durable Object. Not product code.
//!
//! One Durable Object per graph. The graph lives in the object's SQLite
//! (`ctx.storage.sql`); the WASM engine keeps only a byte-bounded cache
//! (`CACHE_MB`, default 16) and reads everything else synchronously from
//! SQLite on demand. The same `SqlStore` and executor as the native spike
//! (`../src`), with a [`Driver`] over `ctx.storage.sql`.
//!
//! Routes (all JSON):
//! - `POST /g/<graph>/seed?lo=&hi=&n=` insert benchmark nodes lo..=hi of an
//!   n-node graph (5 fields, 3 relationships each) in one transaction
//! - `GET  /g/<graph>/q?shape=<point|one_hop|two_hop|filter|scan_limit|scan_all|create|link>&i=`
//! - `GET  /g/<graph>/stats` database size and row counts
//! - `POST /g/<graph>/evict` drop the object (next request is a cold start)
//!
//! Every response carries what the request cost: `billed_rows_read` and
//! `billed_rows_written` from the Durable Object's own cursors (what
//! Cloudflare bills), SQL statements, cache hits/misses, and heap.

mod heap;

use std::cell::{Cell as StdCell, RefCell};

use serde_json::{json, Value as Json};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use worker::*;
use zega_storage_spike::exec;
use zega_storage_spike::gen;
use zega_storage_spike::sqlstore::{Cell, Driver, SqlStore, P};
use zega_storage_spike::store::{GraphStore, Instrumented, StoreError};

#[wasm_bindgen]
extern "C" {
    #[derive(Clone)]
    type HostStorage;
    #[wasm_bindgen(method, structural, catch, js_name = transactionSync)]
    fn transaction_sync(this: &HostStorage, callback: &js_sys::Function) -> std::result::Result<JsValue, JsValue>;
}

/// [`Driver`] over `ctx.storage.sql`. Every call is synchronous: SQLite runs
/// in the same thread as the object, on local disk.
struct DoSql {
    sql: SqlStorage,
    storage: HostStorage,
    read: StdCell<u64>,
    written: StdCell<u64>,
}

fn store_err(e: impl std::fmt::Debug) -> StoreError {
    StoreError(format!("{e:?}"))
}

fn bind(params: &[P<'_>]) -> Vec<SqlStorageValue> {
    params
        .iter()
        .map(|p| match p {
            P::I(v) => SqlStorageValue::Integer(*v),
            P::F(v) => SqlStorageValue::Float(*v),
            P::T(v) => SqlStorageValue::String(v.to_string()),
            P::B(v) => SqlStorageValue::Blob(v.to_vec()),
            P::Null => SqlStorageValue::Null,
        })
        .collect()
}

impl DoSql {
    fn run(&self, sql: &str, params: &[P<'_>], keep: bool) -> std::result::Result<(Vec<Vec<Cell>>, u64), StoreError> {
        let cursor = self.sql.exec(sql, bind(params)).map_err(store_err)?;
        let mut out = Vec::new();
        for row in cursor.raw() {
            let row = row.map_err(store_err)?;
            if keep {
                out.push(
                    row.into_iter()
                        .map(|v| match v {
                            SqlStorageValue::Null => Cell::Null,
                            SqlStorageValue::Boolean(b) => Cell::I(b as i64),
                            SqlStorageValue::Integer(i) => Cell::I(i),
                            SqlStorageValue::Float(f) => Cell::F(f),
                            SqlStorageValue::String(s) => Cell::T(s),
                            SqlStorageValue::Blob(b) => Cell::B(b),
                        })
                        .collect(),
                );
            }
        }
        let written = cursor.rows_written() as u64;
        self.read.set(self.read.get() + cursor.rows_read() as u64);
        self.written.set(self.written.get() + written);
        Ok((out, written))
    }
}

impl Driver for DoSql {
    fn query(&self, sql: &str, params: &[P<'_>]) -> std::result::Result<Vec<Vec<Cell>>, StoreError> {
        self.run(sql, params, true).map(|(rows, _)| rows)
    }

    fn execute(&self, sql: &str, params: &[P<'_>]) -> std::result::Result<u64, StoreError> {
        self.run(sql, params, false).map(|(_, written)| written)
    }

    fn transaction(&self, f: &mut dyn FnMut() -> std::result::Result<(), StoreError>) -> std::result::Result<(), StoreError> {
        // transactionSync runs the callback synchronously and returns only
        // after it has finished, so `f` outlives every use. wasm-bindgen
        // wants a 'static closure; the pointer is never used after return.
        let f: *mut (dyn FnMut() -> std::result::Result<(), StoreError> + '_) = f;
        let f: *mut (dyn FnMut() -> std::result::Result<(), StoreError> + 'static) = unsafe { std::mem::transmute(f) };
        let failure: std::rc::Rc<RefCell<Option<StoreError>>> = Default::default();
        let slot = failure.clone();
        let callback = Closure::once(move || -> std::result::Result<JsValue, JsValue> {
            // SAFETY: see above; called at most once, synchronously.
            match unsafe { (*f)() } {
                Ok(()) => Ok(JsValue::UNDEFINED),
                Err(e) => {
                    let message = e.0.clone();
                    *slot.borrow_mut() = Some(e);
                    // Throwing inside transactionSync rolls the transaction back.
                    Err(JsValue::from_str(&message))
                }
            }
        });
        let result = self.storage.transaction_sync(callback.as_ref().unchecked_ref());
        drop(callback);
        if let Some(e) = failure.borrow_mut().take() {
            return Err(e);
        }
        result.map(|_| ()).map_err(store_err)
    }

    fn billed(&self) -> Option<(u64, u64)> {
        Some((self.read.get(), self.written.get()))
    }
}

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let url = req.url()?;
    let parts: Vec<&str> = url.path().split('/').collect();
    if parts.len() != 4 || parts[1] != "g" || parts[2].is_empty() || parts[2].len() > 64 {
        return Response::error("expected /g/<graph>/<seed|q|stats|evict>", 404);
    }
    let stub = env.durable_object("GRAPHS")?.id_from_name(parts[2])?.get_stub()?;
    // The Workers clock advances across I/O, so this is the object's time
    // plus the hop to it.
    let start = Date::now().as_millis();
    let response = stub.fetch_with_request(req).await?;
    let elapsed = Date::now().as_millis() - start;
    let headers = response.headers().clone();
    headers.set("x-do-ms", &elapsed.to_string())?;
    Ok(response.with_headers(headers))
}

#[durable_object]
pub struct GraphObject {
    sql: SqlStorage,
    storage: HostStorage,
    cache_bytes: usize,
    store: RefCell<Option<SqlStore<DoSql>>>,
}

impl GraphObject {
    fn open(&self) -> Result<bool> {
        if self.store.borrow().is_some() {
            return Ok(false);
        }
        let driver = DoSql { sql: self.sql.clone(), storage: self.storage.clone(), read: StdCell::new(0), written: StdCell::new(0) };
        let mut store = SqlStore::new(driver, self.cache_bytes).map_err(|e| Error::RustError(e.0))?;
        for (ty, field) in gen::RANGES {
            store.declare_range(ty, field).map_err(|e| Error::RustError(e.0))?;
        }
        *self.store.borrow_mut() = Some(store);
        Ok(true)
    }

    fn answer(&self, opened: bool, body: Json, before: zega_storage_spike::store::Stats) -> Result<Response> {
        let store = self.store.borrow();
        let store = store.as_ref().expect("opened");
        let cost = store.stats().since(before);
        let body = json!({
            "ok": true,
            "result": body,
            "opened": opened,
            "cost": {
                "billed_rows_read": cost.billed_rows_read,
                "billed_rows_written": cost.billed_rows_written,
                "statements": cost.statements,
                "cache_hits": cost.cache_hits,
                "cache_misses": cost.cache_misses,
            },
            "heap": { "live": heap::live(), "peak": heap::peak(), "wasm": heap::capacity(), "cache": store.cache_used() },
        });
        Response::from_json(&body)
    }
}

fn param(url: &Url, name: &str) -> Option<u64> {
    url.query_pairs().find(|(k, _)| k == name).and_then(|(_, v)| v.parse().ok())
}

impl DurableObject for GraphObject {
    fn new(state: State, env: Env) -> Self {
        let sql = state.storage().sql();
        let storage = js_sys::Reflect::get(state._inner().as_ref(), &"storage".into())
            .expect("Durable Object storage")
            .unchecked_into();
        let cache_mb = env.var("CACHE_MB").ok().and_then(|v| v.to_string().parse().ok()).unwrap_or(16usize);
        Self { sql, storage, cache_bytes: cache_mb << 20, store: RefCell::new(None) }
    }

    async fn fetch(&self, req: Request) -> Result<Response> {
        let url = req.url()?;
        let action = url.path().rsplit('/').next().unwrap_or("").to_string();
        if action == "evict" {
            *self.store.borrow_mut() = None;
            let response = Response::from_json(&json!({"ok": true}))?;
            response.headers().set("x-zega-abort", "evict")?;
            return Ok(response);
        }
        let opened = self.open()?;
        let before = self.store.borrow().as_ref().expect("opened").stats();
        let fail = |e: String| Response::from_json(&json!({"ok": false, "error": e})).map(|r| r.with_status(500));
        match action.as_str() {
            "seed" => {
                let (Some(lo), Some(hi), Some(n)) = (param(&url, "lo"), param(&url, "hi"), param(&url, "n")) else {
                    return Response::error("seed needs lo, hi, n", 400);
                };
                let result = self.store.borrow_mut().as_mut().expect("opened").apply(gen::ops(lo, hi, n));
                match result {
                    Ok(()) => self.answer(opened, json!({"seeded": [lo, hi]}), before),
                    Err(e) => fail(e.0),
                }
            }
            "stats" => {
                let store = self.store.borrow();
                let store = store.as_ref().expect("opened");
                let count = |table: &str| {
                    store.driver().query(&format!("SELECT count(*) FROM {table}"), &[]).ok().and_then(|r| r.first().and_then(|r| r[0].int().ok()))
                };
                let body = json!({
                    "database_bytes": self.sql.database_size(),
                    "nodes": count("node"),
                    "relationships": count("rel"),
                    "adjacency_rows": count("adj"),
                    "index_rows": count("ixs").zip(count("ixn")).map(|(a, b)| a + b),
                    "next_ids": store.next_ids(),
                });
                self.answer(opened, body, before)
            }
            "q" => {
                let shape = url.query_pairs().find(|(k, _)| k == "shape").map(|(_, v)| v.to_string()).unwrap_or_default();
                let i = param(&url, "i").unwrap_or(1);
                let n = self.store.borrow().as_ref().expect("opened").next_ids().0.saturating_sub(1).max(1);
                let query = match shape.as_str() {
                    "point" => gen::point(i),
                    "one_hop" => gen::one_hop(i),
                    "two_hop" => gen::two_hop(i),
                    "filter" => gen::filter(i),
                    "scan_limit" => gen::scan_limit(i % gen::CITIES),
                    "scan_all" => gen::scan_all(),
                    "create" => gen::create(i),
                    "link" => {
                        let seeded = param(&url, "n").unwrap_or(n);
                        let t = |k: u64| 1 + gen::mix(i + k) % seeded;
                        gen::link(&format!("new{i}"), [t(0), t(1), t(2)])
                    }
                    _ => return Response::error("unknown shape", 400),
                };
                let result = if matches!(shape.as_str(), "create" | "link") {
                    let mut store = self.store.borrow_mut();
                    exec::mutate(store.as_mut().expect("opened"), &query.plan, &gen::uniques())
                } else {
                    let store = self.store.borrow();
                    exec::read(store.as_ref().expect("opened"), &query.plan)
                };
                match result {
                    Ok(result) => self.answer(opened, result, before),
                    Err(e) => fail(e.0),
                }
            }
            _ => Response::error("not found", 404),
        }
    }
}
