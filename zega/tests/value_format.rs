//! The on-disk encoding of `Value` is part of the WAL and snapshot format:
//! bincode writes each variant's discriminant. `tests/fixtures/value-format/`
//! holds a store written by zega 0.1.2 (before `Value` moved out of the
//! legacy parser, zega#55): a snapshot and a WAL that together carry every
//! `Value` variant, as node and relationship properties. It is frozen: the
//! test that wrote it (`write_fixture`, removed in zega 0.2.0 with the legacy
//! query path it used for `List` and `Map` values) ran once, before the move. These tests open a
//! copy of that store, and of each half alone, and check it reads back exactly
//! as it was written.
//!
//! `Zega::snapshot` does not truncate the WAL, so `wal.bin` holds every write
//! and `snapshot.bin` holds the graph as it was at the snapshot (the first
//! node). Opening each file alone proves each format decodes on its own.

use std::path::Path;

use serde_json::Value as Json;
use zega::Zega;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/value-format");

fn copy_store(to: &Path) {
    copy_files(to, &["snapshot.bin", "wal.bin"]);
}

fn copy_files(to: &Path, files: &[&str]) {
    for file in files {
        std::fs::copy(Path::new(FIXTURE).join(file), to.join(file)).unwrap();
    }
}

fn expected() -> Json {
    let text = std::fs::read_to_string(Path::new(FIXTURE).join("expected.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

#[test]
fn store_written_before_value_moved_reads_back_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    copy_store(dir.path());
    let zega = Zega::open(dir.path().to_str().unwrap()).build().unwrap();
    let graph = zega.graph_json().unwrap();
    assert_eq!(graph["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(graph["rels"].as_array().unwrap().len(), 1);
    assert_eq!(graph, expected());

    // Written again by this build and reopened, it is still the same graph.
    zega.snapshot().unwrap();
    drop(zega);
    let reopened = Zega::open(dir.path().to_str().unwrap()).build().unwrap();
    assert_eq!(reopened.graph_json().unwrap(), expected());
}

#[test]
fn snapshot_written_before_value_moved_reads_back_alone() {
    let dir = tempfile::tempdir().unwrap();
    copy_files(dir.path(), &["snapshot.bin"]);
    let zega = Zega::open(dir.path().to_str().unwrap()).build().unwrap();
    let expected = expected();
    // The graph at the snapshot: the first node, no relationship yet.
    assert_eq!(expected["nodes"][0]["tag"], "snapshot");
    assert_eq!(
        zega.graph_json().unwrap(),
        serde_json::json!({ "nodes": [expected["nodes"][0].clone()], "rels": [] })
    );
}

#[test]
fn wal_written_before_value_moved_reads_back_alone() {
    let dir = tempfile::tempdir().unwrap();
    copy_files(dir.path(), &["wal.bin"]);
    let zega = Zega::open(dir.path().to_str().unwrap()).build().unwrap();
    assert_eq!(zega.graph_json().unwrap(), expected());
}
