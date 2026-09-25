//! Differential tests (zegadb/zega#100, from the review of #113): the
//! compact graph against a plain model with the pre-#100 semantics, under
//! random sequences of creates, restores (label changes included), updates,
//! deletes and relationship writes, with ids that are dense, sparse, far
//! and out of order, and values that collide in every index hash. Every so
//! often the whole observable state is compared: records, point reads, the
//! label, unique, declared, spatial and adjacency indexes, id counters, and
//! the same after a snapshot round trip and a `.graph` round trip.
use super::*;
use crate::index::{IndexKind, IndexSpec, Interval, TextPattern};
use crate::location::{Bounds, Point};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default, Clone)]
struct Model {
    nodes: BTreeMap<NodeId, Node>,
    rels: BTreeMap<RelId, Relationship>,
    next_node: NodeId,
    next_rel: RelId,
}

impl Model {
    fn new() -> Self {
        Model { next_node: 1, next_rel: 1, ..Default::default() }
    }
    fn restore_node(&mut self, id: NodeId, labels: Vec<String>, props: HashMap<String, Value>) {
        self.nodes.insert(id, Node { id, labels, props });
        self.next_node = self.next_node.max(id + 1);
    }
    fn update_node(&mut self, id: NodeId, props: HashMap<String, Value>) {
        if let Some(n) = self.nodes.get_mut(&id) {
            n.props.extend(props);
        }
    }
    fn delete_node(&mut self, id: NodeId) {
        if self.nodes.remove(&id).is_some() {
            self.rels.retain(|_, r| r.from != id && r.to != id);
        }
    }
    fn restore_rel(&mut self, id: RelId, kind: String, from: NodeId, to: NodeId, props: HashMap<String, Value>) {
        self.rels.insert(id, Relationship { id, kind, from, to, props });
        self.next_rel = self.next_rel.max(id + 1);
    }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let mut x = self.0;
        x ^= x >> 33;
        x = x.wrapping_mul(0xff51afd7ed558ccd);
        x ^ (x >> 29)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn value(rng: &mut Rng, collide: bool) -> Value {
    let map = |k: &str, v: i64| Value::Map(Box::new(HashMap::from([(k.to_string(), Value::Int(v))])));
    if collide {
        // Every list of length 2 hashes alike: all share one bucket per key.
        return Value::List(vec![Value::Int(rng.below(4) as i64), Value::Int(rng.below(3) as i64)].into());
    }
    match rng.below(18) {
        0 => Value::from_f64(f64::NAN),
        1 => Value::from_f64(-0.0),
        2 => Value::from_f64(0.0),
        3 => Value::Int(0),
        4 => Value::Int(rng.below(5) as i64),
        5 => Value::from_f64(rng.below(5) as f64 * 0.5),
        6 => Value::from(format!("s{}", rng.below(5))),
        7 => Value::from(""),
        8 => Value::Null,
        9 => Value::Bool(rng.below(2) == 0),
        10 => Value::List(vec![Value::Int(rng.below(3) as i64)].into()),
        11 => Value::List(Box::default()),
        12 => map(["a", "b"][rng.below(2) as usize], rng.below(2) as i64),
        13 => Value::Map(Box::default()),
        14 => Value::Point(Point::new(rng.below(170) as f64 - 85.0, rng.below(350) as f64 - 175.0).unwrap()),
        15 => Value::Vector(Box::new(
            crate::vector::Vector::new(&[rng.below(5) as f32 + 0.5, 1.0, -(rng.below(3) as f32)], crate::vector::Metric::Cosine).unwrap(),
        )),
        16 => Value::from_f64(f64::from_bits(0x7ff8_0000_0000_0001)), // another NaN payload
        _ => Value::from(format!("text {} words", rng.below(4))),
    }
}

const KEYS: usize = 12; // > 8: exercises both the scan and the binary search

fn props(rng: &mut Rng, collide: bool) -> HashMap<String, Value> {
    let n = if rng.below(4) == 0 { rng.below(KEYS as u64 + 1) } else { rng.below(4) };
    (0..n).map(|_| (format!("k{}", rng.below(KEYS as u64)), value(rng, collide))).collect()
}

fn labels(rng: &mut Rng) -> Vec<String> {
    let pool = ["A", "B", "C"];
    let n = rng.below(4);
    (0..n).map(|_| pool[rng.below(3) as usize].to_string()).collect() // order and duplicates kept
}

fn node_id(rng: &mut Rng, m: &Model) -> NodeId {
    match rng.below(20) {
        0 => (1u64 << 40) + rng.below(3),
        1 => 5_000 + rng.below(5_000), // past the dense allowance while small
        2 => m.next_node + rng.below(3),
        _ => rng.below(m.next_node.min(3_000) + 2),
    }
}

fn rel_id(rng: &mut Rng, m: &Model) -> RelId {
    match rng.below(15) {
        0 => (1u64 << 45) + rng.below(2),
        1 => 3_000 + rng.below(4_000),
        _ => rng.below(m.next_rel.min(3_000) + 2),
    }
}

/// `unique` pairs the graph indexes; `D` and every other key are answered
/// by a scan, which must agree.
fn uniques() -> Vec<(String, String)> {
    [("A", "k0"), ("B", "k2"), ("C", "k1"), ("A", "k11")]
        .iter()
        .map(|(ty, field)| (ty.to_string(), field.to_string()))
        .collect()
}

fn specs() -> Vec<IndexSpec> {
    vec![
        IndexSpec { kind: IndexKind::Range, type_name: "A".into(), field: "k0".into() },
        IndexSpec { kind: IndexKind::Text, type_name: "B".into(), field: "k1".into() },
    ]
}

fn sorted<'a>(it: impl Iterator<Item = &'a u64>) -> Vec<u64> {
    let mut v: Vec<u64> = it.copied().collect();
    v.sort_unstable();
    v
}

fn check(g: &Graph, m: &Model, probes: &[(String, Value)], ctx: &str) {
    // Full state, in ascending id order.
    let got: Vec<Node> = g.nodes().map(|n| n.to_node()).collect();
    let want: Vec<Node> = m.nodes.values().cloned().collect();
    assert_eq!(got, want, "nodes {ctx}");
    let got: Vec<Relationship> = g.relationships().map(|r| r.to_relationship()).collect();
    let want: Vec<Relationship> = m.rels.values().cloned().collect();
    assert_eq!(got, want, "rels {ctx}");
    assert_eq!(g.node_count(), m.nodes.len());
    assert_eq!(g.relationship_count(), m.rels.len());
    assert_eq!(g.next_ids(), (m.next_node, m.next_rel), "next ids {ctx}");
    // Point reads.
    for (id, n) in &m.nodes {
        let r = g.get_node(*id).expect("node");
        assert_eq!(r.first_label(), n.labels.first().map(String::as_str));
        for k in 0..KEYS {
            let key = format!("k{k}");
            assert_eq!(r.prop(&key), n.props.get(&key), "prop {key} of {id} {ctx}");
        }
    }
    for id in [0u64, 7, 4_999, 1 << 40, (1 << 40) + 5, u64::MAX] {
        assert_eq!(g.get_node(id).is_some(), m.nodes.contains_key(&id));
        assert_eq!(g.get_relationship(id).is_some(), m.rels.contains_key(&id));
    }
    // Label index.
    for label in ["A", "B", "C", "D"] {
        let got = g.nodes_by_label(label).map(|s| sorted(s.iter())).unwrap_or_default();
        let want: Vec<u64> = m.nodes.values().filter(|n| n.labels.iter().any(|l| l == label)).map(|n| n.id).collect();
        assert_eq!(got, want, "label {label} {ctx}");
    }
    // Unique lookups, indexed and scanned, including values that collide.
    for (key, v) in probes {
        let got = g.nodes_by_property(key, v);
        let want: Vec<u64> = m.nodes.values().filter(|n| n.props.get(key) == Some(v)).map(|n| n.id).collect();
        assert_eq!(got, want, "property {key}={v:?} {ctx}");
        for ty in ["A", "B", "C", "D"] {
            let got = g.unique_matches(ty, key, v);
            let want: Vec<u64> = m
                .nodes
                .values()
                .filter(|n| n.labels.iter().any(|l| l == ty) && n.props.get(key) == Some(v))
                .map(|n| n.id)
                .collect();
            assert_eq!(got, want, "unique {ty} {key}={v:?} {ctx}");
        }
    }
    // Adjacency.
    let ends: BTreeSet<u64> = m.rels.values().flat_map(|r| [r.from, r.to]).chain(m.nodes.keys().copied()).collect();
    for n in ends {
        let out: Vec<u64> = m.rels.values().filter(|r| r.from == n).map(|r| r.id).collect();
        let inc: Vec<u64> = m.rels.values().filter(|r| r.to == n).map(|r| r.id).collect();
        assert_eq!(g.outgoing_rels(n).map(|s| sorted(s.iter())).unwrap_or_default(), out, "out {n} {ctx}");
        assert_eq!(g.incoming_rels(n).map(|s| sorted(s.iter())).unwrap_or_default(), inc, "in {n} {ctx}");
        let mut all: Vec<u64> = out.iter().chain(&inc).copied().collect();
        all.sort_unstable();
        all.dedup();
        assert_eq!(g.node_relationship_ids(n), all, "rel ids {n} {ctx}");
    }
    // Spatial: every stored point is a candidate of the whole world, nothing else.
    let world = Bounds::new(Point::new(-90.0, -180.0).unwrap(), Point::new(90.0, 180.0).unwrap()).unwrap();
    for k in 0..KEYS {
        let key = format!("k{k}");
        let got: BTreeSet<u64> = g.spatial_candidates(&key, world).into_iter().collect();
        let want: BTreeSet<u64> =
            m.nodes.values().filter(|n| matches!(n.props.get(&key), Some(Value::Point(_)))).map(|n| n.id).collect();
        assert_eq!(got, want, "spatial {key} {ctx}");
    }
}

/// Declared indexes kept up incrementally answer as ones built from scratch.
fn check_declared(g: &Graph, m: &Model, ctx: &str) {
    let mut fresh = Graph::new();
    for n in m.nodes.values() {
        fresh.restore_node(n.id, n.labels.clone(), n.props.clone());
    }
    fresh.sync_indexes(&specs());
    for bound in [-1e300, 0.0, 1.0] {
        let iv = Interval::at_least(&serde_json::json!(bound)).unwrap();
        let a: BTreeSet<u64> = g.range_candidates(&["A"], "k0", &iv).unwrap().into_iter().collect();
        let b: BTreeSet<u64> = fresh.range_candidates(&["A"], "k0", &iv).unwrap().into_iter().collect();
        assert_eq!(a, b, "range >= {bound} {ctx}");
    }
    let iv = Interval::at_least(&serde_json::json!("")).unwrap();
    let a: BTreeSet<u64> = g.range_candidates(&["A"], "k0", &iv).unwrap().into_iter().collect();
    let b: BTreeSet<u64> = fresh.range_candidates(&["A"], "k0", &iv).unwrap().into_iter().collect();
    assert_eq!(a, b, "range str {ctx}");
    for pat in [TextPattern::Contains("s1"), TextPattern::StartsWith("text"), TextPattern::EndsWith("ds"), TextPattern::Contains("x")] {
        let a: BTreeSet<u64> = g.text_candidates(&["B"], "k1", pat).unwrap().into_iter().collect();
        let b: BTreeSet<u64> = fresh.text_candidates(&["B"], "k1", pat).unwrap().into_iter().collect();
        assert_eq!(a, b, "text {ctx}");
    }
}

fn run(seed: u64, steps: usize, collide: bool, every: usize) {
    let mut rng = Rng(seed);
    let mut g = Graph::new();
    let mut m = Model::new();
    g.sync_indexes(&specs());
    g.sync_uniques(&uniques());
    let mut probes: Vec<(String, Value)> = Vec::new();
    for step in 0..steps {
        match rng.below(100) {
            0..=24 => {
                let (l, p) = (labels(&mut rng), props(&mut rng, collide));
                let id = g.create_node(l.clone(), p.clone());
                assert_eq!(id, m.next_node);
                m.restore_node(id, l, p);
            }
            25..=34 => {
                let id = node_id(&mut rng, &m);
                let (l, p) = (labels(&mut rng), props(&mut rng, collide));
                g.restore_node(id, l.clone(), p.clone());
                m.restore_node(id, l, p);
            }
            35..=49 => {
                let id = node_id(&mut rng, &m);
                let p = props(&mut rng, collide);
                g.update_node(id, p.clone());
                m.update_node(id, p);
            }
            50..=59 => {
                let id = node_id(&mut rng, &m);
                g.delete_node(id);
                m.delete_node(id);
            }
            60..=79 => {
                let (from, to) = (node_id(&mut rng, &m), node_id(&mut rng, &m));
                let to = if rng.below(10) == 0 { from } else { to }; // self loops
                let kind = ["R", "S"][rng.below(2) as usize].to_string();
                let p = if rng.below(3) == 0 { props(&mut rng, collide) } else { HashMap::new() };
                let id = g.create_relationship(kind.clone(), from, to, p.clone());
                assert_eq!(id, m.next_rel);
                m.restore_rel(id, kind, from, to, p);
            }
            80..=86 => {
                let id = rel_id(&mut rng, &m);
                let (from, to) = (node_id(&mut rng, &m), node_id(&mut rng, &m));
                let p = props(&mut rng, collide);
                g.restore_relationship(id, "R".into(), from, to, p.clone());
                m.restore_rel(id, "R".into(), from, to, p);
            }
            87..=97 => {
                let id = rel_id(&mut rng, &m);
                g.delete_relationship(id);
                m.rels.remove(&id);
            }
            // A statement with another schema: some unique indexes go, and
            // come back later built from the nodes already stored.
            _ => {
                let all = uniques();
                let keep: Vec<_> = all.into_iter().filter(|_| rng.below(2) == 0).collect();
                g.sync_uniques(&keep);
                if rng.below(2) == 0 {
                    g.sync_uniques(&uniques());
                }
            }
        }
        if probes.len() < 60 {
            probes.push((format!("k{}", rng.below(KEYS as u64)), value(&mut rng, collide)));
        }
        if step % every == 0 || step + 1 == steps {
            let ctx = format!("seed {seed} step {step}");
            check(&g, &m, &probes, &ctx);
            check_declared(&g, &m, &ctx);
            // Snapshot round trip: the restored graph is the same graph.
            let bytes = crate::wal::encode_snapshot(&g).unwrap();
            let mut back = Graph::new();
            crate::wal::restore_bytes(&mut back, &bytes).unwrap();
            check(&back, &m, &probes, &format!("{ctx} after snapshot"));
            back.sync_uniques(&uniques());
            check(&back, &m, &probes, &format!("{ctx} after snapshot, uniques declared"));
            // The same through a .graph file, when the graph is exportable
            // (a relationship to a missing node, or a repeated label, is not).
            let mut file = Vec::new();
            if crate::graph_file::write(&g, &Default::default(), "test", &mut file).is_ok() {
                let (mut imported, _) = crate::graph_file::read(&file[..]).unwrap();
                imported.sync_uniques(&uniques());
                check(&imported, &m, &probes, &format!("{ctx} after .graph"));
            }
            // A truncated or corrupted snapshot never replaces the graph.
            if bytes.len() > 20 {
                let mut target = Graph::new();
                target.create_node(vec!["Keep".into()], HashMap::new());
                let cut = 1 + rng.below(bytes.len() as u64 - 1) as usize;
                assert!(crate::wal::restore_bytes(&mut target, &bytes[..cut]).is_err(), "truncated at {cut} decoded {ctx}");
                assert_eq!(target.node_count(), 1);
                assert!(target.nodes_by_label("Keep").is_some());
            }
        }
    }
}

#[test]
fn differential_random() {
    for seed in 1..=40u64 {
        run(seed * 0x9e37_79b9, 1_500, false, 97);
    }
}

#[test]
fn differential_colliding_values() {
    for seed in 1..=15u64 {
        run(seed * 0x51_7cc1, 1_000, true, 97);
    }
}

#[test]
fn differential_long() {
    // A long sequence, checked less often: the state grows large enough
    // that every check walks thousands of nodes.
    run(0xdead_beef, 20_000, false, 2_003);
}

/// Shapes are shared: one node gaining or losing a key must not change the
/// others that shared its shape.
#[test]
fn a_shape_is_never_mutated_in_place() {
    let mut g = Graph::new();
    let a = g.create_node(vec!["T".into()], HashMap::from([("x".into(), Value::Int(1)), ("y".into(), Value::Int(2))]));
    let b = g.create_node(vec!["T".into()], HashMap::from([("x".into(), Value::Int(3)), ("y".into(), Value::Int(4))]));
    g.update_node(a, HashMap::from([("z".into(), Value::Int(9))]));
    let nb = g.get_node(b).unwrap();
    assert_eq!(nb.props().count(), 2);
    assert_eq!(nb.prop("z"), None);
    assert_eq!(nb.prop("y"), Some(&Value::Int(4)));
    // Losing a key: restore a with fewer keys.
    g.restore_node(a, vec!["T".into()], HashMap::from([("x".into(), Value::Int(1))]));
    assert_eq!(g.get_node(b).unwrap().props().count(), 2);
    assert_eq!(g.get_node(a).unwrap().props().count(), 1);
}

/// Every write sequence that churns ids upward keeps answering; ids past
/// the allowance live in the overflow map.
#[test]
fn churning_ids_stay_correct() {
    let mut g = Graph::new();
    let mut live = std::collections::VecDeque::new();
    for i in 0..200_000u64 {
        let id = g.create_node(vec!["T".into()], HashMap::from([("n".into(), Value::Int(i as i64))]));
        live.push_back(id);
        if live.len() > 100 {
            g.delete_node(live.pop_front().unwrap());
        }
    }
    assert_eq!(g.node_count(), 100);
    let ids: Vec<u64> = g.nodes().map(|n| n.id).collect();
    assert_eq!(ids, live.iter().copied().collect::<Vec<_>>());
}
