//! zega#32: a write the WAL refused must not be visible, in this session or
//! after a reopen. Every test drives a real store, refuses the append through
//! the WAL's fault seam, then checks the graph, its indexes (label, property,
//! unique, declared range) and the id counters are what they were before.

use std::collections::HashMap;

use serde_json::Value as Json;
use tempfile::TempDir;

use crate::wal::tests::{switchable_wal, AppendSwitch};
use crate::Zega;

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

// ---------------------------------------------------------------------------
// Creates, sets and relationship deletes (ported from the Cypher-style tests
// removed with the legacy path in zega#55; MERGE, FOREACH and DETACH DELETE
// have no ZQL form and went with it)
// ---------------------------------------------------------------------------

const PEOPLE: &str = r#"
    schema { type Person { name: String age?: Int knows -> Person[] } }
    index { range Person { age } }
"#;

fn names(zega: &Zega, query: &str) -> Vec<String> {
    let rows = zega.run_lang(PEOPLE, query).unwrap();
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

/// Replaces `refused_create_is_invisible_and_its_id_is_reused`.
#[test]
fn refused_zql_create_is_invisible_and_its_id_is_reused() {
    let store = Store::open();
    store.zega.run_lang(PEOPLE, r#"mutation { Person(name: "Ada") }"#).unwrap();
    let (_, ids_before) = store.state();

    store.refused(|z| z.run_lang(PEOPLE, r#"mutation { Person(name: "Bob") }"#));
    // The label scan and the name filter both forget Bob.
    assert_eq!(names(&store.zega, "query { Person { name } }"), ["Ada"]);
    assert!(names(&store.zega, r#"query { Person(name: "Bob") { name } }"#).is_empty());

    // The same write goes through once the disk recovers, with Bob's old id.
    store.zega.run_lang(PEOPLE, r#"mutation { Person(name: "Bob") }"#).unwrap();
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

/// Replaces `refused_multi_node_create_leaves_no_node_and_no_relationship`.
#[test]
fn refused_zql_multi_node_create_leaves_no_node_and_no_relationship() {
    let store = Store::open();
    store.refused(|z| {
        z.run_lang(PEOPLE, r#"mutation { Person(name: "Ada") { knows -> Person(name: "Bob") } }"#)
    });
    assert!(names(&store.zega, "query { Person { name } }").is_empty());
    let reopened = store.reopened().graph_json().unwrap();
    assert_eq!(reopened["nodes"], Json::Array(vec![]));
    assert_eq!(reopened["rels"], Json::Array(vec![]));
}

/// Replaces `refused_set_keeps_the_old_value_in_the_node_and_its_index`.
#[test]
fn refused_zql_set_keeps_the_old_value_in_the_node_and_its_index() {
    let store = Store::open();
    store.zega.run_lang(PEOPLE, r#"mutation { Person(name: "Ada" && age: 36) }"#).unwrap();
    store.refused(|z| z.run_lang(PEOPLE, r#"mutation { Person(name: "Ada") set age: 37 }"#));
    let ada = store.zega.run_lang(PEOPLE, r#"query { Person(name: "Ada") { age } }"#).unwrap();
    assert_eq!(ada["age"], 36);
    // The range index on age still files Ada under 36, not 37.
    assert!(names(&store.zega, "query { Person(age >= 37) { name } }").is_empty());
    assert_eq!(names(&store.zega, "query { Person(age: 36) { name } }"), ["Ada"]);
    assert_eq!(names(&store.zega, "query { Person(age < 37) { name } }"), ["Ada"]);

    let before = store.state().0;
    assert_eq!(store.reopened().graph_json().unwrap(), before);
}

/// Replaces `refused_relationship_delete_keeps_it_traversable`. ZQL has no
/// delete statement; `delete_relationship` is the API every host uses.
#[test]
fn refused_relationship_delete_keeps_it_traversable_in_zql() {
    let store = Store::open();
    store
        .zega
        .run_lang(PEOPLE, r#"mutation { Person(name: "Ada") { knows -> Person(name: "Bob") } }"#)
        .unwrap();
    let rel = store.state().0["rels"][0]["id"].as_u64().unwrap();
    store.refused(|z| z.delete_relationship(rel));
    let known = store
        .zega
        .run_lang(PEOPLE, r#"query { Person(name: "Ada") { knows -> Person { name } } }"#)
        .unwrap();
    assert_eq!(known["knows"], serde_json::json!([{ "name": "Bob" }]));
    let before = store.state().0;
    assert_eq!(store.reopened().graph_json().unwrap(), before);
}

/// Replaces `statement_that_fails_part_way_applies_nothing`. No WAL fault: the
/// load inserts Ada and Bob, then its third row repeats Ada's unique name;
/// the first two rows must not stay.
#[test]
fn zql_statement_that_fails_part_way_applies_nothing() {
    let store = Store::open();
    store.zega.run_lang(SCHEMA, r#"mutation { Team(name: "Oilers") { name } }"#).unwrap();
    let before = store.state();
    let document = format!(
        "{SCHEMA}\nmutation json [\"rows.json\"] {{ Player(name: $n && salary: $s) {{ name }} }}"
    );
    let rows = r#"[{"n":"Ada","s":1},{"n":"Bob","s":2},{"n":"Ada","s":3}]"#;
    let sources = HashMap::from([("rows.json".to_string(), rows.to_string())]);
    let error = store.zega.apply_zql_with_sources(&document, &sources).unwrap_err();
    assert!(error.to_string().contains("unique Player { name }"), "{error}");
    assert_eq!(store.state(), before);
    assert_eq!(store.reopened().graph_json().unwrap(), before.0);
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
