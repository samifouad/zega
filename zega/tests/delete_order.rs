//! `order by` a field, ascending or descending, with more than one key; and
//! `delete Type(condition) { @detach @id field }` in a mutation (#73).

use serde_json::{json, Value as Json};
use zega::Zega;

const SCHEMA: &str = r#"
    type Team {
      name: String
      players -> Player[]
    }
    type Player {
      name: String
      team?: String
      salary?: Int
      rating?: Float
      active?: Bool
      players <- Team
    }
"#;

fn roster() -> Zega {
    let db = Zega::in_memory().build().unwrap();
    for create in [
        r#"Player(name: "Ada" && team: "A" && salary: 300 && rating: 2.5 && active: true)"#,
        r#"Player(name: "Ben" && team: "B" && salary: 100 && rating: 3 && active: false)"#,
        r#"Player(name: "Cy" && team: "A" && salary: 200 && rating: 1.5)"#,
        r#"Player(name: "Dee" && team: "B")"#,
        r#"Player(name: "Eve" && team: "A" && salary: 200 && active: true)"#,
    ] {
        db.run_lang(SCHEMA, &format!("mutation {{ {create} {{ name }} }}")).unwrap();
    }
    db
}

fn names(result: &Json) -> Vec<&str> {
    result.as_array().unwrap().iter().map(|row| row["name"].as_str().unwrap()).collect()
}

fn read(db: &Zega, query: &str) -> Json {
    db.run_lang(SCHEMA, query).unwrap_or_else(|error| panic!("{query}\n{error}"))
}

#[test]
fn order_by_a_field_ascending_and_descending_keeps_ties_and_puts_missing_last() {
    let db = roster();
    // Ties keep creation order (Cy before Eve); Dee has no salary and is last both ways.
    assert_eq!(names(&read(&db, "{ Player order by salary { name } }")), ["Ben", "Cy", "Eve", "Ada", "Dee"]);
    assert_eq!(names(&read(&db, "{ Player order by salary asc { name } }")), ["Ben", "Cy", "Eve", "Ada", "Dee"]);
    assert_eq!(names(&read(&db, "{ Player order by salary desc { name } }")), ["Ada", "Cy", "Eve", "Ben", "Dee"]);
    // Strings by code point, floats with ints, bools false first.
    assert_eq!(names(&read(&db, "{ Player order by name desc { name } }")), ["Eve", "Dee", "Cy", "Ben", "Ada"]);
    assert_eq!(names(&read(&db, "{ Player order by rating { name } }")), ["Cy", "Ada", "Ben", "Dee", "Eve"]);
    assert_eq!(names(&read(&db, "{ Player order by active desc { name } }")), ["Ada", "Eve", "Ben", "Cy", "Dee"]);
}

#[test]
fn order_by_several_keys_then_limit_after_the_filter() {
    let db = roster();
    assert_eq!(
        names(&read(&db, "{ Player order by team, salary desc { name } }")),
        ["Ada", "Cy", "Eve", "Ben", "Dee"]
    );
    assert_eq!(names(&read(&db, "{ Player order by salary desc limit 2 { name } }")), ["Ada", "Cy"]);
    assert_eq!(names(&read(&db, r#"{ Player(team = "A") order by salary, name desc { name } }"#)), ["Eve", "Cy", "Ada"]);
    assert_eq!(names(&read(&db, r#"{ Player(team = "B") order by salary desc limit 1 { name } }"#)), ["Ben"]);
}

#[test]
fn order_by_works_on_a_walk_target() {
    let db = roster();
    db.run_lang(SCHEMA, r#"mutation { Team(name: "Oilers") { name } }"#).unwrap();
    for player in ["Ada", "Cy", "Eve"] {
        db.run_lang(SCHEMA, &format!(r#"mutation {{ Team(name: "Oilers") {{ players -> link Player(name: "{player}") {{ name }} }} }}"#))
            .unwrap();
    }
    let team = read(&db, r#"{ Team(name = "Oilers") { players -> Player order by salary desc, name limit 2 { name salary } } }"#);
    assert_eq!(team["players"], json!([{ "name": "Ada", "salary": 300 }, { "name": "Cy", "salary": 200 }]));
}

#[test]
fn a_field_named_order_or_desc_is_still_a_field() {
    // APS 6: no reserved words. `order`, `by`, `desc` stay free for users.
    let schema = "type Invoice { order: Int desc: String by?: String }";
    let db = Zega::in_memory().build().unwrap();
    for (order, desc) in [(3, "c"), (1, "a"), (2, "b")] {
        db.run_lang(schema, &format!(r#"mutation {{ Invoice(order: {order} && desc: "{desc}") {{ order }} }}"#)).unwrap();
    }
    let rows = db.run_lang(schema, "{ Invoice order by order desc { order desc } }").unwrap();
    assert_eq!(rows, json!([{ "order": 3, "desc": "c" }, { "order": 2, "desc": "b" }, { "order": 1, "desc": "a" }]));
    let rows = db.run_lang(schema, "{ Invoice order by desc { desc } }").unwrap();
    assert_eq!(rows, json!([{ "desc": "a" }, { "desc": "b" }, { "desc": "c" }]));
}

#[test]
fn order_by_distance_goes_either_way_and_takes_a_second_key() {
    let schema = "type City { name: String at: Point size?: Int }";
    let db = Zega::in_memory().build().unwrap();
    for (name, lat, size) in [("Near", 0.1, 1), ("Far", 3.0, 2), ("Mid", 1.0, 3), ("Mid2", 1.0, 1)] {
        db.run_lang(schema, &format!(r#"mutation {{ City(name: "{name}" && at: @point({lat}, 0) && size: {size}) {{ name }} }}"#)).unwrap();
    }
    let near = "@distance(at, @point(0, 0))";
    assert_eq!(names(&db.run_lang(schema, &format!("{{ City order by {near} {{ name }} }}")).unwrap()), ["Near", "Mid", "Mid2", "Far"]);
    assert_eq!(names(&db.run_lang(schema, &format!("{{ City order by {near} limit 2 {{ name }} }}")).unwrap()), ["Near", "Mid"]);
    assert_eq!(names(&db.run_lang(schema, &format!("{{ City order by {near} desc {{ name }} }}")).unwrap()), ["Far", "Mid", "Mid2", "Near"]);
    // Farthest first with a limit: the nearest-first search must not stop early.
    assert_eq!(names(&db.run_lang(schema, &format!("{{ City order by {near} desc limit 2 {{ name }} }}")).unwrap()), ["Far", "Mid"]);
    assert_eq!(names(&db.run_lang(schema, &format!("{{ City order by {near}, size {{ name }} }}")).unwrap()), ["Near", "Mid2", "Mid", "Far"]);
    assert_eq!(names(&db.run_lang(schema, &format!("{{ City order by size desc, {near} {{ name }} }}")).unwrap()), ["Mid", "Far", "Near", "Mid2"]);
}

fn diagnosis(schema: &str, query: &str) -> Vec<String> {
    zega::diagnose(schema, query).diagnostics.into_iter().map(|d| d.message).collect()
}

#[test]
fn the_checker_names_what_cannot_be_ordered() {
    let schema = "type City { name: String at: Point embedding?: Vector<2> near -> City[] }";
    assert_eq!(diagnosis(schema, "{ City order by at { name } }"), ["City.at is a Point; order it by distance"]);
    assert_eq!(diagnosis(schema, "{ City order by embedding { name } }"), ["City.embedding is a Vector<2,cosine> and cannot be ordered"]);
    assert_eq!(diagnosis(schema, "{ City order by nme { name } }"), ["City has no field nme"]);
    assert_eq!(diagnosis(schema, "{ City order by near { name } }"), ["City has no field near"]);
    assert_eq!(diagnosis(schema, r#"mutation { City(name: "x" && at: @point(0, 0)) order by name { name } }"#), ["order and limit are only valid in queries"]);
    // Execution stops at the same check.
    let error = Zega::in_memory().build().unwrap().run_lang(schema, "{ City order by at { name } }").unwrap_err();
    assert!(error.to_string().contains("City.at is a Point; order it by distance"), "{error}");
}

#[test]
fn delete_removes_every_match_and_returns_the_count_and_what_was_projected() {
    let db = roster();
    let deleted = db.run_lang(SCHEMA, r#"mutation { delete Player(team = "A" && salary < 250) }"#).unwrap();
    assert_eq!(deleted, json!({ "deleted": 2 }));
    assert_eq!(names(&read(&db, "{ Player { name } }")), ["Ada", "Ben", "Dee"]);
    // Projected values are read before the rows go.
    let ids: Vec<u64> = read(&db, r#"{ Player(team = "B") order by name { @id } }"#).as_array().unwrap().iter().map(|r| r["id"].as_u64().unwrap()).collect();
    let deleted = db.run_lang(SCHEMA, r#"mutation { delete Player(team = "B") { @id name salary } }"#).unwrap();
    assert_eq!(deleted, json!({ "deleted": 2, "rows": [
        { "id": ids[0], "name": "Ben", "salary": 100 },
        { "id": ids[1], "name": "Dee", "salary": null },
    ] }));
    // No match deletes nothing, and says so.
    assert_eq!(db.run_lang(SCHEMA, r#"mutation { delete Player(name = "Nobody") { @id } }"#).unwrap(), json!({ "deleted": 0, "rows": [] }));
    assert_eq!(names(&read(&db, "{ Player { name } }")), ["Ada"]);
}

#[test]
fn delete_refuses_a_node_with_relationships_unless_detach_and_then_removes_them() {
    let db = roster();
    db.run_lang(SCHEMA, r#"mutation { Team(name: "Oilers") { name } }"#).unwrap();
    db.run_lang(SCHEMA, r#"mutation { Team(name: "Oilers") { players -> link Player(name: "Ada") { name } } }"#).unwrap();
    let before = db.graph_json().unwrap();
    let refused = db.run_lang(SCHEMA, r#"mutation { delete Player(team = "A") }"#).unwrap_err().to_string();
    assert!(refused.contains("Player "), "{refused}");
    assert!(refused.contains("has 1 relationship"), "{refused}");
    assert!(refused.contains("add `@detach` to remove them with it; nothing was deleted"), "{refused}");
    // One transaction: Cy and Eve matched too, and are still there.
    assert_eq!(db.graph_json().unwrap(), before);

    let deleted = db.run_lang(SCHEMA, r#"mutation { delete Player(team = "A") { @detach name } }"#).unwrap();
    assert_eq!(deleted, json!({ "deleted": 3, "rows": [{ "name": "Ada" }, { "name": "Cy" }, { "name": "Eve" }] }));
    let graph = db.graph_json().unwrap();
    assert!(graph["rels"].as_array().unwrap().is_empty(), "{graph}");
    assert_eq!(read(&db, r#"{ Team(name = "Oilers") { name players -> Player { name } } }"#)["players"], json!([]));
}

#[test]
fn a_delete_is_written_ahead_and_survives_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_str().unwrap();
    {
        let db = Zega::open(path).wal_flush_every_write().build().unwrap();
        db.run_lang(SCHEMA, r#"mutation { Team(name: "Oilers") { players -> Player(name: "Ada") { name } } }"#).unwrap();
        db.run_lang(SCHEMA, r#"mutation { Player(name: "Ben") { name } }"#).unwrap();
        db.run_lang(SCHEMA, r#"mutation { delete Player(name = "Ada") { @detach } }"#).unwrap();
    }
    let db = Zega::open(path).wal_flush_every_write().build().unwrap();
    assert_eq!(names(&read(&db, "{ Player { name } }")), ["Ben"]);
    assert!(db.graph_json().unwrap()["rels"].as_array().unwrap().is_empty());
}

#[test]
fn the_checker_refuses_a_delete_it_cannot_run() {
    let cases = [
        ("mutation { delete Player() }", "delete needs a condition: which Player rows?"),
        ("mutation { delete Player }", "delete needs a condition: which Player rows?"),
        (r#"{ delete Player(name = "Ada") }"#, "`delete` removes rows"),
        (r#"mutation { delete Player(name = "Ada") set name: "B" }"#, "a delete cannot set fields"),
        (r#"mutation { delete Player(name = "Ada") { players <- Team { name } } }"#, "a delete removes nodes, not relationships"),
        (r#"mutation { delete Player(name = "Ada") order by name }"#, "order and limit are only valid in queries"),
        (r#"mutation { delete Player(nme = "Ada") }"#, "Player has no field nme"),
        (r#"{ Player { name @detach } }"#, "@detach belongs in a delete"),
        (r#"mutation { Team(name: "Oilers") { players -> delete Player(name = "Ada") } }"#, "a delete starts a mutation; it cannot follow an arrow"),
    ];
    for (query, message) in cases {
        let found = diagnosis(SCHEMA, query);
        assert!(found.iter().any(|m| m == message), "{query}: {found:?}");
        // Nothing runs: execution stops at a check, and no row is touched.
        let db = roster();
        let before = db.graph_json().unwrap();
        assert!(db.run_lang(SCHEMA, query).is_err(), "{query} ran");
        assert_eq!(db.graph_json().unwrap(), before, "{query} changed the graph");
    }
    assert_eq!(
        diagnosis(SCHEMA, r#"mutation { delete Player(name = "Ada") { gone: @detach } }"#),
        ["@detach removes relationships; it has no value to name"]
    );
    let error = Zega::in_memory().build().unwrap().run_lang(SCHEMA, r#"mutation { delete Player(name = "Ada") { gone: @detach } }"#).unwrap_err();
    assert!(error.to_string().contains("@detach removes relationships; it has no value to name"), "{error}");
}

#[test]
fn a_load_cannot_delete() {
    let source = r#"
        schema { type Player { name: String } }
        mutation csv "players.csv" { delete Player(name = $name) }
    "#;
    let sources = std::collections::HashMap::from([("players.csv".to_string(), "name\nAda\n".to_string())]);
    let error = Zega::in_memory().build().unwrap().apply_zql_with_sources(source, &sources).unwrap_err();
    assert!(error.to_string().contains("a load inserts rows; it cannot delete them"), "{error}");
}

#[test]
fn a_type_named_delete_is_still_a_type() {
    // `delete` starts a delete only when a type name follows it.
    let schema = "type delete { name: String }";
    let db = Zega::in_memory().build().unwrap();
    db.run_lang(schema, r#"mutation { delete(name: "x") { name } }"#).unwrap();
    assert_eq!(db.run_lang(schema, "{ delete { name } }").unwrap(), json!([{ "name": "x" }]));
    assert_eq!(db.run_lang(schema, r#"mutation { delete delete(name = "x") }"#).unwrap(), json!({ "deleted": 1 }));
    assert_eq!(db.run_lang(schema, "{ delete { name } }").unwrap(), json!([]));
}
