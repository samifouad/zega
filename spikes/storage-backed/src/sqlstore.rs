//! [`GraphStore`] over any SQLite, with a byte-bounded object cache.
//!
//! The SQL is the same everywhere; only the [`Driver`] differs: rusqlite on
//! a machine (`crate::sqlite`), `ctx.storage.sql` in a Durable Object (the
//! `edge` crate). Tables:
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

use std::cell::{Cell as StdCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::cache::ByteLru;
use crate::model::{index_key, node_bytes, Dir, Interval, Key, Node, NodeId, Op, Rel, RelId, Value};
use crate::store::{GraphStore, Instrumented, Result, Stats, StoreError};

pub const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS names(id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE)",
    "CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY, v INTEGER NOT NULL) WITHOUT ROWID",
    "CREATE TABLE IF NOT EXISTS spec(id INTEGER PRIMARY KEY, ty TEXT NOT NULL, field TEXT NOT NULL, UNIQUE(ty, field))",
    "CREATE TABLE IF NOT EXISTS node(id INTEGER PRIMARY KEY, labels BLOB NOT NULL, props BLOB NOT NULL)",
    "CREATE TABLE IF NOT EXISTS lbl(label INTEGER NOT NULL, id INTEGER NOT NULL, PRIMARY KEY(label, id)) WITHOUT ROWID",
    "CREATE TABLE IF NOT EXISTS rel(id INTEGER PRIMARY KEY, kind INTEGER NOT NULL, src INTEGER NOT NULL, dst INTEGER NOT NULL, props BLOB)",
    "CREATE TABLE IF NOT EXISTS adj(node INTEGER NOT NULL, dk INTEGER NOT NULL, other INTEGER NOT NULL, rel INTEGER NOT NULL, PRIMARY KEY(node, dk, other, rel)) WITHOUT ROWID",
    "CREATE TABLE IF NOT EXISTS ixn(spec INTEGER NOT NULL, key REAL NOT NULL, id INTEGER NOT NULL, PRIMARY KEY(spec, key, id)) WITHOUT ROWID",
    "CREATE TABLE IF NOT EXISTS ixs(spec INTEGER NOT NULL, key TEXT NOT NULL, id INTEGER NOT NULL, PRIMARY KEY(spec, key, id)) WITHOUT ROWID",
];

/// A bound parameter.
#[derive(Clone, Copy, Debug)]
pub enum P<'a> {
    I(i64),
    F(f64),
    T(&'a str),
    B(&'a [u8]),
    Null,
}

/// A result cell.
#[derive(Clone, Debug)]
pub enum Cell {
    I(i64),
    F(f64),
    T(String),
    B(Vec<u8>),
    Null,
}

impl Cell {
    pub fn int(&self) -> Result<i64> {
        match self {
            Cell::I(v) => Ok(*v),
            Cell::F(v) => Ok(*v as i64),
            other => Err(StoreError(format!("expected an integer, got {other:?}"))),
        }
    }
    pub fn text(&self) -> Result<&str> {
        match self {
            Cell::T(v) => Ok(v),
            other => Err(StoreError(format!("expected text, got {other:?}"))),
        }
    }
    pub fn blob(&self) -> Result<&[u8]> {
        match self {
            Cell::B(v) => Ok(v),
            other => Err(StoreError(format!("expected a blob, got {other:?}"))),
        }
    }
}

/// What the store needs from SQLite.
pub trait Driver {
    /// Every row of one statement.
    fn query(&self, sql: &str, params: &[P<'_>]) -> Result<Vec<Vec<Cell>>>;
    /// Rows changed by one statement.
    fn execute(&self, sql: &str, params: &[P<'_>]) -> Result<u64>;
    /// Run `f` as one transaction: committed if it returns Ok, rolled back
    /// (every write inside it undone) if it returns Err.
    fn transaction(&self, f: &mut dyn FnMut() -> Result<()>) -> Result<()>;
    /// Rows read and written as the backend itself counts them for billing
    /// (a Durable Object cursor's `rowsRead`/`rowsWritten`), if it does.
    fn billed(&self) -> Option<(u64, u64)> {
        None
    }
}

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

type Adjacency = Arc<[(NodeId, RelId)]>;

pub struct SqlStore<D: Driver> {
    db: D,
    names: RefCell<Names>,
    specs: Vec<Spec>,
    nodes: RefCell<ByteLru<NodeId, Arc<Node>>>,
    rels: RefCell<ByteLru<RelId, Arc<Rel>>>,
    adj: RefCell<ByteLru<(NodeId, i64), Adjacency>>,
    stats: StdCell<Stats>,
    next: StdCell<(NodeId, RelId)>,
}

#[derive(Default)]
struct Touched {
    nodes: HashSet<NodeId>,
    rels: HashSet<RelId>,
    adj: HashSet<(NodeId, i64)>,
    node_deleted: bool,
}

impl<D: Driver> SqlStore<D> {
    /// Open a store over `db`, creating the tables if needed. `object_cache`
    /// bytes hold decoded nodes (60%), adjacency lists (35%) and
    /// relationships (5%).
    pub fn new(db: D, object_cache: usize) -> Result<Self> {
        for statement in SCHEMA {
            db.execute(statement, &[])?;
        }
        let mut store = SqlStore {
            db,
            names: RefCell::new(Names::default()),
            specs: Vec::new(),
            nodes: RefCell::new(ByteLru::new(object_cache * 60 / 100)),
            adj: RefCell::new(ByteLru::new(object_cache * 35 / 100)),
            rels: RefCell::new(ByteLru::new(object_cache * 5 / 100)),
            stats: StdCell::new(Stats::default()),
            next: StdCell::new((1, 1)),
        };
        store.load_catalog()?;
        Ok(store)
    }

    pub fn driver(&self) -> &D {
        &self.db
    }

    fn load_catalog(&mut self) -> Result<()> {
        let mut names = Names::default();
        for row in self.db.query("SELECT id, name FROM names", &[])? {
            let (id, name) = (row[0].int()? as u32, row[1].text()?.to_string());
            names.by_name.insert(name.clone(), id);
            names.by_id.insert(id, name);
        }
        *self.names.borrow_mut() = names;
        self.specs = self
            .db
            .query("SELECT id, ty, field FROM spec", &[])?
            .into_iter()
            .map(|row| Ok(Spec { id: row[0].int()?, ty: row[1].text()?.to_string(), field: row[2].text()?.to_string() }))
            .collect::<Result<_>>()?;
        let mut next = (1, 1);
        for row in self.db.query("SELECT k, v FROM meta", &[])? {
            match row[0].text()? {
                "next_node" => next.0 = row[1].int()? as u64,
                "next_rel" => next.1 = row[1].int()? as u64,
                _ => {}
            }
        }
        self.next.set(next);
        Ok(())
    }

    /// Declare a range index on `ty.field` (the `index { range … }` block and
    /// every unique field). Builds it from the stored nodes.
    pub fn declare_range(&mut self, ty: &str, field: &str) -> Result<()> {
        if self.has_index(ty, field) {
            return Ok(());
        }
        let mut run = || -> Result<()> {
            self.db.execute("INSERT INTO spec(ty, field) VALUES (?1, ?2)", &[P::T(ty), P::T(field)])?;
            let spec = self.db.query("SELECT id FROM spec WHERE ty = ?1 AND field = ?2", &[P::T(ty), P::T(field)])?[0][0].int()?;
            let label = self.intern(ty)?;
            let mut after = 0u64;
            loop {
                let page = self.scan_raw(label, after, 4096)?;
                let Some(last) = page.last() else { break };
                after = last.id;
                for node in &page {
                    if let Some(value) = node.props.get(field) {
                        self.insert_key(spec, value, node.id)?;
                    }
                }
            }
            Ok(())
        };
        let result = self.db.transaction(&mut run);
        self.load_catalog()?;
        result
    }

    fn bump(&self, f: impl FnOnce(&mut Stats)) {
        let mut stats = self.stats.get();
        f(&mut stats);
        self.stats.set(stats);
    }

    fn q(&self, sql: &str, params: &[P<'_>]) -> Result<Vec<Vec<Cell>>> {
        self.bump(|s| s.statements += 1);
        self.db.query(sql, params)
    }

    fn x(&self, sql: &str, params: &[P<'_>]) -> Result<u64> {
        self.bump(|s| s.statements += 1);
        let n = self.db.execute(sql, params)?;
        self.bump(|s| s.rows_written += n);
        Ok(n)
    }

    fn name_id(&self, name: &str) -> Option<i64> {
        self.names.borrow().by_name.get(name).map(|id| *id as i64)
    }

    fn intern(&self, name: &str) -> Result<i64> {
        if let Some(id) = self.name_id(name) {
            return Ok(id);
        }
        self.x("INSERT INTO names(name) VALUES (?1)", &[P::T(name)])?;
        let id = self.q("SELECT id FROM names WHERE name = ?1", &[P::T(name)])?[0][0].int()? as u32;
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
        let names = self.names.borrow();
        let mut out = HashMap::with_capacity(props.len());
        for (k, v) in props {
            let name = names.by_id.get(&k).ok_or_else(|| StoreError(format!("unknown name {k}")))?;
            out.insert(name.clone(), v);
        }
        Ok(out)
    }

    fn decode_node(&self, id: NodeId, labels: &[u8], props: &[u8]) -> Result<Node> {
        let labels: Vec<u32> = bincode::deserialize(labels).map_err(err)?;
        Ok(Node {
            id,
            labels: labels.into_iter().map(|l| self.name_of(l)).collect::<Result<_>>()?,
            props: self.decode_props(props)?,
        })
    }

    fn encode_props(&self, props: &HashMap<String, Value>) -> Result<Vec<u8>> {
        let mut out: Vec<(u32, &Value)> = Vec::with_capacity(props.len());
        for (k, v) in props {
            out.push((self.intern(k)? as u32, v));
        }
        out.sort_by_key(|(k, _)| *k);
        bincode::serialize(&out).map_err(err)
    }

    fn read_node(&self, id: NodeId) -> Result<Option<Node>> {
        let rows = self.q("SELECT labels, props FROM node WHERE id = ?1", &[P::I(id as i64)])?;
        let Some(row) = rows.first() else { return Ok(None) };
        self.bump(|s| s.rows_read += 1);
        self.decode_node(id, row[0].blob()?, row[1].blob()?).map(Some)
    }

    fn scan_raw(&self, label: i64, after: NodeId, limit: usize) -> Result<Vec<Node>> {
        let rows = self.q(
            "SELECT n.id, n.labels, n.props FROM lbl l JOIN node n ON n.id = l.id
             WHERE l.label = ?1 AND l.id > ?2 ORDER BY l.id LIMIT ?3",
            &[P::I(label), P::I(after as i64), P::I(limit as i64)],
        )?;
        // The label row and the node row.
        self.bump(|s| s.rows_read += 2 * rows.len() as u64);
        rows.iter().map(|row| self.decode_node(row[0].int()? as u64, row[1].blob()?, row[2].blob()?)).collect()
    }

    fn specs_for<'a>(&'a self, labels: &'a [String]) -> impl Iterator<Item = &'a Spec> + 'a {
        self.specs.iter().filter(move |spec| labels.contains(&spec.ty))
    }

    fn insert_key(&self, spec: i64, value: &Value, id: NodeId) -> Result<()> {
        match index_key(value) {
            Some(Key::Num(n)) => self.x("INSERT OR IGNORE INTO ixn(spec, key, id) VALUES (?1, ?2, ?3)", &[P::I(spec), P::F(n), P::I(id as i64)])?,
            Some(Key::Str(s)) => self.x("INSERT OR IGNORE INTO ixs(spec, key, id) VALUES (?1, ?2, ?3)", &[P::I(spec), P::T(s), P::I(id as i64)])?,
            None => 0,
        };
        Ok(())
    }

    fn write_node_rows(&self, node: &Node) -> Result<()> {
        let labels: Vec<u32> = node.labels.iter().map(|l| self.intern(l).map(|id| id as u32)).collect::<Result<_>>()?;
        let props = self.encode_props(&node.props)?;
        let label_bytes = bincode::serialize(&labels).map_err(err)?;
        self.x(
            "INSERT OR REPLACE INTO node(id, labels, props) VALUES (?1, ?2, ?3)",
            &[P::I(node.id as i64), P::B(&label_bytes), P::B(&props)],
        )?;
        for label in &labels {
            self.x("INSERT OR IGNORE INTO lbl(label, id) VALUES (?1, ?2)", &[P::I(*label as i64), P::I(node.id as i64)])?;
        }
        for spec in self.specs_for(&node.labels) {
            if let Some(value) = node.props.get(&spec.field) {
                self.insert_key(spec.id, value, node.id)?;
            }
        }
        Ok(())
    }

    fn delete_index_rows(&self, node: &Node) -> Result<()> {
        for spec in self.specs_for(&node.labels) {
            let Some(value) = node.props.get(&spec.field) else { continue };
            match index_key(value) {
                Some(Key::Num(n)) => self.x("DELETE FROM ixn WHERE spec = ?1 AND key = ?2 AND id = ?3", &[P::I(spec.id), P::F(n), P::I(node.id as i64)])?,
                Some(Key::Str(s)) => self.x("DELETE FROM ixs WHERE spec = ?1 AND key = ?2 AND id = ?3", &[P::I(spec.id), P::T(s), P::I(node.id as i64)])?,
                None => 0,
            };
        }
        Ok(())
    }

    fn delete_rel_rows(&self, id: RelId, touched: &mut Touched) -> Result<()> {
        let rows = self.q("SELECT kind, src, dst FROM rel WHERE id = ?1", &[P::I(id as i64)])?;
        let Some(row) = rows.first() else { return Ok(()) };
        let (kind, src, dst) = (row[0].int()?, row[1].int()?, row[2].int()?);
        self.x("DELETE FROM rel WHERE id = ?1", &[P::I(id as i64)])?;
        let del = "DELETE FROM adj WHERE node = ?1 AND dk = ?2 AND other = ?3 AND rel = ?4";
        self.x(del, &[P::I(src), P::I(kind * 2), P::I(dst), P::I(id as i64)])?;
        self.x(del, &[P::I(dst), P::I(kind * 2 + 1), P::I(src), P::I(id as i64)])?;
        touched.rels.insert(id);
        touched.adj.insert((src as u64, kind * 2));
        touched.adj.insert((dst as u64, kind * 2 + 1));
        Ok(())
    }

    fn apply_ops(&self, ops: &[Op], touched: &mut Touched) -> Result<(NodeId, RelId)> {
        let (mut next_node, mut next_rel) = self.next.get();
        for op in ops {
            match op {
                Op::InsertNode { id, labels, props } => {
                    let node = Node { id: *id, labels: labels.clone(), props: props.clone() };
                    if let Some(old) = self.read_node(*id)? {
                        self.delete_index_rows(&old)?;
                    }
                    self.write_node_rows(&node)?;
                    touched.nodes.insert(*id);
                    next_node = next_node.max(id + 1);
                }
                Op::UpdateNode { id, props } => {
                    let Some(mut node) = self.read_node(*id)? else { continue };
                    self.delete_index_rows(&node)?;
                    node.props.extend(props.clone());
                    self.write_node_rows(&node)?;
                    touched.nodes.insert(*id);
                }
                Op::DeleteNode { id } => {
                    let Some(node) = self.read_node(*id)? else { continue };
                    self.delete_index_rows(&node)?;
                    self.x("DELETE FROM lbl WHERE id = ?1", &[P::I(*id as i64)])?;
                    self.x("DELETE FROM node WHERE id = ?1", &[P::I(*id as i64)])?;
                    for row in self.q("SELECT rel FROM adj WHERE node = ?1", &[P::I(*id as i64)])? {
                        self.delete_rel_rows(row[0].int()? as u64, touched)?;
                    }
                    touched.nodes.insert(*id);
                    touched.node_deleted = true;
                }
                Op::InsertRel { id, kind, from, to, props } => {
                    let kind_id = self.intern(kind)?;
                    let props = if props.is_empty() { None } else { Some(self.encode_props(props)?) };
                    self.x(
                        "INSERT OR REPLACE INTO rel(id, kind, src, dst, props) VALUES (?1, ?2, ?3, ?4, ?5)",
                        &[P::I(*id as i64), P::I(kind_id), P::I(*from as i64), P::I(*to as i64), props.as_deref().map_or(P::Null, P::B)],
                    )?;
                    let adj = "INSERT OR IGNORE INTO adj(node, dk, other, rel) VALUES (?1, ?2, ?3, ?4)";
                    self.x(adj, &[P::I(*from as i64), P::I(kind_id * 2), P::I(*to as i64), P::I(*id as i64)])?;
                    self.x(adj, &[P::I(*to as i64), P::I(kind_id * 2 + 1), P::I(*from as i64), P::I(*id as i64)])?;
                    touched.rels.insert(*id);
                    touched.adj.insert((*from, kind_id * 2));
                    touched.adj.insert((*to, kind_id * 2 + 1));
                    next_rel = next_rel.max(id + 1);
                }
                Op::DeleteRel { id } => self.delete_rel_rows(*id, touched)?,
            }
        }
        self.x(
            "INSERT OR REPLACE INTO meta(k, v) VALUES ('next_node', ?1), ('next_rel', ?2)",
            &[P::I(next_node as i64), P::I(next_rel as i64)],
        )?;
        Ok((next_node, next_rel))
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
}

impl<D: Driver> GraphStore for SqlStore<D> {
    fn node(&self, id: NodeId) -> Result<Option<Arc<Node>>> {
        if let Some(node) = self.nodes.borrow_mut().get(&id) {
            self.bump(|s| s.cache_hits += 1);
            return Ok(Some(node));
        }
        self.bump(|s| s.cache_misses += 1);
        let Some(node) = self.read_node(id)? else { return Ok(None) };
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
        self.bump(|s| s.cache_misses += 1);
        let rows = self.q("SELECT kind, src, dst, props FROM rel WHERE id = ?1", &[P::I(id as i64)])?;
        let Some(row) = rows.first() else { return Ok(None) };
        self.bump(|s| s.rows_read += 1);
        let props = match &row[3] {
            Cell::Null => HashMap::new(),
            cell => self.decode_props(cell.blob()?)?,
        };
        let rel = Arc::new(Rel {
            id,
            kind: self.name_of(row[0].int()? as u32)?,
            from: row[1].int()? as u64,
            to: row[2].int()? as u64,
            props,
        });
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
        self.bump(|s| s.cache_misses += 1);
        let rows = self.q(
            "SELECT other, rel FROM adj WHERE node = ?1 AND dk = ?2 ORDER BY other, rel",
            &[P::I(id as i64), P::I(dk)],
        )?;
        let list: Vec<(NodeId, RelId)> =
            rows.iter().map(|r| Ok((r[0].int()? as u64, r[1].int()? as u64))).collect::<Result<_>>()?;
        self.bump(|s| s.rows_read += list.len() as u64);
        let bytes = 32 + list.len() * 16;
        let list: Arc<[(NodeId, RelId)]> = Arc::from(list);
        self.adj.borrow_mut().put((id, dk), list.clone(), bytes);
        Ok(list)
    }

    fn scan_label(&self, label: &str, after: NodeId, limit: usize) -> Result<Vec<Arc<Node>>> {
        let Some(label) = self.name_id(label) else { return Ok(Vec::new()) };
        Ok(self.scan_raw(label, after, limit)?.into_iter().map(Arc::new).collect())
    }

    fn index_range(&self, ty: &str, field: &str, interval: &Interval) -> Result<Option<Vec<NodeId>>> {
        let Some(spec) = self.specs.iter().find(|s| s.ty == ty && s.field == field) else {
            return Ok(None);
        };
        let rows = match interval {
            Interval::Empty => Vec::new(),
            Interval::Num { low, high } => {
                let low = low.unwrap_or(f64::MIN);
                let high = high.unwrap_or(f64::MAX);
                if low > high {
                    Vec::new()
                } else {
                    self.q("SELECT id FROM ixn WHERE spec = ?1 AND key >= ?2 AND key <= ?3", &[P::I(spec.id), P::F(low), P::F(high)])?
                }
            }
            Interval::Str { low, high } => {
                let low = low.clone().unwrap_or_default();
                match high {
                    Some(high) if &low > high => Vec::new(),
                    Some(high) => self.q(
                        "SELECT id FROM ixs WHERE spec = ?1 AND key >= ?2 AND key <= ?3",
                        &[P::I(spec.id), P::T(&low), P::T(high)],
                    )?,
                    None => self.q("SELECT id FROM ixs WHERE spec = ?1 AND key >= ?2", &[P::I(spec.id), P::T(&low)])?,
                }
            }
        };
        self.bump(|s| s.rows_read += rows.len() as u64);
        Ok(Some(rows.iter().map(|r| r[0].int().map(|id| id as u64)).collect::<Result<_>>()?))
    }

    fn has_index(&self, ty: &str, field: &str) -> bool {
        self.specs.iter().any(|s| s.ty == ty && s.field == field)
    }

    fn next_ids(&self) -> (NodeId, RelId) {
        self.next.get()
    }

    fn apply(&mut self, ops: Vec<Op>) -> Result<()> {
        let mut touched = Touched::default();
        let mut next = None;
        let result = self.db.transaction(&mut || {
            next = Some(self.apply_ops(&ops, &mut touched)?);
            Ok(())
        });
        match result {
            Ok(()) => {
                if let Some(next) = next {
                    self.next.set(next);
                }
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
                if touched.node_deleted {
                    adj.clear();
                }
                Ok(())
            }
            Err(error) => {
                // Rolled back: names interned inside the transaction are gone.
                self.load_catalog()?;
                Err(error)
            }
        }
    }
}

impl<D: Driver> Instrumented for SqlStore<D> {
    fn stats(&self) -> Stats {
        let mut stats = self.stats.get();
        if let Some((read, written)) = self.db.billed() {
            stats.billed_rows_read = read;
            stats.billed_rows_written = written;
        }
        stats
    }
}
