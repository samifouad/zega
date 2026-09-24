//! The benchmark graph and the measured queries, each as ZQL text (for the
//! in-memory engine) and as a plan (for [`crate::exec`]). One function builds
//! both, so the two engines always run the same query.
//!
//! Graph: `n` Person nodes, 5 short fields each (`handle` unique, `name`,
//! `age`, `city`, `rank` range-indexed) and 3 `follows` relationships each.

use std::collections::HashMap;

use serde_json::json;

use crate::exec::{Cmp, Item, Pred, Sel};
use crate::model::{Dir, Node, Op, Rel, Snapshot, Value};

pub const SCHEMA: &str = "schema {
  type Person {
    handle: String
    name: String
    age: Int
    city: String
    rank: Int
    follows -> Person[]
  }
}
unique { Person { handle } }
index { range Person { rank } }
";

pub const UNIQUES: &[(&str, &str)] = &[("Person", "handle")];
/// Range indexes the engine keeps for [`SCHEMA`]: the index block plus each
/// unique field (`lang::effective_indexes`).
pub const RANGES: &[(&str, &str)] = &[("Person", "rank"), ("Person", "handle")];
pub const FOLLOWS: usize = 3;
pub const CITIES: u64 = 100;

pub fn uniques() -> Vec<(String, String)> {
    UNIQUES.iter().map(|(t, f)| (t.to_string(), f.to_string())).collect()
}

/// splitmix64: deterministic per (seed, i), so any node can be regenerated.
pub fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

pub fn handle(i: u64) -> String {
    format!("p{i}")
}

pub fn node(i: u64, n: u64) -> Node {
    let r = mix(i);
    let props = HashMap::from([
        ("handle".to_string(), Value::String(handle(i))),
        ("name".to_string(), Value::String(format!("n{:08x}", r as u32))),
        ("age".to_string(), Value::Int(18 + (r >> 32) as i64 % 72)),
        ("city".to_string(), Value::String(format!("c{}", (r >> 40) % CITIES))),
        ("rank".to_string(), Value::Int(((r >> 16) % (n / 10).max(1)) as i64)),
    ]);
    Node { id: i, labels: vec!["Person".to_string()], props }
}

/// The `FOLLOWS` targets of node `i`: distinct, never `i`.
pub fn targets(i: u64, n: u64) -> Vec<u64> {
    let mut out = Vec::with_capacity(FOLLOWS);
    let mut k = 0u64;
    while out.len() < FOLLOWS.min(n as usize - 1) {
        let t = 1 + mix(i.wrapping_mul(31).wrapping_add(k).wrapping_add(0xABCD)) % n;
        k += 1;
        if t != i && !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

pub fn rels(i: u64, n: u64) -> Vec<Rel> {
    targets(i, n)
        .into_iter()
        .enumerate()
        .map(|(k, to)| Rel {
            id: (i - 1) * FOLLOWS as u64 + k as u64 + 1,
            kind: "follows".to_string(),
            from: i,
            to,
            props: HashMap::new(),
        })
        .collect()
}

/// Nodes `lo..=hi` and their relationships as one batch of writes.
pub fn ops(lo: u64, hi: u64, n: u64) -> Vec<Op> {
    let mut out = Vec::new();
    for i in lo..=hi {
        let node = node(i, n);
        out.push(Op::InsertNode { id: node.id, labels: node.labels, props: node.props });
    }
    for i in lo..=hi {
        for rel in rels(i, n) {
            out.push(Op::InsertRel { id: rel.id, kind: rel.kind, from: rel.from, to: rel.to, props: rel.props });
        }
    }
    out
}

/// The whole graph as `Zega::restore_bytes` input.
pub fn snapshot_bytes(n: u64) -> Vec<u8> {
    let mut snapshot = Snapshot { nodes: HashMap::new(), relationships: HashMap::new() };
    for i in 1..=n {
        snapshot.nodes.insert(i, node(i, n));
        for rel in rels(i, n) {
            snapshot.relationships.insert(rel.id, rel);
        }
    }
    bincode::serialize(&snapshot).expect("snapshot encodes")
}

fn props(names: &[&str]) -> Vec<Item> {
    names.iter().map(|n| Item::Prop(n.to_string())).collect()
}

fn follows(target: Sel) -> Item {
    Item::Walk { field: "follows".into(), rel: "follows".into(), dir: Dir::Out, many: true, target }
}

fn person(cond: Vec<Pred>, limit: Option<usize>, items: Vec<Item>) -> Sel {
    Sel { ty: "Person".into(), cond, limit, items }
}

/// One measured query: its name, ZQL text and plan.
pub struct Query {
    pub kind: &'static str,
    pub zql: String,
    pub plan: Sel,
}

/// Point read by the unique key.
pub fn point(i: u64) -> Query {
    Query {
        kind: "point read (unique key)",
        zql: format!("{{ Person(handle: \"{}\") {{ handle name age city rank }} }}", handle(i)),
        plan: person(vec![Pred::Eq("handle".into(), json!(handle(i)))], None, props(&["handle", "name", "age", "city", "rank"])),
    }
}

/// Range filter on the indexed `rank` (about 20 rows).
pub fn filter(rank: u64) -> Query {
    Query {
        kind: "indexed filter (~20 rows)",
        zql: format!("{{ Person(rank >= {rank} && rank <= {}) {{ handle name age }} }}", rank + 1),
        plan: person(
            vec![Pred::Cmp("rank".into(), Cmp::Gte, json!(rank)), Pred::Cmp("rank".into(), Cmp::Lte, json!(rank + 1))],
            None,
            props(&["handle", "name", "age"]),
        ),
    }
}

/// 2 hops from one node found by its unique key: 3 + 9 neighbors.
pub fn two_hop(i: u64) -> Query {
    let hop2 = person(vec![], None, props(&["handle", "name"]));
    let hop1 = person(vec![], None, vec![Item::Prop("handle".into()), follows(hop2)]);
    Query {
        kind: "2-hop traversal (12 nodes)",
        zql: format!(
            "{{ Person(handle: \"{}\") {{ handle follows -> Person {{ handle follows -> Person {{ handle name }} }} }} }}",
            handle(i)
        ),
        plan: person(vec![Pred::Eq("handle".into(), json!(handle(i)))], None, vec![Item::Prop("handle".into()), follows(hop1)]),
    }
}

/// Unindexed filter with a limit: stops after 10 matches (~1,000 rows read).
pub fn scan_limit(city: u64) -> Query {
    Query {
        kind: "scan + limit 10 (unindexed, 1% match)",
        zql: format!("{{ Person(city: \"c{city}\") limit 10 {{ handle name }} }}"),
        plan: person(vec![Pred::Eq("city".into(), json!(format!("c{city}")))], Some(10), props(&["handle", "name"])),
    }
}

/// Unindexed filter that matches nothing: reads every node.
pub fn scan_all() -> Query {
    Query {
        kind: "full scan, no match (worst case)",
        zql: "{ Person(name: \"nobody\") limit 10 { handle } }".to_string(),
        plan: person(vec![Pred::Eq("name".into(), json!("nobody"))], Some(10), props(&["handle"])),
    }
}

/// Create one node: 5 fields, a unique check, two index entries.
pub fn create(k: u64) -> Query {
    let h = format!("new{k}");
    Query {
        kind: "create node (write)",
        zql: format!(
            "mutation {{ Person(handle: \"{h}\" && name: \"fresh\" && age: 30 && city: \"c1\" && rank: 5) {{ handle rank }} }}"
        ),
        plan: person(
            vec![
                Pred::Eq("handle".into(), json!(h)),
                Pred::Eq("name".into(), json!("fresh")),
                Pred::Eq("age".into(), json!(30)),
                Pred::Eq("city".into(), json!("c1")),
                Pred::Eq("rank".into(), json!(5)),
            ],
            None,
            props(&["handle", "rank"]),
        ),
    }
}

/// Find a node by its unique key and link it to 3 others found the same way.
pub fn link(from: &str, links: [u64; 3]) -> Query {
    let link = |t: u64| {
        follows(person(vec![Pred::Eq("handle".into(), json!(handle(t)))], None, props(&["handle"])))
    };
    Query {
        kind: "link 3 relationships (write)",
        zql: format!(
            "mutation {{ Person(handle: \"{from}\") {{ handle follows -> link Person(handle: \"{}\") {{ handle }} follows -> link Person(handle: \"{}\") {{ handle }} follows -> link Person(handle: \"{}\") {{ handle }} }} }}",
            handle(links[0]),
            handle(links[1]),
            handle(links[2])
        ),
        plan: person(
            vec![Pred::Eq("handle".into(), json!(from))],
            None,
            vec![Item::Prop("handle".into()), link(links[0]), link(links[1]), link(links[2])],
        ),
    }
}
