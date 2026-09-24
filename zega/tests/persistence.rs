//! Opening, reopening, the WAL, snapshots and the traversal work budget,
//! through ZQL. These are the behaviours of the engine that outlive the
//! legacy Cypher-shaped language (zega#55); each test names the legacy test
//! it replaces and keeps its assertion.

use std::collections::HashSet;

use serde_json::{json, Value as Json};
use zega::Zega;

const PEOPLE: &str = "schema { type Person { name: String } }";

/// The rows of a ZQL read as a list: one match comes back as an object.
fn rows(result: Json) -> Vec<Json> {
    match result {
        Json::Array(rows) => rows,
        Json::Null => Vec::new(),
        row => vec![row],
    }
}

fn people(zega: &Zega) -> Vec<Json> {
    rows(zega.run_lang(PEOPLE, "query { Person { name } }").unwrap())
}

// ---------------------------------------------------------------------------
// Builder and lifecycle (exhaustive_zega_core §1)
// ---------------------------------------------------------------------------

#[test]
fn open_in_memory_builds_successfully() {
    let zega = Zega::in_memory().build();
    assert!(zega.is_ok());
}

#[test]
fn open_on_disk_creates_directory_and_builds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    let zega = Zega::open(path.to_str().unwrap()).build();
    assert!(zega.is_ok());
    assert!(path.exists());
}

#[test]
fn builder_traversal_work_budget_is_chainable() {
    let zega = Zega::in_memory()
        .traversal_work_budget(10)
        .wal_flush_every_write()
        .build();
    assert!(zega.is_ok());
}

/// Legacy: `empty_query_string_returns_no_rows` and
/// `whitespace_only_query_returns_no_rows`. ZQL's empty read block is the
/// statement that asks for nothing.
#[test]
fn empty_query_block_returns_no_rows() {
    let zega = Zega::in_memory().build().unwrap();
    for source in ["query { }", "query {   \n  \t }"] {
        assert!(rows(zega.run_lang(PEOPLE, source).unwrap()).is_empty(), "{source:?}");
    }
}

// ---------------------------------------------------------------------------
// WAL recovery and snapshots (exhaustive_zega_core §21, lib.rs
// `test_wal_recovery` and `test_snapshot_restore`)
// ---------------------------------------------------------------------------

#[test]
fn wal_recovery_restores_nodes_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_str().unwrap();
    {
        let zega = Zega::open(path).wal_flush_every_write().build().unwrap();
        zega.run_lang(PEOPLE, r#"mutation { Person(name: "Alice") }"#).unwrap();
        // The WAL is flushed on every write; there is no snapshot.
    }
    assert!(!dir.path().join("snapshot.bin").exists());
    {
        let zega = Zega::open(path).wal_flush_every_write().build().unwrap();
        assert_eq!(people(&zega), [json!({ "name": "Alice" })]);
    }
}

#[test]
fn snapshot_then_reopen_restores_graph() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_str().unwrap();
    {
        let zega = Zega::open(path).wal_flush_every_write().build().unwrap();
        zega.run_lang(PEOPLE, r#"mutation { Person(name: "Alice") }"#).unwrap();
        zega.snapshot().unwrap();
    }
    // `snapshot` does not truncate the WAL, which alone would restore Alice;
    // without it, only the snapshot can.
    std::fs::remove_file(dir.path().join("wal.bin")).unwrap();
    {
        let zega = Zega::open(path).wal_flush_every_write().build().unwrap();
        assert_eq!(people(&zega), [json!({ "name": "Alice" })]);
    }
}

#[test]
fn snapshot_on_in_memory_db_is_noop_ok() {
    let zega = Zega::in_memory().build().unwrap();
    zega.run_lang(PEOPLE, r#"mutation { Person(name: "x") }"#).unwrap();
    assert!(zega.snapshot().is_ok());
}

#[test]
fn snapshot_then_more_writes_then_reopen_replays_wal_on_snapshot() {
    const SNAP: &str = "schema { type Snap { id: String } }";
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_str().unwrap();
    {
        let zega = Zega::open(path).wal_flush_every_write().build().unwrap();
        zega.run_lang(SNAP, r#"mutation { Snap(id: "before") }"#).unwrap();
        zega.snapshot().unwrap();
        // This write happens AFTER the snapshot -> only in the WAL.
        zega.run_lang(SNAP, r#"mutation { Snap(id: "after") }"#).unwrap();
    }
    {
        let zega = Zega::open(path).wal_flush_every_write().build().unwrap();
        let ids: HashSet<String> = rows(zega.run_lang(SNAP, "query { Snap { id } }").unwrap())
            .into_iter()
            .map(|row| row["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(ids, HashSet::from(["before".to_string(), "after".to_string()]));
    }
}

#[test]
fn reopen_without_any_writes_yields_empty_graph() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_str().unwrap();
    {
        let _zega = Zega::open(path).build().unwrap();
    }
    let zega = Zega::open(path).build().unwrap();
    assert!(people(&zega).is_empty());
    assert_eq!(zega.graph_json().unwrap(), json!({ "nodes": [], "rels": [] }));
}

#[test]
fn builder_wal_flush_interval_is_chainable_and_builds() {
    const I: &str = "schema { type I { k: Int } }";
    let dir = tempfile::tempdir().unwrap();
    let zega = Zega::open(dir.path().to_str().unwrap())
        .wal_flush_interval(10)
        .build()
        .unwrap();
    zega.run_lang(I, "mutation { I(k: 1) }").unwrap();
    assert_eq!(rows(zega.run_lang(I, "query { I { k } }").unwrap()).len(), 1);
}

// ---------------------------------------------------------------------------
// The traversal work budget (lib.rs
// `test_variable_length_traversal_aborts_at_work_budget`,
// exhaustive_zega_core `traversal_work_budget_exceeded_is_error_value` and
// `builder_low_traversal_budget_triggers_budget_error_on_traversal`)
// ---------------------------------------------------------------------------

const CHAIN: &str = "schema { type N { id: String r -> N[] } }";

/// a -> b -> c -> d, the legacy tests' `seed_relationships` chain.
fn chain(zega: &Zega) {
    zega.run_lang(
        CHAIN,
        r#"mutation { N(id: "a") { r -> N(id: "b") { r -> N(id: "c") { r -> N(id: "d") } } } }"#,
    )
    .unwrap();
}

const WALK: &str = r#"query { N(id: "a") { r *1..10 -> N { id } } }"#;

fn budget_error(zega: &Zega, source: &str) -> String {
    zega.run_lang(CHAIN, source).unwrap_err().to_string()
}

#[test]
fn variable_length_traversal_aborts_at_work_budget() {
    let zega = Zega::in_memory().traversal_work_budget(2).build().unwrap();
    chain(&zega);
    let error = budget_error(&zega, WALK);
    assert!(error.contains("relationship traversal work budget exceeded"), "{error}");

    // The walk reads three relationships; a budget of exactly three is enough,
    // so the error above is the limit of 2, not the walk.
    let zega = Zega::in_memory().traversal_work_budget(3).build().unwrap();
    chain(&zega);
    let reached = zega.run_lang(CHAIN, WALK).unwrap();
    assert_eq!(reached, json!({ "r": [{ "id": "b" }, { "id": "c" }, { "id": "d" }] }));
}

#[test]
fn a_zero_budget_errors_on_the_first_hop() {
    let zega = Zega::in_memory().traversal_work_budget(0).build().unwrap();
    chain(&zega);
    let hop = r#"query { N(id: "a") { r -> N { id } } }"#;
    let error = budget_error(&zega, hop);
    assert!(error.contains("relationship traversal work budget exceeded"), "{error}");

    // One relationship read is all a single hop needs.
    let zega = Zega::in_memory().traversal_work_budget(1).build().unwrap();
    chain(&zega);
    assert_eq!(zega.run_lang(CHAIN, hop).unwrap(), json!({ "r": [{ "id": "b" }] }));
}

