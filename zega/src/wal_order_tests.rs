//! zega#32: a write the WAL refused must not be visible, in this session or
//! after a reopen. Every test drives a real store, refuses the append through
//! the WAL's fault seam, then checks the graph, its indexes (label, property,
//! unique, declared range) and the id counters are what they were before.

use std::collections::HashMap;

use serde_json::Value as Json;
use tempfile::TempDir;

use crate::wal::tests::{switchable_wal, AppendSwitch};
use crate::{Value, Zega};

struct Store {
    zega: Zega,
    switch: AppendSwitch,
    dir: TempDir,
}

impl Store {
    fn open() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut zega = Zega::open(dir.path().to_str().unwrap())
            .wal_flush_every_write()
            .build()
            .unwrap();
        // Same file, same flush-every settings; only the append target can fail.
        let (wal, switch) = switchable_wal(&dir.path().join("wal.bin"));
        zega.wal = wal;
        Store { zega, switch, dir }
    }

    /// Everything a reader can observe, plus the ids the next writes get.
    fn state(&self) -> (Json, (u64, u64)) {
        let ids = self.zega.graph.lock().unwrap().next_ids();
        (self.zega.graph_json().unwrap(), ids)
    }

    /// Run `write` with the WAL refusing appends; it must fail and change nothing.
    fn refused<T: std::fmt::Debug, E: std::fmt::Display>(
        &self,
        write: impl FnOnce(&Zega) -> Result<T, E>,
    ) -> String {
        let before = self.state();
        self.switch.refuse(true);
        let error = write(&self.zega).expect_err("the WAL refused this write").to_string();
        self.switch.refuse(false);
        assert!(error.contains("injected write failure"), "{error}");
        assert_eq!(self.state(), before, "a refused write changed memory");
        error
    }

    fn reopened(self) -> Zega {
        let Store { zega, dir, .. } = self;
        drop(zega);
        Zega::open(dir.path().to_str().unwrap())
            .wal_flush_every_write()
            .build()
            .unwrap()
    }

    fn cypher(&self, query: &str) -> Vec<crate::Row> {
        self.zega.query(query, HashMap::new()).unwrap()
    }
}

fn count(zega: &Zega, query: &str) -> i64 {
    let rows = zega.query(query, HashMap::new()).unwrap();
    match rows[0].fields.values().next() {
        Some(Value::Int(n)) => *n,
        other => panic!("{query}: expected a count, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Cypher-style statements (`Zega::query`)
// ---------------------------------------------------------------------------

#[test]
fn refused_create_is_invisible_and_its_id_is_reused() {
    let store = Store::open();
    store.cypher("CREATE (n:Person {name: 'Ada'})");
    let (_, ids_before) = store.state();

    store.refused(|z| z.query("CREATE (n:Person {name: 'Bob'})", HashMap::new()));
    // Label index and property index both forget Bob.
    assert_eq!(count(&store.zega, "MATCH (n:Person) RETURN count(n) AS c"), 1);
    assert!(store.cypher("MATCH (n:Person {name: 'Bob'}) RETURN n").is_empty());

    // The same write goes through once the disk recovers, with Bob's old id.
    store.cypher("CREATE (n:Person {name: 'Bob'})");
    let (graph, _) = store.state();
    let bob = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["name"] == "Bob")
        .unwrap();
    assert_eq!(bob["id"].as_u64(), Some(ids_before.0));

    let after = store.state().0;
    assert_eq!(store.reopened().graph_json().unwrap(), after);
}

#[test]
fn refused_multi_node_create_leaves_no_node_and_no_relationship() {
    let store = Store::open();
    store.refused(|z| {
        z.query(
            "CREATE (a:Person {name: 'Ada'})-[:KNOWS]->(b:Person {name: 'Bob'})",
            HashMap::new(),
        )
    });
    assert_eq!(count(&store.zega, "MATCH (n:Person) RETURN count(n) AS c"), 0);
    assert_eq!(store.reopened().graph_json().unwrap()["nodes"], Json::Array(vec![]));
}

#[test]
fn refused_set_keeps_the_old_value_in_the_node_and_its_index() {
    let store = Store::open();
    store.cypher("CREATE (n:Person {name: 'Ada', age: 36})");
    store.refused(|z| {
        z.query(
            "MATCH (n:Person {name: 'Ada'}) SET n.age = 37 RETURN n.age AS age",
            HashMap::new(),
        )
    });
    let rows = store.cypher("MATCH (n:Person {name: 'Ada'}) RETURN n.age AS age");
    assert_eq!(rows[0].fields.get("age"), Some(&Value::Int(36)));
    assert!(store.cypher("MATCH (n:Person {age: 37}) RETURN n").is_empty());
    assert_eq!(store.cypher("MATCH (n:Person {age: 36}) RETURN n").len(), 1);

    let before = store.state().0;
    assert_eq!(store.reopened().graph_json().unwrap(), before);
}

#[test]
fn refused_detach_delete_keeps_the_node_and_its_relationships() {
    let store = Store::open();
    store.cypher("CREATE (a:Person {name: 'Ada'})-[:KNOWS]->(b:Person {name: 'Bob'})");
    store.refused(|z| z.query("MATCH (n:Person {name: 'Ada'}) DETACH DELETE n", HashMap::new()));
    assert_eq!(count(&store.zega, "MATCH (n:Person) RETURN count(n) AS c"), 2);
    assert_eq!(
        count(&store.zega, "MATCH (:Person {name: 'Ada'})-[:KNOWS]->(b) RETURN count(b) AS c"),
        1
    );

    let before = store.state().0;
    assert_eq!(store.reopened().graph_json().unwrap(), before);
}

#[test]
fn refused_relationship_delete_keeps_it_traversable() {
    let store = Store::open();
    store.cypher("CREATE (a:Person {name: 'Ada'})-[:KNOWS]->(b:Person {name: 'Bob'})");
    store.refused(|z| z.query("MATCH (:Person)-[r:KNOWS]->() DELETE r", HashMap::new()));
    assert_eq!(
        count(&store.zega, "MATCH (:Person)-[:KNOWS]->(b) RETURN count(b) AS c"),
        1
    );
    let before = store.state().0;
    assert_eq!(store.reopened().graph_json().unwrap(), before);
}

#[test]
fn refused_merge_creates_nothing_and_a_retry_creates_one() {
    let store = Store::open();
    store.refused(|z| {
        z.query(
            "MERGE (n:Person {name: 'Ada'}) ON CREATE SET n.created = 1",
            HashMap::new(),
        )
    });
    assert!(store.cypher("MATCH (n:Person {name: 'Ada'}) RETURN n").is_empty());
    store.cypher("MERGE (n:Person {name: 'Ada'}) ON CREATE SET n.created = 1");
    store.cypher("MERGE (n:Person {name: 'Ada'}) ON CREATE SET n.created = 1");
    assert_eq!(count(&store.zega, "MATCH (n:Person) RETURN count(n) AS c"), 1);
    let before = store.state().0;
    assert_eq!(store.reopened().graph_json().unwrap(), before);
}

#[test]
fn refused_foreach_applies_no_row() {
    let store = Store::open();
    store.cypher("CREATE (n:Batch {name: 'b'})");
    store.refused(|z| {
        z.query(
            "MATCH (b:Batch) FOREACH (x IN [1, 2, 3] | CREATE (:Row {v: x})) RETURN b",
            HashMap::new(),
        )
    });
    assert_eq!(count(&store.zega, "MATCH (n:Row) RETURN count(n) AS c"), 0);
    assert_eq!(store.reopened().query("MATCH (n:Row) RETURN n", HashMap::new()).unwrap().len(), 0);
}

#[test]
fn statement_that_fails_part_way_applies_nothing() {
    // No WAL fault: the statement itself fails after its first writes. The
    // first node is deleted before the second (which still has a
    // relationship) makes plain DELETE fail; the first must come back.
    let store = Store::open();
    store.cypher("CREATE (n:Temp {n: 1})");
    store.cypher("CREATE (a:Temp {n: 2})-[:HAS]->(b:Other)");
    let before = store.state();
    let error = store
        .zega
        .query("MATCH (n:Temp) DELETE n", HashMap::new())
        .unwrap_err();
    assert!(error.to_string().contains("DETACH DELETE"), "{error}");
    assert_eq!(store.state(), before);
    assert_eq!(store.reopened().graph_json().unwrap(), before.0);
}

// ---------------------------------------------------------------------------
// ZQL mutations, loads and the API calls (`run_lang`, `apply_zql`, ...)
// ---------------------------------------------------------------------------

const SCHEMA: &str = r#"
    schema {
      type Player { name: String salary: Int favorite -> Team }
      type Team { name: String }
    }
    unique { Player { name } Team { name } }
    index { range Player { salary } }
"#;

fn players(zega: &Zega, query: &str) -> Vec<String> {
    let rows = zega.run_lang(SCHEMA, query).unwrap();
    let mut names: Vec<String> = match rows {
        Json::Array(rows) => rows,
        Json::Null => Vec::new(),
        row => vec![row],
    }
    .iter()
    .map(|row| row["name"].as_str().unwrap().to_string())
    .collect();
    names.sort();
    names
}

#[test]
fn refused_zql_insert_frees_its_unique_value() {
    let store = Store::open();
    store.refused(|z| {
        z.run_lang(SCHEMA, r#"mutation { Player(name: "Ada" && salary: 10) { name } }"#)
    });
    assert!(players(&store.zega, "{ Player { name } }").is_empty());
    assert!(players(&store.zega, "{ Player(salary > 5) { name } }").is_empty());
    // The unique map never saw "Ada", so she can be inserted now.
    store
        .zega
        .run_lang(SCHEMA, r#"mutation { Player(name: "Ada" && salary: 10) { name } }"#)
        .unwrap();
    assert_eq!(players(&store.zega, "{ Player(salary > 5) { name } }"), ["Ada"]);
    let before = store.state().0;
    assert_eq!(store.reopened().graph_json().unwrap(), before);
}

#[test]
fn refused_zql_set_keeps_old_values_in_unique_and_range_indexes() {
    let store = Store::open();
    store
        .zega
        .run_lang(SCHEMA, r#"mutation { Player(name: "Ada" && salary: 10) { name } }"#)
        .unwrap();
    store.refused(|z| {
        z.run_lang(SCHEMA, r#"mutation { Player(name: "Ada") set name: "Bob", salary: 99 }"#)
    });
    assert_eq!(players(&store.zega, "{ Player(name: \"Ada\") { name } }"), ["Ada"]);
    assert!(players(&store.zega, "{ Player(salary > 50) { name } }").is_empty());
    assert_eq!(players(&store.zega, "{ Player(salary < 50) { name } }"), ["Ada"]);
    // "Bob" was never taken, "Ada" still is.
    store
        .zega
        .run_lang(SCHEMA, r#"mutation { Player(name: "Bob" && salary: 1) { name } }"#)
        .unwrap();
    let dup = store
        .zega
        .run_lang(SCHEMA, r#"mutation { Player(name: "Ada" && salary: 2) { name } }"#)
        .unwrap_err();
    assert!(dup.to_string().contains("unique Player { name }"), "{dup}");
    let before = store.state().0;
    assert_eq!(store.reopened().graph_json().unwrap(), before);
}

#[test]
fn refused_zql_link_leaves_a_single_valued_edge_free() {
    let store = Store::open();
    store
        .zega
        .run_lang(SCHEMA, r#"mutation { Player(name: "Ada" && salary: 1) { name } }"#)
        .unwrap();
    for team in ["Oilers", "Flames"] {
        store
            .zega
            .run_lang(SCHEMA, &format!(r#"mutation {{ Team(name: "{team}") {{ name }} }}"#))
            .unwrap();
    }
    let link = |team: &str| {
        format!(
            r#"mutation {{ Player(name: "Ada") {{ favorite -> link Team(name: "{team}") {{ name }} }} }}"#
        )
    };
    store.refused(|z| z.run_lang(SCHEMA, &link("Oilers")));
    // Had the refused edge stayed, this second target would be rejected.
    store.zega.run_lang(SCHEMA, &link("Flames")).unwrap();
    let before = store.state().0;
    assert_eq!(before["rels"].as_array().unwrap().len(), 1);
    assert_eq!(store.reopened().graph_json().unwrap(), before);
}

#[test]
fn refused_nested_zql_mutation_applies_no_part() {
    let store = Store::open();
    store.refused(|z| {
        z.run_lang(
            SCHEMA,
            r#"mutation {
                Player(name: "Ada" && salary: 1) { name favorite -> Team(name: "Oilers") { name } }
            }"#,
        )
    });
    let before = store.state().0;
    assert_eq!(before["nodes"], Json::Array(vec![]));
    assert_eq!(store.reopened().graph_json().unwrap(), before);
}

#[test]
fn refused_json_and_csv_loads_apply_no_row() {
    for (format, data) in [
        ("json", r#"[{"n":"Ada","s":1},{"n":"Bob","s":2},{"n":"Cy","s":3}]"#),
        ("csv", "n,s\nAda,1\nBob,2\nCy,3\n"),
    ] {
        let store = Store::open();
        let document = format!(
            "{SCHEMA}\nmutation {format} [\"rows.{format}\"] {{ Player(name: $n && salary: $s) {{ name }} }}"
        );
        let sources = HashMap::from([(format!("rows.{format}"), data.to_string())]);
        store.refused(|z| z.apply_zql_with_sources(&document, &sources));
        assert!(players(&store.zega, "{ Player { name } }").is_empty(), "{format}");
        // Every refused name is free again.
        store.zega.apply_zql_with_sources(&document, &sources).unwrap();
        assert_eq!(players(&store.zega, "{ Player { name } }"), ["Ada", "Bob", "Cy"]);
        let before = store.state().0;
        assert_eq!(store.reopened().graph_json().unwrap(), before, "{format}");
    }
}

#[test]
fn refused_api_connect_and_deletes_change_nothing() {
    let store = Store::open();
    store
        .zega
        .run_lang(
            SCHEMA,
            r#"mutation { Player(name: "Ada" && salary: 1) { name favorite -> Team(name: "Oilers") { name } } }"#,
        )
        .unwrap();
    store
        .zega
        .run_lang(SCHEMA, r#"mutation { Player(name: "Bob" && salary: 2) { name } }"#)
        .unwrap();
    let graph = store.state().0;
    let id = |key: &str, value: &str| {
        graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node[key] == value)
            .unwrap()["id"]
            .as_u64()
            .unwrap()
    };
    let (ada, bob, oilers) = (id("name", "Ada"), id("name", "Bob"), id("name", "Oilers"));
    let rel = graph["rels"][0]["id"].as_u64().unwrap();

    store.refused(|z| z.connect_schema(SCHEMA, bob, "favorite", oilers));
    store.refused(|z| z.delete_relationship(rel));
    store.refused(|z| z.delete_node(ada));
    // Ada's unique name is still taken, Bob's edge is still free.
    let dup = store
        .zega
        .run_lang(SCHEMA, r#"mutation { Player(name: "Ada" && salary: 3) { name } }"#)
        .unwrap_err();
    assert!(dup.to_string().contains("unique Player { name }"), "{dup}");
    store.zega.connect_schema(SCHEMA, bob, "favorite", oilers).unwrap();

    let before = store.state().0;
    assert_eq!(store.reopened().graph_json().unwrap(), before);
}
