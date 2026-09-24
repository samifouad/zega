//! SPIKE (zegadb/aps, storage-backed zega). Not product code.
//!
//! The question: can zega answer queries with the graph in storage (Durable
//! Object SQLite) and only a bounded cache in memory? This crate is the
//! measurement rig for that question:
//!
//! - [`store::GraphStore`] is the storage seam the engine would run on.
//! - [`sqlstore::SqlStore`] implements it over SQLite with a byte-bounded
//!   cache, through a small [`sqlstore::Driver`]: rusqlite natively
//!   (`sqlite`), `ctx.storage.sql` in a Durable Object (the `edge` crate).
//! - [`mem::MemStore`] implements it over HashMaps, to separate the cost of
//!   the seam from the cost of storage.
//! - [`exec`] runs the measured ZQL subset (create with links, point read by a
//!   unique key, filter on a range index, 2-hop traversal, scan with limit)
//!   with the same semantics as `zega/src/v2.rs`; `tests/parity.rs` proves the
//!   JSON is identical to `Zega::run_lang` on the same data.
//! - [`gen`] builds the benchmark graph: 5 short fields and 3 relationships
//!   per node.

pub mod cache;
pub mod exec;
pub mod gen;
pub mod mem;
pub mod model;
#[cfg(feature = "native")]
pub mod sqlite;
pub mod sqlstore;
pub mod store;
