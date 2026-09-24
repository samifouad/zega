//! Stage membership, set algebra, index parity, and the discovery wire contract.
use serde_json::{json, Value as Json};
use zega::{diagnose, Zega};

const SCHEMA: &str = r#"schema {
 type Player { name: String bio?: String country?: String age?: Int embedding?: Vector<2> at?: Point playsFor -> Team }
 type Team { name: String country: String }
}"#;

fn db() -> Zega {
    let db = Zega::in_memory().build().unwrap();
    for mutation in [
        r#"mutation { Player(name: "Alice Oilers" && bio: "captain" && country: "CA" && embedding: @vector[1,0] && at: @point(0,0)) { playsFor -> Team(name: "Oilers" && country: "CA") } }"#,
        r#"mutation { Player(name: "Bob" && bio: "Oilers fan" && country: "CA" && embedding: @vector[1,0] && at: @point(0,0.001)) }"#,
        r#"mutation { Player(name: "Carol" && country: "US" && embedding: @vector[0,1] && at: @point(10,10)) }"#,
        r#"mutation { Player(name: "Missing") }"#,
    ] {
        db.run_lang(SCHEMA, mutation).unwrap();
    }
    db
}

fn run(db: &Zega, expr: &str) -> Json {
    db.run_lang(
        SCHEMA,
        &format!("query {{ Player {{ name playsFor -> Team {{ name }} }} }} then {{ {expr} }}"),
    )
    .unwrap()
}
fn ids(result: &Json, stage: usize) -> Vec<u64> {
    result["stages"][stage]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_u64().unwrap())
        .collect()
}
fn edges(result: &Json, stage: usize) -> &[Json] {
    result["stages"][stage]["couldBeEdges"].as_array().unwrap()
}

#[test]
fn text_primitives_scoping_unicode_empty_and_infix() {
    let db = db();
    for (expr, expected) in [
        (r#"findWith { "Oilers" }"#, vec![1, 2, 3]),
        (r#"findWith { "Oilers" in { name } }"#, vec![1, 2]),
        (r#"findWithout { "Oilers" }"#, vec![4, 5]),
        (r#"startsWith { "Oil" }"#, vec![2, 3]),
        (r#"endsWith { "Oilers" }"#, vec![1, 2]),
        (r#"regex { "^(Bob|Carol)$" in { name } }"#, vec![3, 4]),
        (r#"findWith { "" }"#, vec![1, 2, 3, 4, 5]),
        (r#"findWithout { "" }"#, vec![]),
    ] {
        assert_eq!(ids(&run(&db, expr), 1), expected, "{expr}");
    }
    db.run_lang(SCHEMA, r#"mutation { Player(name: "Émile 油") }"#)
        .unwrap();
    assert_eq!(ids(&run(&db, r#"regex { "\\p{Han}" }"#), 1), vec![6]);
    for (op, expected) in [
        ("findWith", vec![json!({"name":"Alice Oilers"})]),
        ("startsWith", vec![]),
        ("endsWith", vec![json!({"name":"Alice Oilers"})]),
    ] {
        assert_eq!(
            db.run_lang(
                SCHEMA,
                &format!("query {{ Player(name {op} \"Oilers\") {{ name }} }}")
            )
            .unwrap(),
            json!(expected)
        );
    }
}

#[test]
fn intersection_union_precedence_and_edge_pruning() {
    let db = db();
    let result = run(
        &db,
        r#"common { Player { country } Team { country } } && findWith { "Oilers" in { name } }"#,
    );
    assert_eq!(ids(&result, 1), vec![1, 2]);
    assert_eq!(
        edges(&result, 1),
        &[
            json!({"from":1,"to":2,"via":{"primitive":"common","fields":["Player.country","Team.country"],"value":"CA"}})
        ]
    );
    let result = run(
        &db,
        r#"findWith { "Carol" } || findWith { "Bob" } && findWith { "captain" }"#,
    );
    assert_eq!(ids(&result, 1), vec![4]);
    let result = run(
        &db,
        r#"(findWith { "Carol" } || findWith { "Bob" }) && findWithout { "captain" }"#,
    );
    assert_eq!(ids(&result, 1), vec![3, 4]);
    let result = run(&db, "common { Player { country } Team { country } } || common { Player { country } Team { country } }");
    assert_eq!(ids(&result, 1), vec![1, 2, 3]);
    assert_eq!(edges(&result, 1).len(), 2);
}

#[test]
fn stages_chain_only_previous_nodes_and_skip_never_serializes_them() {
    let db = db();
    let result = db
        .run_lang(
            SCHEMA,
            r#"query { Player { name playsFor -> Team { name } } } display { skip }
        then { findWith { "Oilers" in { name } } } display { skip }
        then { findWith { "Bob" } || findWith { "Alice" } }"#,
        )
        .unwrap();
    assert_eq!(result["stages"].as_array().unwrap().len(), 1);
    assert_eq!(result["stages"][0]["index"], 2);
    assert_eq!(ids(&result, 0), vec![1]);
    assert!(!result.to_string().contains("Bob"));
    assert!(!result.to_string().contains("Carol"));
    assert_eq!(
        db.run_lang(SCHEMA, r#"query { Player { name } } display { skip }"#)
            .unwrap(),
        Json::Null
    );
    let result = db.run_lang(SCHEMA,r#"query { Player { name } } display { skip } then { findWith { "x" } } display { skip }"#).unwrap();
    assert_eq!(result, json!({"stages":[]}));
}

#[test]
fn query_projection_without_ids_tracks_nested_members_and_actual_edges() {
    let db = db();
    let result = run(&db, r#"findWith { "Oilers" }"#);
    assert_eq!(ids(&result, 0), vec![1, 2, 3, 4, 5]);
    assert_eq!(
        result["stages"][0]["edges"],
        json!([{"id":1,"type":"playsFor","from":1,"to":2,"props":{}}])
    );
    assert_eq!(result["stages"][1]["edges"], result["stages"][0]["edges"]);
    let outside = db
        .run_lang(
            SCHEMA,
            r#"query { Player(name: "Bob") { name } } then { common { Player { country } } }"#,
        )
        .unwrap();
    assert!(ids(&outside, 1).is_empty());
}

#[test]
fn single_query_shape_and_empty_results_remain_compatible() {
    let db = db();
    assert_eq!(
        db.run_lang(SCHEMA, r#"query { Player(name: "Bob") { name } }"#)
            .unwrap(),
        json!({"name":"Bob"})
    );
    assert_eq!(db.run_lang(SCHEMA, "query {}").unwrap(), Json::Null);
    let result = db
        .run_lang(
            SCHEMA,
            r#"query { Player(name: "nobody") { name } } then { findWith { "Oilers" } }"#,
        )
        .unwrap();
    assert!(ids(&result, 0).is_empty());
    assert!(ids(&result, 1).is_empty());
}

#[test]
fn common_tuples_null_and_linear_star_fanout() {
    let db = Zega::in_memory().build().unwrap();
    let schema = "schema { type A { name: String x?: Float y?: Int } type B { name: String other: Float year: Int } }";
    for i in 0..128 {
        db.run_lang(
            schema,
            &format!("mutation {{ A(name: \"{i}\" && x: 0.0 && y: 1) }}"),
        )
        .unwrap();
    }
    db.run_lang(
        schema,
        r#"mutation { B(name: "B" && other: -0.0 && year: 1) }"#,
    )
    .unwrap();
    db.run_lang(schema, r#"mutation { A(name: "missing") }"#)
        .unwrap();
    db.run_lang(
        schema,
        r#"mutation { A(name: "different" && x: 0.0 && y: 2) }"#,
    )
    .unwrap();
    let result = db
        .run_lang(
            schema,
            "query { (A | B) { name } } then { common { A { x y } B { other year } } }",
        )
        .unwrap();
    assert_eq!(ids(&result, 1).len(), 129);
    assert_eq!(edges(&result, 1).len(), 128);
    assert!(edges(&result, 1)
        .iter()
        .all(|edge| edge["from"] == 1 && edge["via"]["value"] == json!([0.0, 1])));
}

#[test]
fn similar_is_pairwise_scoped_weighted_and_respects_boundaries() {
    let db = db();
    for threshold in ["> 0.9", ">= 1"] {
        let result = run(&db, &format!("similar {{ &embedding {threshold} }}"));
        assert_eq!(ids(&result, 1), vec![1, 3]);
        assert_eq!(
            edges(&result, 1),
            &[
                json!({"from":1,"to":3,"via":{"primitive":"similar","fields":["embedding"],"score":1.0}})
            ]
        );
    }
    assert!(ids(&run(&db, "similar { &embedding > 1 }"), 1).is_empty());
    let scoped = db
        .run_lang(
            SCHEMA,
            r#"query { Player(name: "Bob") } then { similar { &embedding > 0.9 } }"#,
        )
        .unwrap();
    assert!(ids(&scoped, 1).is_empty());
}

#[test]
fn near_units_distances_zero_and_previous_scope() {
    let db = db();
    for distance in ["1 km", "1000 m", "1 mi"] {
        let result = run(&db, &format!("near {{ &at < {distance} }}"));
        assert_eq!(ids(&result, 1), vec![1, 3]);
        let edge = &edges(&result, 1)[0];
        assert_eq!(edge["via"]["fields"], json!(["at"]));
        assert!((edge["via"]["distance"].as_f64().unwrap() - 111.19508023353292).abs() < 1e-6);
    }
    assert!(ids(&run(&db, "near { &at < 0 m }"), 1).is_empty());
    db.run_lang(
        SCHEMA,
        r#"mutation { Player(name: "same" && at: @point(0,0)) }"#,
    )
    .unwrap();
    let result = run(&db, "near { &at <= 0 m }");
    assert_eq!(ids(&result, 1), vec![1, 6]);
    assert_eq!(edges(&result, 1)[0]["via"]["distance"], 0.0);
}

#[test]
fn errors_are_checked_even_if_the_query_returns_no_nodes() {
    let db = db();
    for (expr, message) in [
        (r#"findWith { "x" in { age } }"#, "must be String"),
        (r#"regex { "x" in { missing } }"#, "no field missing"),
        ("similar { &name > 0.9 }", "must be Vector"),
        ("near { &country < 1 km }", "must be Point"),
        ("near { &at < 1 feet }", "unit must be m, km, or mi"),
        ("near { &at < -1 m }", "non-negative"),
        ("common { Unknown { country } }", "unknown type Unknown"),
        (
            "common { Player { nonexistent } }",
            "has no field nonexistent",
        ),
        (
            "common { Player { country name } Team { country } }",
            "same number and types",
        ),
        (r#"findWith { "x" in {} }"#, "scope is empty"),
        (r#"regex { "(?=a)" }"#, "lookaround and backreferences"),
    ] {
        let query = format!(
            "query {{ Player(name: \"absent\") {{ playsFor -> Team }} }} then {{ {expr} }}"
        );
        let error = db.run_lang(SCHEMA, &query).unwrap_err().to_string();
        assert!(error.contains(message), "{expr}: {error}");
        assert!(diagnose(SCHEMA, &query).text.contains(message));
    }
}

#[test]
fn wrong_cross_type_fields_and_vector_shapes_are_errors() {
    let db = Zega::in_memory().build().unwrap();
    for (schema, expr, message) in [
        (
            "schema { type A { name: String } type B { name: Int } }",
            r#"findWith { "a" in { name } }"#,
            "must be String",
        ),
        (
            "schema { type A { v: Vector<2> } type B { v: Vector<3> } }",
            "similar { &v > 0.9 }",
            "matching Vector dimensions",
        ),
        (
            "schema { type A { v: Vector<2,dot> } type B { v: Vector<2,cosine> } }",
            "similar { &v > 0.9 }",
            "matching Vector dimensions",
        ),
    ] {
        let error = db
            .run_lang(schema, &format!("query {{ (A | B) }} then {{ {expr} }}"))
            .unwrap_err()
            .to_string();
        assert!(error.contains(message), "{error}");
    }
}

#[test]
fn text_index_accelerates_without_changing_any_operator() {
    let db = Zega::in_memory().build().unwrap();
    let plain = "schema { type A { name: String } }";
    let indexed = format!("{plain} index {{ text A {{ name }} }}");
    for i in 0..300 {
        db.run_lang(
            plain,
            &format!(
                "mutation {{ A(name: \"{}\") }}",
                if i == 7 {
                    "needle".into()
                } else {
                    format!("item-{i}")
                }
            ),
        )
        .unwrap();
    }
    for op in ["findWith", "findWithout", "startsWith", "endsWith"] {
        let query = format!("query {{ A }} then {{ {op} {{ \"needle\" }} }}");
        let before = db.rows_examined().unwrap();
        let scan = db.run_lang(plain, &query).unwrap();
        let scans = db.rows_examined().unwrap() - before;
        let before = db.rows_examined().unwrap();
        let fast = db.run_lang(&indexed, &query).unwrap();
        let indexed_rows = db.rows_examined().unwrap() - before;
        assert_eq!(fast, scan);
        assert!(indexed_rows < scans, "{op}: {indexed_rows} vs {scans}");
    }
}

#[test]
fn output_is_deterministic_and_discovery_never_mutates() {
    let db = db();
    let before = db.graph_json().unwrap();
    let expr="(common { Player { country } Team { country } } || similar { &embedding >= 0 }) || near { &at < 2000 km }";
    let expected = run(&db, expr);
    for _ in 0..20 {
        assert_eq!(run(&db, expr), expected);
    }
    assert_eq!(db.graph_json().unwrap(), before);
}

#[test]
fn path_nodes_are_members_but_search_frontier_nodes_are_not() {
    let db = Zega::in_memory().build().unwrap();
    let schema = "schema { type Stop { name: String road -> Stop[] } } unique { Stop { name } }";
    db.run_lang(
        schema,
        r#"mutation { Stop(name:"A") { road -> Stop(name:"B") { road -> Stop(name:"C") } } }"#,
    )
    .unwrap();
    db.run_lang(schema, r#"mutation { Stop(name:"dead") }"#)
        .unwrap();
    db.connect_schema(schema, 1, "road", 4).unwrap();
    let result=db.run_lang(schema,r#"query { Stop(name:"A") { road *path -> Stop(name:"C") { name } } } then { findWith { "" } }"#).unwrap();
    assert_eq!(ids(&result, 0), vec![1, 2, 3]);
    assert_eq!(ids(&result, 1), vec![1, 2, 3]);
    assert_eq!(result["stages"][0]["edges"].as_array().unwrap().len(), 2);
}

#[test]
fn threshold_pairs_match_independent_dot_products_after_updates_and_deletion() {
    let db = Zega::in_memory().build().unwrap();
    let schema = "schema { type T { n: Int v: Vector<2,dot> } } unique { T { n } }";
    let mut vectors = Vec::new();
    for i in 0..35 {
        let v = [(i % 7) as f64 - 3.0, (i % 5) as f64 - 2.0];
        vectors.push(v);
        db.run_lang(
            schema,
            &format!("mutation {{ T(n:{i} && v:@vector[{},{}]) }}", v[0], v[1]),
        )
        .unwrap();
    }
    vectors[3] = [9.0, 9.0];
    db.run_lang(schema, "mutation { T(n:3) set v:@vector[9,9] }")
        .unwrap();
    db.delete_node(8).unwrap();
    let result = db
        .run_lang(schema, "query { T(n < 30) } then { similar { &v > 4 } }")
        .unwrap();
    let mut expected = Vec::new();
    for i in 0..30 {
        for j in i + 1..30 {
            if i == 7 || j == 7 {
                continue;
            }
            let score = vectors[i][0] * vectors[j][0] + vectors[i][1] * vectors[j][1];
            if score > 4.0 {
                expected.push(json!({"from":i+1,"to":j+1,"via":{"primitive":"similar","fields":["v"],"score":score}}));
            }
        }
    }
    assert_eq!(edges(&result, 1), expected);
}

#[test]
fn spatial_index_handles_antimeridian_poles_and_nulls() {
    let db = Zega::in_memory().build().unwrap();
    let schema = "schema { type T { name: String at?: Point } }";
    for (name, lat, lon) in [
        ("west", 0, 179.999),
        ("east", 0, -179.999),
        ("north1", 90, 0.0),
        ("north2", 90, 180.0),
    ] {
        db.run_lang(
            schema,
            &format!("mutation {{ T(name:\"{name}\" && at:@point({lat},{lon})) }}"),
        )
        .unwrap();
    }
    db.run_lang(schema, "mutation { T(name:\"missing\") }")
        .unwrap();
    let result = db
        .run_lang(schema, "query { T } then { near { &at < 1 km } }")
        .unwrap();
    assert_eq!(ids(&result, 1), vec![1, 2, 3, 4]);
    let pairs: Vec<_> = edges(&result, 1)
        .iter()
        .map(|e| (e["from"].as_u64().unwrap(), e["to"].as_u64().unwrap()))
        .collect();
    assert_eq!(pairs, vec![(1, 2), (3, 4)]);
    assert!(edges(&result, 1)[0]["via"]["distance"].as_f64().unwrap() < 223.0);
    assert_eq!(edges(&result, 1)[1]["via"]["distance"], 0.0);
}

#[test]
fn and_keeps_evidence_from_both_operands_but_chaining_recomputes_it() {
    let db = db();
    let result = run(
        &db,
        "common { Player { country } Team { country } } && similar { &embedding > 0.9 }",
    );
    assert_eq!(ids(&result, 1), vec![1, 3]);
    assert_eq!(edges(&result, 1).len(), 2);
    assert_eq!(edges(&result, 1)[0]["via"]["primitive"], "common");
    assert_eq!(edges(&result, 1)[1]["via"]["primitive"], "similar");
    let result = db
        .run_lang(
            SCHEMA,
            r#"query { Player } then { similar { &embedding > 0.9 } } then { findWith { "" } }"#,
        )
        .unwrap();
    assert_eq!(ids(&result, 2), vec![1, 3]);
    assert!(edges(&result, 2).is_empty());
}

#[test]
fn stage_envelope_protects_identity_from_user_fields_and_obeys_limit() {
    let db = Zega::in_memory().build().unwrap();
    let schema = "schema { type T { id: Int labels: String props: String } }";
    db.run_lang(
        schema,
        r#"mutation { T(id:99 && labels:"user" && props:"value") }"#,
    )
    .unwrap();
    db.run_lang(
        schema,
        r#"mutation { T(id:88 && labels:"second" && props:"value") }"#,
    )
    .unwrap();
    let result = db
        .run_lang(
            schema,
            r#"query { T limit 1 { id } } then { findWith { "user" } }"#,
        )
        .unwrap();
    assert_eq!(ids(&result, 0), vec![1]);
    assert_eq!(
        result["stages"][1]["nodes"],
        json!([{"id":1,"labels":["T"],"props":{"id":99,"labels":"user","props":"value"}}])
    );
}
