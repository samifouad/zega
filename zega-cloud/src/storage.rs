//! SQLite holds native WAL frames and snapshots, without translating graph data.
use js_sys::{Function, Promise};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    io,
    io::Write,
    rc::Rc,
    sync::{Arc, Mutex},
};
use wasm_bindgen::{prelude::*, JsCast};
use worker::{Error, Result, SqlStorage, SqlStorageValue as V};
use zega::{AppendTarget, Zega};

pub const CHUNK: usize = 1024 * 1024;
pub const MAX_ROW: usize = 1900 * 1024; // Leave room for row keys/SQLite overhead under 2 MB.
pub const RECENT_REQUESTS: usize = 4096;

#[wasm_bindgen]
extern "C" {
    #[derive(Clone)]
    pub type HostStorage;
    #[wasm_bindgen(method, structural, catch, js_name = transactionSync)]
    fn transaction_sync(
        this: &HostStorage,
        callback: &Function,
    ) -> std::result::Result<JsValue, JsValue>;
    #[wasm_bindgen(method, structural, catch, js_name = sync)]
    pub fn sync(this: &HostStorage) -> std::result::Result<Promise, JsValue>;
}

/// `Err` becomes a thrown JS exception *inside* transactionSync, causing rollback.
/// Returning a Rust Result as a JS value would accidentally commit the transaction.
pub fn transaction<T: 'static>(
    storage: &HostStorage,
    action: impl FnOnce() -> Result<T> + 'static,
) -> Result<T> {
    let output = Rc::new(RefCell::new(None));
    let slot = output.clone();
    let callback = Closure::once(move || -> std::result::Result<JsValue, JsValue> {
        let value = action().map_err(JsValue::from)?;
        *slot.borrow_mut() = Some(value);
        Ok(JsValue::UNDEFINED)
    });
    storage.transaction_sync(callback.as_ref().unchecked_ref())?;
    let value = output
        .borrow_mut()
        .take()
        .ok_or_else(|| Error::RustError("transaction callback did not run".into()))?;
    Ok(value)
}

#[derive(Default)]
pub struct Tail {
    pub seq: u64,
    pub bytes: u64,
    pub fail_append: bool,
}
pub type SharedTail = Arc<Mutex<Tail>>;

pub struct SqlAppend {
    pub sql: SqlStorage,
    pub tail: SharedTail,
}

fn io_error(error: impl ToString) -> io::Error {
    io::Error::other(error.to_string())
}
pub fn err(error: impl ToString) -> Error {
    Error::RustError(error.to_string())
}

impl Write for SqlAppend {
    fn write(&mut self, frame: &[u8]) -> io::Result<usize> {
        if frame.len() > MAX_ROW {
            return Err(io_error(
                "WAL entry exceeds SQLite spike limit (1900 KiB); use smaller mutation blocks",
            ));
        }
        let mut tail = self.tail.lock().map_err(io_error)?;
        let seq = tail
            .seq
            .checked_add(1)
            .filter(|n| *n < (1 << 53))
            .ok_or_else(|| io_error("WAL sequence exceeds JS integer precision"))?;
        self.sql
            .exec(
                "INSERT INTO wal(seq, entry) VALUES (?, ?)",
                vec![V::Integer(seq as i64), V::Blob(frame.to_vec())],
            )
            .map_err(io_error)?;
        tail.seq = seq;
        tail.bytes += frame.len() as u64;
        if tail.fail_append {
            tail.fail_append = false;
            return Err(io_error("injected failure after WAL INSERT"));
        }
        Ok(frame.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl AppendTarget for SqlAppend {
    fn write_entry(&mut self, header: &[u8; 12], payload: &[u8]) -> io::Result<()> {
        let mut frame = Vec::with_capacity(header.len() + payload.len());
        frame.extend_from_slice(header);
        frame.extend_from_slice(payload);
        self.write_all(&frame)
    }

    fn seek_end(&mut self) -> io::Result<u64> {
        Ok(self.tail.lock().map_err(io_error)?.seq)
    }
    fn truncate(&mut self, last_good: u64) -> io::Result<()> {
        self.sql
            .exec(
                "DELETE FROM wal WHERE seq > ?",
                vec![V::Integer(last_good as i64)],
            )
            .map_err(io_error)?;
        let bytes = number(&self.sql, "SELECT COALESCE(SUM(length(entry)), 0) FROM wal")
            .map_err(io_error)?;
        let mut tail = self.tail.lock().map_err(io_error)?;
        tail.seq = last_good;
        tail.bytes = bytes;
        Ok(())
    }
    // SQL durability belongs to the surrounding storage transaction/output gate.
    // No allowUnconfirmed writes are used. The host also awaits storage.sync
    // between blocks so a later block failure cannot take back an earlier one.
    fn sync(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub fn number(sql: &SqlStorage, query: &str) -> Result<u64> {
    let row = sql
        .exec(query, None)?
        .raw()
        .next()
        .ok_or_else(|| err("missing SQL result"))??;
    match row.first() {
        Some(V::Integer(n)) if *n >= 0 => Ok(*n as u64),
        _ => Err(err("expected nonnegative integer")),
    }
}
fn blob(value: &V) -> Result<&[u8]> {
    match value {
        V::Blob(bytes) => Ok(bytes),
        _ => Err(err("expected BLOB")),
    }
}

pub fn initialize(sql: &SqlStorage) -> Result<()> {
    sql.exec("CREATE TABLE IF NOT EXISTS wal(seq INTEGER PRIMARY KEY, entry BLOB NOT NULL);\
        CREATE TABLE IF NOT EXISTS snapshot(part INTEGER PRIMARY KEY, bytes BLOB NOT NULL);\
        CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value BLOB NOT NULL);\
        CREATE TABLE IF NOT EXISTS requests(id TEXT NOT NULL, block INTEGER NOT NULL, fingerprint TEXT NOT NULL, result BLOB NOT NULL, PRIMARY KEY(id, block));", None)?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    seq: u64,
    parts: usize,
    bytes: usize,
    ids: (u64, u64),
    sha256: String,
}

pub fn restore(sql: &SqlStorage, tail: SharedTail) -> Result<Rc<Zega>> {
    let db = Rc::new(
        Zega::with_append_target(Box::new(SqlAppend {
            sql: sql.clone(),
            tail: tail.clone(),
        }))
        .map_err(err)?,
    );
    let mut covered = 0;
    if let Some(row) = sql
        .exec("SELECT value FROM meta WHERE key = 'snapshot'", None)?
        .raw()
        .next()
    {
        let row = row?;
        let manifest: Manifest = serde_json::from_slice(blob(&row[0])?)?;
        let mut bytes = Vec::with_capacity(manifest.bytes);
        let mut parts = 0;
        for row in sql
            .exec("SELECT part, bytes FROM snapshot ORDER BY part", None)?
            .raw()
        {
            let row = row?;
            if row[0] != V::Integer(parts as i64) {
                return Err(err("snapshot chunk sequence gap"));
            }
            bytes.extend_from_slice(blob(&row[1])?);
            parts += 1;
        }
        if bytes.len() != manifest.bytes
            || parts != manifest.parts
            || format!("{:x}", Sha256::digest(&bytes)) != manifest.sha256
        {
            return Err(err("snapshot manifest mismatch"));
        }
        db.restore_bytes(&bytes).map_err(err)?;
        db.restore_checkpoint_ids(manifest.ids).map_err(err)?;
        covered = manifest.seq;
    }
    let mut seq = covered;
    let mut bytes = 0;
    for row in sql
        .exec(
            "SELECT seq, entry FROM wal WHERE seq > ? ORDER BY seq",
            vec![V::Integer(covered as i64)],
        )?
        .raw()
    {
        let row = row?;
        if row[0] != V::Integer((seq + 1) as i64) {
            return Err(err("WAL sequence gap"));
        }
        let entry = blob(&row[1])?;
        db.replay_wal_entry(entry).map_err(err)?;
        bytes += entry.len() as u64;
        seq += 1;
    }
    *tail.lock().map_err(err)? = Tail {
        seq,
        bytes,
        fail_append: false,
    };
    Ok(db)
}

pub fn snapshot(
    storage: &HostStorage,
    sql: &SqlStorage,
    db: Rc<Zega>,
    tail: SharedTail,
    fail: bool,
) -> Result<usize> {
    let bytes = db.snapshot_bytes().map_err(err)?;
    let seq = tail.lock().map_err(err)?.seq;
    let manifest = Manifest {
        seq,
        parts: bytes.len().div_ceil(CHUNK),
        bytes: bytes.len(),
        ids: db.checkpoint_ids().map_err(err)?,
        sha256: format!("{:x}", Sha256::digest(&bytes)),
    };
    let size = bytes.len();
    let sql = sql.clone();
    transaction(storage, move || {
        sql.exec("DELETE FROM snapshot", None)?;
        for (part, chunk) in bytes.chunks(CHUNK).enumerate() {
            sql.exec(
                "INSERT INTO snapshot(part, bytes) VALUES (?, ?)",
                vec![V::Integer(part as i64), V::Blob(chunk.to_vec())],
            )?;
            if fail {
                return Err(err("injected failure during snapshot replacement"));
            }
        }
        sql.exec(
            "INSERT OR REPLACE INTO meta(key, value) VALUES ('snapshot', ?)",
            vec![V::Blob(serde_json::to_vec(&manifest)?)],
        )?;
        sql.exec(
            "DELETE FROM wal WHERE seq <= ?",
            vec![V::Integer(seq as i64)],
        )?;
        Ok(())
    })?;
    tail.lock().map_err(err)?.bytes = 0;
    Ok(size)
}

pub fn cached(sql: &SqlStorage, id: &str, block: i64, fingerprint: &str) -> Result<Option<String>> {
    let Some(row) = sql
        .exec(
            "SELECT fingerprint, result FROM requests WHERE id = ? AND block = ?",
            vec![id.into(), block.into()],
        )?
        .raw()
        .next()
    else {
        return Ok(None);
    };
    let row = row?;
    if row[0] != V::String(fingerprint.into()) {
        return Err(err("Request-Id was already used for a different request"));
    }
    Ok(Some(
        String::from_utf8(blob(&row[1])?.to_vec()).map_err(err)?,
    ))
}

pub fn remember(
    sql: &SqlStorage,
    id: &str,
    block: i64,
    fingerprint: &str,
    result: &str,
) -> Result<()> {
    if result.len() > MAX_ROW {
        return Err(err("retry result exceeds SQLite row limit"));
    }
    sql.exec(
        "INSERT INTO requests(id, block, fingerprint, result) VALUES (?, ?, ?, ?)",
        vec![
            id.into(),
            block.into(),
            fingerprint.into(),
            V::Blob(result.as_bytes().to_vec()),
        ],
    )?;
    Ok(())
}

pub fn prune(sql: &SqlStorage) -> Result<()> {
    sql.exec("DELETE FROM requests WHERE id IN (SELECT id FROM requests GROUP BY id ORDER BY MAX(rowid) DESC LIMIT -1 OFFSET ?)", vec![V::Integer(RECENT_REQUESTS as i64)])?;
    Ok(())
}
