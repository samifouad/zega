//! Filters on a related node's fields, `rel -> Type(condition)` inside a
//! condition, and relationship counts, `@count(rel)` (#86).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value as Json};
use zega::{Zega, ZegaError};

// ---------------------------------------------------------------------------
// The Flights sample: the two example queries from the issue.

fn flights() -> Zega {
    let db = Zega::in_memory().build().unwrap();
    let source = include_str!("../../browser/samples/flights.zql");
    let sources = HashMap::from([
        ("./samples/flights-countries.csv".to_string(), include_str!("../../browser/samples/flights-countries.csv").to_string()),
        ("./samples/flights-airports.csv".to_string(), include_str!("../../browser/samples/flights-airports.csv").to_string()),
        ("./samples/flights-routes.csv".to_string(), include_str!("../../browser/samples/flights-routes.csv").to_string()),
    ]);
    db.apply_zql_with_sources(source, &sources).unwrap();
    db
}

/// The sample's schema and its `unique` block, which makes `iso` and `code`
/// indexed.
fn flights_schema() -> &'static str {
    let source = include_str!("../../browser/samples/flights.zql");
    &source[..source.find("mutation csv").unwrap()]
}

fn ask(db: &Zega, schema: &str, query: &str) -> Json {
    db.run_lang(schema, query).unwrap_or_else(|error| panic!("{query}\n{error}"))
}

fn codes(result: &Json) -> Vec<&str> {
    result.as_array().unwrap().iter().map(|row| row["code"].as_str().unwrap()).collect()
}

#[test]
fn flights_routes_from_canada_to_japan() {
    let db = flights();
    let schema = flights_schema();
    // The issue's query, as written: every Canadian airport, with its routes
    // to Japan. YYZ has none, so its list is empty.
    let result = ask(
        &db,
        schema,
        r#"{ Airport(country -> Country(iso = "CA")) { code route -> Airport(country -> Country(iso = "JP")) { code } } }"#,
    );
    assert_eq!(
        result,
        json!([
            { "code": "YYC", "route": [{ "code": "NRT" }] },
            { "code": "YYZ", "route": [] },
        ])
    );
    // With the route test in the parentheses too, only airports that fly
    // there are left.
    let result = ask(
        &db,
        schema,
        r#"{ Airport(country -> Country(iso = "CA") && route -> Airport(country -> Country(iso = "JP"))) { code } }"#,
    );
    assert_eq!(codes(&result), ["YYC"]);
}

#[test]
fn flights_hubs_with_more_than_20_routes() {
    let db = flights();
    let schema = flights_schema();
    let result = ask(
        &db,
        schema,
        "{ Airport(@count(route) > 20) order by @count(route) desc { code routes: @count(route) } }",
    );
    // Counted from flights-routes.csv by origin; ties keep creation order.
    assert_eq!(
        result,
        json!([
            { "code": "HEL", "routes": 27 },
            { "code": "BUD", "routes": 25 },
            { "code": "TLV", "routes": 25 },
            { "code": "WAW", "routes": 25 },
            { "code": "NRT", "routes": 24 },
            { "code": "CAI", "routes": 22 },
            { "code": "DEL", "routes": 22 },
            { "code": "DOH", "routes": 22 },
        ])
    );
    // The same from the file, independently of the engine.
    let routes = include_str!("../../browser/samples/flights-routes.csv");
    let mut by_origin: HashMap<&str, u64> = HashMap::new();
    for line in routes.lines().skip(1) {
        *by_origin.entry(line.split(',').next().unwrap().trim_matches('"')).or_default() += 1;
    }
    for row in result.as_array().unwrap() {
        assert_eq!(by_origin[row["code"].as_str().unwrap()], row["routes"].as_u64().unwrap());
    }
    assert_eq!(by_origin.values().filter(|n| **n > 20).count(), 8);
}

#[test]
fn an_indexed_target_is_found_first_and_walked_backwards() {
    let db = flights();
    let schema = flights_schema();
    let before = db.rows_examined().unwrap();
    let result = ask(&db, schema, r#"{ Airport(country -> Country(iso = "CA")) { code } }"#);
    assert_eq!(codes(&result), ["YYC", "YYZ"]);
    // `iso` is unique, so indexed: one Country is tested, then only the two
    // airports that point at it, not all 52.
    assert_eq!(db.rows_examined().unwrap() - before, 3);

    // Two hops: Japan's one airport, then the airports with a route to it.
    let before = db.rows_examined().unwrap();
    let result = ask(&db, schema, r#"{ Airport(route -> Airport(country -> Country(iso = "JP"))) { code } }"#);
    assert_eq!(codes(&result), ["DEL", "HEL", "MEX", "MNL", "SYD", "TPE", "YYC"]);
    assert_eq!(db.rows_examined().unwrap() - before, 1 + 1 + 7);

    // The row's own index wins: one airport by its code, then one walk.
    let before = db.rows_examined().unwrap();
    let result = ask(&db, schema, r#"{ Airport(code = "YYC" && country -> Country(iso = "CA")) { code } }"#);
    assert_eq!(codes(&result), ["YYC"]);
    assert_eq!(db.rows_examined().unwrap() - before, 1);

    // No index on the target: every airport is tested by walking forward.
    let before = db.rows_examined().unwrap();
    let result = ask(&db, schema, r#"{ Airport(country -> Country(name = "Canada")) { code } }"#);
    assert_eq!(codes(&result), ["YYC", "YYZ"]);
    assert_eq!(db.rows_examined().unwrap() - before, 52);
}

// ---------------------------------------------------------------------------
// Semantics on a small graph.

const SCHEMA: &str = r#"
    type Person {
      name: String
      city?: String
      age?: Int
      knows: KNOWS -> Person[]
      knownBy: KNOWS <- Person[]
      lives: LIVES -> Town
      pets -> (Dog | Cat)[]
    }
    type Town {
      name: String
      people: LIVES <- Person[]
    }
    type Dog { name: String }
    type Cat { name: String }
"#;

/// Ann knows Bob and Cy; Bob knows Ann (a cycle); Cy knows nobody; Dee knows
/// herself. Ann and Bob live in Oslo; Dee in Rome; Cy and Eve nowhere.
fn people() -> Zega {
    let db = Zega::in_memory().build().unwrap();
    let run = |q: &str| {
        db.run_lang(SCHEMA, q).unwrap_or_else(|e| panic!("{q}\n{e}"));
    };
    for create in [
        r#"Town(name: "Oslo")"#,
        r#"Town(name: "Rome")"#,
        r#"Person(name: "Ann" && age: 30)"#,
        r#"Person(name: "Bob" && age: 40 && city: "Oslo")"#,
        r#"Person(name: "Cy" && age: 50)"#,
        r#"Person(name: "Dee")"#,
        r#"Person(name: "Eve" && age: 20)"#,
        r#"Dog(name: "Rex")"#,
        r#"Cat(name: "Tom")"#,
        r#"Cat(name: "Kit")"#,
    ] {
        run(&format!("mutation {{ {create} }}"));
    }
    for (from, link) in [
        ("Ann", r#"lives -> link Town(name: "Oslo")"#),
        ("Bob", r#"lives -> link Town(name: "Oslo")"#),
        ("Dee", r#"lives -> link Town(name: "Rome")"#),
        ("Ann", r#"knows -> link Person(name: "Bob")"#),
        ("Ann", r#"knows -> link Person(name: "Cy")"#),
        ("Bob", r#"knows -> link Person(name: "Ann")"#),
        ("Dee", r#"knows -> link Person(name: "Dee")"#),
        ("Ann", r#"pets -> link Dog(name: "Rex")"#),
        ("Ann", r#"pets -> link Cat(name: "Tom")"#),
        ("Cy", r#"pets -> link Cat(name: "Kit")"#),
    ] {
        run(&format!(r#"mutation {{ Person(name: "{from}") {{ {link} }} }}"#));
    }
    db
}

fn names(result: &Json) -> Vec<&str> {
    result.as_array().unwrap().iter().map(|row| row["name"].as_str().unwrap()).collect()
}

fn read(db: &Zega, query: &str) -> Vec<String> {
    names(&ask(db, SCHEMA, query)).into_iter().map(String::from).collect()
}

#[test]
fn a_walk_in_a_condition_holds_when_any_related_node_matches() {
    let db = people();
    // To many: one matching friend is enough (Ann knows Bob, 40, and Cy, 50).
    assert_eq!(read(&db, "{ Person(knows -> Person(age > 45)) { name } }"), ["Ann"]);
    assert_eq!(read(&db, "{ Person(knows -> Person(age < 45)) { name } }"), ["Ann", "Bob"]);
    // To one.
    assert_eq!(read(&db, r#"{ Person(lives -> Town(name = "Oslo")) { name } }"#), ["Ann", "Bob"]);
    // Backwards, against the schema's `<-` field.
    assert_eq!(read(&db, r#"{ Town(people <- Person(name = "Dee")) { name } }"#), ["Rome"]);
    assert_eq!(read(&db, r#"{ Person(knownBy <- Person(name = "Ann")) { name } }"#), ["Bob", "Cy"]);
    // No parentheses: the relationship exists.
    assert_eq!(read(&db, "{ Person(lives -> Town) { name } }"), ["Ann", "Bob", "Dee"]);
    // A union target, and one member of it.
    assert_eq!(read(&db, "{ Person(pets -> (Dog | Cat)) { name } }"), ["Ann", "Cy"]);
    assert_eq!(read(&db, "{ Person(pets -> Dog) { name } }"), ["Ann"]);
    // Joined with && and ||, and grouped.
    assert_eq!(read(&db, r#"{ Person(lives -> Town(name = "Oslo") && age > 35) { name } }"#), ["Bob"]);
    assert_eq!(read(&db, r#"{ Person(lives -> Town(name = "Rome") || age < 25) { name } }"#), ["Dee", "Eve"]);
}

#[test]
fn a_node_without_the_relationship_fails_the_test_and_counts_zero() {
    let db = people();
    // Cy and Eve know nobody, or live nowhere: never in a walk's result.
    assert_eq!(read(&db, "{ Person(knows -> Person(age > 0)) { name } }"), ["Ann", "Bob"]);
    assert_eq!(read(&db, "{ Person(@count(knows) = 0) { name } }"), ["Cy", "Eve"]);
    assert_eq!(read(&db, "{ Person(@count(lives) = 0) { name } }"), ["Cy", "Eve"]);
    // A missing field inside the target: a comparison does not match it, and
    // `!=` does, as in any filter. Dee has no age.
    assert_eq!(read(&db, "{ Person(knows -> Person(age > 0)) { name } }"), ["Ann", "Bob"]);
    assert_eq!(read(&db, r#"{ Person(knows -> Person(city != "Oslo")) { name } }"#), ["Ann", "Bob", "Dee"]);
    // A count in a selection and an ordering.
    let result = ask(&db, SCHEMA, "{ Person order by @count(knows) desc, name { name friends: @count(knows) } }");
    assert_eq!(
        result,
        json!([
            { "name": "Ann", "friends": 2 },
            { "name": "Bob", "friends": 1 },
            { "name": "Dee", "friends": 1 },
            { "name": "Cy", "friends": 0 },
            { "name": "Eve", "friends": 0 },
        ])
    );
    // The key is `count` when unnamed.
    assert_eq!(ask(&db, SCHEMA, r#"{ Person(name = "Ann") { @count(pets) } }"#), json!({ "count": 2 }));
}

#[test]
fn a_filtered_count_expresses_none_all_and_at_least() {
    let db = people();
    // At least two friends over 35: none. At least one: Ann.
    assert_eq!(read(&db, "{ Person(@count(knows -> Person(age > 35)) >= 2) { name } }"), ["Ann"]);
    assert_eq!(read(&db, "{ Person(@count(knows -> Person(age > 45)) >= 2) { name } }"), Vec::<String>::new());
    assert_eq!(read(&db, "{ Person(@count(knows -> Person(age > 45)) >= 1) { name } }"), ["Ann"]);
    // None: nobody they know lives in Oslo (Cy, Eve know nobody; Dee knows herself, in Rome).
    assert_eq!(
        read(&db, r#"{ Person(@count(knows -> Person(lives -> Town(name = "Oslo"))) = 0) { name } }"#),
        ["Cy", "Dee", "Eve"]
    );
    // All: with at least one friend, and no friend under 35.
    assert_eq!(
        read(&db, "{ Person(knows -> Person && @count(knows -> Person(age < 35)) = 0) { name } }"),
        ["Ann", "Dee"]
    );
    // Every comparison, with the count stopping early.
    assert_eq!(read(&db, "{ Person(@count(knows) != 1) { name } }"), ["Ann", "Cy", "Eve"]);
    assert_eq!(read(&db, "{ Person(@count(knows) < 1) { name } }"), ["Cy", "Eve"]);
    assert_eq!(read(&db, "{ Person(@count(knows) <= 1) { name } }"), ["Bob", "Cy", "Dee", "Eve"]);
    assert_eq!(read(&db, "{ Person(@count(knows) > 1) { name } }"), ["Ann"]);
    assert_eq!(read(&db, "{ Person(@count(knows): 2) { name } }"), ["Ann"]);
    assert_eq!(read(&db, "{ Person(@count(pets) = 2) { name } }"), ["Ann"]);
}

#[test]
fn cycles_in_the_data_follow_the_depth_of_the_text() {
    let db = people();
    // Ann -> Bob -> Ann: two hops back to the start. Bob's friend Ann does
    // not know Bob's way back to Ann, so Bob is not in it.
    assert_eq!(read(&db, r#"{ Person(knows -> Person(knows -> Person(name = "Ann"))) { name } }"#), ["Ann"]);
    assert_eq!(read(&db, r#"{ Person(knows -> Person(knows -> Person(name = "Bob"))) { name } }"#), ["Bob"]);
    // Dee knows herself, at any depth.
    assert_eq!(
        read(&db, r#"{ Person(knows -> Person(knows -> Person(knows -> Person(name = "Dee")))) { name } }"#),
        ["Dee"]
    );
    // A walk target in the selection filters with the same test.
    let result = ask(&db, SCHEMA, r#"{ Person(name = "Ann") { knows -> Person(knows -> Person) { name } } }"#);
    assert_eq!(result, json!({ "knows": [{ "name": "Bob" }] }));
}

#[test]
fn set_and_delete_find_rows_by_a_related_node() {
    let db = people();
    ask(&db, SCHEMA, r#"mutation { Person(name = "Bob" && lives -> Town(name = "Oslo")) set city: "Bergen" }"#);
    assert_eq!(ask(&db, SCHEMA, r#"{ Person(city = "Bergen") { name } }"#), json!({ "name": "Bob" }));
    let deleted = ask(&db, SCHEMA, r#"mutation { delete Person(lives -> Town(name = "Rome")) { @detach name } }"#);
    assert_eq!(deleted, json!({ "deleted": 1, "rows": [{ "name": "Dee" }] }));
}

#[test]
fn a_load_binds_columns_inside_a_walk() {
    let db = people();
    let csv = "name,town,city\nAnn,Oslo,Tromso\nCy,,Paris\n";
    let sources = HashMap::from([("rows.csv".to_string(), csv.to_string())]);
    db.run_lang_with_sources(
        SCHEMA,
        r#"mutation csv ["rows.csv"] { Person(name = $name && lives -> Town(name = $town)) set city: $city }"#,
        &sources,
    )
    .unwrap();
    // Ann lives in Oslo, and matched. Cy's empty town drops the walk, the way
    // an empty cell drops its own term, so Cy is found by name alone.
    assert_eq!(ask(&db, SCHEMA, r#"{ Person(city = "Tromso") { name } }"#), json!({ "name": "Ann" }));
    assert_eq!(ask(&db, SCHEMA, r#"{ Person(city = "Paris") { name } }"#), json!({ "name": "Cy" }));
}

// ---------------------------------------------------------------------------
// Parsing and errors that teach the right form.

fn error(schema: &str, query: &str) -> String {
    let db = Zega::in_memory().build().unwrap();
    db.run_lang(schema, query).unwrap_err().to_string()
}

#[test]
fn a_relationship_tested_as_a_field_teaches_the_walk_or_the_count() {
    let schema = flights_schema();
    let message = error(schema, r#"{ Airport(country = "CA") { code } }"#);
    assert!(message.contains("country is a relationship"), "{message}");
    assert!(
        message.contains(r#"help: filter by a field of the related node: `country -> Country(name = "CA")`; Country has name, iso"#),
        "{message}"
    );
    let message = error(schema, "{ Airport(route > 20) { code } }");
    assert!(message.contains("route is a relationship"), "{message}");
    assert!(message.contains("help: count it: `@count(route) > 20`"), "{message}");
    let message = error(schema, r#"{ Airport(country findLike "can") { code } }"#);
    assert!(message.contains("help: test a field of the related node: `country -> Country(field …)`"), "{message}");
}

#[test]
fn the_ways_models_misspell_it_are_parse_errors_that_name_the_form() {
    let schema = flights_schema();
    let message = error(schema, r#"{ Airport(country.iso = "CA") { code } }"#);
    assert!(message.contains("a related node's field is reached through its relationship"), "{message}");
    assert!(message.contains("help: write `country -> Type(iso = …)`, with the type `country` reaches"), "{message}");
    let message = error(schema, r#"{ Airport(country: Country(iso: "CA")) { code } }"#);
    assert!(message.contains("a related node is tested with an arrow"), "{message}");
    assert!(message.contains("help: write `country -> Country(…)`"), "{message}");
    let message = error(schema, "{ Airport(count(route) > 20) { code } }");
    assert!(message.contains("`count` is built in: write `@count`"), "{message}");
    let message = error(schema, "{ Airport(@count(route > 20)) { code } }");
    assert!(message.contains("help: `@count(route)` is the number; compare after it: `@count(route) > 20`"), "{message}");
    let message = error(schema, "{ Airport(@count(route) > -1) { code } }");
    assert!(message.contains("a count is compared with a whole number, 0 or more"), "{message}");
    let message = error(schema, "{ Airport(@count(route)) { code } }");
    assert!(message.contains("@count needs a comparison"), "{message}");
    let message = error(schema, "{ Airport(route -> Airport { code }) { code } }");
    assert!(message.contains("a walk in a condition only tests the related node"), "{message}");
}

#[test]
fn the_checker_checks_a_walk_in_a_condition_like_a_walk_in_a_selection() {
    let schema = flights_schema();
    let message = error(schema, r#"{ Airport(country <- Country(iso = "CA")) { code } }"#);
    assert!(message.contains("Airport.country does not point <-"), "{message}");
    let message = error(schema, r#"{ Airport(country -> Airport(code = "CA")) { code } }"#);
    assert!(message.contains("Airport.country does not reach Airport"), "{message}");
    // The target's condition is checked against the target's type.
    let message = error(schema, r#"{ Airport(country -> Country(code = "CA")) { code } }"#);
    assert!(message.contains("Country has no field code"), "{message}");
    let message = error(schema, "{ Airport(@count(code) > 1) { code } }");
    assert!(message.contains("@count counts a relationship; Airport.code is a field"), "{message}");
    let message = error(schema, "{ Airport(@count(routes) > 1) { code } }");
    assert!(message.contains("Airport has no relationship routes"), "{message}");
    let message = error(schema, r#"mutation { Airport(code: "X" && name: "X" && city: "X" && at: @point(0, 0)) { @count(route) } }"#);
    assert!(message.contains("@count is read by a query, not a mutation"), "{message}");
}

#[test]
fn a_walk_backwards_and_less_than_a_negative_number_are_told_apart() {
    let db = people();
    // `<-` before a name is an arrow; before a number, `<` and a negative.
    assert_eq!(read(&db, r#"{ Person(knownBy <-Person(name = "Bob")) { name } }"#), ["Ann"]);
    assert_eq!(read(&db, "{ Person(age <-5 || age >= 50) { name } }"), ["Cy"]);
    assert_eq!(read(&db, r#"{ Person(age <-5 || knownBy <- Person(name = "Bob")) { name } }"#), ["Ann"]);
}

// ---------------------------------------------------------------------------
// Work: every relationship a walk reads is charged to the statement.

const NET: &str = "type N { name: String x -> N[] }";

/// A complete graph of `n` nodes: every node points at every other.
fn net(builder: zega::ZegaBuilder, n: usize) -> Zega {
    let db = builder.build().unwrap();
    let nodes: Vec<String> = (0..n).map(|i| format!(r#"{{"name":"n{i}"}}"#)).collect();
    let mut edges = Vec::new();
    for a in 0..n {
        for b in 0..n {
            if a != b {
                edges.push(format!(r#"{{"a":"n{a}","b":"n{b}"}}"#));
            }
        }
    }
    let sources = HashMap::from([
        ("nodes.json".to_string(), format!("[{}]", nodes.join(","))),
        ("edges.json".to_string(), format!("[{}]", edges.join(","))),
    ]);
    db.run_lang_with_sources(NET, r#"mutation json ["nodes.json"] { N(name: $name) }"#, &sources).unwrap();
    db.run_lang_with_sources(NET, r#"mutation json ["edges.json"] { N(name: $a) { x -> link N(name: $b) } }"#, &sources)
        .unwrap();
    db
}

/// Four hops and a test nobody passes: n^5 reads with no index to start from.
const NOWHERE: &str = r#"{ N(x -> N(x -> N(x -> N(x -> N(name = "none"))))) { name } }"#;

#[test]
fn a_nested_walk_stops_at_the_query_time_limit() {
    let data = tempfile::tempdir().unwrap();
    let path = data.path().to_str().unwrap().to_string();
    drop(net(Zega::open(&path), 60));
    let limit = Duration::from_millis(200);
    let db = Zega::open(&path).query_time_limit(limit).traversal_work_budget(usize::MAX).build().unwrap();
    let started = Instant::now();
    let error = db.run_lang(NET, NOWHERE).unwrap_err();
    let took = started.elapsed();
    assert!(matches!(error, ZegaError::QueryTimeLimit { limit: l } if l == limit), "{error}");
    assert!(took < Duration::from_secs(3), "stopped after {took:?}");
}

#[test]
fn a_nested_walk_spends_the_traversal_budget() {
    let db = net(Zega::in_memory().traversal_work_budget(10_000), 20);
    let error = db.run_lang(NET, NOWHERE).unwrap_err();
    assert!(error.to_string().contains("relationship traversal work budget exceeded"), "{error}");
    // One hop fits: 20 nodes, each reading its 19 relationships.
    let result = db.run_lang(NET, r#"{ N(x -> N(name = "n0")) { name } }"#).unwrap();
    assert_eq!(result.as_array().unwrap().len(), 19);
}
