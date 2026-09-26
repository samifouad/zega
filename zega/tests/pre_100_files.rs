//! Data directories written by the engine before zegadb/zega#100 changed how
//! a graph is held in memory. The WAL, snapshot and `.graph` bytes did not
//! change, so they must open to exactly the graph that engine read back.
//!
//! The fixtures were written by `main` at 081493a (a throwaway example, run
//! once): `store/` is a snapshot plus a WAL of later statements (an update
//! with a null, a new node, a relationship with a property, a detach
//! delete), and `import/` is a WAL whose first entry replaces the graph with
//! a `.graph` file, followed by more writes. `store.json` and `import.json`
//! are what that engine's `graph_json` returned after reopening them.

use std::path::Path;

use serde_json::{json, Value as Json};
use zega::Zega;

const SCHEMA: &str = r#"schema {
  type City {
    name: String
    pop: Int
    area: Float
    capital: Bool
    note?: String
    at: Point from (lat, lon)
    emb: Vector<3>
    road: ROAD -> City[] { km: Float }
  }
}
unique { City { name } }"#;

/// Open a copy of fixture directory `name`, so opening it (which may
/// repair or mark the WAL) never changes the checked-in bytes.
fn open(name: &str) -> (tempfile::TempDir, Zega) {
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pre-100").join(name);
    let dir = tempfile::tempdir().unwrap();
    copy(&from, &dir.path().join(name));
    let zega = Zega::open(dir.path().join(name).to_str().unwrap()).build().unwrap();
    (dir, zega)
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn expected(name: &str) -> Json {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pre-100").join(name);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn a_snapshot_and_wal_from_before_open_to_the_same_graph() {
    let (dir, zega) = open("store");
    assert_eq!(zega.graph_json().unwrap(), expected("store.json"));
    // The unique index, the relationship and its property answer as before.
    assert_eq!(
        zega.run_lang(SCHEMA, r#"{ City(name: "Halifax") { pop road -> City { name &km } } }"#).unwrap(),
        json!({"pop": 439819, "road": [{"name": "東京", "km": 10338.0}]})
    );
    let taken = zega.run_lang(SCHEMA, r#"mutation { City(name: "Ottawa" && pop: 1 && area: 1.0 && capital: false && at: @point(0, 0) && emb: @vector[1, 0, 0]) { name } }"#);
    assert!(taken.unwrap_err().to_string().contains("already used"));
    // Written on, snapshotted and reopened, it keeps both old and new.
    zega.run_lang(SCHEMA, r#"mutation { City(name: "Regina" && pop: 226404 && area: 179.97 && capital: false && at: @point(50.4452, -104.6189) && emb: @vector[1, 1, 1]) { name } }"#).unwrap();
    zega.snapshot().unwrap();
    let after = zega.graph_json().unwrap();
    drop(zega);
    let reopened = Zega::open(dir.path().join("store").to_str().unwrap()).build().unwrap();
    assert_eq!(reopened.graph_json().unwrap(), after);
    assert_eq!(after["nodes"].as_array().unwrap().len(), 5);
}

#[test]
fn a_wal_holding_a_graph_import_from_before_opens_to_the_same_graph() {
    let (_dir, zega) = open("import");
    assert_eq!(zega.graph_json().unwrap(), expected("import.json"));
    assert_eq!(
        zega.run_lang(SCHEMA, r#"{ City(name: "Bergen") { road -> City { name &km } } }"#).unwrap(),
        json!({"road": [{"name": "Oslo", "km": 463.0}]})
    );
}
