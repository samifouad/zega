//! The [`Driver`] over rusqlite: SQLite on a machine, standing in for a
//! Durable Object's `ctx.storage.sql` (which is SQLite too).

use std::path::Path;

use rusqlite::types::{ToSqlOutput, Value as SqlValue, ValueRef};
use rusqlite::{params_from_iter, Connection};

use crate::sqlstore::{Cell, Driver, SqlStore, P};
use crate::store::{Result, StoreError};

fn err(e: impl std::fmt::Display) -> StoreError {
    StoreError(e.to_string())
}

pub struct Options {
    /// Total memory for caching: half to decoded nodes and adjacency, half to
    /// SQLite's page cache.
    pub cache_bytes: usize,
    /// `synchronous=FULL` (fsync every commit) instead of `NORMAL`.
    pub sync_full: bool,
}

pub struct Rusqlite {
    pub conn: Connection,
}

pub type SqliteStore = SqlStore<Rusqlite>;

impl SqlStore<Rusqlite> {
    pub fn open(path: &Path, options: Options) -> Result<Self> {
        let conn = Connection::open(path).map_err(err)?;
        let page_kib = (options.cache_bytes / 2 / 1024).max(64);
        conn.execute_batch(&format!(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous={}; PRAGMA cache_size=-{page_kib};
             PRAGMA mmap_size=0; PRAGMA temp_store=MEMORY;",
            if options.sync_full { "FULL" } else { "NORMAL" }
        ))
        .map_err(err)?;
        SqlStore::new(Rusqlite { conn }, options.cache_bytes / 2)
    }

    pub fn connection(&self) -> &Connection {
        &self.driver().conn
    }
}

impl rusqlite::ToSql for P<'_> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Borrowed(match self {
            P::I(v) => ValueRef::Integer(*v),
            P::F(v) => ValueRef::Real(*v),
            P::T(v) => ValueRef::Text(v.as_bytes()),
            P::B(v) => ValueRef::Blob(v),
            P::Null => ValueRef::Null,
        }))
    }
}

impl Driver for Rusqlite {
    fn query(&self, sql: &str, params: &[P<'_>]) -> Result<Vec<Vec<Cell>>> {
        let mut stmt = self.conn.prepare_cached(sql).map_err(err)?;
        let columns = stmt.column_count();
        let mut rows = stmt.query(params_from_iter(params.iter())).map_err(err)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(err)? {
            let mut cells = Vec::with_capacity(columns);
            for i in 0..columns {
                cells.push(match row.get::<_, SqlValue>(i).map_err(err)? {
                    SqlValue::Integer(v) => Cell::I(v),
                    SqlValue::Real(v) => Cell::F(v),
                    SqlValue::Text(v) => Cell::T(v),
                    SqlValue::Blob(v) => Cell::B(v),
                    SqlValue::Null => Cell::Null,
                });
            }
            out.push(cells);
        }
        Ok(out)
    }

    fn execute(&self, sql: &str, params: &[P<'_>]) -> Result<u64> {
        let mut stmt = self.conn.prepare_cached(sql).map_err(err)?;
        stmt.execute(params_from_iter(params.iter())).map(|n| n as u64).map_err(err)
    }

    fn transaction(&self, f: &mut dyn FnMut() -> Result<()>) -> Result<()> {
        self.conn.execute_batch("BEGIN").map_err(err)?;
        match f() {
            Ok(()) => self.conn.execute_batch("COMMIT").map_err(err),
            Err(error) => {
                self.conn.execute_batch("ROLLBACK").map_err(err)?;
                Err(error)
            }
        }
    }
}
