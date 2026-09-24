//! [`GraphStore`] over SQLite, with a byte-bounded object cache.
//!
//! The tables are what a Durable Object would hold in `ctx.storage.sql`:
//!
//! | table | row | serves |
//! |---|---|---|
//! | `node(id, labels, props)` | one per node | point read by id |
//! | `lbl(label, id)` | one per node label | label scan in id order |
//! | `rel(id, kind, src, dst, props)` | one per relationship | edge properties |
//! | `adj(node, dk, other, rel)` | two per relationship (out and in) | adjacency by node, kind, direction |
//! | `ixn(spec, key, id)` / `ixs(...)` | one per indexed value | range index, unique lookups |
//! | `names(id, name)` | one per label, kind, field | small integers in every other table |
//!
//! Every lookup is a primary-key seek or a primary-key range scan. Nothing is
//! read at open beyond `names`, `spec` and `meta`: a cold object answers its
//! first query without replaying a log or loading a snapshot.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use rusqlite::{params, Connection, OptionalExtension};

use crate::cache::ByteLru;
use crate::model::{index_key, node_bytes, Dir, Interval, Key, Node, NodeId, Op, Rel, RelId, Value};
use crate::store::{GraphStore, Instrumented, Result, Stats, StoreError};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS names(id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY, v INTEGER NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS spec(id INTEGER PRIMARY KEY, ty TEXT NOT NULL, field TEXT NOT NULL, UNIQUE(ty, field));
CREATE TABLE IF NOT EXISTS node(id INTEGER PRIMARY KEY, labels BLOB NOT NULL, props BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS lbl(label INTEGER NOT NULL, id INTEGER NOT NULL, PRIMARY KEY(label, id)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS rel(id INTEGER PRIMARY KEY, kind INTEGER NOT NULL, src INTEGER NOT NULL, dst INTEGER NOT NULL, props BLOB);
CREATE TABLE IF NOT EXISTS adj(node INTEGER NOT NULL, dk INTEGER NOT NULL, other INTEGER NOT NULL, rel INTEGER NOT NULL, PRIMARY KEY(node, dk, other, rel)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS ixn(spec INTEGER NOT NULL, key REAL NOT NULL, id INTEGER NOT NULL, PRIMARY KEY(spec, key, id)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS ixs(spec INTEGER NOT NULL, key TEXT NOT NULL, id INTEGER NOT NULL, PRIMARY KEY(spec, key, id)) WITHOUT ROWID;
";

fn err(e: impl std::fmt::Display) -> StoreError {
    StoreError(e.to_string())
}

#[derive(Default)]
struct Names {
    by_name: HashMap<String, u32>,
    by_id: HashMap<u32, String>,
}

struct Spec {
    id: i64,
    ty: String,
    field: String,
}

pub struct Options {
    /// Total memory for caching: half to decoded nodes and adjacency, half to
    /// SQLite's page cache.
    pub cache_bytes: usize,
    /// `synchronous=FULL` (fsync every commit) instead of `NORMAL`.
    pub sync_full: bool,
}

pub struct SqliteStore {
    conn: Connection,
    names: RefCell<Names>,
    specs: Vec<Spec>,
    nodes: RefCell<ByteLru<NodeId, Arc<Node>>>,
    rels: RefCell<ByteLru<RelId, Arc<Rel>>>,
    adj: RefCell<ByteLru<(NodeId, i64), Arc<[(NodeId, RelId)]>>>,
    stats: Cell<Stats>,
    next: Cell<(NodeId, RelId)>,
}

impl SqliteStore {
    pub fn open(path: &Path, options: Options) -> Result<Self> {
        let conn = Connection::open(path).map_err(err)?;
        let page_kib = (options.cache_bytes / 2 / 1024).max(64);
        conn.execute_batch(&format!(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous={}; PRAGMA cache_size=-{page_kib};
             PRAGMA mmap_size=0; PRAGMA temp_store=MEMORY;",
            if options.sync_full { "FULL" } else { "NORMAL" }
        ))
        .map_err(err)?;
        conn.execute_batch(SCHEMA).map_err(err)?;
        // Three objects share the other half: nodes 60%, adjacency 35%, rels 5%.
        let objects = options.cache_bytes / 2;
        let mut store = SqliteStore {
            conn,
            names: RefCell::new(Names::default()),
            specs: Vec::new(),
            nodes: RefCell::new(ByteLru::new(objects * 60 / 100)),
            adj: RefCell::new(ByteLru::new(objects * 35 / 100)),
            rels: RefCell::new(ByteLru::new(objects * 5 / 100)),
            stats: Cell::new(Stats::default()),
            next: Cell::new((1, 1)),
        };
        store.load_catalog()?;
        Ok(store)
    }

    fn load_catalog(&mut self) -> Result<()> {
        let mut names = Names::default();
        {
            let mut stmt = self.conn.prepare("SELECT id, name FROM names").map_err(err)?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, u32>(0)?, r.get::<_, String>(1)?)))
                .map_err(err)?;
            for row in rows {
                let (id, name) = row.map_err(err)?;
                names.by_name.insert(name.clone(), id);
                names.by_id.insert(id, name);
            }
        }
        *self.names.borrow_mut() = names;
        let mut specs = Vec::new();
        {
            let mut stmt = self.conn.prepare("SELECT id, ty, field FROM spec").map_err(err)?;
            let rows = stmt
                .query_map([], |r| Ok(Spec { id: r.get(0)?, ty: r.get(1)?, field: r.get(2)? }))
                .map_err(err)?;
            for row in rows {
                specs.push(row.map_err(err)?);
            }
        }
        self.specs = specs;
        let get = |k: &str| -> Result<u64> {
            self.conn
                .query_row("SELECT v FROM meta WHERE k = ?1", [k], |r| r.get::<_, i64>(0))
                .optional()
                .map_err(err)
                .map(|v| v.unwrap_or(1) as u64)
        };
        self.next.set((get("next_node")?, get("next_rel")?));
        Ok(())
    }

    /// Declare a range index on `ty.field` (the `index { range … }` block and
    /// every unique field). Builds it from the stored nodes.
    pub fn declare_range(&mut self, ty: &str, field: &str) -> Result<()> {
        if self.has_index(ty, field) {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction().map_err(err)?;
        tx.execute("INSERT INTO spec(ty, field) VALUES (?1, ?2)", params![ty, field]).map_err(err)?;
        let spec = tx.last_insert_rowid();
        let label = self.intern(&tx, ty)?;
        let mut after = 0u64;
        loop {
            let page = self.scan_raw(&tx, label, after, 4096)?;
            let Some(last) = page.last() else { break };
            after = last.id;
            for node in &page {
                if let Some(value) = node.props.get(field) {
                    insert_key(&tx, spec, value, node.id)?;
                }
            }
        }
        tx.commit().map_err(err)?;
        self.load_catalog()
    }

    fn bump(&self, f: impl FnOnce(&mut Stats)) {
        let mut stats = self.stats.get();
        f(&mut stats);
        self.stats.set(stats);
    }

    fn name_id(&self, name: &str) -> Option<i64> {
        self.names.borrow().by_name.get(name).map(|id| *id as i64)
    }

    fn intern(&self, conn: &Connection, name: &str) -> Result<i64> {
        if let Some(id) = self.name_id(name) {
            return Ok(id);
        }
        conn.execute("INSERT INTO names(name) VALUES (?1)", [name]).map_err(err)?;
        let id = conn.last_insert_rowid() as u32;
        let mut names = self.names.borrow_mut();
        names.by_name.insert(name.to_string(), id);
        names.by_id.insert(id, name.to_string());
        Ok(id as i64)
    }

    fn name_of(&self, id: u32) -> Result<String> {
        self.names.borrow().by_id.get(&id).cloned().ok_or_else(|| StoreError(format!("unknown name {id}")))
    }

    fn decode_props(&self, props: &[u8]) -> Result<HashMap<String, Value>> {
        let props: Vec<(u32, Value)> = bincode::deserialize(props).map_err(err)?;
        props.into_iter().map(|(k, v)| Ok((self.name_of(k)?, v))).collect()
    }

    fn decode_node(&self, id: NodeId, labels: &[u8], props: &[u8]) -> Result<Node> {
        let labels: Vec<u32> = bincode::deserialize(labels).map_err(err)?;
        Ok(Node {
            id,
            labels: labels.into_iter().map(|l| self.name_of(l)).collect::<Result<_>>()?,
            props: self.decode_props(props)?,
        })
    }

    fn encode_props(&self, conn: &Connection, props: &HashMap<String, Value>) -> Result<Vec<u8>> {
        let mut out: Vec<(u32, &Value)> = Vec::with_capacity(props.len());
        for (k, v) in props {
            out.push((self.intern(conn, k)? as u32, v));
        }
        out.sort_by_key(|(k, _)| *k);
        bincode::serialize(&out).map_err(err)
    }

    fn read_node(&self, conn: &Connection, id: NodeId) -> Result<Option<Node>> {
        self.bump(|s| s.statements += 1);
        let mut stmt = conn.prepare_cached("SELECT labels, props FROM node WHERE id = ?1").map_err(err)?;
        let row = stmt
            .query_row([id as i64], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?)))
            .optional()
            .map_err(err)?;
        match row {
            None => Ok(None),
            Some((labels, props)) => {
                self.bump(|s| s.rows_read += 1);
                self.decode_node(id, &labels, &props).map(Some)
            }
        }
    }

    fn scan_raw(&self, conn: &Connection, label: i64, after: NodeId, limit: usize) -> Result<Vec<Node>> {
        self.bump(|s| s.statements += 1);
        let mut stmt = conn
            .prepare_cached(
                "SELECT n.id, n.labels, n.props FROM lbl l JOIN node n ON n.id = l.id
                 WHERE l.label = ?1 AND l.id > ?2 ORDER BY l.id LIMIT ?3",
            )
            .map_err(err)?;
        let rows = stmt
            .query_map(params![label, after as i64, limit as i64], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?, r.get::<_, Vec<u8>>(2)?))
            })
            .map_err(err)?;
        let mut out = Vec::new();
        for row in rows {
            let (id, labels, props) = row.map_err(err)?;
            out.push(self.decode_node(id as u64, &labels, &props)?);
        }
        // The label row and the node row.
        self.bump(|s| s.rows_read += 2 * out.len() as u64);
        Ok(out)
    }

    fn specs_for<'a>(&'a self, labels: &'a [String]) -> impl Iterator<Item = &'a Spec> + 'a {
        self.specs.iter().filter(move |spec| labels.iter().any(|l| *l == spec.ty))
    }

    fn write_node_rows(&self, conn: &Connection, node: &Node, written: &mut u64) -> Result<()> {
        let labels: Vec<u32> = node
            .labels
            .iter()
            .map(|l| self.intern(conn, l).map(|id| id as u32))
            .collect::<Result<_>>()?;
        let props = self.encode_props(conn, &node.props)?;
        conn.prepare_cached("INSERT OR REPLACE INTO node(id, labels, props) VALUES (?1, ?2, ?3)")
            .map_err(err)?
            .execute(params![node.id as i64, bincode::serialize(&labels).map_err(err)?, props])
            .map_err(err)?;
        *written += 1;
        for label in &labels {
            conn.prepare_cached("INSERT OR IGNORE INTO lbl(label, id) VALUES (?1, ?2)")
                .map_err(err)?
                .execute(params![*label as i64, node.id as i64])
                .map_err(err)?;
            *written += 1;
        }
        for spec in self.specs_for(&node.labels) {
            if let Some(value) = node.props.get(&spec.field) {
                *written += insert_key(conn, spec.id, value, node.id)?;
            }
        }
        Ok(())
    }

    fn delete_index_rows(&self, conn: &Connection, node: &Node, written: &mut u64) -> Result<()> {
        for spec in self.specs_for(&node.labels) {
            let Some(value) = node.props.get(&spec.field) else { continue };
            let changed = match index_key(value) {
                Some(Key::Num(n)) => conn
                    .prepare_cached("DELETE FROM ixn WHERE spec = ?1 AND key = ?2 AND id = ?3")
                    .map_err(err)?
                    .execute(params![spec.id, n, node.id as i64]),
                Some(Key::Str(s)) => conn
                    .prepare_cached("DELETE FROM ixs WHERE spec = ?1 AND key = ?2 AND id = ?3")
                    .map_err(err)?
                    .execute(params![spec.id, s, node.id as i64]),
                None => Ok(0),
            };
            *written += changed.map_err(err)? as u64;
        }
        Ok(())
    }

    fn apply_ops(&self, conn: &Connection, ops: &[Op], touched: &mut Touched) -> Result<(u64, (NodeId, RelId))> {
        let mut written = 0u64;
        let (mut next_node, mut next_rel) = self.next.get();
        for op in ops {
            match op {
                Op::InsertNode { id, labels, props } => {
                    let node = Node { id: *id, labels: labels.clone(), props: props.clone() };
                    if let Some(old) = self.read_node(conn, *id)? {
                        self.delete_index_rows(conn, &old, &mut written)?;
                    }
                    self.write_node_rows(conn, &node, &mut written)?;
                    touched.nodes.insert(*id);
                    next_node = next_node.max(id + 1);
                }
                Op::UpdateNode { id, props } => {
                    let Some(mut node) = self.read_node(conn, *id)? else { continue };
                    self.delete_index_rows(conn, &node, &mut written)?;
                    node.props.extend(props.clone());
                    self.write_node_rows(conn, &node, &mut written)?;
                    touched.nodes.insert(*id);
                }
                Op::DeleteNode { id } => {
                    let Some(node) = self.read_node(conn, *id)? else { continue };
                    self.delete_index_rows(conn, &node, &mut written)?;
                    written += conn.execute("DELETE FROM lbl WHERE id = ?1", [*id as i64]).map_err(err)? as u64;
                    written += conn.execute("DELETE FROM node WHERE id = ?1", [*id as i64]).map_err(err)? as u64;
                    let incident: Vec<(i64, i64, i64)> = {
                        let mut stmt = conn.prepare_cached("SELECT dk, other, rel FROM adj WHERE node = ?1").map_err(err)?;
                        let rows = stmt
                            .query_map([*id as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                            .map_err(err)?;
                        rows.collect::<std::result::Result<_, _>>().map_err(err)?
                    };
                    for (dk, other, rel) in incident {
                        written += self.delete_rel_rows(conn, rel as u64, touched)?;
                        touched.adj.insert((other as u64, dk ^ 1));
                    }
                    touched.nodes.insert(*id);
                    touched.all_adj_of.insert(*id);
                }
                Op::InsertRel { id, kind, from, to, props } => {
                    let kind_id = self.intern(conn, kind)?;
                    let props = if props.is_empty() { None } else { Some(self.encode_props(conn, props)?) };
                    conn.prepare_cached("INSERT OR REPLACE INTO rel(id, kind, src, dst, props) VALUES (?1, ?2, ?3, ?4, ?5)")
                        .map_err(err)?
                        .execute(params![*id as i64, kind_id, *from as i64, *to as i64, props])
                        .map_err(err)?;
                    let mut adj = conn
                        .prepare_cached("INSERT OR IGNORE INTO adj(node, dk, other, rel) VALUES (?1, ?2, ?3, ?4)")
                        .map_err(err)?;
                    adj.execute(params![*from as i64, kind_id * 2, *to as i64, *id as i64]).map_err(err)?;
                    adj.execute(params![*to as i64, kind_id * 2 + 1, *from as i64, *id as i64]).map_err(err)?;
                    written += 3;
                    touched.rels.insert(*id);
                    touched.adj.insert((*from, kind_id * 2));
                    touched.adj.insert((*to, kind_id * 2 + 1));
                    next_rel = next_rel.max(id + 1);
                }
                Op::DeleteRel { id } => {
                    written += self.delete_rel_rows(conn, *id, touched)?;
                }
            }
        }
        conn.prepare_cached("INSERT OR REPLACE INTO meta(k, v) VALUES ('next_node', ?1), ('next_rel', ?2)")
            .map_err(err)?
            .execute(params![next_node as i64, next_rel as i64])
            .map_err(err)?;
        written += 2;
        Ok((written, (next_node, next_rel)))
    }

    fn delete_rel_rows(&self, conn: &Connection, id: RelId, touched: &mut Touched) -> Result<u64> {
        let row: Option<(i64, i64, i64)> = conn
            .query_row("SELECT kind, src, dst FROM rel WHERE id = ?1", [id as i64], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()
            .map_err(err)?;
        let Some((kind, src, dst)) = row else { return Ok(0) };
        let mut written = conn.execute("DELETE FROM rel WHERE id = ?1", [id as i64]).map_err(err)? as u64;
        let mut del = conn
            .prepare_cached("DELETE FROM adj WHERE node = ?1 AND dk = ?2 AND other = ?3 AND rel = ?4")
            .map_err(err)?;
        written += del.execute(params![src, kind * 2, dst, id as i64]).map_err(err)? as u64;
        written += del.execute(params![dst, kind * 2 + 1, src, id as i64]).map_err(err)? as u64;
        touched.rels.insert(id);
        touched.adj.insert((src as u64, kind * 2));
        touched.adj.insert((dst as u64, kind * 2 + 1));
        Ok(written)
    }

    /// Bytes currently held by the object caches.
    pub fn cache_used(&self) -> usize {
        self.nodes.borrow().used() + self.adj.borrow().used() + self.rels.borrow().used()
    }

    pub fn clear_cache(&self) {
        self.nodes.borrow_mut().clear();
        self.adj.borrow_mut().clear();
        self.rels.borrow_mut().clear();
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }
}

#[derive(Default)]
struct Touched {
    nodes: HashSet<NodeId>,
    rels: HashSet<RelId>,
    adj: HashSet<(NodeId, i64)>,
    all_adj_of: HashSet<NodeId>,
}

fn insert_key(conn: &Connection, spec: i64, value: &Value, id: NodeId) -> Result<u64> {
    let changed = match index_key(value) {
        Some(Key::Num(n)) => conn
            .prepare_cached("INSERT OR IGNORE INTO ixn(spec, key, id) VALUES (?1, ?2, ?3)")
            .map_err(err)?
            .execute(params![spec, n, id as i64]),
        Some(Key::Str(s)) => conn
            .prepare_cached("INSERT OR IGNORE INTO ixs(spec, key, id) VALUES (?1, ?2, ?3)")
            .map_err(err)?
            .execute(params![spec, s, id as i64]),
        None => Ok(0),
    };
    changed.map(|n| n as u64).map_err(err)
}

impl GraphStore for SqliteStore {
    fn node(&self, id: NodeId) -> Result<Option<Arc<Node>>> {
        if let Some(node) = self.nodes.borrow_mut().get(&id) {
            self.bump(|s| s.cache_hits += 1);
            return Ok(Some(node));
        }
        self.bump(|s| s.cache_misses += 1);
        let Some(node) = self.read_node(&self.conn, id)? else { return Ok(None) };
        let bytes = node_bytes(&node);
        let node = Arc::new(node);
        self.nodes.borrow_mut().put(id, node.clone(), bytes);
        Ok(Some(node))
    }

    fn relationship(&self, id: RelId) -> Result<Option<Arc<Rel>>> {
        if let Some(rel) = self.rels.borrow_mut().get(&id) {
            self.bump(|s| s.cache_hits += 1);
            return Ok(Some(rel));
        }
        self.bump(|s| {
            s.cache_misses += 1;
            s.statements += 1;
        });
        let row: Option<(u32, i64, i64, Option<Vec<u8>>)> = self
            .conn
            .prepare_cached("SELECT kind, src, dst, props FROM rel WHERE id = ?1")
            .map_err(err)?
            .query_row([id as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .optional()
            .map_err(err)?;
        let Some((kind, src, dst, props)) = row else { return Ok(None) };
        self.bump(|s| s.rows_read += 1);
        let props = match props {
            None => HashMap::new(),
            Some(bytes) => self.decode_props(&bytes)?,
        };
        let kind = self.name_of(kind)?;
        let rel = Arc::new(Rel { id, kind, from: src as u64, to: dst as u64, props });
        self.rels.borrow_mut().put(id, rel.clone(), 128);
        Ok(Some(rel))
    }

    fn adjacency(&self, id: NodeId, kind: &str, dir: Dir) -> Result<Arc<[(NodeId, RelId)]>> {
        let Some(kind) = self.name_id(kind) else { return Ok(Arc::from(Vec::new())) };
        let dk = kind * 2 + if dir == Dir::In { 1 } else { 0 };
        if let Some(list) = self.adj.borrow_mut().get(&(id, dk)) {
            self.bump(|s| s.cache_hits += 1);
            return Ok(list);
        }
        self.bump(|s| {
            s.cache_misses += 1;
            s.statements += 1;
        });
        let mut stmt = self
            .conn
            .prepare_cached("SELECT other, rel FROM adj WHERE node = ?1 AND dk = ?2 ORDER BY other, rel")
            .map_err(err)?;
        let rows = stmt
            .query_map(params![id as i64, dk], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64)))
            .map_err(err)?;
        let list: Vec<(NodeId, RelId)> = rows.collect::<std::result::Result<_, _>>().map_err(err)?;
        self.bump(|s| s.rows_read += list.len() as u64);
        let bytes = 32 + list.len() * 16;
        let list: Arc<[(NodeId, RelId)]> = Arc::from(list);
        self.adj.borrow_mut().put((id, dk), list.clone(), bytes);
        Ok(list)
    }

    fn scan_label(&self, label: &str, after: NodeId, limit: usize) -> Result<Vec<Arc<Node>>> {
        let Some(label) = self.name_id(label) else { return Ok(Vec::new()) };
        Ok(self.scan_raw(&self.conn, label, after, limit)?.into_iter().map(Arc::new).collect())
    }

    fn index_range(&self, ty: &str, field: &str, interval: &Interval) -> Result<Option<Vec<NodeId>>> {
        let Some(spec) = self.specs.iter().find(|s| s.ty == ty && s.field == field) else {
            return Ok(None);
        };
        self.bump(|s| s.statements += 1);
        let collect = |stmt: &mut rusqlite::CachedStatement<'_>, p: &[&dyn rusqlite::ToSql]| -> Result<Vec<NodeId>> {
            let rows = stmt.query_map(p, |r| r.get::<_, i64>(0)).map_err(err)?;
            rows.map(|r| r.map(|id| id as u64).map_err(err)).collect()
        };
        let ids = match interval {
            Interval::Empty => Vec::new(),
            Interval::Num { low, high } => {
                let low = low.unwrap_or(f64::NEG_INFINITY);
                let high = high.unwrap_or(f64::INFINITY);
                if low > high {
                    Vec::new()
                } else {
                    let mut stmt = self
                        .conn
                        .prepare_cached("SELECT id FROM ixn WHERE spec = ?1 AND key >= ?2 AND key <= ?3")
                        .map_err(err)?;
                    collect(&mut stmt, &[&spec.id, &low, &high])?
                }
            }
            Interval::Str { low, high } => {
                let low = low.clone().unwrap_or_default();
                match high {
                    Some(high) if &low > high => Vec::new(),
                    Some(high) => {
                        let mut stmt = self
                            .conn
                            .prepare_cached("SELECT id FROM ixs WHERE spec = ?1 AND key >= ?2 AND key <= ?3")
                            .map_err(err)?;
                        collect(&mut stmt, &[&spec.id, &low, high])?
                    }
                    None => {
                        let mut stmt = self
                            .conn
                            .prepare_cached("SELECT id FROM ixs WHERE spec = ?1 AND key >= ?2")
                            .map_err(err)?;
                        collect(&mut stmt, &[&spec.id, &low])?
                    }
                }
            }
        };
        self.bump(|s| s.rows_read += ids.len() as u64);
        Ok(Some(ids))
    }

    fn has_index(&self, ty: &str, field: &str) -> bool {
        self.specs.iter().any(|s| s.ty == ty && s.field == field)
    }

    fn next_ids(&self) -> (NodeId, RelId) {
        self.next.get()
    }

    fn apply(&mut self, ops: Vec<Op>) -> Result<()> {
        let mut touched = Touched::default();
        let tx = self.conn.unchecked_transaction().map_err(err)?;
        let result = self.apply_ops(&tx, &ops, &mut touched);
        let result = result.and_then(|done| tx.commit().map_err(err).map(|()| done));
        match result {
            Ok((written, next)) => {
                self.next.set(next);
                self.bump(|s| {
                    s.rows_written += written;
                    s.statements += ops.len() as u64;
                });
                let mut nodes = self.nodes.borrow_mut();
                for id in &touched.nodes {
                    nodes.remove(id);
                }
                let mut rels = self.rels.borrow_mut();
                for id in &touched.rels {
                    rels.remove(id);
                }
                let mut adj = self.adj.borrow_mut();
                for key in &touched.adj {
                    adj.remove(key);
                }
                if !touched.all_adj_of.is_empty() {
                    // Rare (node deletes): drop adjacency wholesale.
                    adj.clear();
                }
                Ok(())
            }
            Err(error) => {
                // The transaction rolled back; names interned inside it are gone.
                self.load_catalog()?;
                Err(error)
            }
        }
    }
}

impl Instrumented for SqliteStore {
    fn stats(&self) -> Stats {
        self.stats.get()
    }
}
