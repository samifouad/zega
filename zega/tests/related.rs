//! Walks in a filter (#86): `has`, `!have`, `in`/`with`, `same`, and
//! `N hops` / `within N hops`. The schema supplies every type.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value as Json};
use zega::{Zega, ZegaError};

fn ask(db: &Zega, schema: &str, query: &str) -> Json {
    db.run_lang(schema, query).unwrap_or_else(|error| panic!("{query}\n{error}"))
}

fn column<'a>(result: &'a Json, key: &str) -> Vec<&'a str> {
    result
        .as_array()
        .unwrap_or_else(|| panic!("a list: {result}"))
        .iter()
        .map(|row| row[key].as_str().unwrap())
        .collect()
}

fn error(schema: &str, query: &str) -> String {
    Zega::in_memory().build().unwrap().run_lang(schema, query).unwrap_err().to_string()
}

// ---------------------------------------------------------------------------
// The Flights sample. Its routes are `route: ROUTE -> Airport[]`, the
// `flights` of the design comment on #86.

fn flights() -> Zega {
    let db = Zega::in_memory().build().unwrap();
    let source = include_str!("../../browser/samples/flights.zql");
    let file = |name: &str, text: &str| (format!("./samples/{name}"), text.to_string());
    let sources = HashMap::from([
        file("flights-countries.csv", include_str!("../../browser/samples/flights-countries.csv")),
        file("flights-airports.csv", include_str!("../../browser/samples/flights-airports.csv")),
        file("flights-routes.csv", include_str!("../../browser/samples/flights-routes.csv")),
    ]);
    db.apply_zql_with_sources(source, &sources).unwrap();
    db
}

/// The sample's schema and `unique` block, which makes `iso` and `code` indexed.
fn flights_schema() -> &'static str {
    let source = include_str!("../../browser/samples/flights.zql");
    &source[..source.find("mutation csv").unwrap()]
}

#[test]
fn the_flights_queries_from_the_issue() {
    let db = flights();
    let schema = flights_schema();
    let codes = |query: &str| column(&ask(&db, schema, query), "code").into_iter().map(String::from).collect::<Vec<_>>();
    assert_eq!(codes(r#"{ Airport(has country(iso = "CA")) { code } }"#), ["YYC", "YYZ"]);
    assert_eq!(codes(r#"{ Airport(has country(iso = "CA") && has route in country(iso = "JP")) { code } }"#), ["YYC"]);
    // Every airport with no route to an airport in the US, found from the
    // sample's files by hand; ATL and the other US airports fly to none.
    assert_eq!(
        codes(r#"{ Airport(!have route in country(iso = "US")) { code } }"#),
        [
            "ADD", "AMS", "ATH", "ATL", "BCN", "BKK", "BUD", "CAI", "CDG", "CMN", "DME", "FRA", "HEL", "IST", "JED",
            "KUL", "LGW", "LIS", "MLA", "MNL", "OSL", "PRG", "SIN", "TLV", "TPE",
        ]
    );
    // "One stop": Japan is exactly two routes away, and not one.
    assert_eq!(
        codes(r#"{ Airport(has route 2 hops in country(iso = "JP")) { code } }"#),
        ["ADD", "BUD", "GRU", "JNB", "TLV", "WAW"]
    );
    let countries = ask(&db, schema, r#"{ Country(has airports with route within 3 hops in country(iso = "JP")) { name } }"#);
    // Not Japan: NRT reaching NRT again would be a round trip.
    assert_eq!(
        column(&countries, "name"),
        [
            "Australia", "Brazil", "Canada", "Egypt", "Ethiopia", "Finland", "Hungary", "India", "Israel", "Mexico",
            "Morocco", "Philippines", "Poland", "South Africa", "Taiwan",
        ]
    );
}

#[test]
fn an_indexed_end_is_found_first_and_walked_backwards() {
    let db = flights();
    let schema = flights_schema();
    let examined = |query: &str| {
        let before = db.rows_examined().unwrap();
        let result = ask(&db, schema, query);
        (column(&result, "code").into_iter().map(String::from).collect::<Vec<_>>(), db.rows_examined().unwrap() - before)
    };
    // `iso` is unique, so indexed: one Country, then only the two airports
    // that point at it, not all 52.
    assert_eq!(examined(r#"{ Airport(has country(iso = "CA")) { code } }"#), (vec!["YYC".into(), "YYZ".into()], 3));
    // Two hops: Japan, then the seven airports with a route to NRT.
    let (codes, rows) = examined(r#"{ Airport(has route in country(iso = "JP")) { code } }"#);
    assert_eq!(codes, ["DEL", "HEL", "MEX", "MNL", "SYD", "TPE", "YYC"]);
    assert_eq!(rows, 1 + 7);
    // The row's own index wins: one airport by its code, then one walk.
    assert_eq!(examined(r#"{ Airport(code = "YYC" && has country(iso = "CA")) { code } }"#), (vec!["YYC".into()], 1));
    // No index on the end: every airport walks forward.
    assert_eq!(examined(r#"{ Airport(has country(name = "Canada")) { code } }"#), (vec!["YYC".into(), "YYZ".into()], 52));
}

// ---------------------------------------------------------------------------
// A hockey graph, the one in the stepper linked from #86, with a Seattle
// goalie and a second node for Sweden added.

const HOCKEY: &str = r#"
    type Player {
      name: String
      position: String
      team: PLAYS_FOR -> Team
      birthplace: BORN_IN -> City
    }
    type Team {
      name: String
      league: MEMBER_OF -> League
      arena: HOME -> Arena
      players: PLAYS_FOR <- Player[]
    }
    type League {
      name: String
      country: BASED_IN -> Country
    }
    type Arena {
      name: String
      city: LOCATED_IN -> City
    }
    type City {
      name: String
      country: LOCATED_IN -> Country
    }
    type Country {
      name: String
      iso: String
    }
"#;

fn hockey() -> Zega {
    let db = Zega::in_memory().build().unwrap();
    let run = |q: String| {
        db.run_lang(HOCKEY, &q).unwrap_or_else(|e| panic!("{q}\n{e}"));
    };
    for (name, iso) in [("Canada", "CA"), ("United States", "US"), ("Sweden", "SE"), ("Finland", "FI"), ("Sweden", "SE")] {
        run(format!(r#"mutation {{ Country(name: "{name}" && iso: "{iso}") }}"#));
    }
    // The two Swedens differ only in their ids.
    let ids: Vec<u64> = ask(&db, HOCKEY, "{ Country { @id } }")
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_u64().unwrap())
        .collect();
    let [ca, us, se, fi, second_se] = ids[..] else { panic!("five countries: {ids:?}") };
    // Uppsala's Sweden is the second node: the same values, another node.
    for (city, country) in [("Toronto", ca), ("Calgary", ca), ("Seattle", us), ("Stockholm", se), ("Helsinki", fi), ("Uppsala", second_se)] {
        run(format!(r#"mutation {{ City(name: "{city}") }}"#));
        run(format!(r#"mutation {{ City(name: "{city}") {{ country -> link Country(@id = {country}) }} }}"#));
    }
    for (league, country) in [("NHL", us), ("SHL", se)] {
        run(format!(r#"mutation {{ League(name: "{league}") }}"#));
        run(format!(r#"mutation {{ League(name: "{league}") {{ country -> link Country(@id = {country}) }} }}"#));
    }
    for (arena, city) in [("Lakeshore Arena", "Toronto"), ("Summit Arena", "Calgary"), ("Sound Arena", "Seattle"), ("Nordic Hall", "Stockholm")] {
        run(format!(r#"mutation {{ Arena(name: "{arena}") }}"#));
        run(format!(r#"mutation {{ Arena(name: "{arena}") {{ city -> link City(name: "{city}") }} }}"#));
    }
    for (team, league, arena) in [
        ("Toronto Harbour", "NHL", "Lakeshore Arena"),
        ("Calgary Bighorns", "NHL", "Summit Arena"),
        ("Seattle Tide", "NHL", "Sound Arena"),
        ("Stockholm Wolves", "SHL", "Nordic Hall"),
    ] {
        run(format!(r#"mutation {{ Team(name: "{team}") }}"#));
        run(format!(r#"mutation {{ Team(name: "{team}") {{ league -> link League(name: "{league}") }} }}"#));
        run(format!(r#"mutation {{ Team(name: "{team}") {{ arena -> link Arena(name: "{arena}") }} }}"#));
    }
    for (player, position, team, city) in [
        ("Sam Tremblay", "C", "Toronto Harbour", "Calgary"),
        ("Liam Roy", "D", "Calgary Bighorns", "Toronto"),
        ("Mika Lund", "G", "Calgary Bighorns", "Helsinki"),
        ("Jake Miller", "W", "Seattle Tide", "Seattle"),
        ("Aapo Virta", "W", "Seattle Tide", "Helsinki"),
        ("Erik Holm", "D", "Stockholm Wolves", "Uppsala"),
        ("Gus Hale", "G", "Seattle Tide", "Seattle"),
    ] {
        run(format!(r#"mutation {{ Player(name: "{player}" && position: "{position}") }}"#));
        run(format!(r#"mutation {{ Player(name: "{player}") {{ team -> link Team(name: "{team}") }} }}"#));
        run(format!(r#"mutation {{ Player(name: "{player}") {{ birthplace -> link City(name: "{city}") }} }}"#));
    }
    db
}

fn names(db: &Zega, query: &str) -> Vec<String> {
    column(&ask(db, HOCKEY, query), "name").into_iter().map(String::from).collect()
}

#[test]
fn has_and_in_follow_the_schema_one_hop_each() {
    let db = hockey();
    assert_eq!(names(&db, r#"{ Player(has birthplace in country(iso = "CA")) { name } }"#), ["Sam Tremblay", "Liam Roy"]);
    assert_eq!(names(&db, r#"{ Player(has team in arena in city(name = "Calgary")) { name } }"#), ["Liam Roy", "Mika Lund"]);
    // `with` is `in`, worded for English; a test can sit on any hop.
    assert_eq!(
        names(&db, r#"{ Team(has players(position = "G") with birthplace(name = "Seattle")) { name } }"#),
        ["Seattle Tide"]
    );
    // No test: the relationship exists. Against the arrow, as the schema says.
    assert_eq!(names(&db, "{ City(has country) { name } }").len(), 6);
    assert_eq!(names(&db, r#"{ Team(has players(name = "Erik Holm")) { name } }"#), ["Stockholm Wolves"]);
}

#[test]
fn to_many_means_any_and_not_have_means_none() {
    let db = hockey();
    // Calgary has one Canadian-born player of two: any is enough.
    assert_eq!(
        names(&db, r#"{ Team(has players with birthplace in country(iso = "CA")) { name } }"#),
        ["Toronto Harbour", "Calgary Bighorns"]
    );
    assert_eq!(
        names(&db, r#"{ Team(!have players with birthplace in country(iso = "CA")) { name } }"#),
        ["Seattle Tide", "Stockholm Wolves"]
    );
    // A node without the relationship has none of it: no one plays in the SHL
    // but Erik, and no team has a Swedish-born goalie.
    assert_eq!(names(&db, r#"{ Team(!have players(position = "G") with birthplace in country(iso = "SE")) { name } }"#).len(), 4);
}

#[test]
fn same_continues_from_the_node_the_earlier_chain_reached() {
    let db = hockey();
    assert_eq!(
        names(&db, r#"{ Player(has team in league(name = "NHL") && same team in arena in city in country(iso = "CA")) { name } }"#),
        ["Sam Tremblay", "Liam Roy", "Mika Lund"]
    );
    // Seattle has a Finn and a goalie, but not one player who is both.
    assert_eq!(
        names(&db, r#"{ Team(has players with birthplace in country(iso = "FI") && same players(position = "G")) { name } }"#),
        ["Calgary Bighorns"]
    );
    assert_eq!(
        names(&db, r#"{ Team(has players with birthplace in country(iso = "FI") && has players(position = "G")) { name } }"#),
        ["Calgary Bighorns", "Seattle Tide"]
    );
    // A later chain can continue from a node a `same` chain reached.
    assert_eq!(
        names(&db, r#"{ Player(has team(name = "Seattle Tide") && same team in arena && same arena in city(name = "Seattle")) { name } }"#),
        ["Jake Miller", "Aapo Virta", "Gus Hale"]
    );
}

#[test]
fn in_same_joins_on_the_node_not_its_values() {
    let db = hockey();
    // Born in the country their team's arena is in. Erik was born in
    // Uppsala, whose Sweden is a second node with the same values as
    // Stockholm's: a join compares nodes, so he is not in it.
    assert_eq!(
        names(&db, "{ Player(has birthplace in country && has team in arena in city in same country) { name } }"),
        ["Sam Tremblay", "Liam Roy", "Jake Miller", "Gus Hale"]
    );
}

// ---------------------------------------------------------------------------
// Repeated hops.

const NET: &str = "type Stop { name: String next -> Stop[] }";

/// A -> B -> C -> D -> A (a cycle), and A -> C directly.
fn ring() -> Zega {
    let db = Zega::in_memory().build().unwrap();
    for name in ["A", "B", "C", "D"] {
        ask(&db, NET, &format!(r#"mutation {{ Stop(name: "{name}") }}"#));
    }
    for (from, to) in [("A", "B"), ("B", "C"), ("C", "D"), ("D", "A"), ("A", "C")] {
        ask(&db, NET, &format!(r#"mutation {{ Stop(name: "{from}") {{ next -> link Stop(name: "{to}") }} }}"#));
    }
    db
}

#[test]
fn hops_reach_each_node_at_its_shortest_distance_and_never_go_back() {
    let db = ring();
    let stops = |query: &str| names_of(&ask(&db, NET, query));
    // From A: B and C are one hop, D two. C is not "2 hops" as well.
    assert_eq!(stops(r#"{ Stop(has next 2 hops(name = "D")) { name } }"#), ["A", "B"]);
    assert_eq!(stops(r#"{ Stop(has next 2 hops(name = "C")) { name } }"#), ["D"]);
    assert_eq!(stops(r#"{ Stop(has next within 2 hops(name = "C")) { name } }"#), ["A", "B", "D"]);
    // No round trips: A reaches every other stop, never itself.
    assert_eq!(stops(r#"{ Stop(has next within 6 hops(name = "A")) { name } }"#), ["B", "C", "D"]);
    // The same in a selection, as `*2..2` and `*1..2` read it.
    let hops = ask(&db, NET, r#"{ Stop(name: "A") { next 2 hops -> Stop { name @hops } } }"#);
    let star = ask(&db, NET, r#"{ Stop(name: "A") { next *2..2 -> Stop { name @hops } } }"#);
    assert_eq!(hops, star);
    assert_eq!(hops, json!({ "next": [{ "hops": 2, "name": "D" }] }));
    let within = ask(&db, NET, r#"{ Stop(name: "A") { next within 2 hops -> Stop { name } } }"#);
    assert_eq!(within, ask(&db, NET, r#"{ Stop(name: "A") { next *1..2 -> Stop { name } } }"#));
}

fn names_of(result: &Json) -> Vec<String> {
    column(result, "name").into_iter().map(String::from).collect()
}

#[test]
fn hops_are_capped_at_six() {
    for query in ["{ Stop(has next 7 hops) { name } }", "{ Stop(has next within 0 hops) { name } }", "{ Stop { next 7 hops -> Stop { name } } }"] {
        let message = error(NET, query);
        assert!(message.contains("a relationship repeats 1 to 6 hops"), "{query}\n{message}");
    }
}

/// S fans out to 60 stops and each of those to 60 more; T hangs two hops
/// under the first. A search from S alone reads every fan to find T; from both
/// ends at once, it meets after a handful of reads.
#[test]
fn within_hops_to_an_indexed_end_meets_in_the_middle() {
    let schema = "schema { type Stop { name: String next -> Stop[] } } unique { Stop { name } }";
    let mut nodes = vec![r#"{"name":"S"}"#.to_string(), r#"{"name":"T"}"#.to_string()];
    let mut edges = vec![r#"{"a":"c0-0","b":"T"}"#.to_string()];
    for c in 0..60 {
        nodes.push(format!(r#"{{"name":"c{c}"}}"#));
        edges.push(format!(r#"{{"a":"S","b":"c{c}"}}"#));
        for g in 0..60 {
            nodes.push(format!(r#"{{"name":"c{c}-{g}"}}"#));
            edges.push(format!(r#"{{"a":"c{c}","b":"c{c}-{g}"}}"#));
        }
    }
    let sources = HashMap::from([
        ("nodes.json".to_string(), format!("[{}]", nodes.join(","))),
        ("edges.json".to_string(), format!("[{}]", edges.join(","))),
    ]);
    let db = Zega::in_memory().traversal_work_budget(1_000).build().unwrap();
    let load = Zega::in_memory().build().unwrap();
    for target in [&db, &load] {
        target.run_lang_with_sources(schema, r#"mutation json ["nodes.json"] { Stop(name: $name) }"#, &sources).unwrap();
    }
    // The budget counts per statement; a load of 3,600 links needs more.
    let bytes = {
        load.run_lang_with_sources(schema, r#"mutation json ["edges.json"] { Stop(name: $a) { next -> link Stop(name: $b) } }"#, &sources)
            .unwrap();
        load.snapshot_bytes().unwrap()
    };
    db.restore_bytes(&bytes).unwrap();
    let result = ask(&db, schema, r#"{ Stop(has next within 3 hops(name = "T")) { name } }"#);
    assert_eq!(names_of(&result), ["S", "c0", "c0-0"]);
}

// ---------------------------------------------------------------------------
// Writes and loads find rows through a chain.

#[test]
fn set_delete_and_load_find_rows_through_a_chain() {
    let db = hockey();
    ask(&db, HOCKEY, r#"mutation { Player(name = "Gus Hale" && has team(name = "Seattle Tide")) set position: "D" }"#);
    assert_eq!(ask(&db, HOCKEY, r#"{ Player(position = "G") { name } }"#), json!({ "name": "Mika Lund" }));
    let csv = "player,team,position\nMika Lund,Calgary Bighorns,W\nSam Tremblay,,W\n";
    db.run_lang_with_sources(
        HOCKEY,
        r#"mutation csv ["rows.csv"] { Player(name = $player && has team(name = $team)) set position: $position }"#,
        &HashMap::from([("rows.csv".to_string(), csv.to_string())]),
    )
    .unwrap();
    // Sam's empty team drops the chain, the way an empty cell drops its term.
    assert_eq!(names(&db, r#"{ Player(position startsExact "W") { name } }"#), ["Sam Tremblay", "Mika Lund", "Jake Miller", "Aapo Virta"]);
    let deleted = ask(&db, HOCKEY, r#"mutation { delete Player(has birthplace in country(iso = "SE")) { @detach name } }"#);
    // Erik's Sweden is the second node; it has the same iso, so he goes.
    assert_eq!(deleted, json!({ "deleted": 1, "rows": [{ "name": "Erik Holm" }] }));
}

// ---------------------------------------------------------------------------
// Errors that show the right form.

#[test]
fn the_common_mistakes_are_errors_that_show_the_chain() {
    let schema = flights_schema();
    for (query, message, help) in [
        (r#"{ Airport(country = "CA") { code } }"#, "country is a relationship", r#"help: test a field of the related node: `has country(name = "CA")`; Country has name, iso"#),
        (r#"{ Airport(country.iso = "CA") { code } }"#, "a related node's field is reached with `has`", r#"help: write `has country(iso = "CA")`"#),
        (r#"{ Airport(!has route in country(iso = "US")) { code } }"#, "`!has` is not ZQL: none is `!have`", r#"help: write `!have route in country(iso = "US")`"#),
        (r#"{ Airport(country -> Country(iso = "CA")) { code } }"#, "a walk in a filter doesn't name the type", r#"help: write `has country(iso = "CA")`"#),
        (
            r#"{ Airport(route -> Airport(country -> Country(iso = "JP"))) { code } }"#,
            "a walk in a filter doesn't name the type",
            r#"help: write `has route in country(iso = "JP")`"#,
        ),
        (r#"{ Airport(country: Country(iso: "CA")) { code } }"#, "a related node is tested with `has`, not a value", r#"help: write `has country(iso: "CA")`"#),
        (r#"{ Airport(has route(has country)) { code } }"#, "a test in (…) checks this node's own fields", "walks never nest"),
        (r#"{ Airport(has code(name = "x")) { code } }"#, "Airport.code is a field, not a relationship", "help: test a field directly: `code = …`"),
        (r#"{ Airport(has country 2 hops) { code } }"#, "`country` can't repeat: Country has no `country`", ""),
        (r#"{ Airport(has routes) { code } }"#, "Airport has no relationship routes", "did you mean"),
    ] {
        let got = error(schema, query);
        assert!(got.contains(message), "{query}\n{got}");
        assert!(got.contains(help), "{query}\n{got}");
    }
}

#[test]
fn same_is_refused_where_it_names_nothing_or_two_nodes() {
    for (query, message) in [
        (r#"{ Player(has team(name = "A") || same team in arena) { name } }"#, "`same` can't follow `||`"),
        (r#"{ Player(has team && !have same team in arena) { name } }"#, "`same` can't follow `!have`"),
        (r#"{ Player(!have team(name = "A") && same team in arena) { name } }"#, "`same team` can't refer into `!have`"),
        (r#"{ Player(same team in arena) { name } }"#, "`same team` has no earlier chain"),
        (r#"{ Player(has birthplace && same team in arena) { name } }"#, "no earlier chain in this `&&` group reaches `team`"),
        (r#"{ Player(has team && has team in arena && same team(name = "A")) { name } }"#, "`same team` is ambiguous: `team` is reached 2 times earlier"),
        (r#"{ Player(has team in same arena in city) { name } }"#, "a chain ends at `in same …`"),
    ] {
        let got = error(HOCKEY, query);
        assert!(got.contains(message), "{query}\n{got}");
    }
    let schema = "type P { name: String home -> H work -> W } type H { city -> C1 } type W { city -> C2 } type C1 { name: String } type C2 { name: String }";
    let got = error(schema, "{ P(has home in city && has work in same city) { name } }");
    assert!(got.contains("`in same city` can never arrive at the same node"), "{got}");
}

#[test]
fn has_same_and_in_are_still_names() {
    // A field called `has`, another called `same`, and a relationship
    // called `in`: chain words only where a chain goes.
    let schema = "type Box { has: Int same?: Int in -> Box[] }";
    let db = Zega::in_memory().build().unwrap();
    ask(&db, schema, "mutation { Box(has: 2 && same: 3) }");
    ask(&db, schema, "mutation { Box(has: 1) }");
    ask(&db, schema, "mutation { Box(has: 1) { in -> link Box(has: 2) } }");
    assert_eq!(ask(&db, schema, "{ Box(has = 1 && has in(has = 2 && same >= 3)) { has } }"), json!([{ "has": 1 }]));
    // `<-` before a number is still less-than a negative.
    assert_eq!(ask(&db, schema, "{ Box(has <-5 || has in) { has } }"), json!([{ "has": 1 }]));
}

// ---------------------------------------------------------------------------
// Work: every relationship a chain reads is charged to the statement.

const DENSE: &str = "type N { name: String x -> N[] }";

/// A complete graph of `n` nodes: every node points at every other.
fn dense(builder: zega::ZegaBuilder, n: usize) -> Zega {
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
    db.run_lang_with_sources(DENSE, r#"mutation json ["nodes.json"] { N(name: $name) }"#, &sources).unwrap();
    db.run_lang_with_sources(DENSE, r#"mutation json ["edges.json"] { N(name: $a) { x -> link N(name: $b) } }"#, &sources)
        .unwrap();
    db
}

/// Five hops and a test nobody passes, with no index to start from.
const NOWHERE: &str = r#"{ N(has x in x in x in x in x(name = "none")) { name } }"#;

#[test]
fn a_long_chain_stops_at_the_query_time_limit() {
    let data = tempfile::tempdir().unwrap();
    let path = data.path().to_str().unwrap().to_string();
    drop(dense(Zega::open(&path), 120));
    let limit = Duration::from_millis(200);
    let db = Zega::open(&path).query_time_limit(limit).traversal_work_budget(usize::MAX).build().unwrap();
    let started = Instant::now();
    let error = db.run_lang(DENSE, NOWHERE).unwrap_err();
    assert!(matches!(error, ZegaError::QueryTimeLimit { limit: l } if l == limit), "{error}");
    assert!(started.elapsed() < Duration::from_secs(3), "stopped after {:?}", started.elapsed());
}

#[test]
fn a_chain_spends_the_traversal_budget() {
    let db = dense(Zega::in_memory().traversal_work_budget(10_000), 20);
    let error = db.run_lang(DENSE, NOWHERE).unwrap_err();
    assert!(error.to_string().contains("relationship traversal work budget exceeded"), "{error}");
    // One hop fits: 20 nodes, each reading its 19 relationships.
    let result = db.run_lang(DENSE, r#"{ N(has x(name = "n0")) { name } }"#).unwrap();
    assert_eq!(result.as_array().unwrap().len(), 19);
}
