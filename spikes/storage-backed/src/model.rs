//! The engine's data model, as `zega/src/graph/mod.rs` and `zega/src/wal`
//! define it. Field order matches, so bincode bytes of [`Snapshot`] restore
//! into the real engine with `Zega::restore_bytes`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
pub use zega::Value;

pub type NodeId = u64;
pub type RelId = u64;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub labels: Vec<String>,
    pub props: HashMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rel {
    pub id: RelId,
    pub kind: String,
    pub from: NodeId,
    pub to: NodeId,
    pub props: HashMap<String, Value>,
}

/// The layout of `zega::wal::Snapshot`.
#[derive(Serialize, Deserialize)]
pub struct Snapshot {
    pub nodes: HashMap<NodeId, Node>,
    pub relationships: HashMap<RelId, Rel>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dir {
    Out,
    In,
}

/// One write, as the engine's journal records it (`zega::wal::Operation`).
/// A statement's operations are applied as one transaction.
#[derive(Clone, Debug)]
pub enum Op {
    InsertNode { id: NodeId, labels: Vec<String>, props: HashMap<String, Value> },
    UpdateNode { id: NodeId, props: HashMap<String, Value> },
    DeleteNode { id: NodeId },
    InsertRel { id: RelId, kind: String, from: NodeId, to: NodeId, props: HashMap<String, Value> },
    DeleteRel { id: RelId },
}

/// Inclusive bounds within one kind of value, as `zega/src/index.rs`.
#[derive(Clone, Debug, PartialEq)]
pub enum Interval {
    Empty,
    Num { low: Option<f64>, high: Option<f64> },
    Str { low: Option<String>, high: Option<String> },
}

impl Interval {
    pub fn intersect(self, other: Self) -> Self {
        fn low<T: PartialOrd>(a: Option<T>, b: Option<T>) -> Option<T> {
            match (a, b) {
                (Some(a), Some(b)) => Some(if a >= b { a } else { b }),
                (a, b) => a.or(b),
            }
        }
        fn high<T: PartialOrd>(a: Option<T>, b: Option<T>) -> Option<T> {
            match (a, b) {
                (Some(a), Some(b)) => Some(if a <= b { a } else { b }),
                (a, b) => a.or(b),
            }
        }
        match (self, other) {
            (Interval::Num { low: a, high: b }, Interval::Num { low: c, high: d }) => {
                Interval::Num { low: low(a, c), high: high(b, d) }
            }
            (Interval::Str { low: a, high: b }, Interval::Str { low: c, high: d }) => {
                Interval::Str { low: low(a, c), high: high(b, d) }
            }
            _ => Interval::Empty,
        }
    }
}

/// The key a range index stores for a value, as `zega/src/index.rs::key`:
/// numbers as f64 (NaN not stored, -0.0 as 0.0), strings as themselves.
pub enum Key<'a> {
    Num(f64),
    Str(&'a str),
}

pub fn index_key(value: &Value) -> Option<Key<'_>> {
    let number = |n: f64| {
        if n.is_nan() {
            None
        } else if n == 0.0 {
            Some(Key::Num(0.0))
        } else {
            Some(Key::Num(n))
        }
    };
    match value {
        Value::Int(v) => number(*v as f64),
        Value::Float(bits) => number(f64::from_bits(*bits)),
        Value::String(s) => Some(Key::Str(s)),
        _ => None,
    }
}

/// Approximate heap bytes of a node, for the cache's byte budget.
pub fn node_bytes(node: &Node) -> usize {
    let mut bytes = 64 + node.labels.iter().map(|l| 24 + l.len()).sum::<usize>();
    for (k, v) in &node.props {
        bytes += 24 + k.len() + 40 + value_bytes(v);
    }
    bytes
}

fn value_bytes(value: &Value) -> usize {
    match value {
        Value::String(s) => s.len(),
        Value::List(items) => items.iter().map(|v| 40 + value_bytes(v)).sum(),
        Value::Map(map) => map.iter().map(|(k, v)| 64 + k.len() + value_bytes(v)).sum(),
        Value::Vector(v) => v.dimensions() * 4,
        _ => 0,
    }
}
