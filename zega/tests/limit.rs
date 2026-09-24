//! `limit n` stops a scan at the nth match (zegadb/zega#82). Rows are tested
//! in ascending id order, which is also the result order, so the first n
//! matches are the answer. The proof counts rows tested (`rows_examined`),
//! not time.

use serde_json::{json, Value as Json};
use std::collections::HashMap;
use zega::Zega;

const PEOPLE: usize = 3000;
const CITIES: usize = 100;

fn schema(blocks: &str) -> String {
    format!(
        "schema {{\n  type Person {{\n    n: Int\n    city: String\n    at: Point\n  }}\n}}\n{blocks}"
    )
}

/// 3000 people, created in order, spread evenly over 100 cities: `c7` is 1%
/// of them, people 7, 107, 207 and so on.
fn db(schema: &str) -> Zega {
    let db = Zega::in_memory().build().unwrap();
    let rows: Vec<Json> = (0..PEOPLE)
        .map(|n| json!({ "n": n, "city": format!("c{}", n % CITIES), "at": { "lat": 0.0, "lon": n as f64 / 1000.0 } }))
        .collect();
    let sources = HashMap::from([("./people.json".to_string(), Json::Array(rows).to_string())]);
    db.run_lang_with_sources(
        schema,
        "mutation json [\"./people.json\"] {\n  Person(n: $n && city: $city && at: $at) { n }\n}\n",
        &sources,
    )
    .unwrap();
    db
}

/// Rows the query tested, and its result.
fn measure(db: &Zega, schema: &str, query: &str) -> (u64, Json) {
    let before = db.rows_examined().unwrap();
    let result = db.run_lang(schema, query).unwrap();
    (db.rows_examined().unwrap() - before, result)
}

fn ns(result: &Json) -> Vec<u64> {
    result
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["n"].as_u64().unwrap())
        .collect()
}

#[test]
fn a_limit_10_scan_stops_at_the_tenth_match_instead_of_testing_every_node() {
    let schema = schema("");
    let db = db(&schema);
    let (tested, result) = measure(
        &db,
        &schema,
        "query {\n  Person(city = \"c7\") limit 10 { n }\n}\n",
    );
    // The first ten people in c7, in creation order.
    assert_eq!(
        ns(&result),
        vec![7, 107, 207, 307, 407, 507, 607, 707, 807, 907]
    );
    // About limit / selectivity = 10 / 1% rows, not all 3000: exactly the rows
    // up to and including the tenth match, person 907.
    assert_eq!(tested, 908, "rows tested for limit 10");
    // Without a limit, every row is still tested. (`n >= 0` keeps the same rows
    // and makes it a list: an equality filter alone is a one-row lookup.)
    let (tested, all) = measure(
        &db,
        &schema,
        "query {\n  Person(city = \"c7\" && n >= 0) { n }\n}\n",
    );
    assert_eq!(tested, PEOPLE as u64);
    assert_eq!(ns(&all)[..10], ns(&result)[..]);
}

#[test]
fn stopping_early_returns_the_same_rows_as_testing_everything() {
    let schema = schema("");
    let db = db(&schema);
    for (condition, limit) in [
        ("city = \"c0\"", 1),
        ("city = \"c99\" || n < 5", 7),
        ("n >= 2990", 50),
        ("city = \"nowhere\"", 3),
        ("city = \"c3\"", 0),
        ("city startsWith \"c1\" && n > 100", 25),
    ] {
        let (_, limited) = measure(
            &db,
            &schema,
            &format!("query {{\n  Person({condition}) limit {limit} {{ n }}\n}}\n"),
        );
        let (_, all) = measure(
            &db,
            &schema,
            &format!("query {{\n  Person(({condition}) && n >= 0) {{ n }}\n}}\n"),
        );
        let expected: Vec<u64> = ns(&all).into_iter().take(limit).collect();
        assert_eq!(ns(&limited), expected, "{condition} limit {limit}");
    }
}

#[test]
fn an_indexed_limit_also_stops_early() {
    let schema = schema("index {\n  range Person { n }\n}\n");
    let db = db(&schema);
    // The index narrows the scan to 2000 candidates; limit 10 tests ten of them.
    let (tested, result) = measure(
        &db,
        &schema,
        "query {\n  Person(n >= 1000) limit 10 { n }\n}\n",
    );
    assert_eq!(ns(&result), (1000..1010).collect::<Vec<_>>());
    assert_eq!(tested, 10);
}

#[test]
fn order_by_distance_still_ranks_the_whole_set_before_the_limit() {
    let schema = schema("");
    let db = db(&schema);
    // Person n sits at longitude n / 1000, so the c7 people nearest longitude
    // 2.9995 are the last ones created: ranking, not id order, decides.
    let result = db
        .run_lang(
            &schema,
            "query {\n  Person(city = \"c7\") order by @distance(at, @point(0, 2.9995)) limit 3 { n }\n}\n",
        )
        .unwrap();
    assert_eq!(ns(&result), vec![2907, 2807, 2707]);
}
