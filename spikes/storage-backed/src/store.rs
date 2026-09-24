//! The storage seam: what the engine asks of wherever the graph lives.
//!
//! Every read the ZQL executor makes today (`zega/src/v2.rs`) is one of these
//! calls. Nothing here hands out the whole graph: a scan is paged, adjacency
//! is per node and relationship kind, and an index answers with candidates.
//! Writes arrive as one statement's operations and commit as one transaction,
//! so the engine's undo journal becomes a plain list of operations.

use std::sync::Arc;

use crate::model::{Dir, Interval, Node, NodeId, Op, Rel, RelId};

#[derive(Debug)]
pub struct StoreError(pub String);

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StoreError {}

pub type Result<T> = std::result::Result<T, StoreError>;

pub trait GraphStore {
    /// One node by id. Cached.
    fn node(&self, id: NodeId) -> Result<Option<Arc<Node>>>;

    /// One relationship by id (for edge properties). Cached.
    fn relationship(&self, id: RelId) -> Result<Option<Arc<Rel>>>;

    /// `(neighbor, relationship)` pairs of one kind in one direction, sorted
    /// by `(neighbor, relationship)` the way `v2::neighbors` sorts. Cached.
    fn adjacency(&self, id: NodeId, kind: &str, dir: Dir) -> Result<Arc<[(NodeId, RelId)]>>;

    /// Up to `limit` nodes with `label` and id > `after`, in id order. Not
    /// cached: a scan must not evict the working set.
    fn scan_label(&self, label: &str, after: NodeId, limit: usize) -> Result<Vec<Arc<Node>>>;

    /// Candidate ids from the range index on `ty.field`, or None when that
    /// field has no index. Every node the interval can match is included;
    /// the caller still tests each one (as `zega/src/index.rs`).
    fn index_range(&self, ty: &str, field: &str, interval: &Interval) -> Result<Option<Vec<NodeId>>>;

    /// Whether `ty.field` has a range index.
    fn has_index(&self, ty: &str, field: &str) -> bool;

    /// The ids the next created node and relationship get.
    fn next_ids(&self) -> (NodeId, RelId);

    /// Apply one statement's writes as one transaction: all or nothing.
    fn apply(&mut self, ops: Vec<Op>) -> Result<()>;
}

/// Counters a store keeps, for the benchmark and the pricing model.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    /// SQL statements executed: a round trip each over a network store (D1).
    pub statements: u64,
    /// Rows returned by those statements: what DO SQLite and D1 bill as
    /// rows read (a lower bound; an index seek also reads its b-tree path).
    pub rows_read: u64,
    /// Rows inserted, updated or deleted (table and index rows).
    pub rows_written: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    /// Rows read and written as the storage backend itself bills them
    /// (Durable Object cursors), when it reports them.
    pub billed_rows_read: u64,
    pub billed_rows_written: u64,
}

impl Stats {
    pub fn since(self, before: Stats) -> Stats {
        Stats {
            statements: self.statements - before.statements,
            rows_read: self.rows_read - before.rows_read,
            rows_written: self.rows_written - before.rows_written,
            cache_hits: self.cache_hits - before.cache_hits,
            cache_misses: self.cache_misses - before.cache_misses,
            billed_rows_read: self.billed_rows_read - before.billed_rows_read,
            billed_rows_written: self.billed_rows_written - before.billed_rows_written,
        }
    }
}

/// A store that counts its work.
pub trait Instrumented {
    fn stats(&self) -> Stats;
}
