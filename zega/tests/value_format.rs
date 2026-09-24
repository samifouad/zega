//! The on-disk encoding of `Value` is part of the WAL and snapshot format:
//! bincode writes each variant's discriminant. `tests/fixtures/value-format/`
//! holds a store written by zega 0.1.2 (before `Value` moved out of the
//! legacy parser, zega#55): a snapshot and a WAL that together carry every
//! `Value` variant, as node and relationship properties. This test opens a
//! copy of that store and checks it reads back exactly as it was written.

use std::path::Path;

use serde_json::Value as Json;
use zega::Zega;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/value-format");

fn copy_store(to: &Path) {
    for file in ["snapshot.bin", "wal.bin"] {
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
    // Both halves of the store carry data: one node came from the snapshot,
    // the other and the relationship from the WAL.
    assert_eq!(graph["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(graph["rels"].as_array().unwrap().len(), 1);
    assert_eq!(graph, expected());

    // Written again by this build and reopened, it is still the same graph.
    zega.snapshot().unwrap();
    drop(zega);
    let reopened = Zega::open(dir.path().to_str().unwrap()).build().unwrap();
    assert_eq!(reopened.graph_json().unwrap(), expected());
}

/// Writes the fixture. It ran once against zega 0.1.2, before the move, and
/// its output is committed; it uses the legacy query path (the only writer
/// that accepts `List` and `Map` values), so it goes with that path.
#[test]
#[ignore = "writes tests/fixtures/value-format; run by hand only"]
fn write_fixture() {
    use std::collections::HashMap;
    use zega::location::Point;
    use zega::vector::{Metric, Vector};
    use zega::Value;

    let every_variant = |tag: &str| -> HashMap<String, Value> {
        HashMap::from([
            ("tag".to_string(), Value::String(tag.to_string())),
            ("s".to_string(), Value::String("text".to_string())),
            ("i".to_string(), Value::Int(-42)),
            ("f".to_string(), Value::Float(1.5f64.to_bits())),
            ("b".to_string(), Value::Bool(true)),
            (
                "l".to_string(),
                Value::List(vec![Value::Int(1), Value::String("two".to_string()), Value::Null]),
            ),
            (
                "m".to_string(),
                Value::Map(HashMap::from([
                    ("k".to_string(), Value::Int(7)),
                    ("nested".to_string(), Value::List(vec![Value::Bool(false)])),
                ])),
            ),
            ("p".to_string(), Value::Point(Point::new(51.0447, -114.0719).unwrap())),
            ("v".to_string(), Value::Vector(Vector::new(&[0.25, -0.5, 1.0], Metric::Cosine).unwrap())),
        ])
    };
    let create = "CREATE (n:Kinds {tag: $tag, s: $s, i: $i, f: $f, b: $b, l: $l, m: $m, p: $p, v: $v})";

    let dir = Path::new(FIXTURE);
    for file in ["snapshot.bin", "wal.bin"] {
        let _ = std::fs::remove_file(dir.join(file));
    }
    std::fs::create_dir_all(dir).unwrap();
    let zega = Zega::open(dir.to_str().unwrap()).wal_flush_every_write().build().unwrap();
    zega.query(create, every_variant("snapshot")).unwrap();
    zega.snapshot().unwrap();
    // After the snapshot: only in the WAL.
    zega.query(create, every_variant("wal")).unwrap();
    zega.query(
        "MATCH (a:Kinds {tag: 'snapshot'}) MATCH (b:Kinds {tag: 'wal'}) \
         CREATE (a)-[:R {s: $s, i: $i, f: $f, b: $b, l: $l, m: $m, p: $p, v: $v}]->(b)",
        every_variant("rel"),
    )
    .unwrap();
    let graph = zega.graph_json().unwrap();
    drop(zega);
    std::fs::write(
        dir.join("expected.json"),
        serde_json::to_string_pretty(&graph).unwrap() + "\n",
    )
    .unwrap();
}
