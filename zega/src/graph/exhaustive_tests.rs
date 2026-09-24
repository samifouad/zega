//! Exhaustive tests for the `zega::graph` module.
//!
//! Covers the full surface of [`super::Graph`]:
//! node + relationship CRUD, label/property indexing & lookups,
//! adjacency (outgoing/incoming) bookkeeping, cascade deletion,
//! id-counter semantics (create vs restore vs set_state), traversal
//! correctness (variable-length, relationship-uniqueness / trail,
//! cycle termination), pattern-match shapes, property type
//! round-trips, and empty-/large-graph behavior.
//!
//! Errors are values in Zega — these tests assert on `Option`/state,
//! never on panics. The CRUD entry points (`create_node`,
//! `create_relationship`, `delete_*`, `update_node`) are infallible
//! by signature, so we assert on observable graph state instead.

use std::collections::{HashMap, HashSet};
use super::{Graph, Node, NodeId, RelId, Relationship};
use crate::value::Value;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a props map from `(key, value)` pairs.
fn props(pairs: &[(&str, Value)]) -> HashMap<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

/// Convenience: a `Vec<String>` from `&str` slices.
fn labels(ls: &[&str]) -> Vec<String> {
    ls.iter().map(|s| s.to_string()).collect()
}

fn s(v: &str) -> Value {
    Value::String(v.to_string())
}

/// Collect all node ids in the graph (order-independent).
fn node_id_set(g: &Graph) -> HashSet<NodeId> {
    g.all_nodes().keys().copied().collect()
}

/// Collect all relationship ids in the graph (order-independent).
fn rel_id_set(g: &Graph) -> HashSet<RelId> {
    g.all_relationships().keys().copied().collect()
}

// ===========================================================================
// Construction & empty-graph behavior
// ===========================================================================

#[test]
fn new_graph_is_empty() {
    let g = Graph::new();
    assert!(g.all_nodes().is_empty());
    assert!(g.all_relationships().is_empty());
}

#[test]
fn default_graph_equivalent_to_new() {
    let g = Graph::default();
    assert!(g.all_nodes().is_empty());
    assert!(g.all_relationships().is_empty());
}

#[test]
fn empty_graph_lookups_return_none() {
    let g = Graph::new();
    assert!(g.get_node(1).is_none());
    assert!(g.get_node(0).is_none());
    assert!(g.get_relationship(1).is_none());
    assert!(g.nodes_by_label("Person").is_none());
    assert!(g.nodes_by_property("name", &s("Alice")).is_none());
    assert!(g.outgoing_rels(1).is_none());
    assert!(g.incoming_rels(1).is_none());
}

#[test]
fn empty_graph_delete_is_noop() {
    let mut g = Graph::new();
    // Deleting nonexistent ids must not panic and must leave graph empty.
    g.delete_node(999);
    g.delete_relationship(999);
    assert!(g.all_nodes().is_empty());
    assert!(g.all_relationships().is_empty());
}

// ===========================================================================
// Node CRUD
// ===========================================================================

#[test]
fn create_node_returns_first_id_one() {
    let mut g = Graph::new();
    let id = g.create_node(labels(&["Person"]), HashMap::new());
    assert_eq!(id, 1, "first created node id should be 1");
}

#[test]
fn create_node_ids_are_sequential_and_unique() {
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let b = g.create_node(vec![], HashMap::new());
    let c = g.create_node(vec![], HashMap::new());
    assert_eq!((a, b, c), (1, 2, 3));
    let set: HashSet<_> = [a, b, c].into_iter().collect();
    assert_eq!(set.len(), 3);
}

#[test]
fn create_node_stores_labels_and_props() {
    let mut g = Graph::new();
    let p = props(&[("name", s("Alice")), ("age", Value::Int(30))]);
    let id = g.create_node(labels(&["Person", "Admin"]), p.clone());
    let node = g.get_node(id).expect("node must exist");
    assert_eq!(node.id, id);
    assert_eq!(node.labels, vec!["Person".to_string(), "Admin".to_string()]);
    assert_eq!(node.props.get("name"), Some(&s("Alice")));
    assert_eq!(node.props.get("age"), Some(&Value::Int(30)));
    assert_eq!(node.props.len(), 2);
}

#[test]
fn create_node_with_no_labels_or_props() {
    let mut g = Graph::new();
    let id = g.create_node(vec![], HashMap::new());
    let node = g.get_node(id).unwrap();
    assert!(node.labels.is_empty());
    assert!(node.props.is_empty());
}

#[test]
fn get_node_returns_none_for_unknown_id() {
    let mut g = Graph::new();
    let id = g.create_node(vec![], HashMap::new());
    assert!(g.get_node(id + 1).is_none());
    assert!(g.get_node(0).is_none());
}

#[test]
fn delete_node_removes_node_and_returns_none_after() {
    let mut g = Graph::new();
    let id = g.create_node(labels(&["Person"]), props(&[("name", s("Bob"))]));
    assert!(g.get_node(id).is_some());
    g.delete_node(id);
    assert!(g.get_node(id).is_none());
    assert!(g.all_nodes().is_empty());
}

#[test]
fn delete_node_clears_from_label_index() {
    let mut g = Graph::new();
    let id = g.create_node(labels(&["Person"]), HashMap::new());
    g.delete_node(id);
    // Index entry may persist but must no longer contain the id.
    let set = g.nodes_by_label("Person");
    assert!(set.is_none_or(|s| !s.contains(&id)));
}

#[test]
fn delete_node_clears_from_property_index() {
    let mut g = Graph::new();
    let id = g.create_node(vec![], props(&[("name", s("Carol"))]));
    g.delete_node(id);
    let set = g.nodes_by_property("name", &s("Carol"));
    assert!(set.is_none_or(|s| !s.contains(&id)));
}

#[test]
fn delete_nonexistent_node_is_noop() {
    let mut g = Graph::new();
    let id = g.create_node(vec![], HashMap::new());
    g.delete_node(id + 100);
    assert!(g.get_node(id).is_some());
    assert_eq!(g.all_nodes().len(), 1);
}

#[test]
fn delete_node_twice_is_safe() {
    let mut g = Graph::new();
    let id = g.create_node(labels(&["X"]), HashMap::new());
    g.delete_node(id);
    g.delete_node(id); // second delete must be a no-op, not a panic
    assert!(g.get_node(id).is_none());
}

// ===========================================================================
// update_node semantics (merge via extend + re-index)
// ===========================================================================

#[test]
fn update_node_merges_new_props() {
    let mut g = Graph::new();
    let id = g.create_node(vec![], props(&[("a", Value::Int(1))]));
    g.update_node(id, props(&[("b", Value::Int(2))]));
    let node = g.get_node(id).unwrap();
    assert_eq!(node.props.get("a"), Some(&Value::Int(1)));
    assert_eq!(node.props.get("b"), Some(&Value::Int(2)));
    assert_eq!(node.props.len(), 2);
}

#[test]
fn update_node_overwrites_existing_prop() {
    let mut g = Graph::new();
    let id = g.create_node(vec![], props(&[("name", s("old"))]));
    g.update_node(id, props(&[("name", s("new"))]));
    let node = g.get_node(id).unwrap();
    assert_eq!(node.props.get("name"), Some(&s("new")));
}

#[test]
fn update_node_reindexes_property_old_value_dropped() {
    let mut g = Graph::new();
    let id = g.create_node(vec![], props(&[("name", s("old"))]));
    g.update_node(id, props(&[("name", s("new"))]));
    // Old value no longer maps to the node.
    let old = g.nodes_by_property("name", &s("old"));
    assert!(old.is_none_or(|set| !set.contains(&id)));
    // New value does.
    let new = g.nodes_by_property("name", &s("new")).unwrap();
    assert!(new.contains(&id));
}

#[test]
fn update_node_on_missing_node_is_noop() {
    let mut g = Graph::new();
    g.update_node(42, props(&[("x", Value::Int(1))]));
    assert!(g.get_node(42).is_none());
}

#[test]
fn update_node_with_empty_props_keeps_existing() {
    let mut g = Graph::new();
    let id = g.create_node(vec![], props(&[("k", Value::Int(7))]));
    g.update_node(id, HashMap::new());
    let node = g.get_node(id).unwrap();
    assert_eq!(node.props.get("k"), Some(&Value::Int(7)));
}

#[test]
fn update_node_does_not_change_labels() {
    let mut g = Graph::new();
    let id = g.create_node(labels(&["Person"]), HashMap::new());
    g.update_node(id, props(&[("x", Value::Int(1))]));
    let node = g.get_node(id).unwrap();
    assert_eq!(node.labels, vec!["Person".to_string()]);
    assert!(g.nodes_by_label("Person").unwrap().contains(&id));
}

// ===========================================================================
// Label index
// ===========================================================================

#[test]
fn nodes_by_label_groups_multiple_nodes() {
    let mut g = Graph::new();
    let a = g.create_node(labels(&["Person"]), HashMap::new());
    let b = g.create_node(labels(&["Person"]), HashMap::new());
    let _c = g.create_node(labels(&["Company"]), HashMap::new());
    let people = g.nodes_by_label("Person").unwrap();
    assert_eq!(people.len(), 2);
    assert!(people.contains(&a));
    assert!(people.contains(&b));
}

#[test]
fn node_with_multiple_labels_indexed_under_each() {
    let mut g = Graph::new();
    let id = g.create_node(labels(&["Person", "Admin", "User"]), HashMap::new());
    assert!(g.nodes_by_label("Person").unwrap().contains(&id));
    assert!(g.nodes_by_label("Admin").unwrap().contains(&id));
    assert!(g.nodes_by_label("User").unwrap().contains(&id));
}

#[test]
fn nodes_by_label_unknown_label_is_none() {
    let mut g = Graph::new();
    g.create_node(labels(&["Person"]), HashMap::new());
    assert!(g.nodes_by_label("Ghost").is_none());
}

#[test]
fn label_lookup_is_case_sensitive() {
    let mut g = Graph::new();
    g.create_node(labels(&["Person"]), HashMap::new());
    assert!(g.nodes_by_label("person").is_none());
    assert!(g.nodes_by_label("PERSON").is_none());
}

#[test]
fn duplicate_label_on_one_node_indexed_once() {
    let mut g = Graph::new();
    // HashSet membership means a repeated label still yields a single entry.
    let id = g.create_node(labels(&["Tag", "Tag"]), HashMap::new());
    let set = g.nodes_by_label("Tag").unwrap();
    assert_eq!(set.len(), 1);
    assert!(set.contains(&id));
}

// ===========================================================================
// Property index
// ===========================================================================

#[test]
fn nodes_by_property_groups_matching_nodes() {
    let mut g = Graph::new();
    let a = g.create_node(vec![], props(&[("city", s("NYC"))]));
    let b = g.create_node(vec![], props(&[("city", s("NYC"))]));
    let _c = g.create_node(vec![], props(&[("city", s("LA"))]));
    let nyc = g.nodes_by_property("city", &s("NYC")).unwrap();
    assert_eq!(nyc.len(), 2);
    assert!(nyc.contains(&a) && nyc.contains(&b));
}

#[test]
fn nodes_by_property_distinguishes_value_types() {
    let mut g = Graph::new();
    let int_node = g.create_node(vec![], props(&[("v", Value::Int(1))]));
    let str_node = g.create_node(vec![], props(&[("v", s("1"))]));
    // Int(1) and String("1") are distinct index keys.
    let by_int = g.nodes_by_property("v", &Value::Int(1)).unwrap();
    assert!(by_int.contains(&int_node));
    assert!(!by_int.contains(&str_node));
    let by_str = g.nodes_by_property("v", &s("1")).unwrap();
    assert!(by_str.contains(&str_node));
    assert!(!by_str.contains(&int_node));
}

#[test]
fn nodes_by_property_unknown_key_is_none() {
    let mut g = Graph::new();
    g.create_node(vec![], props(&[("a", Value::Int(1))]));
    assert!(g.nodes_by_property("b", &Value::Int(1)).is_none());
}

#[test]
fn nodes_by_property_unknown_value_is_none() {
    let mut g = Graph::new();
    g.create_node(vec![], props(&[("a", Value::Int(1))]));
    assert!(g.nodes_by_property("a", &Value::Int(2)).is_none());
}

#[test]
fn node_with_multiple_props_indexed_under_each() {
    let mut g = Graph::new();
    let id = g.create_node(vec![], props(&[("x", Value::Int(1)), ("y", s("q"))]));
    assert!(g.nodes_by_property("x", &Value::Int(1)).unwrap().contains(&id));
    assert!(g.nodes_by_property("y", &s("q")).unwrap().contains(&id));
}

// ===========================================================================
// Property type round-trips (all Value variants)
// ===========================================================================

#[test]
fn prop_roundtrip_string() {
    let mut g = Graph::new();
    let id = g.create_node(vec![], props(&[("k", s("hello"))]));
    assert_eq!(g.get_node(id).unwrap().props.get("k"), Some(&s("hello")));
}

#[test]
fn prop_roundtrip_int_including_extremes() {
    let mut g = Graph::new();
    for v in [0i64, 1, -1, i64::MAX, i64::MIN] {
        let id = g.create_node(vec![], props(&[("n", Value::Int(v))]));
        assert_eq!(g.get_node(id).unwrap().props.get("n"), Some(&Value::Int(v)));
        assert!(g.nodes_by_property("n", &Value::Int(v)).unwrap().contains(&id));
    }
}

#[test]
fn prop_roundtrip_float_via_bits() {
    let mut g = Graph::new();
    let v = Value::from_f64(std::f64::consts::PI);
    let id = g.create_node(vec![], props(&[("pi", v.clone())]));
    let got = g.get_node(id).unwrap().props.get("pi").cloned().unwrap();
    assert_eq!(got, v);
    assert_eq!(got.to_f64(), Some(std::f64::consts::PI));
}

#[test]
fn prop_roundtrip_float_special_values() {
    let mut g = Graph::new();
    // NaN's bit pattern is preserved through the index key (Eq is on bits).
    let nan = Value::from_f64(f64::NAN);
    let inf = Value::from_f64(f64::INFINITY);
    let neg_inf = Value::from_f64(f64::NEG_INFINITY);
    let id = g.create_node(
        vec![],
        props(&[("nan", nan.clone()), ("inf", inf.clone()), ("ninf", neg_inf.clone())]),
    );
    let node = g.get_node(id).unwrap();
    assert_eq!(node.props.get("nan"), Some(&nan));
    assert_eq!(node.props.get("inf"), Some(&inf));
    assert_eq!(node.props.get("ninf"), Some(&neg_inf));
    assert!(node.props.get("inf").unwrap().to_f64().unwrap().is_infinite());
    assert!(node.props.get("nan").unwrap().to_f64().unwrap().is_nan());
}

#[test]
fn prop_roundtrip_bool() {
    let mut g = Graph::new();
    let id = g.create_node(vec![], props(&[("t", Value::Bool(true)), ("f", Value::Bool(false))]));
    let node = g.get_node(id).unwrap();
    assert_eq!(node.props.get("t"), Some(&Value::Bool(true)));
    assert_eq!(node.props.get("f"), Some(&Value::Bool(false)));
}

#[test]
fn prop_roundtrip_list() {
    let mut g = Graph::new();
    let list = Value::List(vec![Value::Int(1), s("two"), Value::Bool(true)]);
    let id = g.create_node(vec![], props(&[("items", list.clone())]));
    assert_eq!(g.get_node(id).unwrap().props.get("items"), Some(&list));
}

#[test]
fn prop_roundtrip_map() {
    let mut g = Graph::new();
    let mut inner = HashMap::new();
    inner.insert("a".to_string(), Value::Int(1));
    inner.insert("b".to_string(), s("x"));
    let map = Value::Map(inner);
    let id = g.create_node(vec![], props(&[("meta", map.clone())]));
    assert_eq!(g.get_node(id).unwrap().props.get("meta"), Some(&map));
}

#[test]
fn prop_roundtrip_null() {
    let mut g = Graph::new();
    let id = g.create_node(vec![], props(&[("maybe", Value::Null)]));
    assert_eq!(g.get_node(id).unwrap().props.get("maybe"), Some(&Value::Null));
    assert!(g.nodes_by_property("maybe", &Value::Null).unwrap().contains(&id));
}

#[test]
fn prop_roundtrip_unicode_and_empty_string() {
    let mut g = Graph::new();
    let id = g.create_node(
        vec![],
        props(&[("emoji", s("héllo 🌍 日本語")), ("empty", s(""))]),
    );
    let node = g.get_node(id).unwrap();
    assert_eq!(node.props.get("emoji"), Some(&s("héllo 🌍 日本語")));
    assert_eq!(node.props.get("empty"), Some(&s("")));
}

#[test]
fn unicode_label_is_indexed() {
    let mut g = Graph::new();
    let id = g.create_node(labels(&["日本語", "🚀"]), HashMap::new());
    assert!(g.nodes_by_label("日本語").unwrap().contains(&id));
    assert!(g.nodes_by_label("🚀").unwrap().contains(&id));
}

// ===========================================================================
// Relationship CRUD
// ===========================================================================

#[test]
fn create_relationship_returns_first_id_one() {
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let b = g.create_node(vec![], HashMap::new());
    let rid = g.create_relationship("KNOWS".to_string(), a, b, HashMap::new());
    assert_eq!(rid, 1, "first relationship id should be 1");
}

#[test]
fn rel_ids_independent_from_node_ids() {
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new()); // node 1
    let b = g.create_node(vec![], HashMap::new()); // node 2
    let r = g.create_relationship("R".to_string(), a, b, HashMap::new()); // rel 1
    assert_eq!(r, 1, "rel counter is separate from node counter");
}

#[test]
fn create_relationship_stores_fields_and_props() {
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let b = g.create_node(vec![], HashMap::new());
    let rid = g.create_relationship(
        "LIKES".to_string(),
        a,
        b,
        props(&[("since", Value::Int(2020))]),
    );
    let rel = g.get_relationship(rid).unwrap();
    assert_eq!(rel.id, rid);
    assert_eq!(rel.kind, "LIKES");
    assert_eq!(rel.from, a);
    assert_eq!(rel.to, b);
    assert_eq!(rel.props.get("since"), Some(&Value::Int(2020)));
}

#[test]
fn create_relationship_updates_adjacency() {
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let b = g.create_node(vec![], HashMap::new());
    let rid = g.create_relationship("R".to_string(), a, b, HashMap::new());
    assert!(g.outgoing_rels(a).unwrap().contains(&rid));
    assert!(g.incoming_rels(b).unwrap().contains(&rid));
    // No reverse direction.
    assert!(g.incoming_rels(a).is_none_or(|s| !s.contains(&rid)));
    assert!(g.outgoing_rels(b).is_none_or(|s| !s.contains(&rid)));
}

#[test]
fn get_relationship_unknown_id_is_none() {
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let b = g.create_node(vec![], HashMap::new());
    let rid = g.create_relationship("R".to_string(), a, b, HashMap::new());
    assert!(g.get_relationship(rid + 1).is_none());
}

#[test]
fn self_loop_relationship_indexed_both_directions() {
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let rid = g.create_relationship("SELF".to_string(), a, a, HashMap::new());
    assert!(g.outgoing_rels(a).unwrap().contains(&rid));
    assert!(g.incoming_rels(a).unwrap().contains(&rid));
}

#[test]
fn parallel_relationships_between_same_pair() {
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let b = g.create_node(vec![], HashMap::new());
    let r1 = g.create_relationship("R".to_string(), a, b, HashMap::new());
    let r2 = g.create_relationship("R".to_string(), a, b, HashMap::new());
    assert_ne!(r1, r2);
    let out = g.outgoing_rels(a).unwrap();
    assert!(out.contains(&r1) && out.contains(&r2));
    assert_eq!(out.len(), 2);
}

#[test]
fn relationship_to_nonexistent_node_still_recorded() {
    // The store does not validate endpoint existence — it just indexes ids.
    let mut g = Graph::new();
    let rid = g.create_relationship("DANGLE".to_string(), 100, 200, HashMap::new());
    let rel = g.get_relationship(rid).unwrap();
    assert_eq!(rel.from, 100);
    assert_eq!(rel.to, 200);
    assert!(g.outgoing_rels(100).unwrap().contains(&rid));
    assert!(g.incoming_rels(200).unwrap().contains(&rid));
}

// ===========================================================================
// Relationship deletion
// ===========================================================================

#[test]
fn delete_relationship_removes_and_clears_adjacency() {
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let b = g.create_node(vec![], HashMap::new());
    let rid = g.create_relationship("R".to_string(), a, b, HashMap::new());
    g.delete_relationship(rid);
    assert!(g.get_relationship(rid).is_none());
    assert!(g.outgoing_rels(a).is_none_or(|s| !s.contains(&rid)));
    assert!(g.incoming_rels(b).is_none_or(|s| !s.contains(&rid)));
}

#[test]
fn delete_relationship_keeps_endpoints() {
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let b = g.create_node(vec![], HashMap::new());
    let rid = g.create_relationship("R".to_string(), a, b, HashMap::new());
    g.delete_relationship(rid);
    assert!(g.get_node(a).is_some());
    assert!(g.get_node(b).is_some());
}

#[test]
fn delete_nonexistent_relationship_is_noop() {
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let b = g.create_node(vec![], HashMap::new());
    let rid = g.create_relationship("R".to_string(), a, b, HashMap::new());
    g.delete_relationship(rid + 50);
    assert!(g.get_relationship(rid).is_some());
}

#[test]
fn delete_one_of_parallel_rels_keeps_other() {
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let b = g.create_node(vec![], HashMap::new());
    let r1 = g.create_relationship("R".to_string(), a, b, HashMap::new());
    let r2 = g.create_relationship("R".to_string(), a, b, HashMap::new());
    g.delete_relationship(r1);
    assert!(g.get_relationship(r1).is_none());
    assert!(g.get_relationship(r2).is_some());
    assert!(g.outgoing_rels(a).unwrap().contains(&r2));
    assert_eq!(g.outgoing_rels(a).unwrap().len(), 1);
}

// ===========================================================================
// Cascade deletion (delete_node removes connected relationships)
// ===========================================================================

#[test]
fn delete_node_cascades_outgoing_relationships() {
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let b = g.create_node(vec![], HashMap::new());
    let rid = g.create_relationship("R".to_string(), a, b, HashMap::new());
    g.delete_node(a);
    assert!(g.get_relationship(rid).is_none());
    // The surviving endpoint's incoming adjacency no longer references rid.
    assert!(g.incoming_rels(b).is_none_or(|s| !s.contains(&rid)));
}

#[test]
fn delete_node_cascades_incoming_relationships() {
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let b = g.create_node(vec![], HashMap::new());
    let rid = g.create_relationship("R".to_string(), a, b, HashMap::new());
    g.delete_node(b); // b is the target
    assert!(g.get_relationship(rid).is_none());
    assert!(g.outgoing_rels(a).is_none_or(|s| !s.contains(&rid)));
}

#[test]
fn delete_node_cascades_both_directions() {
    let mut g = Graph::new();
    let center = g.create_node(vec![], HashMap::new());
    let up = g.create_node(vec![], HashMap::new());
    let down = g.create_node(vec![], HashMap::new());
    let r_in = g.create_relationship("IN".to_string(), up, center, HashMap::new());
    let r_out = g.create_relationship("OUT".to_string(), center, down, HashMap::new());
    g.delete_node(center);
    assert!(g.get_relationship(r_in).is_none());
    assert!(g.get_relationship(r_out).is_none());
    assert!(g.all_relationships().is_empty());
}

#[test]
fn delete_node_cascades_self_loop() {
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let rid = g.create_relationship("SELF".to_string(), a, a, HashMap::new());
    g.delete_node(a);
    assert!(g.get_relationship(rid).is_none());
    assert!(g.all_relationships().is_empty());
}

#[test]
fn delete_node_with_parallel_rels_removes_all() {
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let b = g.create_node(vec![], HashMap::new());
    let r1 = g.create_relationship("R".to_string(), a, b, HashMap::new());
    let r2 = g.create_relationship("R".to_string(), a, b, HashMap::new());
    g.delete_node(a);
    assert!(g.get_relationship(r1).is_none());
    assert!(g.get_relationship(r2).is_none());
    assert!(g.all_relationships().is_empty());
}

// ===========================================================================
// restore_node / restore_relationship + id-counter (fetch_max) semantics
// ===========================================================================

#[test]
fn restore_node_sets_explicit_id() {
    let mut g = Graph::new();
    g.restore_node(42, labels(&["Restored"]), props(&[("k", Value::Int(9))]));
    let node = g.get_node(42).unwrap();
    assert_eq!(node.id, 42);
    assert!(g.nodes_by_label("Restored").unwrap().contains(&42));
    assert!(g.nodes_by_property("k", &Value::Int(9)).unwrap().contains(&42));
}

#[test]
fn restore_node_advances_counter_so_next_create_avoids_collision() {
    let mut g = Graph::new();
    g.restore_node(100, vec![], HashMap::new());
    let next = g.create_node(vec![], HashMap::new());
    assert_eq!(next, 101, "create after restore(100) must be 101");
}

#[test]
fn restore_node_lower_id_does_not_rewind_counter() {
    let mut g = Graph::new();
    let high = g.create_node(vec![], HashMap::new()); // 1
    let _ = g.create_node(vec![], HashMap::new()); // 2
    let _ = g.create_node(vec![], HashMap::new()); // 3, counter now 4
    g.restore_node(high, vec![], HashMap::new()); // restore id 1 (low)
    let next = g.create_node(vec![], HashMap::new());
    assert_eq!(next, 4, "fetch_max must not rewind below existing high-water mark");
}

#[test]
fn restore_relationship_sets_explicit_id_and_adjacency() {
    let mut g = Graph::new();
    g.restore_relationship(77, "REL".to_string(), 1, 2, props(&[("w", Value::Int(5))]));
    let rel = g.get_relationship(77).unwrap();
    assert_eq!(rel.id, 77);
    assert_eq!(rel.kind, "REL");
    assert_eq!(rel.from, 1);
    assert_eq!(rel.to, 2);
    assert!(g.outgoing_rels(1).unwrap().contains(&77));
    assert!(g.incoming_rels(2).unwrap().contains(&77));
}

#[test]
fn restore_relationship_advances_rel_counter() {
    let mut g = Graph::new();
    g.restore_relationship(50, "R".to_string(), 1, 2, HashMap::new());
    let next = g.create_relationship("R".to_string(), 1, 2, HashMap::new());
    assert_eq!(next, 51);
}

#[test]
fn restore_node_overwrites_existing_id() {
    let mut g = Graph::new();
    g.restore_node(5, labels(&["First"]), props(&[("v", Value::Int(1))]));
    g.restore_node(5, labels(&["Second"]), props(&[("v", Value::Int(2))]));
    let node = g.get_node(5).unwrap();
    assert_eq!(node.labels, vec!["Second".to_string()]);
    assert_eq!(node.props.get("v"), Some(&Value::Int(2)));
    // New value indexed.
    assert!(g.nodes_by_property("v", &Value::Int(2)).unwrap().contains(&5));
}

// ===========================================================================
// all_nodes / all_relationships accessors
// ===========================================================================

#[test]
fn all_nodes_reflects_inserts_and_deletes() {
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let b = g.create_node(vec![], HashMap::new());
    assert_eq!(node_id_set(&g), [a, b].into_iter().collect());
    g.delete_node(a);
    assert_eq!(node_id_set(&g), [b].into_iter().collect());
}

#[test]
fn all_relationships_reflects_inserts_and_deletes() {
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let b = g.create_node(vec![], HashMap::new());
    let r1 = g.create_relationship("R".to_string(), a, b, HashMap::new());
    let r2 = g.create_relationship("R".to_string(), b, a, HashMap::new());
    assert_eq!(rel_id_set(&g), [r1, r2].into_iter().collect());
    g.delete_relationship(r1);
    assert_eq!(rel_id_set(&g), [r2].into_iter().collect());
}

// ===========================================================================
// set_state — rebuild from external snapshot
// ===========================================================================

#[test]
fn set_state_rebuilds_indexes_and_adjacency() {
    let mut g = Graph::new();
    let mut nodes = HashMap::new();
    nodes.insert(
        10u64,
        Node {
            id: 10,
            labels: labels(&["Person"]),
            props: props(&[("name", s("Zed"))]),
        },
    );
    nodes.insert(
        20u64,
        Node {
            id: 20,
            labels: labels(&["Person"]),
            props: props(&[("name", s("Yan"))]),
        },
    );
    let mut rels = HashMap::new();
    rels.insert(
        5u64,
        Relationship {
            id: 5,
            kind: "KNOWS".to_string(),
            from: 10,
            to: 20,
            props: HashMap::new(),
        },
    );
    g.set_state(nodes, rels);

    assert_eq!(g.all_nodes().len(), 2);
    assert_eq!(g.all_relationships().len(), 1);
    let people = g.nodes_by_label("Person").unwrap();
    assert!(people.contains(&10) && people.contains(&20));
    assert!(g.nodes_by_property("name", &s("Zed")).unwrap().contains(&10));
    assert!(g.outgoing_rels(10).unwrap().contains(&5));
    assert!(g.incoming_rels(20).unwrap().contains(&5));
}

#[test]
fn set_state_replaces_prior_contents() {
    let mut g = Graph::new();
    g.create_node(labels(&["Old"]), HashMap::new());
    g.set_state(HashMap::new(), HashMap::new());
    assert!(g.all_nodes().is_empty());
    assert!(g.all_relationships().is_empty());
    // Old label index entry must no longer match.
    assert!(g.nodes_by_label("Old").is_none_or(|s| s.is_empty()));
}

#[test]
fn set_state_sets_counters_above_max_ids() {
    let mut g = Graph::new();
    let mut nodes = HashMap::new();
    nodes.insert(
        30u64,
        Node { id: 30, labels: vec![], props: HashMap::new() },
    );
    let mut rels = HashMap::new();
    rels.insert(
        15u64,
        Relationship {
            id: 15,
            kind: "R".to_string(),
            from: 30,
            to: 30,
            props: HashMap::new(),
        },
    );
    g.set_state(nodes, rels);
    assert_eq!(g.create_node(vec![], HashMap::new()), 31);
    assert_eq!(g.create_relationship("R".to_string(), 30, 30, HashMap::new()), 16);
}

#[test]
fn set_state_empty_resets_counters_to_one() {
    let mut g = Graph::new();
    g.create_node(vec![], HashMap::new());
    g.create_node(vec![], HashMap::new());
    g.set_state(HashMap::new(), HashMap::new());
    // With empty maps, max id is 0, so next ids restart at 1.
    assert_eq!(g.create_node(vec![], HashMap::new()), 1);
    assert_eq!(g.create_relationship("R".to_string(), 1, 1, HashMap::new()), 1);
}

// ===========================================================================
// Traversal — manual BFS/DFS over the public adjacency API
// ===========================================================================

/// Build a chain: n0 -> n1 -> n2 -> ... -> n{len-1}. Returns node ids.
fn build_chain(g: &mut Graph, len: usize) -> Vec<NodeId> {
    let ids: Vec<NodeId> = (0..len).map(|_| g.create_node(vec![], HashMap::new())).collect();
    for w in ids.windows(2) {
        g.create_relationship("NEXT".to_string(), w[0], w[1], HashMap::new());
    }
    ids
}

/// Follow outgoing relationships to the directly-reachable neighbor ids.
fn out_neighbors(g: &Graph, node: NodeId) -> HashSet<NodeId> {
    g.outgoing_rels(node)
        .map(|rels| {
            rels.iter()
                .filter_map(|rid| g.get_relationship(*rid).map(|r| r.to))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn traversal_single_hop_neighbors() {
    let mut g = Graph::new();
    let ids = build_chain(&mut g, 3);
    assert_eq!(out_neighbors(&g, ids[0]), [ids[1]].into_iter().collect());
    assert_eq!(out_neighbors(&g, ids[1]), [ids[2]].into_iter().collect());
    assert!(out_neighbors(&g, ids[2]).is_empty());
}

#[test]
fn traversal_variable_length_reachability_full_chain() {
    // Variable-length [*]: reach every downstream node from the head.
    let mut g = Graph::new();
    let ids = build_chain(&mut g, 6);
    let reached = reachable_unbounded(&g, ids[0]);
    let expected: HashSet<NodeId> = ids[1..].iter().copied().collect();
    assert_eq!(reached, expected);
}

/// Unbounded reachability (variable-length [*]) excluding the start node,
/// with a visited-set so cycles terminate.
fn reachable_unbounded(g: &Graph, start: NodeId) -> HashSet<NodeId> {
    let mut visited = HashSet::new();
    let mut stack = vec![start];
    while let Some(n) = stack.pop() {
        for nb in out_neighbors(g, n) {
            // The visited set guarantees termination even with cycles.
            if visited.insert(nb) {
                stack.push(nb);
            }
        }
    }
    visited.remove(&start);
    visited
}

#[test]
fn traversal_bounded_length_two_hops() {
    // Variable-length [*1..2]: reach nodes within 1 or 2 hops.
    let mut g = Graph::new();
    let ids = build_chain(&mut g, 5);
    let reached = reachable_within(&g, ids[0], 2);
    let expected: HashSet<NodeId> = [ids[1], ids[2]].into_iter().collect();
    assert_eq!(reached, expected);
}

/// Bounded reachability up to `max_hops` (BFS by level).
fn reachable_within(g: &Graph, start: NodeId, max_hops: usize) -> HashSet<NodeId> {
    let mut result = HashSet::new();
    let mut frontier: HashSet<NodeId> = [start].into_iter().collect();
    for _ in 0..max_hops {
        let mut next = HashSet::new();
        for &n in &frontier {
            for nb in out_neighbors(g, n) {
                next.insert(nb);
            }
        }
        result.extend(next.iter().copied());
        frontier = next;
        if frontier.is_empty() {
            break;
        }
    }
    result.remove(&start);
    result
}

#[test]
fn traversal_zero_hops_reaches_nothing() {
    let mut g = Graph::new();
    let ids = build_chain(&mut g, 3);
    assert!(reachable_within(&g, ids[0], 0).is_empty());
}

#[test]
fn traversal_cycle_terminates_with_visited_set() {
    // a -> b -> c -> a : an unbounded walk must terminate.
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let b = g.create_node(vec![], HashMap::new());
    let c = g.create_node(vec![], HashMap::new());
    g.create_relationship("R".to_string(), a, b, HashMap::new());
    g.create_relationship("R".to_string(), b, c, HashMap::new());
    g.create_relationship("R".to_string(), c, a, HashMap::new());
    let reached = reachable_unbounded(&g, a);
    // From a, reach b, c, and back to a (a re-enters visited via the cycle).
    assert!(reached.contains(&b));
    assert!(reached.contains(&c));
}

#[test]
fn traversal_self_loop_terminates() {
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    g.create_relationship("LOOP".to_string(), a, a, HashMap::new());
    // Must not hang. `a` is its own out-neighbor, but `reachable_within`
    // removes the start node, so the bounded result is empty — the point
    // of this test is that the bounded walk terminates rather than looping.
    let reached = reachable_within(&g, a, 3);
    assert!(reached.is_empty());
    // Unbounded reachability also terminates and excludes the start.
    let unbounded = reachable_unbounded(&g, a);
    assert!(unbounded.is_empty());
}

/// Enumerate all simple directed paths (relationship-uniqueness / trail:
/// no relationship reused) from `start`, returned as sequences of node ids.
fn all_trails(g: &Graph, start: NodeId) -> Vec<Vec<NodeId>> {
    fn walk(
        g: &Graph,
        node: NodeId,
        used_rels: &mut HashSet<RelId>,
        path: &mut Vec<NodeId>,
        out: &mut Vec<Vec<NodeId>>,
    ) {
        out.push(path.clone());
        if let Some(rels) = g.outgoing_rels(node) {
            // Deterministic order for stable assertions.
            let mut sorted: Vec<RelId> = rels.iter().copied().collect();
            sorted.sort_unstable();
            for rid in sorted {
                if used_rels.contains(&rid) {
                    continue; // trail semantics: each relationship used at most once
                }
                if let Some(rel) = g.get_relationship(rid) {
                    used_rels.insert(rid);
                    path.push(rel.to);
                    walk(g, rel.to, used_rels, path, out);
                    path.pop();
                    used_rels.remove(&rid);
                }
            }
        }
    }
    let mut out = Vec::new();
    let mut used = HashSet::new();
    let mut path = vec![start];
    walk(g, start, &mut used, &mut path, &mut out);
    out
}

#[test]
fn trail_does_not_reuse_relationship_on_cycle() {
    // Triangle cycle a->b->c->a. A trail from a may traverse each edge once,
    // producing the path [a, b, c, a] but never reusing an edge to loop forever.
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let b = g.create_node(vec![], HashMap::new());
    let c = g.create_node(vec![], HashMap::new());
    g.create_relationship("R".to_string(), a, b, HashMap::new());
    g.create_relationship("R".to_string(), b, c, HashMap::new());
    g.create_relationship("R".to_string(), c, a, HashMap::new());
    let trails = all_trails(&g, a);
    // The longest trail returns to a after using all three distinct edges.
    let longest = trails.iter().map(|t| t.len()).max().unwrap();
    assert_eq!(longest, 4, "trail [a,b,c,a] uses 3 edges, 4 node-steps");
    assert!(trails.contains(&vec![a, b, c, a]));
    // No trail exceeds the edge count (would require reusing an edge).
    assert!(trails.iter().all(|t| t.len() <= 4));
}

#[test]
fn trail_distinguishes_parallel_edges() {
    // Two parallel edges a=>b. Trail semantics treat them as distinct,
    // so there are two one-step trails to b (plus the empty start trail).
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let b = g.create_node(vec![], HashMap::new());
    g.create_relationship("R".to_string(), a, b, HashMap::new());
    g.create_relationship("R".to_string(), a, b, HashMap::new());
    let trails = all_trails(&g, a);
    let to_b = trails.iter().filter(|t| t.as_slice() == [a, b]).count();
    assert_eq!(to_b, 2, "each parallel edge yields its own trail");
}

#[test]
fn trail_diamond_two_distinct_paths() {
    // a->b->d and a->c->d : two distinct trails reach d.
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let b = g.create_node(vec![], HashMap::new());
    let c = g.create_node(vec![], HashMap::new());
    let d = g.create_node(vec![], HashMap::new());
    g.create_relationship("R".to_string(), a, b, HashMap::new());
    g.create_relationship("R".to_string(), a, c, HashMap::new());
    g.create_relationship("R".to_string(), b, d, HashMap::new());
    g.create_relationship("R".to_string(), c, d, HashMap::new());
    let trails = all_trails(&g, a);
    assert!(trails.contains(&vec![a, b, d]));
    assert!(trails.contains(&vec![a, c, d]));
}

// ===========================================================================
// Pattern-match shapes (filter by label / property / rel-kind over public API)
// ===========================================================================

/// Match (start)-[:KIND]->(x) and return the matched `x` ids.
fn match_out_by_kind(g: &Graph, start: NodeId, kind: &str) -> HashSet<NodeId> {
    g.outgoing_rels(start)
        .map(|rels| {
            rels.iter()
                .filter_map(|rid| g.get_relationship(*rid))
                .filter(|r| r.kind == kind)
                .map(|r| r.to)
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn pattern_match_relationship_kind_filter() {
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let b = g.create_node(vec![], HashMap::new());
    let c = g.create_node(vec![], HashMap::new());
    g.create_relationship("KNOWS".to_string(), a, b, HashMap::new());
    g.create_relationship("LIKES".to_string(), a, c, HashMap::new());
    assert_eq!(match_out_by_kind(&g, a, "KNOWS"), [b].into_iter().collect());
    assert_eq!(match_out_by_kind(&g, a, "LIKES"), [c].into_iter().collect());
    assert!(match_out_by_kind(&g, a, "HATES").is_empty());
}

#[test]
fn pattern_match_node_by_label_and_property_intersection() {
    // (:Person {active:true}) — intersect label index with property index.
    let mut g = Graph::new();
    let p1 = g.create_node(labels(&["Person"]), props(&[("active", Value::Bool(true))]));
    let _p2 = g.create_node(labels(&["Person"]), props(&[("active", Value::Bool(false))]));
    let _c = g.create_node(labels(&["Company"]), props(&[("active", Value::Bool(true))]));
    let persons = g.nodes_by_label("Person").unwrap();
    let actives = g.nodes_by_property("active", &Value::Bool(true)).unwrap();
    let matched: HashSet<NodeId> = persons.intersection(actives).copied().collect();
    assert_eq!(matched, [p1].into_iter().collect());
}

#[test]
fn pattern_match_directed_distinguishes_from_and_to() {
    // (a)-[:R]->(b) must not match as (b)-[:R]->(a).
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let b = g.create_node(vec![], HashMap::new());
    g.create_relationship("R".to_string(), a, b, HashMap::new());
    assert_eq!(match_out_by_kind(&g, a, "R"), [b].into_iter().collect());
    assert!(match_out_by_kind(&g, b, "R").is_empty());
}

#[test]
fn pattern_match_undirected_uses_both_adjacency_sets() {
    // (a)-[:R]-(x) undirected: combine outgoing.to and incoming.from.
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let b = g.create_node(vec![], HashMap::new());
    let c = g.create_node(vec![], HashMap::new());
    g.create_relationship("R".to_string(), a, b, HashMap::new()); // a -> b
    g.create_relationship("R".to_string(), c, a, HashMap::new()); // c -> a
    let mut neighbors: HashSet<NodeId> = HashSet::new();
    if let Some(out) = g.outgoing_rels(a) {
        for rid in out {
            neighbors.insert(g.get_relationship(*rid).unwrap().to);
        }
    }
    if let Some(inc) = g.incoming_rels(a) {
        for rid in inc {
            neighbors.insert(g.get_relationship(*rid).unwrap().from);
        }
    }
    assert_eq!(neighbors, [b, c].into_iter().collect());
}

#[test]
fn pattern_match_no_match_on_empty_label_after_delete() {
    let mut g = Graph::new();
    let id = g.create_node(labels(&["Temp"]), HashMap::new());
    g.delete_node(id);
    let matched = g.nodes_by_label("Temp").map(|s| s.len()).unwrap_or(0);
    assert_eq!(matched, 0);
}

// ===========================================================================
// Large-graph behavior
// ===========================================================================

#[test]
fn large_graph_node_count_and_lookups() {
    let mut g = Graph::new();
    const N: u64 = 2_000;
    for i in 0..N {
        let lbl = if i % 2 == 0 { "Even" } else { "Odd" };
        g.create_node(labels(&[lbl]), props(&[("i", Value::Int(i as i64))]));
    }
    assert_eq!(g.all_nodes().len() as u64, N);
    assert_eq!(g.nodes_by_label("Even").unwrap().len() as u64, N / 2);
    assert_eq!(g.nodes_by_label("Odd").unwrap().len() as u64, N / 2);
    // Spot-check property lookup.
    let want = N - 1;
    let set = g.nodes_by_property("i", &Value::Int(want as i64)).unwrap();
    assert_eq!(set.len(), 1);
}

#[test]
fn large_chain_full_traversal() {
    let mut g = Graph::new();
    let ids = build_chain(&mut g, 500);
    let reached = reachable_unbounded(&g, ids[0]);
    assert_eq!(reached.len(), 499, "all downstream nodes reachable in a 500-chain");
    assert!(reached.contains(ids.last().unwrap()));
}

#[test]
fn large_star_fanout_adjacency() {
    // One hub with many spokes.
    let mut g = Graph::new();
    let hub = g.create_node(labels(&["Hub"]), HashMap::new());
    const SPOKES: usize = 1_000;
    for _ in 0..SPOKES {
        let spoke = g.create_node(vec![], HashMap::new());
        g.create_relationship("SPOKE".to_string(), hub, spoke, HashMap::new());
    }
    assert_eq!(g.outgoing_rels(hub).unwrap().len(), SPOKES);
    assert_eq!(out_neighbors(&g, hub).len(), SPOKES);
}

#[test]
fn large_graph_delete_hub_cascades_all_edges() {
    let mut g = Graph::new();
    let hub = g.create_node(vec![], HashMap::new());
    const SPOKES: usize = 300;
    for _ in 0..SPOKES {
        let spoke = g.create_node(vec![], HashMap::new());
        g.create_relationship("S".to_string(), hub, spoke, HashMap::new());
    }
    assert_eq!(g.all_relationships().len(), SPOKES);
    g.delete_node(hub);
    assert!(g.all_relationships().is_empty(), "hub delete cascades all spoke edges");
    assert_eq!(g.all_nodes().len(), SPOKES, "spoke nodes survive");
}

// ===========================================================================
// Struct shape / serde derive sanity (Node, Relationship are Clone + Serialize)
// ===========================================================================

#[test]
fn node_is_cloneable_and_fields_public() {
    let n = Node {
        id: 1,
        labels: labels(&["A"]),
        props: props(&[("k", Value::Int(1))]),
    };
    let c = n.clone();
    assert_eq!(c.id, 1);
    assert_eq!(c.labels, vec!["A".to_string()]);
    assert_eq!(c.props.get("k"), Some(&Value::Int(1)));
}

#[test]
fn relationship_is_cloneable_and_fields_public() {
    let r = Relationship {
        id: 9,
        kind: "K".to_string(),
        from: 1,
        to: 2,
        props: HashMap::new(),
    };
    let c = r.clone();
    assert_eq!((c.id, c.from, c.to), (9, 1, 2));
    assert_eq!(c.kind, "K");
}

// ===========================================================================
// Mixed end-to-end scenario
// ===========================================================================

#[test]
fn end_to_end_social_graph_scenario() {
    let mut g = Graph::new();
    let alice = g.create_node(labels(&["Person"]), props(&[("name", s("Alice"))]));
    let bob = g.create_node(labels(&["Person"]), props(&[("name", s("Bob"))]));
    let carol = g.create_node(labels(&["Person"]), props(&[("name", s("Carol"))]));
    let acme = g.create_node(labels(&["Company"]), props(&[("name", s("Acme"))]));

    g.create_relationship("KNOWS".to_string(), alice, bob, HashMap::new());
    g.create_relationship("KNOWS".to_string(), bob, carol, HashMap::new());
    let employs = g.create_relationship("WORKS_AT".to_string(), alice, acme, HashMap::new());

    // Label query.
    assert_eq!(g.nodes_by_label("Person").unwrap().len(), 3);
    assert_eq!(g.nodes_by_label("Company").unwrap().len(), 1);

    // Property query.
    assert!(g.nodes_by_property("name", &s("Alice")).unwrap().contains(&alice));

    // Friend-of-friend reachability from Alice through KNOWS-only graph.
    assert_eq!(match_out_by_kind(&g, alice, "KNOWS"), [bob].into_iter().collect());

    // Update a property and confirm re-index.
    g.update_node(alice, props(&[("name", s("Alicia"))]));
    assert!(g.nodes_by_property("name", &s("Alicia")).unwrap().contains(&alice));
    assert!(g
        .nodes_by_property("name", &s("Alice"))
        .is_none_or(|set| !set.contains(&alice)));

    // Delete the company; the WORKS_AT edge must cascade away.
    g.delete_node(acme);
    assert!(g.get_relationship(employs).is_none());
    assert!(g.nodes_by_label("Company").is_none_or(|s| s.is_empty()));

    // KNOWS chain still intact.
    assert_eq!(match_out_by_kind(&g, bob, "KNOWS"), [carol].into_iter().collect());
}

// ===========================================================================
// Shared index-bucket integrity (the gap the author missed: every prior
// delete/update test removed the *sole* holder of a label/property value,
// so a bug that wiped the whole bucket instead of just one id would pass.
// These tests force a second node to share the bucket.)
// ===========================================================================

#[test]
fn delete_node_keeps_other_nodes_in_shared_label_bucket() {
    let mut g = Graph::new();
    let a = g.create_node(labels(&["Person"]), HashMap::new());
    let b = g.create_node(labels(&["Person"]), HashMap::new());
    g.delete_node(a);
    // b must remain indexed under the shared label; only a is gone.
    let people = g.nodes_by_label("Person").unwrap();
    assert!(people.contains(&b));
    assert!(!people.contains(&a));
    assert_eq!(people.len(), 1);
}

#[test]
fn delete_node_keeps_other_nodes_in_shared_property_bucket() {
    let mut g = Graph::new();
    let a = g.create_node(vec![], props(&[("city", s("NYC"))]));
    let b = g.create_node(vec![], props(&[("city", s("NYC"))]));
    g.delete_node(a);
    let nyc = g.nodes_by_property("city", &s("NYC")).unwrap();
    assert!(nyc.contains(&b) && !nyc.contains(&a));
    assert_eq!(nyc.len(), 1);
}

#[test]
fn update_node_keeps_other_nodes_in_shared_property_bucket() {
    // a and b both have name=Alice. Re-indexing a away from Alice must NOT
    // evict b — the removal in update_node only removes a's own id.
    let mut g = Graph::new();
    let a = g.create_node(vec![], props(&[("name", s("Alice"))]));
    let b = g.create_node(vec![], props(&[("name", s("Alice"))]));
    g.update_node(a, props(&[("name", s("Bob"))]));
    let alices = g.nodes_by_property("name", &s("Alice")).unwrap();
    assert!(alices.contains(&b), "b must survive a's re-index");
    assert!(!alices.contains(&a));
    assert_eq!(alices.len(), 1);
    assert!(g.nodes_by_property("name", &s("Bob")).unwrap().contains(&a));
}

#[test]
fn delete_node_keeps_shared_dangling_endpoint_adjacency() {
    // Two relationships from distinct sources both target node `t`.
    // Deleting one source must cascade only its own edge out of t's incoming
    // set, leaving the other edge intact.
    let mut g = Graph::new();
    let s1 = g.create_node(vec![], HashMap::new());
    let s2 = g.create_node(vec![], HashMap::new());
    let t = g.create_node(vec![], HashMap::new());
    let r1 = g.create_relationship("R".to_string(), s1, t, HashMap::new());
    let r2 = g.create_relationship("R".to_string(), s2, t, HashMap::new());
    g.delete_node(s1);
    assert!(g.get_relationship(r1).is_none());
    assert!(g.get_relationship(r2).is_some());
    let inc = g.incoming_rels(t).unwrap();
    assert!(inc.contains(&r2) && !inc.contains(&r1));
    assert_eq!(inc.len(), 1);
}

// ===========================================================================
// update_node finer-grained semantics
// ===========================================================================

#[test]
fn update_node_same_value_is_stable_remove_then_readd() {
    // update_node removes every old (k,v) for the id then re-adds the merged
    // set. Re-writing a prop to its EXACT current value must leave it indexed.
    let mut g = Graph::new();
    let id = g.create_node(vec![], props(&[("k", Value::Int(7))]));
    g.update_node(id, props(&[("k", Value::Int(7))]));
    let node = g.get_node(id).unwrap();
    assert_eq!(node.props.get("k"), Some(&Value::Int(7)));
    assert!(g.nodes_by_property("k", &Value::Int(7)).unwrap().contains(&id));
}

#[test]
fn update_node_overwrite_keeps_untouched_props_indexed() {
    // Node has two props; update only one. The other must remain queryable.
    // (update_node strips ALL of the node's old (k,v) before re-adding, so an
    // off-by-one in the re-add loop would drop the untouched prop.)
    let mut g = Graph::new();
    let id = g.create_node(vec![], props(&[("a", Value::Int(1)), ("b", s("keep"))]));
    g.update_node(id, props(&[("a", Value::Int(2))]));
    let node = g.get_node(id).unwrap();
    assert_eq!(node.props.get("a"), Some(&Value::Int(2)));
    assert_eq!(node.props.get("b"), Some(&s("keep")));
    // Untouched prop still indexed; old value of a dropped.
    assert!(g.nodes_by_property("b", &s("keep")).unwrap().contains(&id));
    assert!(g.nodes_by_property("a", &Value::Int(2)).unwrap().contains(&id));
    assert!(g.nodes_by_property("a", &Value::Int(1)).is_none_or(|set| !set.contains(&id)));
}

#[test]
fn update_node_adds_first_prop_to_propless_node() {
    // Node created with empty props; update introduces its first property,
    // which must then be reachable through the property index.
    let mut g = Graph::new();
    let id = g.create_node(labels(&["Bare"]), HashMap::new());
    assert!(g.nodes_by_property("fresh", &Value::Int(1)).is_none());
    g.update_node(id, props(&[("fresh", Value::Int(1))]));
    assert!(g.nodes_by_property("fresh", &Value::Int(1)).unwrap().contains(&id));
}

// ===========================================================================
// restore_node / restore_relationship overwrite: stale-index prevention.
// ===========================================================================

#[test]
fn restore_node_overwrite_removes_stale_label_index_entry() {
    let mut g = Graph::new();
    g.restore_node(6, labels(&["First"]), HashMap::new());
    g.restore_node(5, labels(&["First"]), HashMap::new());
    g.restore_node(5, labels(&["Second"]), HashMap::new());
    assert_eq!(g.get_node(5).unwrap().labels, vec!["Second".to_string()]);
    assert!(g.nodes_by_label("Second").unwrap().contains(&5));
    assert!(
        !g.nodes_by_label("First").unwrap().contains(&5),
        "restore_node must scrub prior label index entries"
    );
    assert!(g.nodes_by_label("First").unwrap().contains(&6));
}

#[test]
fn restore_node_overwrite_removes_stale_property_index_entry() {
    let mut g = Graph::new();
    g.restore_node(6, vec![], props(&[("v", Value::Int(1))]));
    g.restore_node(5, vec![], props(&[("v", Value::Int(1))]));
    g.restore_node(5, vec![], props(&[("v", Value::Int(2))]));
    assert_eq!(g.get_node(5).unwrap().props.get("v"), Some(&Value::Int(2)));
    assert!(g.nodes_by_property("v", &Value::Int(2)).unwrap().contains(&5));
    assert!(
        !g.nodes_by_property("v", &Value::Int(1)).unwrap().contains(&5),
        "restore_node must scrub the prior property value"
    );
    assert!(g.nodes_by_property("v", &Value::Int(1)).unwrap().contains(&6));
}

#[test]
fn restore_relationship_overwrite_removes_stale_adjacency() {
    let mut g = Graph::new();
    g.restore_relationship(8, "R".to_string(), 1, 2, HashMap::new());
    g.restore_relationship(7, "R".to_string(), 1, 2, HashMap::new());
    g.restore_relationship(7, "R".to_string(), 3, 4, HashMap::new());
    let rel = g.get_relationship(7).unwrap();
    assert_eq!((rel.from, rel.to), (3, 4));
    assert!(g.outgoing_rels(3).unwrap().contains(&7));
    assert!(g.incoming_rels(4).unwrap().contains(&7));
    assert!(
        !g.outgoing_rels(1).unwrap().contains(&7),
        "restore_relationship must scrub the prior 'from' adjacency"
    );
    assert!(
        !g.incoming_rels(2).unwrap().contains(&7),
        "restore_relationship must scrub the prior 'to' adjacency"
    );
    assert!(g.outgoing_rels(1).unwrap().contains(&8));
    assert!(g.incoming_rels(2).unwrap().contains(&8));
}

#[test]
fn restore_node_with_id_zero_is_storable() {
    // Boundary: id 0 is a valid NodeId (u64). It can be restored and fetched,
    // and the counter advances to 1 (0 + 1).
    let mut g = Graph::new();
    g.restore_node(0, labels(&["Zero"]), HashMap::new());
    assert!(g.get_node(0).is_some());
    assert!(g.nodes_by_label("Zero").unwrap().contains(&0));
    // fetch_max(0 + 1) leaves the counter at 1, so the next create is 1.
    assert_eq!(g.create_node(vec![], HashMap::new()), 1);
}

#[test]
fn restore_relationship_with_id_zero_is_storable() {
    let mut g = Graph::new();
    g.restore_relationship(0, "Z".to_string(), 1, 2, HashMap::new());
    assert!(g.get_relationship(0).is_some());
    assert!(g.outgoing_rels(1).unwrap().contains(&0));
    assert_eq!(g.create_relationship("R".to_string(), 1, 2, HashMap::new()), 1);
}

#[test]
fn restore_node_then_delete_scrubs_indexes() {
    // delete_node operates off the live node record, so deleting a restored
    // node cleans the entries that record put in place.
    let mut g = Graph::new();
    g.restore_node(9, labels(&["R"]), props(&[("k", s("v"))]));
    g.delete_node(9);
    assert!(g.get_node(9).is_none());
    assert!(g.nodes_by_label("R").is_none_or(|s| !s.contains(&9)));
    assert!(g.nodes_by_property("k", &s("v")).is_none_or(|s| !s.contains(&9)));
}

// ===========================================================================
// Float-as-index-key boundary cases (Value Eq is on bits, Hash is on bits).
// ===========================================================================

#[test]
fn nodes_by_property_finds_nan_via_bit_equality() {
    // f64::NAN != f64::NAN, but Value derives Eq and compares Float by raw
    // bits — so the *same* NaN bit pattern is a usable, findable index key.
    let mut g = Graph::new();
    let nan = Value::from_f64(f64::NAN);
    let id = g.create_node(vec![], props(&[("x", nan.clone())]));
    let found = g.nodes_by_property("x", &nan).unwrap();
    assert!(found.contains(&id), "canonical NaN bit pattern is a findable key");
}

#[test]
fn nodes_by_property_distinguishes_positive_and_negative_zero() {
    // +0.0 and -0.0 compare equal as f64 but have distinct bit patterns, so
    // Value::Float keys them separately (Eq/Hash are on bits).
    let mut g = Graph::new();
    let pos = Value::from_f64(0.0);
    let neg = Value::from_f64(-0.0);
    assert_ne!(pos, neg, "distinct bit patterns are distinct Value keys");
    let p = g.create_node(vec![], props(&[("z", pos.clone())]));
    let n = g.create_node(vec![], props(&[("z", neg.clone())]));
    let by_pos = g.nodes_by_property("z", &pos).unwrap();
    assert!(by_pos.contains(&p) && !by_pos.contains(&n));
    let by_neg = g.nodes_by_property("z", &neg).unwrap();
    assert!(by_neg.contains(&n) && !by_neg.contains(&p));
}

#[test]
fn nodes_by_property_int_and_float_are_distinct_keys() {
    // Value::Int(0) and Value::Float(bits of 0.0) are different enum variants,
    // hence different index keys despite numeric "0" intuition.
    let mut g = Graph::new();
    let i = g.create_node(vec![], props(&[("v", Value::Int(0))]));
    let f = g.create_node(vec![], props(&[("v", Value::from_f64(0.0))]));
    let by_int = g.nodes_by_property("v", &Value::Int(0)).unwrap();
    assert!(by_int.contains(&i) && !by_int.contains(&f));
    let by_float = g.nodes_by_property("v", &Value::from_f64(0.0)).unwrap();
    assert!(by_float.contains(&f) && !by_float.contains(&i));
}

// ===========================================================================
// List / Map property keys: Hash collides on length, but Eq is structural.
// The property_index must still discriminate same-length-but-unequal values.
// ===========================================================================

#[test]
fn nodes_by_property_list_value_roundtrips_as_key() {
    let mut g = Graph::new();
    let list = Value::List(vec![Value::Int(1), Value::Int(2)]);
    let id = g.create_node(vec![], props(&[("tags", list.clone())]));
    assert!(g.nodes_by_property("tags", &list).unwrap().contains(&id));
}

#[test]
fn nodes_by_property_discriminates_same_length_different_lists() {
    // [1,2] and [3,4] hash to the same bucket (Hash uses len) but are NOT Eq,
    // so the property index must keep them as distinct keys.
    let mut g = Graph::new();
    let l_a = Value::List(vec![Value::Int(1), Value::Int(2)]);
    let l_b = Value::List(vec![Value::Int(3), Value::Int(4)]);
    assert_ne!(l_a, l_b);
    let a = g.create_node(vec![], props(&[("l", l_a.clone())]));
    let b = g.create_node(vec![], props(&[("l", l_b.clone())]));
    let by_a = g.nodes_by_property("l", &l_a).unwrap();
    assert!(by_a.contains(&a) && !by_a.contains(&b), "same-length lists must not alias");
    let by_b = g.nodes_by_property("l", &l_b).unwrap();
    assert!(by_b.contains(&b) && !by_b.contains(&a));
}

#[test]
fn nodes_by_property_discriminates_same_length_different_maps() {
    let mut g = Graph::new();
    let mut m_a = HashMap::new();
    m_a.insert("k".to_string(), Value::Int(1));
    let mut m_b = HashMap::new();
    m_b.insert("k".to_string(), Value::Int(2));
    let v_a = Value::Map(m_a);
    let v_b = Value::Map(m_b);
    assert_ne!(v_a, v_b);
    let a = g.create_node(vec![], props(&[("m", v_a.clone())]));
    let b = g.create_node(vec![], props(&[("m", v_b.clone())]));
    assert!(g.nodes_by_property("m", &v_a).unwrap().contains(&a));
    assert!(!g.nodes_by_property("m", &v_a).unwrap().contains(&b));
    assert!(g.nodes_by_property("m", &v_b).unwrap().contains(&b));
}

#[test]
fn nodes_by_property_empty_list_and_empty_map_are_distinct_keys() {
    // Both hash to len 0 and are different variants — must not collide.
    let mut g = Graph::new();
    let empty_list = Value::List(vec![]);
    let empty_map = Value::Map(HashMap::new());
    assert_ne!(empty_list, empty_map);
    let a = g.create_node(vec![], props(&[("e", empty_list.clone())]));
    let b = g.create_node(vec![], props(&[("e", empty_map.clone())]));
    assert!(g.nodes_by_property("e", &empty_list).unwrap().contains(&a));
    assert!(!g.nodes_by_property("e", &empty_list).unwrap().contains(&b));
    assert!(g.nodes_by_property("e", &empty_map).unwrap().contains(&b));
}

#[test]
fn nodes_by_property_null_distinct_from_empty_string() {
    // Value::Null and Value::String("") are different variants / keys.
    let mut g = Graph::new();
    let n = g.create_node(vec![], props(&[("p", Value::Null)]));
    let e = g.create_node(vec![], props(&[("p", s(""))]));
    let by_null = g.nodes_by_property("p", &Value::Null).unwrap();
    assert!(by_null.contains(&n) && !by_null.contains(&e));
    let by_empty = g.nodes_by_property("p", &s("")).unwrap();
    assert!(by_empty.contains(&e) && !by_empty.contains(&n));
}

// ===========================================================================
// Property-key string boundaries (empty / whitespace / unicode keys).
// ===========================================================================

#[test]
fn property_key_empty_and_whitespace_and_unicode() {
    let mut g = Graph::new();
    let id = g.create_node(
        vec![],
        props(&[("", Value::Int(1)), (" ", Value::Int(2)), ("名前", s("x"))]),
    );
    assert!(g.nodes_by_property("", &Value::Int(1)).unwrap().contains(&id));
    assert!(g.nodes_by_property(" ", &Value::Int(2)).unwrap().contains(&id));
    assert!(g.nodes_by_property("名前", &s("x")).unwrap().contains(&id));
    // Whitespace keys are distinct from each other and from the empty key.
    assert!(g.nodes_by_property(" ", &Value::Int(1)).is_none());
}

#[test]
fn empty_string_label_is_indexed() {
    // An empty-string label is a valid, distinct index key.
    let mut g = Graph::new();
    let id = g.create_node(labels(&[""]), HashMap::new());
    assert!(g.nodes_by_label("").unwrap().contains(&id));
    assert!(g.nodes_by_label(" ").is_none());
}

// ===========================================================================
// Relationship kind / props boundaries.
// ===========================================================================

#[test]
fn relationship_empty_and_unicode_kind() {
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let b = g.create_node(vec![], HashMap::new());
    let r_empty = g.create_relationship("".to_string(), a, b, HashMap::new());
    let r_uni = g.create_relationship("関係".to_string(), a, b, HashMap::new());
    assert_eq!(g.get_relationship(r_empty).unwrap().kind, "");
    assert_eq!(g.get_relationship(r_uni).unwrap().kind, "関係");
    // Kind is part of the match filter, not the index — both still adjacency-linked.
    assert!(g.outgoing_rels(a).unwrap().contains(&r_empty));
    assert_eq!(match_out_by_kind(&g, a, ""), [b].into_iter().collect());
    assert_eq!(match_out_by_kind(&g, a, "関係"), [b].into_iter().collect());
}

#[test]
fn relationship_stores_rich_value_props() {
    // Relationship props accept every Value variant just like node props.
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let b = g.create_node(vec![], HashMap::new());
    let rid = g.create_relationship(
        "R".to_string(),
        a,
        b,
        props(&[
            ("weight", Value::from_f64(1.5)),
            ("tags", Value::List(vec![s("x"), s("y")])),
            ("flag", Value::Bool(true)),
            ("nada", Value::Null),
        ]),
    );
    let rel = g.get_relationship(rid).unwrap();
    assert_eq!(rel.props.get("weight").unwrap().to_f64(), Some(1.5));
    assert_eq!(rel.props.get("tags"), Some(&Value::List(vec![s("x"), s("y")])));
    assert_eq!(rel.props.get("flag"), Some(&Value::Bool(true)));
    assert_eq!(rel.props.get("nada"), Some(&Value::Null));
}

#[test]
fn self_loop_delete_clears_both_adjacency_sets() {
    // A self-loop registers in both outgoing and incoming for the same node.
    // delete_relationship must clear both (rel.from == rel.to).
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let rid = g.create_relationship("SELF".to_string(), a, a, HashMap::new());
    g.delete_relationship(rid);
    assert!(g.get_relationship(rid).is_none());
    assert!(g.outgoing_rels(a).is_none_or(|s| !s.contains(&rid)));
    assert!(g.incoming_rels(a).is_none_or(|s| !s.contains(&rid)));
    // The node itself survives the edge deletion.
    assert!(g.get_node(a).is_some());
}

// ===========================================================================
// Counter independence and post-set_state interleaving.
// ===========================================================================

#[test]
fn node_and_rel_counters_advance_independently() {
    // Creating many nodes must not move the relationship counter and vice versa.
    let mut g = Graph::new();
    for _ in 0..5 {
        g.create_node(vec![], HashMap::new());
    }
    // First relationship is still id 1 despite 5 nodes existing.
    let r = g.create_relationship("R".to_string(), 1, 2, HashMap::new());
    assert_eq!(r, 1);
    // And the next node is 6, unaffected by the relationship.
    assert_eq!(g.create_node(vec![], HashMap::new()), 6);
}

#[test]
fn create_after_set_state_continues_from_high_water_mark() {
    // set_state sets counters above the max id present; subsequent creates and
    // restores interleave correctly without colliding with snapshot ids.
    let mut g = Graph::new();
    let mut nodes = HashMap::new();
    nodes.insert(7u64, Node { id: 7, labels: vec![], props: HashMap::new() });
    g.set_state(nodes, HashMap::new());
    let a = g.create_node(vec![], HashMap::new());
    assert_eq!(a, 8);
    // A restore of a much higher id then bumps the counter again.
    g.restore_node(100, vec![], HashMap::new());
    assert_eq!(g.create_node(vec![], HashMap::new()), 101);
}

#[test]
fn set_state_with_dangling_relationship_endpoints() {
    // set_state does not validate that rel endpoints exist as nodes; it just
    // rebuilds adjacency from the relationship records as given.
    let mut g = Graph::new();
    let mut rels = HashMap::new();
    rels.insert(
        3u64,
        Relationship {
            id: 3,
            kind: "R".to_string(),
            from: 50,
            to: 60,
            props: HashMap::new(),
        },
    );
    g.set_state(HashMap::new(), rels);
    assert!(g.all_nodes().is_empty());
    assert_eq!(g.all_relationships().len(), 1);
    assert!(g.outgoing_rels(50).unwrap().contains(&3));
    assert!(g.incoming_rels(60).unwrap().contains(&3));
    // Counter follows the rel id even with no nodes.
    assert_eq!(g.create_relationship("R".to_string(), 50, 60, HashMap::new()), 4);
    assert_eq!(g.create_node(vec![], HashMap::new()), 1);
}

#[test]
fn set_state_overwrites_indexes_not_merges() {
    // A node present only in the FIRST state must not survive a second
    // set_state that omits it — including its label/property index entries.
    let mut g = Graph::new();
    let mut first = HashMap::new();
    first.insert(
        1u64,
        Node { id: 1, labels: labels(&["Gone"]), props: props(&[("k", s("v"))]) },
    );
    g.set_state(first, HashMap::new());
    assert!(g.nodes_by_label("Gone").unwrap().contains(&1));

    let mut second = HashMap::new();
    second.insert(
        2u64,
        Node { id: 2, labels: labels(&["Here"]), props: HashMap::new() },
    );
    g.set_state(second, HashMap::new());
    assert!(g.get_node(1).is_none());
    assert!(g.get_node(2).is_some());
    assert!(g.nodes_by_label("Gone").is_none_or(|s| !s.contains(&1)));
    assert!(g.nodes_by_property("k", &s("v")).is_none_or(|s| !s.contains(&1)));
    assert!(g.nodes_by_label("Here").unwrap().contains(&2));
}

#[test]
fn set_state_node_with_many_labels_and_props_fully_indexed() {
    let mut g = Graph::new();
    let mut nodes = HashMap::new();
    nodes.insert(
        1u64,
        Node {
            id: 1,
            labels: labels(&["A", "B", "C"]),
            props: props(&[("x", Value::Int(1)), ("y", s("two"))]),
        },
    );
    g.set_state(nodes, HashMap::new());
    assert!(g.nodes_by_label("A").unwrap().contains(&1));
    assert!(g.nodes_by_label("B").unwrap().contains(&1));
    assert!(g.nodes_by_label("C").unwrap().contains(&1));
    assert!(g.nodes_by_property("x", &Value::Int(1)).unwrap().contains(&1));
    assert!(g.nodes_by_property("y", &s("two")).unwrap().contains(&1));
}

// ===========================================================================
// Adjacency accessor edge cases.
// ===========================================================================

#[test]
fn outgoing_and_incoming_none_for_node_with_no_edges() {
    // A freshly created node with no relationships has no adjacency entry at
    // all (the maps are populated lazily on edge creation), so the accessors
    // return None rather than an empty set.
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    assert!(g.outgoing_rels(a).is_none());
    assert!(g.incoming_rels(a).is_none());
}

#[test]
fn node_with_only_incoming_has_no_outgoing_entry() {
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let b = g.create_node(vec![], HashMap::new());
    g.create_relationship("R".to_string(), a, b, HashMap::new());
    // b only receives an edge.
    assert!(g.incoming_rels(b).is_some());
    assert!(g.outgoing_rels(b).is_none());
    // a only sends.
    assert!(g.outgoing_rels(a).is_some());
    assert!(g.incoming_rels(a).is_none());
}

// ===========================================================================
// Bidirectional / mutual relationship traversal.
// ===========================================================================

#[test]
fn mutual_relationships_two_separate_edges() {
    // a->b and b->a are two independent edges with distinct ids; each shows up
    // in the appropriate adjacency set.
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let b = g.create_node(vec![], HashMap::new());
    let ab = g.create_relationship("R".to_string(), a, b, HashMap::new());
    let ba = g.create_relationship("R".to_string(), b, a, HashMap::new());
    assert_ne!(ab, ba);
    assert!(g.outgoing_rels(a).unwrap().contains(&ab));
    assert!(g.incoming_rels(a).unwrap().contains(&ba));
    assert_eq!(out_neighbors(&g, a), [b].into_iter().collect());
    assert_eq!(out_neighbors(&g, b), [a].into_iter().collect());
}

#[test]
fn trail_mutual_edges_allow_there_and_back() {
    // With a->b and b->a as distinct edges, a trail may go a,b,a (two edges,
    // neither reused) but cannot extend further (both edges now consumed).
    let mut g = Graph::new();
    let a = g.create_node(vec![], HashMap::new());
    let b = g.create_node(vec![], HashMap::new());
    g.create_relationship("R".to_string(), a, b, HashMap::new());
    g.create_relationship("R".to_string(), b, a, HashMap::new());
    let trails = all_trails(&g, a);
    assert!(trails.contains(&vec![a, b, a]));
    let longest = trails.iter().map(|t| t.len()).max().unwrap();
    assert_eq!(longest, 3, "two distinct edges => at most node-steps a,b,a");
}

// ===========================================================================
// Serde round-trip of Node / Relationship (the derive is part of the public
// contract — Zega snapshots persist these structs).
// ===========================================================================

#[test]
fn node_serde_json_roundtrip() {
    let n = Node {
        id: 7,
        labels: labels(&["Person", "Admin"]),
        props: props(&[("name", s("Alice")), ("age", Value::Int(30)), ("nil", Value::Null)]),
    };
    let json = serde_json::to_string(&n).expect("serialize node");
    let back: Node = serde_json::from_str(&json).expect("deserialize node");
    assert_eq!(back.id, n.id);
    assert_eq!(back.labels, n.labels);
    assert_eq!(back.props.get("name"), Some(&s("Alice")));
    assert_eq!(back.props.get("age"), Some(&Value::Int(30)));
    assert_eq!(back.props.get("nil"), Some(&Value::Null));
}

#[test]
fn relationship_serde_json_roundtrip() {
    let r = Relationship {
        id: 11,
        kind: "KNOWS".to_string(),
        from: 1,
        to: 2,
        props: props(&[("since", Value::Int(2020)), ("w", Value::from_f64(0.5))]),
    };
    let json = serde_json::to_string(&r).expect("serialize rel");
    let back: Relationship = serde_json::from_str(&json).expect("deserialize rel");
    assert_eq!(back.id, r.id);
    assert_eq!(back.kind, r.kind);
    assert_eq!((back.from, back.to), (1, 2));
    assert_eq!(back.props.get("since"), Some(&Value::Int(2020)));
    assert_eq!(back.props.get("w").unwrap().to_f64(), Some(0.5));
}

#[test]
fn set_state_from_serde_roundtripped_snapshot() {
    // End-to-end: build a graph, serialize its node/rel maps, deserialize, and
    // rebuild via set_state — the rehydrated graph must answer the same queries.
    let mut g = Graph::new();
    let a = g.create_node(labels(&["Person"]), props(&[("name", s("Ada"))]));
    let b = g.create_node(labels(&["Person"]), HashMap::new());
    let r = g.create_relationship("KNOWS".to_string(), a, b, HashMap::new());

    let nodes_json = serde_json::to_string(g.all_nodes()).expect("ser nodes");
    let rels_json = serde_json::to_string(g.all_relationships()).expect("ser rels");
    let nodes: HashMap<NodeId, Node> = serde_json::from_str(&nodes_json).expect("de nodes");
    let rels: HashMap<RelId, Relationship> = serde_json::from_str(&rels_json).expect("de rels");

    let mut g2 = Graph::new();
    g2.set_state(nodes, rels);
    assert_eq!(g2.all_nodes().len(), 2);
    assert!(g2.nodes_by_label("Person").unwrap().contains(&a));
    assert!(g2.nodes_by_property("name", &s("Ada")).unwrap().contains(&a));
    assert!(g2.outgoing_rels(a).unwrap().contains(&r));
    assert!(g2.incoming_rels(b).unwrap().contains(&r));
}
