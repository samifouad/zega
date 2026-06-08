//! Exhaustive integration tests for the `zega-core` integrating engine.
//!
//! These exercise the public API surface (`Zega`, `ZegaBuilder`, `query`,
//! `query_with_context`, KV helpers, contexts, policies) and the query
//! execution paths: MATCH/WHERE expression evaluation, RETURN projection +
//! aliases, ORDER BY, LIMIT, aggregations + GROUP BY, null/Option handling,
//! Neo4j-style type coercion, and error-as-value paths.
//!
//! Everything is driven through the public surface so that no private
//! internals are required. Errors are values in Zega; we assert on
//! `Result`/`ZegaError`, never on panics.

use std::collections::{HashMap, HashSet};

use zega_core::{Row, Value, Zega, ZegaContext, ZegaError};
use zega_core::{ExprValue, PolicyCondition, PolicyExpr, PolicyTargets};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Fresh in-memory database for query tests.
fn db() -> Zega {
    Zega::in_memory().build().unwrap()
}

fn params(pairs: &[(&str, Value)]) -> HashMap<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

fn no_params() -> HashMap<String, Value> {
    HashMap::new()
}

#[test]
fn create_then_return_projects_created_binding() {
    let zega = db();
    let rows = zega
        .query(
            "CREATE (n:T {a: $x}) RETURN n.a AS a",
            params(&[("x", Value::Int(42))]),
        )
        .unwrap();
    assert_eq!(one_field(&rows, "a"), Value::Int(42));
}

fn claims(entries: &[(&str, &str)]) -> HashMap<String, Value> {
    entries
        .iter()
        .map(|(k, v)| ((*k).to_string(), Value::String((*v).to_string())))
        .collect()
}

/// Extract a single scalar field from the single returned row.
fn one_field(rows: &[Row], field: &str) -> Value {
    assert_eq!(rows.len(), 1, "expected exactly one row, got {}", rows.len());
    rows[0]
        .fields
        .get(field)
        .cloned()
        .unwrap_or_else(|| panic!("missing field {field}"))
}

/// Collect a scalar field across all rows.
fn collect_field(rows: &[Row], field: &str) -> Vec<Value> {
    rows.iter()
        .map(|r| r.fields.get(field).cloned().unwrap_or(Value::Null))
        .collect()
}

/// Read `var.prop` out of a row whose `var` field is a node-Map.
fn map_field(row: &Row, var: &str, prop: &str) -> Option<Value> {
    match row.fields.get(var) {
        Some(Value::Map(m)) => m.get(prop).cloned(),
        _ => None,
    }
}

// ===========================================================================
// 1. Builder / open / lifecycle
// ===========================================================================

#[test]
fn open_in_memory_builds_successfully() {
    let zega = Zega::in_memory().build();
    assert!(zega.is_ok());
}

#[test]
fn open_on_disk_creates_directory_and_builds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    let zega = Zega::open(path.to_str().unwrap()).build();
    assert!(zega.is_ok());
    assert!(path.exists());
}

#[test]
fn builder_traversal_work_budget_is_chainable() {
    let zega = Zega::in_memory()
        .traversal_work_budget(10)
        .wal_flush_every_write()
        .build();
    assert!(zega.is_ok());
}

#[test]
fn builder_jwt_hmac_and_issuer_chainable() {
    let zega = Zega::in_memory()
        .jwt_issuer("zega")
        .jwt_hmac_secret(b"secret".to_vec())
        .build();
    assert!(zega.is_ok());
}

#[test]
fn empty_query_string_returns_no_rows() {
    let zega = db();
    let rows = zega.query("", no_params()).unwrap();
    assert!(rows.is_empty());
}

#[test]
fn whitespace_only_query_returns_no_rows() {
    let zega = db();
    let rows = zega.query("   \n  \t ", no_params()).unwrap();
    assert!(rows.is_empty());
}

// ===========================================================================
// 2. CREATE + MATCH basics
// ===========================================================================

#[test]
fn create_then_match_node_by_label() {
    let zega = db();
    zega.query("CREATE (n:Person {name: 'Alice'})", no_params())
        .unwrap();
    let rows = zega
        .query("MATCH (n:Person) RETURN n", no_params())
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].fields.contains_key("n"));
}

#[test]
fn create_with_parameter_then_match_with_parameter() {
    let zega = db();
    let p = params(&[("name", Value::String("Bob".into()))]);
    zega.query("CREATE (n:Person {name: $name})", p.clone())
        .unwrap();
    let rows = zega
        .query("MATCH (n:Person {name: $name}) RETURN n", p)
        .unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn match_node_identifier_returns_map_with_id_and_labels() {
    let zega = db();
    zega.query("CREATE (n:Person {name: 'Alice', age: 30})", no_params())
        .unwrap();
    let rows = zega
        .query("MATCH (n:Person) RETURN n", no_params())
        .unwrap();
    let Value::Map(m) = rows[0].fields.get("n").unwrap() else {
        panic!("node identifier should be a Map");
    };
    assert!(matches!(m.get("id"), Some(Value::Int(_))));
    assert_eq!(
        m.get("labels"),
        Some(&Value::List(vec![Value::String("Person".into())]))
    );
    assert_eq!(m.get("name"), Some(&Value::String("Alice".into())));
    assert_eq!(m.get("age"), Some(&Value::Int(30)));
}

#[test]
fn match_nonexistent_label_returns_empty() {
    let zega = db();
    zega.query("CREATE (n:Person {name: 'Alice'})", no_params())
        .unwrap();
    let rows = zega
        .query("MATCH (n:Ghost) RETURN n", no_params())
        .unwrap();
    assert!(rows.is_empty());
}

#[test]
fn match_property_filter_in_pattern_selects_matching_node() {
    let zega = db();
    zega.query("CREATE (n:Person {name: 'Alice'})", no_params())
        .unwrap();
    zega.query("CREATE (n:Person {name: 'Bob'})", no_params())
        .unwrap();
    let rows = zega
        .query(
            "MATCH (n:Person {name: 'Bob'}) RETURN n.name AS name",
            no_params(),
        )
        .unwrap();
    assert_eq!(one_field(&rows, "name"), Value::String("Bob".into()));
}

#[test]
fn match_all_nodes_no_label_returns_everything() {
    let zega = db();
    zega.query("CREATE (n:A {x: 1})", no_params()).unwrap();
    zega.query("CREATE (n:B {x: 2})", no_params()).unwrap();
    let rows = zega.query("MATCH (n) RETURN n", no_params()).unwrap();
    assert_eq!(rows.len(), 2);
}

#[test]
fn create_relationship_makes_two_nodes_and_edge_traversable() {
    let zega = db();
    zega.query(
        "CREATE (u:User {id: 'u1'})-[:PLACED]->(o:Order {id: 'o1'})",
        no_params(),
    )
    .unwrap();
    let rows = zega
        .query(
            "MATCH (u:User {id: 'u1'})-[:PLACED]->(o:Order) RETURN o.id AS id",
            no_params(),
        )
        .unwrap();
    assert_eq!(one_field(&rows, "id"), Value::String("o1".into()));
}

// ===========================================================================
// 3. RETURN projections, aliases, property access
// ===========================================================================

#[test]
fn return_property_access_projects_scalar() {
    let zega = db();
    zega.query("CREATE (n:Person {name: 'Alice', age: 41})", no_params())
        .unwrap();
    let rows = zega
        .query("MATCH (n:Person) RETURN n.age AS age", no_params())
        .unwrap();
    assert_eq!(one_field(&rows, "age"), Value::Int(41));
}

#[test]
fn return_alias_names_the_output_field() {
    let zega = db();
    zega.query("CREATE (n:Person {name: 'Alice'})", no_params())
        .unwrap();
    let rows = zega
        .query("MATCH (n:Person) RETURN n.name AS who", no_params())
        .unwrap();
    assert!(rows[0].fields.contains_key("who"));
    assert!(!rows[0].fields.contains_key("name"));
}

#[test]
fn return_without_alias_uses_expr_string_as_field_name() {
    let zega = db();
    zega.query("CREATE (n:Person {name: 'Alice'})", no_params())
        .unwrap();
    let rows = zega
        .query("MATCH (n:Person) RETURN n.name", no_params())
        .unwrap();
    // expr_to_string yields "n.name" for the unaliased field key.
    assert_eq!(
        rows[0].fields.get("n.name"),
        Some(&Value::String("Alice".into()))
    );
}

#[test]
fn return_multiple_items_produces_all_fields() {
    let zega = db();
    zega.query("CREATE (n:Person {name: 'Alice', age: 30})", no_params())
        .unwrap();
    let rows = zega
        .query(
            "MATCH (n:Person) RETURN n.name AS name, n.age AS age",
            no_params(),
        )
        .unwrap();
    assert_eq!(rows[0].fields.get("name"), Some(&Value::String("Alice".into())));
    assert_eq!(rows[0].fields.get("age"), Some(&Value::Int(30)));
}

#[test]
fn return_missing_property_yields_null() {
    let zega = db();
    zega.query("CREATE (n:Person {name: 'Alice'})", no_params())
        .unwrap();
    let rows = zega
        .query("MATCH (n:Person) RETURN n.nonexistent AS missing", no_params())
        .unwrap();
    assert_eq!(one_field(&rows, "missing"), Value::Null);
}

#[test]
fn return_node_id_pseudo_property() {
    let zega = db();
    zega.query("CREATE (n:Person {name: 'Alice'})", no_params())
        .unwrap();
    let rows = zega
        .query("MATCH (n:Person) RETURN n.id AS id", no_params())
        .unwrap();
    assert!(matches!(one_field(&rows, "id"), Value::Int(_)));
}

// ===========================================================================
// 4. WHERE: comparison operators
// ===========================================================================

fn seed_numbers(zega: &Zega) {
    for v in [1_i64, 2, 3, 4, 5] {
        zega.query(
            "CREATE (n:Num {v: $v})",
            params(&[("v", Value::Int(v))]),
        )
        .unwrap();
    }
}

#[test]
fn where_eq_matches_exact_value() {
    let zega = db();
    seed_numbers(&zega);
    let rows = zega
        .query("MATCH (n:Num) WHERE n.v = 3 RETURN n.v AS v", no_params())
        .unwrap();
    assert_eq!(one_field(&rows, "v"), Value::Int(3));
}

#[test]
fn where_ne_excludes_value() {
    let zega = db();
    seed_numbers(&zega);
    let rows = zega
        .query("MATCH (n:Num) WHERE n.v != 3 RETURN n.v AS v", no_params())
        .unwrap();
    assert_eq!(rows.len(), 4);
    assert!(!collect_field(&rows, "v").contains(&Value::Int(3)));
}

#[test]
fn where_gt_strictly_greater() {
    let zega = db();
    seed_numbers(&zega);
    let rows = zega
        .query("MATCH (n:Num) WHERE n.v > 3 RETURN n.v AS v", no_params())
        .unwrap();
    let mut got: Vec<_> = collect_field(&rows, "v");
    got.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(got, vec![Value::Int(4), Value::Int(5)]);
}

#[test]
fn where_lt_strictly_less() {
    let zega = db();
    seed_numbers(&zega);
    let rows = zega
        .query("MATCH (n:Num) WHERE n.v < 3 RETURN n.v AS v", no_params())
        .unwrap();
    assert_eq!(rows.len(), 2);
}

#[test]
fn where_gte_includes_boundary() {
    let zega = db();
    seed_numbers(&zega);
    let rows = zega
        .query("MATCH (n:Num) WHERE n.v >= 3 RETURN n.v AS v", no_params())
        .unwrap();
    assert_eq!(rows.len(), 3);
}

#[test]
fn where_lte_includes_boundary() {
    let zega = db();
    seed_numbers(&zega);
    let rows = zega
        .query("MATCH (n:Num) WHERE n.v <= 3 RETURN n.v AS v", no_params())
        .unwrap();
    assert_eq!(rows.len(), 3);
}

#[test]
fn where_string_comparison_lexicographic() {
    let zega = db();
    for s in ["apple", "banana", "cherry"] {
        zega.query(
            "CREATE (n:Word {w: $w})",
            params(&[("w", Value::String(s.into()))]),
        )
        .unwrap();
    }
    let rows = zega
        .query(
            "MATCH (n:Word) WHERE n.w > 'apple' RETURN n.w AS w",
            no_params(),
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
}

#[test]
fn where_bool_equality() {
    let zega = db();
    zega.query("CREATE (n:Flag {active: true})", no_params())
        .unwrap();
    zega.query("CREATE (n:Flag {active: false})", no_params())
        .unwrap();
    let rows = zega
        .query(
            "MATCH (n:Flag) WHERE n.active = true RETURN n.active AS a",
            no_params(),
        )
        .unwrap();
    assert_eq!(one_field(&rows, "a"), Value::Bool(true));
}

// ===========================================================================
// 5. WHERE: boolean operators (AND / OR) + precedence
// ===========================================================================

fn seed_people(zega: &Zega) {
    let people = [
        ("Alice", 30, "gold"),
        ("Bob", 25, "silver"),
        ("Carol", 40, "gold"),
        ("Dan", 18, "bronze"),
    ];
    for (name, age, tier) in people {
        zega.query(
            "CREATE (n:Person {name: $name, age: $age, tier: $tier})",
            params(&[
                ("name", Value::String(name.into())),
                ("age", Value::Int(age)),
                ("tier", Value::String(tier.into())),
            ]),
        )
        .unwrap();
    }
}

#[test]
fn where_and_requires_both_conditions() {
    let zega = db();
    seed_people(&zega);
    let rows = zega
        .query(
            "MATCH (n:Person) WHERE n.tier = 'gold' AND n.age > 35 RETURN n.name AS name",
            no_params(),
        )
        .unwrap();
    assert_eq!(one_field(&rows, "name"), Value::String("Carol".into()));
}

#[test]
fn where_or_matches_either_condition() {
    let zega = db();
    seed_people(&zega);
    let rows = zega
        .query(
            "MATCH (n:Person) WHERE n.age < 20 OR n.age > 38 RETURN n.name AS name",
            no_params(),
        )
        .unwrap();
    let names: HashSet<_> = collect_field(&rows, "name").into_iter().collect();
    assert_eq!(
        names,
        HashSet::from([
            Value::String("Dan".into()),
            Value::String("Carol".into())
        ])
    );
}

#[test]
fn where_and_binds_tighter_than_or() {
    // a AND b OR c  ==  (a AND b) OR c
    let zega = db();
    seed_people(&zega);
    let rows = zega
        .query(
            "MATCH (n:Person) WHERE n.tier = 'gold' AND n.age > 35 OR n.tier = 'silver' RETURN n.name AS name",
            no_params(),
        )
        .unwrap();
    let names: HashSet<_> = collect_field(&rows, "name").into_iter().collect();
    assert_eq!(
        names,
        HashSet::from([
            Value::String("Carol".into()),
            Value::String("Bob".into())
        ])
    );
}

#[test]
fn where_parenthesized_groups_override_precedence() {
    let zega = db();
    seed_people(&zega);
    // (gold OR silver) AND age < 30 -> only Bob (silver, 25)
    let rows = zega
        .query(
            "MATCH (n:Person) WHERE (n.tier = 'gold' OR n.tier = 'silver') AND n.age < 30 RETURN n.name AS name",
            no_params(),
        )
        .unwrap();
    assert_eq!(one_field(&rows, "name"), Value::String("Bob".into()));
}

#[test]
fn where_false_filters_all_rows() {
    let zega = db();
    seed_people(&zega);
    let rows = zega
        .query(
            "MATCH (n:Person) WHERE n.tier = 'nonexistent' RETURN n",
            no_params(),
        )
        .unwrap();
    assert!(rows.is_empty());
}

#[test]
fn where_compares_two_bound_node_properties() {
    let zega = db();
    zega.query("CREATE (n:T {a: 5, b: 5})", no_params())
        .unwrap();
    zega.query("CREATE (n:T {a: 5, b: 9})", no_params())
        .unwrap();
    let rows = zega
        .query("MATCH (n:T) WHERE n.a = n.b RETURN n.a AS a", no_params())
        .unwrap();
    assert_eq!(one_field(&rows, "a"), Value::Int(5));
}

// ===========================================================================
// 6. Type coercion / mismatched comparisons (Neo4j-style, partial_cmp None)
// ===========================================================================

#[test]
fn ordered_comparison_across_int_and_float_yields_no_match() {
    // partial_cmp returns None for Int vs Float; Gt therefore false.
    let zega = db();
    zega.query("CREATE (n:M {v: 3})", no_params()).unwrap();
    let rows = zega
        .query("MATCH (n:M) WHERE n.v > 2.0 RETURN n", no_params())
        .unwrap();
    assert!(
        rows.is_empty(),
        "Int vs Float ordered comparison is not comparable"
    );
}

#[test]
fn equality_across_int_and_float_is_false() {
    // 3 (Int) != 3.0 (Float) because Value equality is by-variant.
    let zega = db();
    zega.query("CREATE (n:M {v: 3})", no_params()).unwrap();
    let rows = zega
        .query("MATCH (n:M) WHERE n.v = 3.0 RETURN n", no_params())
        .unwrap();
    assert!(rows.is_empty());
}

#[test]
fn equality_across_string_and_int_is_false() {
    let zega = db();
    zega.query("CREATE (n:M {v: 3})", no_params()).unwrap();
    let rows = zega
        .query("MATCH (n:M) WHERE n.v = '3' RETURN n", no_params())
        .unwrap();
    assert!(rows.is_empty());
}

#[test]
fn float_to_float_ordered_comparison_works() {
    let zega = db();
    zega.query("CREATE (n:M {v: 2.5})", no_params()).unwrap();
    let rows = zega
        .query("MATCH (n:M) WHERE n.v > 1.5 RETURN n", no_params())
        .unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn and_coerces_non_bool_operand_to_false() {
    // n.name is a String, not a bool; `as_bool().unwrap_or(false)` -> false,
    // so `n.name AND true` is false for every row.
    let zega = db();
    zega.query("CREATE (n:P {name: 'x'})", no_params()).unwrap();
    let rows = zega
        .query(
            "MATCH (n:P) WHERE n.name AND n.name = 'x' RETURN n",
            no_params(),
        )
        .unwrap();
    assert!(rows.is_empty());
}

// ===========================================================================
// 7. NULL / Option handling
// ===========================================================================

#[test]
fn missing_parameter_resolves_to_null_and_filters_out() {
    let zega = db();
    zega.query("CREATE (n:P {name: 'Alice'})", no_params())
        .unwrap();
    // $missing is not provided; resolves to Null; name = Null is false.
    let rows = zega
        .query("MATCH (n:P) WHERE n.name = $missing RETURN n", no_params())
        .unwrap();
    assert!(rows.is_empty());
}

#[test]
fn explicit_null_literal_equality_is_false_against_value() {
    let zega = db();
    zega.query("CREATE (n:P {name: 'Alice'})", no_params())
        .unwrap();
    let rows = zega
        .query("MATCH (n:P) WHERE n.name = null RETURN n", no_params())
        .unwrap();
    assert!(rows.is_empty());
}

#[test]
fn ordered_comparison_with_null_is_false() {
    let zega = db();
    zega.query("CREATE (n:P {name: 'Alice'})", no_params())
        .unwrap();
    let rows = zega
        .query("MATCH (n:P) WHERE n.absent > 3 RETURN n", no_params())
        .unwrap();
    assert!(rows.is_empty());
}

// ===========================================================================
// 8. ORDER BY
// ===========================================================================

fn seed_scores(zega: &Zega) {
    let data = [("a", 3), ("b", 1), ("c", 2), ("d", 1)];
    for (name, score) in data {
        zega.query(
            "CREATE (n:S {name: $name, score: $score})",
            params(&[
                ("name", Value::String(name.into())),
                ("score", Value::Int(score)),
            ]),
        )
        .unwrap();
    }
}

#[test]
fn order_by_ascending_sorts_low_to_high() {
    let zega = db();
    seed_scores(&zega);
    let rows = zega
        .query(
            "MATCH (n:S) RETURN n.score AS score ORDER BY n.score ASC",
            no_params(),
        )
        .unwrap();
    let got = collect_field(&rows, "score");
    assert_eq!(
        got,
        vec![Value::Int(1), Value::Int(1), Value::Int(2), Value::Int(3)]
    );
}

#[test]
fn order_by_descending_sorts_high_to_low() {
    let zega = db();
    seed_scores(&zega);
    let rows = zega
        .query(
            "MATCH (n:S) RETURN n.score AS score ORDER BY n.score DESC",
            no_params(),
        )
        .unwrap();
    let got = collect_field(&rows, "score");
    assert_eq!(
        got,
        vec![Value::Int(3), Value::Int(2), Value::Int(1), Value::Int(1)]
    );
}

#[test]
fn order_by_defaults_to_ascending() {
    let zega = db();
    seed_scores(&zega);
    let rows = zega
        .query(
            "MATCH (n:S) RETURN n.score AS score ORDER BY n.score",
            no_params(),
        )
        .unwrap();
    let got = collect_field(&rows, "score");
    assert_eq!(got.first(), Some(&Value::Int(1)));
    assert_eq!(got.last(), Some(&Value::Int(3)));
}

#[test]
fn order_by_multi_key_secondary_breaks_ties() {
    let zega = db();
    seed_scores(&zega);
    // Primary score ASC, secondary name DESC. Tie at score=1 between b and d.
    let rows = zega
        .query(
            "MATCH (n:S) RETURN n.name AS name, n.score AS score ORDER BY n.score ASC, n.name DESC",
            no_params(),
        )
        .unwrap();
    let names = collect_field(&rows, "name");
    // score 1: d before b (name DESC); then c (2); then a (3).
    assert_eq!(
        names,
        vec![
            Value::String("d".into()),
            Value::String("b".into()),
            Value::String("c".into()),
            Value::String("a".into()),
        ]
    );
}

#[test]
fn order_by_string_field() {
    let zega = db();
    seed_scores(&zega);
    let rows = zega
        .query(
            "MATCH (n:S) RETURN n.name AS name ORDER BY n.name ASC",
            no_params(),
        )
        .unwrap();
    let names = collect_field(&rows, "name");
    assert_eq!(
        names,
        vec![
            Value::String("a".into()),
            Value::String("b".into()),
            Value::String("c".into()),
            Value::String("d".into()),
        ]
    );
}

#[test]
fn order_by_return_alias() {
    let zega = db();
    seed_scores(&zega);
    let rows = zega
        .query(
            "MATCH (n:S) RETURN n.name AS name, n.score AS score ORDER BY score DESC",
            no_params(),
        )
        .unwrap();
    assert_eq!(
        collect_field(&rows, "score"),
        vec![Value::Int(3), Value::Int(2), Value::Int(1), Value::Int(1)]
    );
}

#[test]
fn order_by_nulls_last_ascending_and_first_descending() {
    let zega = db();
    zega.query("CREATE (n:S {score: 2})", no_params()).unwrap();
    zega.query("CREATE (n:S)", no_params()).unwrap();
    zega.query("CREATE (n:S {score: 1})", no_params()).unwrap();

    let ascending = zega
        .query(
            "MATCH (n:S) RETURN n.score AS score ORDER BY n.score ASC",
            no_params(),
        )
        .unwrap();
    assert_eq!(
        collect_field(&ascending, "score"),
        vec![Value::Int(1), Value::Int(2), Value::Null]
    );

    let descending = zega
        .query(
            "MATCH (n:S) RETURN n.score AS score ORDER BY n.score DESC",
            no_params(),
        )
        .unwrap();
    assert_eq!(
        collect_field(&descending, "score"),
        vec![Value::Null, Value::Int(2), Value::Int(1)]
    );
}

// ===========================================================================
// 9. LIMIT
// ===========================================================================

#[test]
fn limit_truncates_result_set() {
    let zega = db();
    seed_scores(&zega);
    let rows = zega
        .query(
            "MATCH (n:S) RETURN n.score AS score ORDER BY n.score ASC LIMIT 2",
            no_params(),
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(collect_field(&rows, "score"), vec![Value::Int(1), Value::Int(1)]);
}

#[test]
fn limit_larger_than_result_set_returns_all() {
    let zega = db();
    seed_scores(&zega);
    let rows = zega
        .query("MATCH (n:S) RETURN n LIMIT 100", no_params())
        .unwrap();
    assert_eq!(rows.len(), 4);
}

#[test]
fn limit_zero_returns_no_rows() {
    let zega = db();
    seed_scores(&zega);
    let rows = zega
        .query("MATCH (n:S) RETURN n LIMIT 0", no_params())
        .unwrap();
    assert!(rows.is_empty());
}

#[test]
fn limit_via_parameter() {
    let zega = db();
    seed_scores(&zega);
    let rows = zega
        .query(
            "MATCH (n:S) RETURN n LIMIT $lim",
            params(&[("lim", Value::Int(1))]),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
}

// ===========================================================================
// 10. Aggregations
// ===========================================================================

fn seed_orders(zega: &Zega) {
    // 3 orders owned by 2 users.
    let u1 = "u1";
    let u2 = "u2";
    zega.query(
        "CREATE (u:User {id: $u, tier: 'gold'})",
        params(&[("u", Value::String(u1.into()))]),
    )
    .unwrap();
    zega.query(
        "CREATE (u:User {id: $u, tier: 'silver'})",
        params(&[("u", Value::String(u2.into()))]),
    )
    .unwrap();
    let orders = [("u1", 10, "book"), ("u1", 20, "book"), ("u2", 30, "game")];
    for (owner, total, cat) in orders {
        zega.query(
            "MATCH (u:User {id: $u}) CREATE (u)-[:PLACED]->(o:Order {total: $t, category: $c})",
            params(&[
                ("u", Value::String(owner.into())),
                ("t", Value::Int(total)),
                ("c", Value::String(cat.into())),
            ]),
        )
        .unwrap();
    }
}

#[test]
fn aggregate_count_star_counts_all_rows() {
    let zega = db();
    seed_orders(&zega);
    let rows = zega
        .query("MATCH (o:Order) RETURN count(*) AS c", no_params())
        .unwrap();
    assert_eq!(one_field(&rows, "c"), Value::Int(3));
}

#[test]
fn aggregate_count_variable_counts_bound_rows() {
    let zega = db();
    seed_orders(&zega);
    let rows = zega
        .query("MATCH (o:Order) RETURN count(o) AS c", no_params())
        .unwrap();
    assert_eq!(one_field(&rows, "c"), Value::Int(3));
}

#[test]
fn aggregate_count_on_empty_match_is_zero() {
    let zega = db();
    let rows = zega
        .query("MATCH (o:Order) RETURN count(*) AS c", no_params())
        .unwrap();
    assert_eq!(one_field(&rows, "c"), Value::Int(0));
}

#[test]
fn aggregate_sum_of_ints() {
    let zega = db();
    seed_orders(&zega);
    let rows = zega
        .query("MATCH (o:Order) RETURN sum(o.total) AS s", no_params())
        .unwrap();
    assert_eq!(one_field(&rows, "s"), Value::Int(60));
}

#[test]
fn aggregate_avg_returns_float() {
    let zega = db();
    seed_orders(&zega);
    let rows = zega
        .query("MATCH (o:Order) RETURN avg(o.total) AS a", no_params())
        .unwrap();
    assert_eq!(one_field(&rows, "a").to_f64(), Some(20.0));
}

#[test]
fn aggregate_min_and_max() {
    let zega = db();
    seed_orders(&zega);
    let rows = zega
        .query(
            "MATCH (o:Order) RETURN min(o.total) AS mn, max(o.total) AS mx",
            no_params(),
        )
        .unwrap();
    assert_eq!(rows[0].fields.get("mn"), Some(&Value::Int(10)));
    assert_eq!(rows[0].fields.get("mx"), Some(&Value::Int(30)));
}

#[test]
fn aggregate_collect_gathers_values_into_list() {
    let zega = db();
    seed_orders(&zega);
    let rows = zega
        .query("MATCH (o:Order) RETURN collect(o.total) AS list", no_params())
        .unwrap();
    let Value::List(mut list) = one_field(&rows, "list") else {
        panic!("collect should return a list");
    };
    list.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(list, vec![Value::Int(10), Value::Int(20), Value::Int(30)]);
}

#[test]
fn aggregate_count_distinct_counts_unique_values() {
    let zega = db();
    seed_orders(&zega);
    let rows = zega
        .query(
            "MATCH (o:Order) RETURN count(DISTINCT o.category) AS c",
            no_params(),
        )
        .unwrap();
    assert_eq!(one_field(&rows, "c"), Value::Int(2));
}

#[test]
fn aggregate_sum_with_float_promotes_result_to_float() {
    let zega = db();
    zega.query("CREATE (n:N {v: 1})", no_params()).unwrap();
    zega.query("CREATE (n:N {v: 2.5})", no_params()).unwrap();
    let rows = zega
        .query("MATCH (n:N) RETURN sum(n.v) AS s", no_params())
        .unwrap();
    assert_eq!(one_field(&rows, "s").to_f64(), Some(3.5));
}

#[test]
fn aggregate_sum_skips_null_values() {
    let zega = db();
    zega.query("CREATE (n:N {v: 5})", no_params()).unwrap();
    zega.query("CREATE (n:N {other: 1})", no_params()).unwrap(); // no v -> Null
    let rows = zega
        .query("MATCH (n:N) RETURN sum(n.v) AS s", no_params())
        .unwrap();
    assert_eq!(one_field(&rows, "s"), Value::Int(5));
}

#[test]
fn aggregate_avg_of_all_null_is_null() {
    let zega = db();
    zega.query("CREATE (n:N {other: 1})", no_params()).unwrap();
    let rows = zega
        .query("MATCH (n:N) RETURN avg(n.v) AS a", no_params())
        .unwrap();
    assert_eq!(one_field(&rows, "a"), Value::Null);
}

#[test]
fn aggregate_sum_of_non_numeric_is_execution_error() {
    let zega = db();
    zega.query("CREATE (n:N {v: 'hello'})", no_params())
        .unwrap();
    let err = zega
        .query("MATCH (n:N) RETURN sum(n.v) AS s", no_params())
        .unwrap_err();
    assert!(matches!(err, ZegaError::Execution(_)));
}

#[test]
fn aggregate_min_max_of_empty_set_is_null() {
    let zega = db();
    let rows = zega
        .query(
            "MATCH (n:N) RETURN min(n.v) AS mn, max(n.v) AS mx",
            no_params(),
        )
        .unwrap();
    // No bindings -> empty group still produces a single aggregate row of nulls.
    assert_eq!(rows[0].fields.get("mn"), Some(&Value::Null));
    assert_eq!(rows[0].fields.get("mx"), Some(&Value::Null));
}

// ===========================================================================
// 11. GROUP BY (implicit grouping by non-aggregate RETURN keys)
// ===========================================================================

#[test]
fn group_by_single_key_counts_per_group() {
    let zega = db();
    seed_orders(&zega);
    let rows = zega
        .query(
            "MATCH (u:User)-[:PLACED]->(o:Order) RETURN u.id AS user, count(o) AS c",
            no_params(),
        )
        .unwrap();
    let counts: HashMap<_, _> = rows
        .iter()
        .map(|r| (r.fields["user"].clone(), r.fields["c"].clone()))
        .collect();
    assert_eq!(counts[&Value::String("u1".into())], Value::Int(2));
    assert_eq!(counts[&Value::String("u2".into())], Value::Int(1));
}

#[test]
fn group_by_order_by_count_star_then_mixed_key_matches_neo4j() {
    let zega = db();
    for key in [
        Value::String("str".into()),
        Value::String("str".into()),
        Value::Bool(false),
        Value::Bool(false),
        Value::Bool(false),
        Value::Int(1),
        Value::Int(1),
    ] {
        zega.query("CREATE (n:G {k: $k})", params(&[("k", key)]))
            .unwrap();
    }
    zega.query("CREATE (n:G)", no_params()).unwrap();

    let rows = zega
        .query(
            "MATCH (n:G) RETURN n.k AS k, count(*) AS c ORDER BY count(*) ASC, n.k ASC",
            no_params(),
        )
        .unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| (row.fields["k"].clone(), row.fields["c"].clone()))
            .collect::<Vec<_>>(),
        vec![
            (Value::Null, Value::Int(1)),
            (Value::String("str".into()), Value::Int(2)),
            (Value::Int(1), Value::Int(2)),
            (Value::Bool(false), Value::Int(3)),
        ]
    );
}

#[test]
fn group_by_with_sum_aggregates_per_group() {
    let zega = db();
    seed_orders(&zega);
    let rows = zega
        .query(
            "MATCH (u:User)-[:PLACED]->(o:Order) RETURN u.id AS user, sum(o.total) AS total",
            no_params(),
        )
        .unwrap();
    let totals: HashMap<_, _> = rows
        .iter()
        .map(|r| (r.fields["user"].clone(), r.fields["total"].clone()))
        .collect();
    assert_eq!(totals[&Value::String("u1".into())], Value::Int(30));
    assert_eq!(totals[&Value::String("u2".into())], Value::Int(30));
}

#[test]
fn group_by_multiple_keys() {
    let zega = db();
    // Two distinct (tier, category) groupings.
    zega.query("CREATE (n:R {tier: 'a', cat: 'x', v: 1})", no_params())
        .unwrap();
    zega.query("CREATE (n:R {tier: 'a', cat: 'x', v: 2})", no_params())
        .unwrap();
    zega.query("CREATE (n:R {tier: 'a', cat: 'y', v: 4})", no_params())
        .unwrap();
    let rows = zega
        .query(
            "MATCH (n:R) RETURN n.tier AS tier, n.cat AS cat, sum(n.v) AS s",
            no_params(),
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    let groups: HashMap<_, _> = rows
        .iter()
        .map(|r| ((r.fields["tier"].clone(), r.fields["cat"].clone()), r.fields["s"].clone()))
        .collect();
    assert_eq!(
        groups[&(Value::String("a".into()), Value::String("x".into()))],
        Value::Int(3)
    );
    assert_eq!(
        groups[&(Value::String("a".into()), Value::String("y".into()))],
        Value::Int(4)
    );
}

#[test]
fn where_filters_before_aggregation() {
    let zega = db();
    seed_orders(&zega);
    let rows = zega
        .query(
            "MATCH (u:User)-[:PLACED]->(o:Order) WHERE o.total > 15 RETURN u.id AS user, count(o) AS c",
            no_params(),
        )
        .unwrap();
    let counts: HashMap<_, _> = rows
        .iter()
        .map(|r| (r.fields["user"].clone(), r.fields["c"].clone()))
        .collect();
    assert_eq!(counts[&Value::String("u1".into())], Value::Int(1));
    assert_eq!(counts[&Value::String("u2".into())], Value::Int(1));
}

// ===========================================================================
// 12. Relationship traversal
// ===========================================================================

fn seed_chain(zega: &Zega) {
    // a -> b -> c -> d, anonymous-labeled nodes with id property.
    zega.query(
        "CREATE (a {id: 'a'})-[:R]->(b {id: 'b'})",
        no_params(),
    )
    .unwrap();
    zega.query(
        "MATCH (b {id: 'b'}) CREATE (b)-[:R]->(c {id: 'c'})",
        no_params(),
    )
    .unwrap();
    zega.query(
        "MATCH (c {id: 'c'}) CREATE (c)-[:R]->(d {id: 'd'})",
        no_params(),
    )
    .unwrap();
}

fn sorted_ids(rows: &[Row], var: &str) -> Vec<String> {
    let mut ids: Vec<String> = rows
        .iter()
        .filter_map(|r| map_field(r, var, "id"))
        .filter_map(|v| v.as_string().map(str::to_string))
        .collect();
    ids.sort();
    ids
}

#[test]
fn single_hop_outgoing_traversal() {
    let zega = db();
    seed_chain(&zega);
    let rows = zega
        .query("MATCH (a {id: 'a'})-[:R]->(x) RETURN x", no_params())
        .unwrap();
    assert_eq!(sorted_ids(&rows, "x"), vec!["b"]);
}

#[test]
fn variable_length_traversal_returns_all_endpoints() {
    let zega = db();
    seed_chain(&zega);
    let rows = zega
        .query("MATCH (a {id: 'a'})-[:R*1..3]->(x) RETURN x", no_params())
        .unwrap();
    assert_eq!(sorted_ids(&rows, "x"), vec!["b", "c", "d"]);
}

#[test]
fn variable_length_traversal_exact_length() {
    let zega = db();
    seed_chain(&zega);
    let rows = zega
        .query("MATCH (a {id: 'a'})-[:R*2..2]->(x) RETURN x", no_params())
        .unwrap();
    assert_eq!(sorted_ids(&rows, "x"), vec!["c"]);
}

#[test]
fn unbounded_traversal_with_cycle_terminates_via_uniqueness() {
    let zega = db();
    zega.query("CREATE (a {id: 'a'})-[:R]->(b {id: 'b'})", no_params())
        .unwrap();
    zega.query(
        "MATCH (b {id: 'b'}) CREATE (b)-[:R]->(c {id: 'c'})",
        no_params(),
    )
    .unwrap();
    zega.query(
        "MATCH (c {id: 'c'}) MATCH (a {id: 'a'}) CREATE (c)-[:R]->(a)",
        no_params(),
    )
    .unwrap();
    let rows = zega
        .query("MATCH (a {id: 'a'})-[:R*]->(x) RETURN x", no_params())
        .unwrap();
    assert_eq!(sorted_ids(&rows, "x"), vec!["a", "b", "c"]);
}

#[test]
fn traversal_work_budget_exceeded_is_error_value() {
    let zega = Zega::in_memory().traversal_work_budget(2).build().unwrap();
    seed_chain(&zega);
    let err = zega
        .query("MATCH (a {id: 'a'})-[:R*]->(x) RETURN x", no_params())
        .unwrap_err();
    assert!(matches!(
        err,
        ZegaError::TraversalWorkBudgetExceeded { limit: 2 }
    ));
}

#[test]
fn return_relationship_variable_is_map_with_metadata() {
    let zega = db();
    zega.query(
        "CREATE (a {id: 'a'})-[:R {weight: 7}]->(b {id: 'b'})",
        no_params(),
    )
    .unwrap();
    let rows = zega
        .query("MATCH (a {id: 'a'})-[r:R]->(b) RETURN r", no_params())
        .unwrap();
    let Value::Map(m) = one_field(&rows, "r") else {
        panic!("relationship variable should be a map");
    };
    assert_eq!(m.get("type"), Some(&Value::String("R".into())));
    assert_eq!(m.get("weight"), Some(&Value::Int(7)));
    assert!(matches!(m.get("id"), Some(Value::Int(_))));
}

#[test]
fn incoming_direction_traversal() {
    let zega = db();
    seed_chain(&zega);
    let rows = zega
        .query("MATCH (b {id: 'b'})<-[:R]-(x) RETURN x", no_params())
        .unwrap();
    assert_eq!(sorted_ids(&rows, "x"), vec!["a"]);
}

// ===========================================================================
// 13. KV operations: SQL surface + helper methods
// ===========================================================================

#[test]
fn kv_set_and_get_via_query() {
    let zega = db();
    zega.query("SET KEY 'k' = 'v'", no_params()).unwrap();
    let rows = zega.query("GET KEY 'k'", no_params()).unwrap();
    assert_eq!(one_field(&rows, "value"), Value::String("v".into()));
}

#[test]
fn kv_get_missing_key_returns_null_value() {
    let zega = db();
    let rows = zega.query("GET KEY 'absent'", no_params()).unwrap();
    assert_eq!(one_field(&rows, "value"), Value::Null);
}

#[test]
fn kv_set_via_parameters() {
    let zega = db();
    zega.query(
        "SET KEY $k = $v",
        params(&[
            ("k", Value::String("session".into())),
            ("v", Value::Int(42)),
        ]),
    )
    .unwrap();
    let rows = zega
        .query("GET KEY $k", params(&[("k", Value::String("session".into()))]))
        .unwrap();
    assert_eq!(one_field(&rows, "value"), Value::Int(42));
}

#[test]
fn kv_del_via_query_removes_value() {
    let zega = db();
    zega.query("SET KEY 'k' = 'v'", no_params()).unwrap();
    zega.query("DEL KEY 'k'", no_params()).unwrap();
    let rows = zega.query("GET KEY 'k'", no_params()).unwrap();
    assert_eq!(one_field(&rows, "value"), Value::Null);
}

#[test]
fn kv_incr_on_new_key_starts_at_one() {
    let zega = db();
    let rows = zega.query("INCR KEY 'counter'", no_params()).unwrap();
    assert_eq!(one_field(&rows, "value"), Value::Int(1));
}

#[test]
fn kv_incr_increments_existing_int() {
    let zega = db();
    zega.query("SET KEY 'counter' = 5", no_params()).unwrap();
    let rows = zega.query("INCR KEY 'counter'", no_params()).unwrap();
    assert_eq!(one_field(&rows, "value"), Value::Int(6));
}

#[test]
fn kv_incr_on_non_int_returns_null() {
    let zega = db();
    zega.query("SET KEY 'k' = 'not-a-number'", no_params())
        .unwrap();
    let rows = zega.query("INCR KEY 'k'", no_params()).unwrap();
    assert_eq!(one_field(&rows, "value"), Value::Null);
}

#[test]
fn kv_helper_methods_set_get_del() {
    let zega = db();
    zega.kv_set("hk".into(), Value::String("hv".into()), None)
        .unwrap();
    assert_eq!(zega.kv_get("hk"), Some(Value::String("hv".into())));
    assert!(zega.kv_del("hk").unwrap());
    assert_eq!(zega.kv_get("hk"), None);
}

#[test]
fn kv_helper_del_missing_returns_false() {
    let zega = db();
    assert!(!zega.kv_del("never-existed").unwrap());
}

#[test]
fn kv_set_overwrites_previous_value() {
    let zega = db();
    zega.query("SET KEY 'k' = 'first'", no_params()).unwrap();
    zega.query("SET KEY 'k' = 'second'", no_params()).unwrap();
    let rows = zega.query("GET KEY 'k'", no_params()).unwrap();
    assert_eq!(one_field(&rows, "value"), Value::String("second".into()));
}

// ===========================================================================
// 14. MERGE
// ===========================================================================

#[test]
fn merge_creates_node_when_absent() {
    let zega = db();
    zega.query("MERGE (n:Tag {name: 'rust'})", no_params())
        .unwrap();
    let rows = zega
        .query("MATCH (n:Tag) RETURN n.name AS name", no_params())
        .unwrap();
    assert_eq!(one_field(&rows, "name"), Value::String("rust".into()));
}

#[test]
fn merge_does_not_duplicate_existing_node() {
    let zega = db();
    zega.query("MERGE (n:Tag {name: 'rust'})", no_params())
        .unwrap();
    zega.query("MERGE (n:Tag {name: 'rust'})", no_params())
        .unwrap();
    let rows = zega.query("MATCH (n:Tag) RETURN n", no_params()).unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn merge_on_create_sets_extra_property() {
    let zega = db();
    zega.query(
        "MERGE (n:Tag {name: 'rust'}) ON CREATE SET n.created = true",
        no_params(),
    )
    .unwrap();
    let rows = zega
        .query("MATCH (n:Tag) RETURN n.created AS created", no_params())
        .unwrap();
    assert_eq!(one_field(&rows, "created"), Value::Bool(true));
}

// ===========================================================================
// 15. MATCH ... CREATE
// ===========================================================================

#[test]
fn match_create_reuses_matched_node_as_relationship_source() {
    let zega = db();
    zega.query("CREATE (u:User {id: 'u1'})", no_params())
        .unwrap();
    zega.query(
        "MATCH (u:User {id: 'u1'}) CREATE (u)-[:PLACED]->(o:Order {id: 'o1'})",
        no_params(),
    )
    .unwrap();
    let rows = zega
        .query(
            "MATCH (u:User {id: 'u1'})-[:PLACED]->(o:Order) RETURN o.id AS id",
            no_params(),
        )
        .unwrap();
    assert_eq!(one_field(&rows, "id"), Value::String("o1".into()));
}

#[test]
fn match_create_on_no_match_creates_nothing() {
    let zega = db();
    zega.query(
        "MATCH (u:User {id: 'nope'}) CREATE (u)-[:PLACED]->(o:Order {id: 'o1'})",
        no_params(),
    )
    .unwrap();
    let rows = zega.query("MATCH (o:Order) RETURN o", no_params()).unwrap();
    assert!(rows.is_empty());
}

// ===========================================================================
// 16. Error-as-value: malformed queries never panic
// ===========================================================================

#[test]
fn unbalanced_parens_is_parse_error() {
    let zega = db();
    let err = zega.query("MATCH (n:Person RETURN n", no_params()).unwrap_err();
    assert!(matches!(err, ZegaError::Parse(_)));
}

#[test]
fn garbage_input_is_parse_error_not_panic() {
    let zega = db();
    let err = zega
        .query("$$$ not a query @@@", no_params())
        .unwrap_err();
    assert!(matches!(err, ZegaError::Parse(_)));
}

#[test]
fn unknown_function_is_parse_error() {
    let zega = db();
    let err = zega
        .query("MATCH (n:N) RETURN frobnicate(n.v)", no_params())
        .unwrap_err();
    assert!(matches!(err, ZegaError::Parse(_)));
}

#[test]
fn relationship_length_min_exceeds_max_is_parse_error() {
    let zega = db();
    let err = zega
        .query("MATCH (a)-[:R*5..2]->(b) RETURN b", no_params())
        .unwrap_err();
    assert!(matches!(err, ZegaError::Parse(_)));
}

#[test]
fn where_with_no_expression_is_parse_error() {
    let zega = db();
    let err = zega
        .query("MATCH (n:Person) WHERE RETURN n", no_params())
        .unwrap_err();
    assert!(matches!(err, ZegaError::Parse(_)));
}

#[test]
fn match_without_return_clause_returns_rows_with_empty_fields() {
    // A bare MATCH (no RETURN) is valid: return_clause defaults to empty,
    // so each matched binding yields a Row with no fields (not an error).
    let zega = db();
    zega.query("CREATE (n:Person {name: 'Alice'})", no_params())
        .unwrap();
    let rows = zega.query("MATCH (n:Person)", no_params()).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].fields.is_empty());
}

#[test]
fn star_argument_on_non_count_is_parse_error() {
    let zega = db();
    let err = zega
        .query("MATCH (n:N) RETURN sum(*)", no_params())
        .unwrap_err();
    assert!(matches!(err, ZegaError::Parse(_)));
}

// ===========================================================================
// 17. Context resolution
// ===========================================================================

#[test]
fn query_with_anonymous_context_behaves_like_query() {
    let zega = db();
    zega.query("CREATE (n:Person {name: 'Alice'})", no_params())
        .unwrap();
    let rows = zega
        .query_with_context(
            "MATCH (n:Person) RETURN n",
            no_params(),
            ZegaContext::anonymous(),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn query_with_system_context_succeeds_without_jwt_config() {
    let zega = db();
    zega.query("CREATE (n:Person {name: 'Alice'})", no_params())
        .unwrap();
    let rows = zega
        .query_with_context(
            "MATCH (n:Person) RETURN n",
            no_params(),
            ZegaContext::system(),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn query_with_claims_context_succeeds_without_jwt_config() {
    let zega = db();
    zega.query("CREATE (n:Person {name: 'Alice'})", no_params())
        .unwrap();
    let rows = zega
        .query_with_context(
            "MATCH (n:Person) RETURN n",
            no_params(),
            ZegaContext::claims(claims(&[("role", "admin")])),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn jwt_context_without_config_is_jwt_error() {
    let zega = db(); // no jwt config configured
    let err = zega
        .query_with_context(
            "MATCH (n:Person) RETURN n",
            no_params(),
            ZegaContext::jwt("some.token.here"),
        )
        .unwrap_err();
    assert!(matches!(err, ZegaError::Jwt(_)));
}

#[test]
fn jwt_context_with_invalid_token_is_error() {
    let zega = Zega::in_memory()
        .jwt_hmac_secret(b"secret".to_vec())
        .build()
        .unwrap();
    let err = zega
        .query_with_context(
            "MATCH (n:Person) RETURN n",
            no_params(),
            ZegaContext::jwt("not-a-valid-jwt"),
        )
        .unwrap_err();
    assert!(matches!(err, ZegaError::Jwt(_)));
}

// ===========================================================================
// 18. Policy enforcement (label + KV)
// ===========================================================================

fn create_doc(zega: &Zega, name: &str, org: &str) {
    zega.query(
        "CREATE (n:Doc {name: $name, org_id: $org})",
        params(&[
            ("name", Value::String(name.into())),
            ("org", Value::String(org.into())),
        ]),
    )
    .unwrap();
}

#[test]
fn policy_tenant_filter_scopes_results_by_context() {
    let zega = Zega::in_memory()
        .policy(
            "doc_tenant",
            PolicyTargets::Labels(vec!["Doc".into()]),
            PolicyCondition::AllowWhen(PolicyExpr::Eq(
                ExprValue::ContextField(".org_id".into()),
                ExprValue::NodeField("node.org_id".into()),
            )),
        )
        .build()
        .unwrap();
    create_doc(&zega, "doc1", "org1");
    create_doc(&zega, "doc2", "org2");

    let rows = zega
        .query_with_context(
            "MATCH (n:Doc) RETURN n.name AS name",
            no_params(),
            ZegaContext::claims(claims(&[("org_id", "org1")])),
        )
        .unwrap();
    assert_eq!(one_field(&rows, "name"), Value::String("doc1".into()));
}

#[test]
fn policy_system_context_bypasses_tenant_filter() {
    let zega = Zega::in_memory()
        .policy(
            "doc_tenant",
            PolicyTargets::Labels(vec!["Doc".into()]),
            PolicyCondition::AllowWhen(PolicyExpr::Eq(
                ExprValue::ContextField(".org_id".into()),
                ExprValue::NodeField("node.org_id".into()),
            )),
        )
        .build()
        .unwrap();
    create_doc(&zega, "doc1", "org1");
    create_doc(&zega, "doc2", "org2");

    let rows = zega
        .query_with_context(
            "MATCH (n:Doc) RETURN n",
            no_params(),
            ZegaContext::system(),
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
}

#[test]
fn policy_system_only_denies_non_system_context() {
    let zega = Zega::in_memory()
        .policy(
            "secret",
            PolicyTargets::Labels(vec!["Secret".into()]),
            PolicyCondition::SystemOnly,
        )
        .build()
        .unwrap();
    zega.query("CREATE (n:Secret {name: 's'})", no_params())
        .unwrap();
    let err = zega
        .query_with_context(
            "MATCH (n:Secret) RETURN n",
            no_params(),
            ZegaContext::anonymous(),
        )
        .unwrap_err();
    assert!(matches!(err, ZegaError::PermissionDenied(_)));
}

#[test]
fn policy_system_only_allows_system_context() {
    let zega = Zega::in_memory()
        .policy(
            "secret",
            PolicyTargets::Labels(vec!["Secret".into()]),
            PolicyCondition::SystemOnly,
        )
        .build()
        .unwrap();
    zega.query("CREATE (n:Secret {name: 's'})", no_params())
        .unwrap();
    let rows = zega
        .query_with_context(
            "MATCH (n:Secret) RETURN n",
            no_params(),
            ZegaContext::system(),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn policy_role_in_list_allows_matching_role() {
    let zega = Zega::in_memory()
        .policy(
            "doc_roles",
            PolicyTargets::Labels(vec!["Doc".into()]),
            PolicyCondition::AllowWhen(PolicyExpr::In(
                ExprValue::ContextField(".role".into()),
                ExprValue::Literal(Value::List(vec![
                    Value::String("admin".into()),
                    Value::String("owner".into()),
                ])),
            )),
        )
        .build()
        .unwrap();
    create_doc(&zega, "doc1", "org1");

    let admin = zega
        .query_with_context(
            "MATCH (n:Doc) RETURN n",
            no_params(),
            ZegaContext::claims(claims(&[("role", "admin")])),
        )
        .unwrap();
    let viewer = zega
        .query_with_context(
            "MATCH (n:Doc) RETURN n",
            no_params(),
            ZegaContext::claims(claims(&[("role", "viewer")])),
        )
        .unwrap();
    assert_eq!(admin.len(), 1);
    assert!(viewer.is_empty());
}

#[test]
fn policy_kv_prefix_filter_blocks_other_tenants_keys() {
    let zega = Zega::in_memory()
        .policy(
            "kv_tenant",
            PolicyTargets::Kv,
            PolicyCondition::AllowWhen(PolicyExpr::Eq(
                ExprValue::NodeField("key_prefix".into()),
                ExprValue::ContextField(".org_id".into()),
            )),
        )
        .build()
        .unwrap();
    zega.query("SET KEY 'org1:file' = 'allowed'", no_params())
        .unwrap();
    zega.query("SET KEY 'org2:file' = 'denied'", no_params())
        .unwrap();

    let allowed = zega
        .query_with_context(
            "GET KEY 'org1:file'",
            no_params(),
            ZegaContext::claims(claims(&[("org_id", "org1")])),
        )
        .unwrap();
    let denied = zega
        .query_with_context(
            "GET KEY 'org2:file'",
            no_params(),
            ZegaContext::claims(claims(&[("org_id", "org1")])),
        )
        .unwrap();
    assert_eq!(allowed[0].fields.get("value"), Some(&Value::String("allowed".into())));
    assert_eq!(denied[0].fields.get("value"), Some(&Value::Null));
}

// ===========================================================================
// 19. Multi-statement queries
// ===========================================================================

#[test]
fn multiple_statements_in_one_query_string_execute_in_sequence() {
    let zega = db();
    // CREATE produces no rows; MATCH RETURN does. Results are concatenated.
    let rows = zega
        .query(
            "CREATE (n:P {name: 'Alice'}) MATCH (n:P) RETURN n.name AS name",
            no_params(),
        )
        .unwrap();
    assert_eq!(one_field(&rows, "name"), Value::String("Alice".into()));
}

// ===========================================================================
// 20. Unicode / large / boundary data
// ===========================================================================

#[test]
fn unicode_string_property_round_trips() {
    let zega = db();
    let p = params(&[("name", Value::String("日本語🚀café".into()))]);
    zega.query("CREATE (n:P {name: $name})", p.clone()).unwrap();
    let rows = zega
        .query("MATCH (n:P {name: $name}) RETURN n.name AS name", p)
        .unwrap();
    assert_eq!(one_field(&rows, "name"), Value::String("日本語🚀café".into()));
}

#[test]
fn large_integer_value_round_trips() {
    let zega = db();
    let p = params(&[("v", Value::Int(i64::MAX))]);
    zega.query("CREATE (n:P {v: $v})", p.clone()).unwrap();
    let rows = zega
        .query("MATCH (n:P {v: $v}) RETURN n.v AS v", p)
        .unwrap();
    assert_eq!(one_field(&rows, "v"), Value::Int(i64::MAX));
}

#[test]
fn sum_integer_overflow_is_execution_error() {
    let zega = db();
    zega.query(
        "CREATE (n:N {v: $v})",
        params(&[("v", Value::Int(i64::MAX))]),
    )
    .unwrap();
    zega.query("CREATE (n:N {v: 1})", no_params()).unwrap();
    let err = zega
        .query("MATCH (n:N) RETURN sum(n.v) AS s", no_params())
        .unwrap_err();
    assert!(matches!(err, ZegaError::Execution(_)));
}

#[test]
fn empty_string_property_is_distinct_from_missing() {
    let zega = db();
    let p = params(&[("name", Value::String(String::new()))]);
    zega.query("CREATE (n:P {name: $name})", p.clone()).unwrap();
    let rows = zega
        .query("MATCH (n:P {name: $name}) RETURN n.name AS name", p)
        .unwrap();
    assert_eq!(one_field(&rows, "name"), Value::String(String::new()));
}

#[test]
fn many_nodes_count_is_accurate() {
    let zega = db();
    for i in 0..500 {
        zega.query(
            "CREATE (n:Bulk {i: $i})",
            params(&[("i", Value::Int(i))]),
        )
        .unwrap();
    }
    let rows = zega
        .query("MATCH (n:Bulk) RETURN count(*) AS c", no_params())
        .unwrap();
    assert_eq!(one_field(&rows, "c"), Value::Int(500));
}

// ===========================================================================
// 21. WAL recovery / snapshot (on-disk persistence)
// ===========================================================================

#[test]
fn wal_recovery_restores_nodes_and_kv_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_str().unwrap();
    {
        let zega = Zega::open(path).wal_flush_every_write().build().unwrap();
        zega.query("CREATE (n:Person {name: 'Alice'})", no_params())
            .unwrap();
        zega.query("SET KEY 'k' = 'v'", no_params()).unwrap();
    }
    {
        let zega = Zega::open(path).wal_flush_every_write().build().unwrap();
        let rows = zega
            .query("MATCH (n:Person) RETURN n", no_params())
            .unwrap();
        assert_eq!(rows.len(), 1);
        let kv = zega.query("GET KEY 'k'", no_params()).unwrap();
        assert_eq!(kv[0].fields.get("value"), Some(&Value::String("v".into())));
    }
}

#[test]
fn snapshot_then_reopen_restores_graph() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_str().unwrap();
    {
        let zega = Zega::open(path).wal_flush_every_write().build().unwrap();
        zega.query("CREATE (n:Person {name: 'Alice'})", no_params())
            .unwrap();
        zega.snapshot().unwrap();
    }
    {
        let zega = Zega::open(path).wal_flush_every_write().build().unwrap();
        let rows = zega
            .query("MATCH (n:Person) RETURN n", no_params())
            .unwrap();
        assert_eq!(rows.len(), 1);
    }
}

#[test]
fn snapshot_on_in_memory_db_is_noop_ok() {
    let zega = db();
    zega.query("CREATE (n:P {name: 'x'})", no_params()).unwrap();
    assert!(zega.snapshot().is_ok());
}

// ===========================================================================
// APPENDED: adversarial-completeness pass
//
// Everything below was added by the completeness critic to close gaps in the
// original suite. Same precision rules: exact signatures from source, errors
// asserted as values (never panics), boundary + round-trip coverage.
// ===========================================================================

// ---------------------------------------------------------------------------
// JWT verification: full happy-path round-trips + every error branch in
// `jwt::verify` (reached through the public `ZegaContext::Jwt` surface).
//
// These mint real tokens so the HS256 signature/claims path actually runs,
// rather than only exercising the "no config" / "obviously invalid" branches.
// ---------------------------------------------------------------------------

mod jwt_support {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    type HmacSha256 = Hmac<Sha256>;

    /// Build an HS256 JWT from a serde_json payload object, signing with `secret`.
    pub fn make_hs256(secret: &[u8], payload: &serde_json::Value) -> String {
        let header = serde_json::json!({ "alg": "HS256", "typ": "JWT" });
        let header_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).unwrap());
        let payload_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(payload).unwrap());
        let signing_input = format!("{header_b64}.{payload_b64}");
        let mut mac = HmacSha256::new_from_slice(secret).unwrap();
        mac.update(signing_input.as_bytes());
        let sig = mac.finalize().into_bytes();
        let sig_b64 = URL_SAFE_NO_PAD.encode(sig);
        format!("{signing_input}.{sig_b64}")
    }

    /// Build a token with an attacker-controlled header `alg`, still signed with
    /// HMAC over the (header, payload) — used to probe alg/key mismatch branches.
    pub fn make_with_alg(alg: &str, secret: &[u8], payload: &serde_json::Value) -> String {
        let header = serde_json::json!({ "alg": alg, "typ": "JWT" });
        let header_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).unwrap());
        let payload_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(payload).unwrap());
        let signing_input = format!("{header_b64}.{payload_b64}");
        let mut mac = HmacSha256::new_from_slice(secret).unwrap();
        mac.update(signing_input.as_bytes());
        let sig_b64 = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        format!("{signing_input}.{sig_b64}")
    }

    pub fn now_secs() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }
}

#[test]
fn jwt_valid_hs256_token_resolves_claims_into_policy_context() {
    let secret = b"super-secret-key";
    // A policy that only passes when the resolved claim org_id matches the node.
    let zega = Zega::in_memory()
        .jwt_hmac_secret(secret.to_vec())
        .policy(
            "doc_tenant",
            PolicyTargets::Labels(vec!["Doc".into()]),
            PolicyCondition::AllowWhen(PolicyExpr::Eq(
                ExprValue::ContextField(".org_id".into()),
                ExprValue::NodeField("node.org_id".into()),
            )),
        )
        .build()
        .unwrap();
    create_doc(&zega, "doc1", "org1");
    create_doc(&zega, "doc2", "org2");

    let token = jwt_support::make_hs256(
        secret,
        &serde_json::json!({ "org_id": "org1", "sub": "user-1" }),
    );
    let rows = zega
        .query_with_context(
            "MATCH (n:Doc) RETURN n.name AS name",
            no_params(),
            ZegaContext::jwt(&token),
        )
        .unwrap();
    // Only the org1 doc is visible: proves the claim was actually decoded.
    assert_eq!(one_field(&rows, "name"), Value::String("doc1".into()));
}

#[test]
fn jwt_valid_token_with_matching_issuer_is_accepted() {
    let secret = b"k";
    let zega = Zega::in_memory()
        .jwt_issuer("tana")
        .jwt_hmac_secret(secret.to_vec())
        .build()
        .unwrap();
    zega.query("CREATE (n:Doc {name: 'd', org_id: 'o'})", no_params())
        .unwrap();
    let token = jwt_support::make_hs256(secret, &serde_json::json!({ "iss": "tana" }));
    let rows = zega
        .query_with_context("MATCH (n:Doc) RETURN n", no_params(), ZegaContext::jwt(&token))
        .unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn jwt_issuer_mismatch_is_jwt_error() {
    let secret = b"k";
    let zega = Zega::in_memory()
        .jwt_issuer("tana")
        .jwt_hmac_secret(secret.to_vec())
        .build()
        .unwrap();
    let token = jwt_support::make_hs256(secret, &serde_json::json!({ "iss": "evil" }));
    let err = zega
        .query_with_context("MATCH (n) RETURN n", no_params(), ZegaContext::jwt(&token))
        .unwrap_err();
    assert!(matches!(err, ZegaError::Jwt(_)));
}

#[test]
fn jwt_missing_issuer_when_required_is_jwt_error() {
    let secret = b"k";
    let zega = Zega::in_memory()
        .jwt_issuer("tana")
        .jwt_hmac_secret(secret.to_vec())
        .build()
        .unwrap();
    let token = jwt_support::make_hs256(secret, &serde_json::json!({ "sub": "x" }));
    let err = zega
        .query_with_context("MATCH (n) RETURN n", no_params(), ZegaContext::jwt(&token))
        .unwrap_err();
    assert!(matches!(err, ZegaError::Jwt(_)));
}

#[test]
fn jwt_expired_token_is_jwt_error() {
    let secret = b"k";
    let zega = Zega::in_memory()
        .jwt_hmac_secret(secret.to_vec())
        .build()
        .unwrap();
    let past = jwt_support::now_secs().saturating_sub(10_000);
    let token = jwt_support::make_hs256(secret, &serde_json::json!({ "exp": past }));
    let err = zega
        .query_with_context("MATCH (n) RETURN n", no_params(), ZegaContext::jwt(&token))
        .unwrap_err();
    assert!(matches!(err, ZegaError::Jwt(_)));
}

#[test]
fn jwt_unexpired_token_with_future_exp_is_accepted() {
    let secret = b"k";
    let zega = Zega::in_memory()
        .jwt_hmac_secret(secret.to_vec())
        .build()
        .unwrap();
    zega.query("CREATE (n:Doc {name: 'd'})", no_params()).unwrap();
    let future = jwt_support::now_secs() + 10_000;
    let token = jwt_support::make_hs256(secret, &serde_json::json!({ "exp": future }));
    let rows = zega
        .query_with_context("MATCH (n:Doc) RETURN n", no_params(), ZegaContext::jwt(&token))
        .unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn jwt_signature_signed_with_wrong_secret_is_jwt_error() {
    let zega = Zega::in_memory()
        .jwt_hmac_secret(b"right-secret".to_vec())
        .build()
        .unwrap();
    // Token minted with a different secret -> HMAC verify fails.
    let token = jwt_support::make_hs256(b"wrong-secret", &serde_json::json!({ "sub": "x" }));
    let err = zega
        .query_with_context("MATCH (n) RETURN n", no_params(), ZegaContext::jwt(&token))
        .unwrap_err();
    assert!(matches!(err, ZegaError::Jwt(_)));
}

#[test]
fn jwt_rs256_alg_against_hmac_key_is_jwt_error() {
    let secret = b"k";
    let zega = Zega::in_memory()
        .jwt_hmac_secret(secret.to_vec())
        .build()
        .unwrap();
    // header claims RS256 but the configured key is HMAC -> mismatch branch.
    let token = jwt_support::make_with_alg("RS256", secret, &serde_json::json!({ "sub": "x" }));
    let err = zega
        .query_with_context("MATCH (n) RETURN n", no_params(), ZegaContext::jwt(&token))
        .unwrap_err();
    assert!(matches!(err, ZegaError::Jwt(_)));
}

#[test]
fn jwt_unsupported_alg_is_jwt_error() {
    let secret = b"k";
    let zega = Zega::in_memory()
        .jwt_hmac_secret(secret.to_vec())
        .build()
        .unwrap();
    let token = jwt_support::make_with_alg("none", secret, &serde_json::json!({ "sub": "x" }));
    let err = zega
        .query_with_context("MATCH (n) RETURN n", no_params(), ZegaContext::jwt(&token))
        .unwrap_err();
    assert!(matches!(err, ZegaError::Jwt(_)));
}

#[test]
fn jwt_wrong_number_of_segments_is_jwt_error() {
    let secret = b"k";
    let zega = Zega::in_memory()
        .jwt_hmac_secret(secret.to_vec())
        .build()
        .unwrap();
    for bad in ["only-one-part", "two.parts", "a.b.c.d"] {
        let err = zega
            .query_with_context("MATCH (n) RETURN n", no_params(), ZegaContext::jwt(bad))
            .unwrap_err();
        assert!(matches!(err, ZegaError::Jwt(_)), "token {bad:?} should error");
    }
}

#[test]
fn jwt_non_base64url_segment_is_jwt_error() {
    let secret = b"k";
    let zega = Zega::in_memory()
        .jwt_hmac_secret(secret.to_vec())
        .build()
        .unwrap();
    // '!' is not in the base64url alphabet.
    let err = zega
        .query_with_context("MATCH (n) RETURN n", no_params(), ZegaContext::jwt("!!!.!!!.!!!"))
        .unwrap_err();
    assert!(matches!(err, ZegaError::Jwt(_)));
}

#[test]
fn jwt_array_and_numeric_claims_decode_into_zega_values() {
    let secret = b"k";
    let zega = Zega::in_memory()
        .jwt_hmac_secret(secret.to_vec())
        // role IN [admin] policy proves a JSON array claim became a Zega list
        // that the IN-lowering can match against.
        .policy(
            "roles",
            PolicyTargets::Labels(vec!["Doc".into()]),
            PolicyCondition::AllowWhen(PolicyExpr::In(
                ExprValue::ContextField(".role".into()),
                ExprValue::Literal(Value::List(vec![Value::String("admin".into())])),
            )),
        )
        .build()
        .unwrap();
    create_doc(&zega, "doc1", "org1");
    let token = jwt_support::make_hs256(
        secret,
        &serde_json::json!({ "role": "admin", "level": 7, "ratio": 1.5 }),
    );
    let rows = zega
        .query_with_context("MATCH (n:Doc) RETURN n", no_params(), ZegaContext::jwt(&token))
        .unwrap();
    assert_eq!(rows.len(), 1);
}

// ---------------------------------------------------------------------------
// Relationship pseudo-properties via property access (bound_property branch:
// id / type / from / to / props), plus relationship props passthrough.
// ---------------------------------------------------------------------------

fn seed_one_edge(zega: &Zega) {
    // (a {id:'a'}) -[:R {weight:7}]-> (b {id:'b'})
    zega.query("CREATE (a:N {id: 'a'})", no_params()).unwrap();
    zega.query("CREATE (b:N {id: 'b'})", no_params()).unwrap();
    zega.query(
        "MATCH (a:N {id: 'a'}) MATCH (b:N {id: 'b'}) CREATE (a)-[:R {weight: 7}]->(b)",
        no_params(),
    )
    .unwrap();
}

#[test]
fn relationship_pseudo_property_type_via_property_access() {
    let zega = db();
    seed_one_edge(&zega);
    let rows = zega
        .query(
            "MATCH (a:N {id: 'a'})-[r:R]->(b:N) RETURN r.type AS t",
            no_params(),
        )
        .unwrap();
    assert_eq!(one_field(&rows, "t"), Value::String("R".into()));
}

#[test]
fn relationship_pseudo_property_from_and_to_are_node_ids() {
    let zega = db();
    seed_one_edge(&zega);
    let rows = zega
        .query(
            "MATCH (a:N {id: 'a'})-[r:R]->(b:N) RETURN r.from AS f, r.to AS t",
            no_params(),
        )
        .unwrap();
    // from/to are i64 node ids; exact values are graph-assigned but must be ints.
    assert!(matches!(one_field(&rows, "f"), Value::Int(_)));
    assert!(matches!(one_field(&rows, "t"), Value::Int(_)));
}

#[test]
fn relationship_user_property_via_property_access() {
    let zega = db();
    seed_one_edge(&zega);
    let rows = zega
        .query(
            "MATCH (a:N {id: 'a'})-[r:R]->(b:N) RETURN r.weight AS w",
            no_params(),
        )
        .unwrap();
    assert_eq!(one_field(&rows, "w"), Value::Int(7));
}

#[test]
fn relationship_props_pseudo_property_is_map_of_user_props() {
    let zega = db();
    seed_one_edge(&zega);
    let rows = zega
        .query(
            "MATCH (a:N {id: 'a'})-[r:R]->(b:N) RETURN r.props AS p",
            no_params(),
        )
        .unwrap();
    match one_field(&rows, "p") {
        Value::Map(m) => assert_eq!(m.get("weight"), Some(&Value::Int(7))),
        other => panic!("expected map of relationship props, got {other:?}"),
    }
}

#[test]
fn relationship_property_filter_in_pattern_selects_matching_edge() {
    let zega = db();
    zega.query("CREATE (a:N {id: 'a'})", no_params()).unwrap();
    zega.query("CREATE (b:N {id: 'b'})", no_params()).unwrap();
    zega.query("CREATE (c:N {id: 'c'})", no_params()).unwrap();
    zega.query(
        "MATCH (a:N {id: 'a'}) MATCH (b:N {id: 'b'}) CREATE (a)-[:R {weight: 1}]->(b)",
        no_params(),
    )
    .unwrap();
    zega.query(
        "MATCH (a:N {id: 'a'}) MATCH (c:N {id: 'c'}) CREATE (a)-[:R {weight: 9}]->(c)",
        no_params(),
    )
    .unwrap();
    // Only the weight=9 edge endpoint should come back.
    let rows = zega
        .query(
            "MATCH (a:N {id: 'a'})-[:R {weight: 9}]->(x:N) RETURN x.id AS id",
            no_params(),
        )
        .unwrap();
    assert_eq!(one_field(&rows, "id"), Value::String("c".into()));
}

#[test]
fn relationship_wrong_kind_in_pattern_yields_no_match() {
    let zega = db();
    seed_one_edge(&zega);
    let rows = zega
        .query(
            "MATCH (a:N {id: 'a'})-[:NONSUCH]->(b:N) RETURN b",
            no_params(),
        )
        .unwrap();
    assert!(rows.is_empty());
}

// ---------------------------------------------------------------------------
// Direction::Both and undirected variable-length traversal.
// ---------------------------------------------------------------------------

#[test]
fn undirected_single_hop_matches_both_orientations() {
    let zega = db();
    seed_one_edge(&zega); // a -> b
    // Starting from b, an undirected hop should still reach a.
    let rows = zega
        .query(
            "MATCH (b:N {id: 'b'})-[:R]-(other:N) RETURN other.id AS id",
            no_params(),
        )
        .unwrap();
    assert_eq!(one_field(&rows, "id"), Value::String("a".into()));
}

#[test]
fn undirected_single_hop_from_source_also_matches() {
    let zega = db();
    seed_one_edge(&zega); // a -> b
    let rows = zega
        .query(
            "MATCH (a:N {id: 'a'})-[:R]-(other:N) RETURN other.id AS id",
            no_params(),
        )
        .unwrap();
    assert_eq!(one_field(&rows, "id"), Value::String("b".into()));
}

#[test]
fn undirected_variable_length_traverses_against_edge_direction() {
    let zega = db();
    // chain a -> b -> c (all outgoing)
    for id in ["a", "b", "c"] {
        zega.query(
            "CREATE (n:N {id: $id})",
            params(&[("id", Value::String(id.into()))]),
        )
        .unwrap();
    }
    zega.query(
        "MATCH (a:N {id: 'a'}) MATCH (b:N {id: 'b'}) CREATE (a)-[:R]->(b)",
        no_params(),
    )
    .unwrap();
    zega.query(
        "MATCH (b:N {id: 'b'}) MATCH (c:N {id: 'c'}) CREATE (b)-[:R]->(c)",
        no_params(),
    )
    .unwrap();
    // From c, undirected *1..2 should reach b (1 hop) and a (2 hops).
    let rows = zega
        .query(
            "MATCH (c:N {id: 'c'})-[:R*1..2]-(x:N) RETURN x.id AS id",
            no_params(),
        )
        .unwrap();
    let ids = collect_field(&rows, "id")
        .into_iter()
        .collect::<HashSet<_>>();
    assert_eq!(
        ids,
        HashSet::from([Value::String("a".into()), Value::String("b".into())])
    );
}

#[test]
fn unbounded_star_traversal_reaches_all_reachable_endpoints() {
    let zega = db();
    for id in ["a", "b", "c", "d"] {
        zega.query(
            "CREATE (n:N {id: $id})",
            params(&[("id", Value::String(id.into()))]),
        )
        .unwrap();
    }
    for (from, to) in [("a", "b"), ("b", "c"), ("c", "d")] {
        zega.query(
            "MATCH (f:N {id: $f}) MATCH (t:N {id: $t}) CREATE (f)-[:R]->(t)",
            params(&[
                ("f", Value::String(from.into())),
                ("t", Value::String(to.into())),
            ]),
        )
        .unwrap();
    }
    // `*` defaults to min 1, unbounded max.
    let rows = zega
        .query(
            "MATCH (a:N {id: 'a'})-[:R*]->(x:N) RETURN x.id AS id",
            no_params(),
        )
        .unwrap();
    let ids = collect_field(&rows, "id")
        .into_iter()
        .collect::<HashSet<_>>();
    assert_eq!(
        ids,
        HashSet::from([
            Value::String("b".into()),
            Value::String("c".into()),
            Value::String("d".into()),
        ])
    );
}

// ---------------------------------------------------------------------------
// ORDER BY across value types: floats, bools, and incomparable values.
// ---------------------------------------------------------------------------

#[test]
fn order_by_float_field_sorts_numerically() {
    let zega = db();
    for v in [2.5_f64, 0.5, 1.5] {
        zega.query(
            "CREATE (n:F {v: $v})",
            params(&[("v", Value::from_f64(v))]),
        )
        .unwrap();
    }
    let rows = zega
        .query("MATCH (n:F) RETURN n.v AS v ORDER BY n.v ASC", no_params())
        .unwrap();
    let got: Vec<f64> = collect_field(&rows, "v")
        .into_iter()
        .map(|v| v.to_f64().unwrap())
        .collect();
    assert_eq!(got, vec![0.5, 1.5, 2.5]);
}

#[test]
fn order_by_bool_field_orders_false_before_true() {
    let zega = db();
    for b in [true, false, true] {
        zega.query(
            "CREATE (n:B {flag: $b})",
            params(&[("b", Value::Bool(b))]),
        )
        .unwrap();
    }
    let rows = zega
        .query(
            "MATCH (n:B) RETURN n.flag AS flag ORDER BY n.flag ASC",
            no_params(),
        )
        .unwrap();
    let got = collect_field(&rows, "flag");
    assert_eq!(got[0], Value::Bool(false));
    assert_eq!(*got.last().unwrap(), Value::Bool(true));
}

#[test]
fn order_by_descending_float() {
    let zega = db();
    for v in [1.0_f64, 3.0, 2.0] {
        zega.query("CREATE (n:F {v: $v})", params(&[("v", Value::from_f64(v))]))
            .unwrap();
    }
    let rows = zega
        .query("MATCH (n:F) RETURN n.v AS v ORDER BY n.v DESC", no_params())
        .unwrap();
    let got: Vec<f64> = collect_field(&rows, "v")
        .into_iter()
        .map(|v| v.to_f64().unwrap())
        .collect();
    assert_eq!(got, vec![3.0, 2.0, 1.0]);
}

#[test]
fn order_by_mixed_types_uses_neo4j_orderability_with_null_last() {
    let zega = db();
    for (id, value) in [
        ("int", Value::Int(2)),
        ("str", Value::String("x".into())),
        ("bool", Value::Bool(false)),
        ("float", Value::from_f64(1.5)),
    ] {
        zega.query(
            "CREATE (n:M {id: $id, k: $k})",
            params(&[("id", Value::String(id.into())), ("k", value)]),
        )
        .unwrap();
    }
    zega.query("CREATE (n:M {id: 'null'})", no_params())
        .unwrap();

    let ascending = zega
        .query("MATCH (n:M) RETURN n.k AS k ORDER BY n.k ASC", no_params())
        .unwrap();
    assert_eq!(
        collect_field(&ascending, "k"),
        vec![
            Value::String("x".into()),
            Value::Bool(false),
            Value::from_f64(1.5),
            Value::Int(2),
            Value::Null,
        ]
    );

    let descending = zega
        .query("MATCH (n:M) RETURN n.k AS k ORDER BY n.k DESC", no_params())
        .unwrap();
    assert_eq!(
        collect_field(&descending, "k"),
        vec![
            Value::Null,
            Value::Int(2),
            Value::from_f64(1.5),
            Value::Bool(false),
            Value::String("x".into()),
        ]
    );
}

#[test]
fn order_by_int_and_float_compares_exact_numeric_values() {
    let zega = db();
    for (id, key) in [
        ("int_hi", Value::Int(9_007_199_254_740_993)),
        ("float_lo", Value::from_f64(9_007_199_254_740_992.0)),
        ("int_neg", Value::Int(-9_007_199_254_740_993)),
        ("float_neg", Value::from_f64(-9_007_199_254_740_992.0)),
    ] {
        zega.query(
            "CREATE (n:M {id: $id, k: $k})",
            params(&[("id", Value::String(id.into())), ("k", key)]),
        )
        .unwrap();
    }
    let rows = zega
        .query(
            "MATCH (n:M) RETURN n.id AS id ORDER BY n.k ASC",
            no_params(),
        )
        .unwrap();
    assert_eq!(
        collect_field(&rows, "id"),
        ["int_neg", "float_neg", "float_lo", "int_hi"]
            .into_iter()
            .map(|id| Value::String(id.into()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn order_by_mixed_primary_and_secondary_directions_match_neo4j() {
    let zega = db();
    for (id, key, tie) in [
        ("s_b", Value::String("same".into()), 2),
        ("s_a", Value::String("same".into()), 1),
        ("b_b", Value::Bool(false), 2),
        ("b_a", Value::Bool(false), 1),
        ("n_b", Value::Int(1), 2),
        ("n_a", Value::from_f64(1.0), 1),
    ] {
        zega.query(
            "CREATE (n:M {id: $id, k: $k, tie: $tie})",
            params(&[
                ("id", Value::String(id.into())),
                ("k", key),
                ("tie", Value::Int(tie)),
            ]),
        )
        .unwrap();
    }
    let rows = zega
        .query(
            "MATCH (n:M) RETURN n.id AS id ORDER BY n.k ASC, n.tie DESC",
            no_params(),
        )
        .unwrap();
    assert_eq!(
        collect_field(&rows, "id"),
        ["s_b", "s_a", "b_b", "b_a", "n_b", "n_a"]
            .into_iter()
            .map(|id| Value::String(id.into()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn order_by_maps_and_lists_precede_scalar_values() {
    let zega = db();
    let mut map = HashMap::new();
    map.insert("a".into(), Value::Int(1));
    for (id, key) in [
        ("map", Value::Map(map)),
        ("list", Value::List(vec![Value::Int(1)])),
        ("str", Value::String("a".into())),
        ("bool", Value::Bool(false)),
        ("num", Value::Int(1)),
    ] {
        zega.query(
            "CREATE (n:M {id: $id, k: $k})",
            params(&[("id", Value::String(id.into())), ("k", key)]),
        )
        .unwrap();
    }
    let rows = zega
        .query(
            "MATCH (n:M) RETURN n.id AS id ORDER BY n.k ASC",
            no_params(),
        )
        .unwrap();
    assert_eq!(
        collect_field(&rows, "id"),
        ["map", "list", "str", "bool", "num"]
            .into_iter()
            .map(|id| Value::String(id.into()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn order_by_then_limit_applies_limit_after_sort() {
    let zega = db();
    for v in [5_i64, 1, 3, 2, 4] {
        zega.query("CREATE (n:O {v: $v})", params(&[("v", Value::Int(v))]))
            .unwrap();
    }
    let rows = zega
        .query(
            "MATCH (n:O) RETURN n.v AS v ORDER BY n.v ASC LIMIT 2",
            no_params(),
        )
        .unwrap();
    assert_eq!(collect_field(&rows, "v"), vec![Value::Int(1), Value::Int(2)]);
}

#[test]
fn limit_with_non_int_value_does_not_truncate() {
    // LIMIT only truncates when the expression evaluates to Value::Int; a float
    // limit is a no-op per exec_match.
    let zega = db();
    for v in [1_i64, 2, 3] {
        zega.query("CREATE (n:L {v: $v})", params(&[("v", Value::Int(v))]))
            .unwrap();
    }
    let rows = zega
        .query(
            "MATCH (n:L) RETURN n.v AS v LIMIT $lim",
            params(&[("lim", Value::from_f64(2.0))]),
        )
        .unwrap();
    assert_eq!(rows.len(), 3);
}

// ---------------------------------------------------------------------------
// WHERE: Ne across mismatched types, OR short-circuit-style truth tables,
// and relationship-property predicates.
// ---------------------------------------------------------------------------

#[test]
fn where_ne_across_mismatched_types_is_true() {
    // Int 1 != String "1" -> the row passes (Ne uses structural inequality).
    let zega = db();
    zega.query("CREATE (n:T {v: 1})", no_params()).unwrap();
    let rows = zega
        .query(
            "MATCH (n:T) WHERE n.v != $s RETURN n",
            params(&[("s", Value::String("1".into()))]),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn where_or_passes_when_only_second_disjunct_true() {
    let zega = db();
    zega.query("CREATE (n:T {a: 1, b: 2})", no_params()).unwrap();
    let rows = zega
        .query(
            "MATCH (n:T) WHERE n.a = $x OR n.b = $y RETURN n",
            params(&[("x", Value::Int(99)), ("y", Value::Int(2))]),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn where_on_relationship_property_filters_traversal() {
    let zega = db();
    zega.query("CREATE (a:N {id: 'a'})", no_params()).unwrap();
    zega.query("CREATE (b:N {id: 'b'})", no_params()).unwrap();
    zega.query("CREATE (c:N {id: 'c'})", no_params()).unwrap();
    zega.query(
        "MATCH (a:N {id: 'a'}) MATCH (b:N {id: 'b'}) CREATE (a)-[:R {w: 1}]->(b)",
        no_params(),
    )
    .unwrap();
    zega.query(
        "MATCH (a:N {id: 'a'}) MATCH (c:N {id: 'c'}) CREATE (a)-[:R {w: 5}]->(c)",
        no_params(),
    )
    .unwrap();
    let rows = zega
        .query(
            "MATCH (a:N {id: 'a'})-[r:R]->(x:N) WHERE r.w > $min RETURN x.id AS id",
            params(&[("min", Value::Int(3))]),
        )
        .unwrap();
    assert_eq!(one_field(&rows, "id"), Value::String("c".into()));
}

// ---------------------------------------------------------------------------
// Aggregations: DISTINCT variants routing through the generic path, min/max
// over strings, collect with null-skipping and DISTINCT, avg(DISTINCT).
// ---------------------------------------------------------------------------

fn seed_amounts(zega: &Zega, label: &str, amounts: &[Value]) {
    for a in amounts {
        zega.query(
            &format!("CREATE (n:{label} {{a: $a}})"),
            params(&[("a", a.clone())]),
        )
        .unwrap();
    }
}

#[test]
fn aggregate_sum_distinct_sums_unique_values_only() {
    let zega = db();
    seed_amounts(
        &zega,
        "S",
        &[Value::Int(2), Value::Int(2), Value::Int(3)],
    );
    let rows = zega
        .query("MATCH (n:S) RETURN sum(DISTINCT n.a) AS s", no_params())
        .unwrap();
    // distinct {2,3} -> 5, not 7.
    assert_eq!(one_field(&rows, "s"), Value::Int(5));
}

#[test]
fn aggregate_avg_distinct_averages_unique_values() {
    let zega = db();
    seed_amounts(
        &zega,
        "A",
        &[Value::Int(2), Value::Int(2), Value::Int(4)],
    );
    let rows = zega
        .query("MATCH (n:A) RETURN avg(DISTINCT n.a) AS avg", no_params())
        .unwrap();
    // distinct {2,4} -> 3.0
    assert_eq!(one_field(&rows, "avg").to_f64(), Some(3.0));
}

#[test]
fn aggregate_min_max_over_strings_uses_lexicographic_order() {
    let zega = db();
    seed_amounts(
        &zega,
        "Str",
        &[
            Value::String("banana".into()),
            Value::String("apple".into()),
            Value::String("cherry".into()),
        ],
    );
    let rows = zega
        .query(
            "MATCH (n:Str) RETURN min(n.a) AS lo, max(n.a) AS hi",
            no_params(),
        )
        .unwrap();
    assert_eq!(one_field(&rows, "lo"), Value::String("apple".into()));
    assert_eq!(one_field(&rows, "hi"), Value::String("cherry".into()));
}

#[test]
fn aggregate_collect_skips_null_values() {
    let zega = db();
    // one node has property `a`, one lacks it -> missing yields Null which is skipped.
    zega.query("CREATE (n:C {a: 1})", no_params()).unwrap();
    zega.query("CREATE (n:C {other: 9})", no_params()).unwrap();
    let rows = zega
        .query("MATCH (n:C) RETURN collect(n.a) AS xs", no_params())
        .unwrap();
    match one_field(&rows, "xs") {
        Value::List(xs) => assert_eq!(xs, vec![Value::Int(1)]),
        other => panic!("expected list, got {other:?}"),
    }
}

#[test]
fn aggregate_collect_distinct_dedupes() {
    let zega = db();
    seed_amounts(
        &zega,
        "CD",
        &[Value::Int(1), Value::Int(1), Value::Int(2)],
    );
    let rows = zega
        .query("MATCH (n:CD) RETURN collect(DISTINCT n.a) AS xs", no_params())
        .unwrap();
    match one_field(&rows, "xs") {
        Value::List(mut xs) => {
            xs.sort_by(|l, r| l.partial_cmp(r).unwrap());
            assert_eq!(xs, vec![Value::Int(1), Value::Int(2)]);
        }
        other => panic!("expected list, got {other:?}"),
    }
}

#[test]
fn aggregate_count_distinct_over_identifier_variable() {
    // count(DISTINCT n) routes through the generic distinct path (n resolves to
    // a node-Map per binding; all distinct) rather than the count(var) fast path.
    let zega = db();
    seed_amounts(&zega, "CV", &[Value::Int(1), Value::Int(2), Value::Int(3)]);
    let rows = zega
        .query("MATCH (n:CV) RETURN count(DISTINCT n) AS c", no_params())
        .unwrap();
    assert_eq!(one_field(&rows, "c"), Value::Int(3));
}

#[test]
fn aggregate_sum_of_floats_only_is_float() {
    let zega = db();
    seed_amounts(
        &zega,
        "FS",
        &[Value::from_f64(1.5), Value::from_f64(2.5)],
    );
    let rows = zega
        .query("MATCH (n:FS) RETURN sum(n.a) AS s", no_params())
        .unwrap();
    assert_eq!(one_field(&rows, "s").to_f64(), Some(4.0));
}

#[test]
fn aggregate_min_of_empty_distinct_group_is_null() {
    let zega = db();
    // No nodes of this label -> min over empty -> Null.
    let rows = zega
        .query("MATCH (n:Nope) RETURN min(n.a) AS m", no_params())
        .unwrap();
    assert_eq!(one_field(&rows, "m"), Value::Null);
}

// ---------------------------------------------------------------------------
// Property round-trips through CREATE for every Value variant.
// ---------------------------------------------------------------------------

#[test]
fn create_preserves_int_bool_float_string_properties() {
    let zega = db();
    let p = params(&[
        ("i", Value::Int(-7)),
        ("b", Value::Bool(true)),
        ("f", Value::from_f64(3.25)),
        ("s", Value::String("hi".into())),
    ]);
    zega.query(
        "CREATE (n:V {i: $i, b: $b, f: $f, s: $s})",
        p.clone(),
    )
    .unwrap();
    let rows = zega
        .query(
            "MATCH (n:V) RETURN n.i AS i, n.b AS b, n.f AS f, n.s AS s",
            no_params(),
        )
        .unwrap();
    assert_eq!(one_field(&rows, "i"), Value::Int(-7));
    assert_eq!(one_field(&rows, "b"), Value::Bool(true));
    assert_eq!(one_field(&rows, "f").to_f64(), Some(3.25));
    assert_eq!(one_field(&rows, "s"), Value::String("hi".into()));
}

#[test]
fn create_preserves_list_and_map_properties_via_parameters() {
    let zega = db();
    let list = Value::List(vec![Value::Int(1), Value::Int(2)]);
    let mut m = HashMap::new();
    m.insert("nested".to_string(), Value::String("v".into()));
    let map = Value::Map(m);
    zega.query(
        "CREATE (n:V {xs: $xs, obj: $obj})",
        params(&[("xs", list.clone()), ("obj", map.clone())]),
    )
    .unwrap();
    let rows = zega
        .query("MATCH (n:V) RETURN n.xs AS xs, n.obj AS obj", no_params())
        .unwrap();
    assert_eq!(one_field(&rows, "xs"), list);
    assert_eq!(one_field(&rows, "obj"), map);
}

#[test]
fn match_node_labels_pseudo_property_via_property_access() {
    let zega = db();
    zega.query("CREATE (n:Alpha {id: 'x'})", no_params()).unwrap();
    let rows = zega
        .query("MATCH (n:Alpha {id: 'x'}) RETURN n.labels AS l", no_params())
        .unwrap();
    assert_eq!(
        one_field(&rows, "l"),
        Value::List(vec![Value::String("Alpha".into())])
    );
}

#[test]
fn negative_integer_value_round_trips() {
    let zega = db();
    zega.query(
        "CREATE (n:Neg {v: $v})",
        params(&[("v", Value::Int(i64::MIN))]),
    )
    .unwrap();
    let rows = zega
        .query("MATCH (n:Neg) RETURN n.v AS v", no_params())
        .unwrap();
    assert_eq!(one_field(&rows, "v"), Value::Int(i64::MIN));
}

// ---------------------------------------------------------------------------
// KV: TTL on SET, kv_set helper with ttl, non-string key coercion, INCR WAL,
// helper-vs-query interop.
// ---------------------------------------------------------------------------

#[test]
fn kv_set_with_ttl_clause_is_readable_immediately() {
    let zega = db();
    // A generous TTL so the value is still present when we read it back.
    zega.query("SET KEY 'k' = 'v' TTL 3600", no_params()).unwrap();
    let rows = zega.query("GET KEY 'k'", no_params()).unwrap();
    assert_eq!(rows[0].fields.get("value"), Some(&Value::String("v".into())));
}

#[test]
fn kv_set_helper_with_ttl_round_trips() {
    let zega = db();
    zega.kv_set("hk".into(), Value::Int(42), Some(3600)).unwrap();
    assert_eq!(zega.kv_get("hk"), Some(Value::Int(42)));
}

#[test]
fn kv_int_key_is_coerced_to_string_form() {
    // exec_kv_set stringifies a non-string key via Display; reading with the
    // same int key must hit the same coerced string.
    let zega = db();
    zega.query("SET KEY 123 = 'v'", no_params()).unwrap();
    let rows = zega.query("GET KEY 123", no_params()).unwrap();
    assert_eq!(rows[0].fields.get("value"), Some(&Value::String("v".into())));
    // And the helper sees the stringified key too.
    assert_eq!(zega.kv_get("123"), Some(Value::String("v".into())));
}

#[test]
fn kv_helper_and_query_surface_share_one_store() {
    let zega = db();
    zega.kv_set("shared".into(), Value::String("from-helper".into()), None)
        .unwrap();
    let rows = zega.query("GET KEY 'shared'", no_params()).unwrap();
    assert_eq!(
        rows[0].fields.get("value"),
        Some(&Value::String("from-helper".into()))
    );
    // And a query-side DEL is visible to the helper getter.
    zega.query("DEL KEY 'shared'", no_params()).unwrap();
    assert_eq!(zega.kv_get("shared"), None);
}

#[test]
fn kv_incr_twice_accumulates() {
    let zega = db();
    zega.query("INCR KEY 'c'", no_params()).unwrap();
    let rows = zega.query("INCR KEY 'c'", no_params()).unwrap();
    assert_eq!(rows[0].fields.get("value"), Some(&Value::Int(2)));
}

#[test]
fn kv_incr_persists_through_wal_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_str().unwrap();
    {
        let zega = Zega::open(path).wal_flush_every_write().build().unwrap();
        zega.query("INCR KEY 'c'", no_params()).unwrap();
        zega.query("INCR KEY 'c'", no_params()).unwrap();
    }
    {
        let zega = Zega::open(path).wal_flush_every_write().build().unwrap();
        let rows = zega.query("GET KEY 'c'", no_params()).unwrap();
        assert_eq!(rows[0].fields.get("value"), Some(&Value::Int(2)));
    }
}

// ---------------------------------------------------------------------------
// MERGE: unlabeled element always creates; multi-statement MERGE then SET.
// ---------------------------------------------------------------------------

#[test]
fn merge_unlabeled_node_always_creates_new() {
    // With no label, exec_merge cannot search for an existing node, so two
    // identical MERGEs produce two nodes.
    let zega = db();
    zega.query("MERGE (n {k: 'v'})", no_params()).unwrap();
    zega.query("MERGE (n {k: 'v'})", no_params()).unwrap();
    let rows = zega.query("MATCH (n) RETURN n", no_params()).unwrap();
    assert_eq!(rows.len(), 2);
}

#[test]
fn merge_on_create_does_not_fire_on_existing_match() {
    let zega = db();
    zega.query("CREATE (n:Tag {name: 'rust'})", no_params())
        .unwrap();
    // The node already exists, so ON CREATE SET must NOT run.
    zega.query(
        "MERGE (n:Tag {name: 'rust'}) ON CREATE SET n.created = true",
        no_params(),
    )
    .unwrap();
    let rows = zega
        .query("MATCH (n:Tag) RETURN n.created AS c", no_params())
        .unwrap();
    assert_eq!(one_field(&rows, "c"), Value::Null);
}

// ---------------------------------------------------------------------------
// Additional policy coverage: Neq / Or / Not / And conditions, AlwaysAllow,
// PolicyTargets::All on labeled + unlabeled nodes, In-as-prefix on KV.
// ---------------------------------------------------------------------------

#[test]
fn policy_always_allow_does_not_filter() {
    let zega = Zega::in_memory()
        .policy(
            "open",
            PolicyTargets::Labels(vec!["Doc".into()]),
            PolicyCondition::AlwaysAllow,
        )
        .build()
        .unwrap();
    create_doc(&zega, "d1", "o1");
    create_doc(&zega, "d2", "o2");
    let rows = zega
        .query_with_context(
            "MATCH (n:Doc) RETURN n",
            no_params(),
            ZegaContext::claims(claims(&[("org_id", "o1")])),
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
}

#[test]
fn policy_neq_condition_excludes_matching_field() {
    // AllowWhen ctx.org_id != node.org_id  -> keep rows whose org differs.
    let zega = Zega::in_memory()
        .policy(
            "not_own",
            PolicyTargets::Labels(vec!["Doc".into()]),
            PolicyCondition::AllowWhen(PolicyExpr::Neq(
                ExprValue::ContextField(".org_id".into()),
                ExprValue::NodeField("node.org_id".into()),
            )),
        )
        .build()
        .unwrap();
    create_doc(&zega, "mine", "o1");
    create_doc(&zega, "theirs", "o2");
    let rows = zega
        .query_with_context(
            "MATCH (n:Doc) RETURN n.name AS name",
            no_params(),
            ZegaContext::claims(claims(&[("org_id", "o1")])),
        )
        .unwrap();
    assert_eq!(one_field(&rows, "name"), Value::String("theirs".into()));
}

#[test]
fn policy_and_condition_requires_both() {
    let zega = Zega::in_memory()
        .policy(
            "both",
            PolicyTargets::Labels(vec!["Doc".into()]),
            PolicyCondition::AllowWhen(PolicyExpr::And(
                Box::new(PolicyExpr::Eq(
                    ExprValue::ContextField(".org_id".into()),
                    ExprValue::NodeField("node.org_id".into()),
                )),
                Box::new(PolicyExpr::Eq(
                    ExprValue::ContextField(".role".into()),
                    ExprValue::Literal(Value::String("admin".into())),
                )),
            )),
        )
        .build()
        .unwrap();
    create_doc(&zega, "d1", "o1");
    // Right org but wrong role -> filtered out.
    let denied = zega
        .query_with_context(
            "MATCH (n:Doc) RETURN n",
            no_params(),
            ZegaContext::claims(claims(&[("org_id", "o1"), ("role", "viewer")])),
        )
        .unwrap();
    assert!(denied.is_empty());
    // Right org and right role -> allowed.
    let allowed = zega
        .query_with_context(
            "MATCH (n:Doc) RETURN n",
            no_params(),
            ZegaContext::claims(claims(&[("org_id", "o1"), ("role", "admin")])),
        )
        .unwrap();
    assert_eq!(allowed.len(), 1);
}

#[test]
fn policy_or_condition_passes_on_either_branch() {
    let zega = Zega::in_memory()
        .policy(
            "either",
            PolicyTargets::Labels(vec!["Doc".into()]),
            PolicyCondition::AllowWhen(PolicyExpr::Or(
                Box::new(PolicyExpr::Eq(
                    ExprValue::ContextField(".role".into()),
                    ExprValue::Literal(Value::String("admin".into())),
                )),
                Box::new(PolicyExpr::Eq(
                    ExprValue::ContextField(".org_id".into()),
                    ExprValue::NodeField("node.org_id".into()),
                )),
            )),
        )
        .build()
        .unwrap();
    create_doc(&zega, "d1", "o1");
    // Wrong org but admin role -> first disjunct passes.
    let rows = zega
        .query_with_context(
            "MATCH (n:Doc) RETURN n",
            no_params(),
            ZegaContext::claims(claims(&[("org_id", "other"), ("role", "admin")])),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn policy_not_condition_negates_inner() {
    // Not(role == banned) -> keep rows when role is anything but "banned".
    let zega = Zega::in_memory()
        .policy(
            "not_banned",
            PolicyTargets::Labels(vec!["Doc".into()]),
            PolicyCondition::AllowWhen(PolicyExpr::Not(Box::new(PolicyExpr::Eq(
                ExprValue::ContextField(".role".into()),
                ExprValue::Literal(Value::String("banned".into())),
            )))),
        )
        .build()
        .unwrap();
    create_doc(&zega, "d1", "o1");
    let allowed = zega
        .query_with_context(
            "MATCH (n:Doc) RETURN n",
            no_params(),
            ZegaContext::claims(claims(&[("role", "viewer")])),
        )
        .unwrap();
    assert_eq!(allowed.len(), 1);
    let denied = zega
        .query_with_context(
            "MATCH (n:Doc) RETURN n",
            no_params(),
            ZegaContext::claims(claims(&[("role", "banned")])),
        )
        .unwrap();
    assert!(denied.is_empty());
}

#[test]
fn policy_targets_all_applies_to_labeled_and_unlabeled_nodes() {
    let zega = Zega::in_memory()
        .policy(
            "global_tenant",
            PolicyTargets::All,
            PolicyCondition::AllowWhen(PolicyExpr::Eq(
                ExprValue::ContextField(".org_id".into()),
                ExprValue::NodeField("node.org_id".into()),
            )),
        )
        .build()
        .unwrap();
    // labeled node
    create_doc(&zega, "d1", "o1");
    create_doc(&zega, "d2", "o2");
    // unlabeled node with an org_id property
    zega.query(
        "CREATE (n {org_id: $org})",
        params(&[("org", Value::String("o1".into()))]),
    )
    .unwrap();

    let labeled = zega
        .query_with_context(
            "MATCH (n:Doc) RETURN n",
            no_params(),
            ZegaContext::claims(claims(&[("org_id", "o1")])),
        )
        .unwrap();
    assert_eq!(labeled.len(), 1);

    // Unlabeled match: All policy applies via applies_to_unlabeled_node.
    let unlabeled = zega
        .query_with_context(
            "MATCH (n) RETURN n",
            no_params(),
            ZegaContext::claims(claims(&[("org_id", "o1")])),
        )
        .unwrap();
    // Both the o1 Doc and the o1 unlabeled node survive (2), o2 Doc filtered.
    assert_eq!(unlabeled.len(), 2);
}

#[test]
fn policy_kv_prefix_allows_matching_tenant_key() {
    // Positive companion to the existing "blocks other tenants" test: the
    // allowed-prefix key returns its real value, not the filtered Null.
    let zega = Zega::in_memory()
        .policy(
            "kv_tenant",
            PolicyTargets::Kv,
            PolicyCondition::AllowWhen(PolicyExpr::Eq(
                ExprValue::NodeField("key_prefix".into()),
                ExprValue::ContextField(".org_id".into()),
            )),
        )
        .build()
        .unwrap();
    // Seed under system context (system bypasses policy in plan_statement).
    zega.query_with_context("SET KEY 'org1:file' = 'secret'", no_params(), ZegaContext::system())
        .unwrap();

    let allowed = zega
        .query_with_context(
            "GET KEY 'org1:file'",
            no_params(),
            ZegaContext::claims(claims(&[("org_id", "org1")])),
        )
        .unwrap();
    assert_eq!(
        allowed[0].fields.get("value"),
        Some(&Value::String("secret".into()))
    );
}

#[test]
fn policy_system_only_on_kv_denies_non_system_get() {
    let zega = Zega::in_memory()
        .policy(
            "kv_locked",
            PolicyTargets::Kv,
            PolicyCondition::SystemOnly,
        )
        .build()
        .unwrap();
    let err = zega
        .query_with_context("GET KEY 'anything'", no_params(), ZegaContext::anonymous())
        .unwrap_err();
    assert!(matches!(err, ZegaError::PermissionDenied(_)));
}

// ---------------------------------------------------------------------------
// Multi-statement sequencing edge cases + bare MATCH inside a sequence.
// ---------------------------------------------------------------------------

#[test]
fn create_then_match_in_single_query_string_sequences_correctly() {
    let zega = db();
    // CREATE produces no rows; the trailing MATCH sees the created node.
    let rows = zega
        .query(
            "CREATE (n:Seq {id: 's'}) MATCH (n:Seq) RETURN n.id AS id",
            no_params(),
        )
        .unwrap();
    assert_eq!(one_field(&rows, "id"), Value::String("s".into()));
}

#[test]
fn empty_param_map_with_no_param_references_is_fine() {
    let zega = db();
    zega.query("CREATE (n:E {k: 1})", no_params()).unwrap();
    let rows = zega.query("MATCH (n:E) RETURN n.k AS k", no_params()).unwrap();
    assert_eq!(one_field(&rows, "k"), Value::Int(1));
}

// ---------------------------------------------------------------------------
// Snapshot + WAL layering: snapshot captures state, post-snapshot writes go to
// WAL, and reopen replays the WAL on top of the restored snapshot.
// ---------------------------------------------------------------------------

#[test]
fn snapshot_then_more_writes_then_reopen_replays_wal_on_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_str().unwrap();
    {
        let zega = Zega::open(path).wal_flush_every_write().build().unwrap();
        zega.query("CREATE (n:Snap {id: 'before'})", no_params())
            .unwrap();
        zega.snapshot().unwrap();
        // This write happens AFTER the snapshot -> only in the WAL.
        zega.query("CREATE (n:Snap {id: 'after'})", no_params())
            .unwrap();
    }
    {
        let zega = Zega::open(path).wal_flush_every_write().build().unwrap();
        let rows = zega
            .query("MATCH (n:Snap) RETURN n.id AS id", no_params())
            .unwrap();
        let ids: HashSet<_> = collect_field(&rows, "id").into_iter().collect();
        assert_eq!(
            ids,
            HashSet::from([
                Value::String("before".into()),
                Value::String("after".into()),
            ])
        );
    }
}

#[test]
fn reopen_without_any_writes_yields_empty_graph() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_str().unwrap();
    {
        let _zega = Zega::open(path).build().unwrap();
    }
    let zega = Zega::open(path).build().unwrap();
    let rows = zega.query("MATCH (n) RETURN n", no_params()).unwrap();
    assert!(rows.is_empty());
}

// ---------------------------------------------------------------------------
// Builder option coverage that the original suite did not exercise directly.
// ---------------------------------------------------------------------------

#[test]
fn builder_wal_flush_interval_is_chainable_and_builds() {
    let dir = tempfile::tempdir().unwrap();
    let zega = Zega::open(dir.path().to_str().unwrap())
        .wal_flush_interval(10)
        .build()
        .unwrap();
    zega.query("CREATE (n:I {k: 1})", no_params()).unwrap();
    let rows = zega.query("MATCH (n:I) RETURN n", no_params()).unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn builder_jwt_rsa_public_key_pem_is_chainable() {
    // We don't need a valid PEM to build; configuring the key is enough, and a
    // malformed PEM only fails at verify time. This proves the builder method
    // exists and the config path is wired without requiring an RSA keypair.
    let zega = Zega::in_memory()
        .jwt_rsa_public_key_pem(b"-----BEGIN PUBLIC KEY-----\nnotreal\n-----END PUBLIC KEY-----".to_vec())
        .build()
        .unwrap();
    // A bogus token still resolves to a Jwt error (config present, verify fails).
    let err = zega
        .query_with_context("MATCH (n) RETURN n", no_params(), ZegaContext::jwt("a.b.c"))
        .unwrap_err();
    assert!(matches!(err, ZegaError::Jwt(_)));
}

#[test]
fn builder_low_traversal_budget_triggers_budget_error_on_traversal() {
    let zega = Zega::in_memory().traversal_work_budget(0).build().unwrap();
    zega.query("CREATE (a:N {id: 'a'})", no_params()).unwrap();
    zega.query("CREATE (b:N {id: 'b'})", no_params()).unwrap();
    zega.query(
        "MATCH (a:N {id: 'a'}) MATCH (b:N {id: 'b'}) CREATE (a)-[:R]->(b)",
        no_params(),
    )
    .unwrap();
    // Any single hop consumes budget; a budget of 0 must error on the first hop.
    let err = zega
        .query(
            "MATCH (a:N {id: 'a'})-[:R]->(x:N) RETURN x",
            no_params(),
        )
        .unwrap_err();
    assert!(matches!(
        err,
        ZegaError::TraversalWorkBudgetExceeded { limit: 0 }
    ));
}

// ---------------------------------------------------------------------------
// query() and query_with_context(Anonymous) must be observationally identical.
// ---------------------------------------------------------------------------

#[test]
fn query_equals_query_with_anonymous_context_for_reads() {
    let zega = db();
    zega.query("CREATE (n:Eq {id: 'x'})", no_params()).unwrap();
    let a = zega.query("MATCH (n:Eq) RETURN n.id AS id", no_params()).unwrap();
    let b = zega
        .query_with_context(
            "MATCH (n:Eq) RETURN n.id AS id",
            no_params(),
            ZegaContext::Anonymous,
        )
        .unwrap();
    assert_eq!(collect_field(&a, "id"), collect_field(&b, "id"));
}

// ---------------------------------------------------------------------------
// Anonymous self-loop / relationship uniqueness: an undirected traversal over a
// self-loop must not loop forever (used_relationships uniqueness).
// ---------------------------------------------------------------------------

#[test]
fn variable_length_over_self_loop_terminates() {
    let zega = db();
    zega.query("CREATE (a:N {id: 'a'})", no_params()).unwrap();
    // self loop a -> a
    zega.query(
        "MATCH (a:N {id: 'a'}) MATCH (b:N {id: 'a'}) CREATE (a)-[:R]->(b)",
        no_params(),
    )
    .unwrap();
    // Should terminate (uniqueness stops re-walking the same edge) and not panic.
    let rows = zega
        .query(
            "MATCH (a:N {id: 'a'})-[:R*1..5]->(x:N) RETURN x.id AS id",
            no_params(),
        )
        .unwrap();
    // Reaches itself once via the single loop edge.
    assert_eq!(collect_field(&rows, "id"), vec![Value::String("a".into())]);
}
