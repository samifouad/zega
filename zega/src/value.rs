use std::collections::HashMap;
use std::fmt;

/// A dynamic value type used throughout Zega for properties and parameters.
///
/// 24 bytes (zegadb/zega#100): a stored property is one of these, so its
/// size is paid once per property of every node. The payloads that used to
/// set the size are boxed: a string is an exact-length `Box<str>` (16 bytes,
/// no spare capacity), a list a `Box<[Value]>`, and the rare map and vector
/// sit behind one pointer. A point (16 bytes) stays inline because spatial
/// queries read it on every candidate. `Box<T>` serializes exactly as `T`,
/// so the WAL, snapshot and `.graph` bytes are unchanged.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Value {
    String(Box<str>),
    Int(i64),
    Float(u64), // stored as bits for Eq/Hash; use from_bits/to_bits
    Bool(bool),
    List(Box<[Value]>),
    Map(Box<HashMap<String, Value>>),
    Null,
    // Append variants: the existing discriminants are part of the WAL format.
    Point(crate::location::Point),
    Vector(Box<crate::vector::Vector>),
}

const _: () = assert!(std::mem::size_of::<Value>() == 24);

impl Value {
    pub fn as_string(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn from_f64(v: f64) -> Self {
        Value::Float(v.to_bits())
    }

    pub fn to_f64(&self) -> Option<f64> {
        match self {
            Value::Float(bits) => Some(f64::from_bits(*bits)),
            _ => None,
        }
    }
}

impl std::hash::Hash for Value {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Value::String(s) => s.hash(state),
            Value::Int(i) => i.hash(state),
            Value::Float(bits) => bits.hash(state),
            Value::Bool(b) => b.hash(state),
            Value::List(l) => l.len().hash(state),
            Value::Map(m) => m.len().hash(state),
            Value::Point(p) => p.hash(state),
            Value::Vector(v) => v.hash(state),
            Value::Null => {}
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::String(s) => write!(f, "{}", s),
            Value::Int(i) => write!(f, "{}", i),
            Value::Float(bits) => write!(f, "{}", f64::from_bits(*bits)),
            Value::Bool(b) => write!(f, "{}", b),
            Value::List(l) => {
                let items: Vec<String> = l.iter().map(|v| v.to_string()).collect();
                write!(f, "[{}]", items.join(", "))
            }
            Value::Map(m) => write!(f, "{{...{} keys}}", m.len()),
            Value::Point(p) => write!(f, "point({}, {})", p.lat(), p.lon()),
            Value::Vector(v) => write!(f, "vector{}", v.to_json()),
            Value::Null => write!(f, "null"),
        }
    }
}

impl PartialOrd for Value {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a.partial_cmp(b),
            (Value::Float(a), Value::Float(b)) => {
                f64::from_bits(*a).partial_cmp(&f64::from_bits(*b))
            }
            (Value::String(a), Value::String(b)) => a.partial_cmp(b),
            (Value::Bool(a), Value::Bool(b)) => a.partial_cmp(b),
            _ => None,
        }
    }
}

impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::String(v.into_boxed_str())
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::String(v.into())
    }
}

impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Int(v)
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::location::Point;
    use crate::vector::{Metric, Vector};

    /// `Value` as it was before zegadb/zega#100 boxed its payloads. The WAL,
    /// snapshots and `.graph` files written by that engine hold exactly these
    /// bytes, so the compact `Value` has to write and read the same.
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    enum Unboxed {
        String(String),
        Int(i64),
        Float(u64),
        Bool(bool),
        List(Vec<Unboxed>),
        Map(HashMap<String, Unboxed>),
        Null,
        Point(Point),
        Vector(Vector),
    }

    fn pairs() -> Vec<(Value, Unboxed)> {
        let point = Point::new(51.05, -114.07).unwrap();
        let vector = Vector::new(&[0.5, -1.0, 2.0], Metric::L2).unwrap();
        vec![
            (Value::from("héllo"), Unboxed::String("héllo".into())),
            (Value::from(""), Unboxed::String(String::new())),
            (Value::Int(-7), Unboxed::Int(-7)),
            (Value::from_f64(-0.0), Unboxed::Float((-0.0f64).to_bits())),
            (Value::Bool(true), Unboxed::Bool(true)),
            (
                Value::List(vec![Value::Int(1), Value::from("two"), Value::Null].into()),
                Unboxed::List(vec![Unboxed::Int(1), Unboxed::String("two".into()), Unboxed::Null]),
            ),
            (Value::List(Box::default()), Unboxed::List(Vec::new())),
            (
                Value::Map(Box::new(HashMap::from([("k".to_string(), Value::Bool(false))]))),
                Unboxed::Map(HashMap::from([("k".to_string(), Unboxed::Bool(false))])),
            ),
            (Value::Null, Unboxed::Null),
            (Value::Point(point), Unboxed::Point(point)),
            (Value::Vector(Box::new(vector.clone())), Unboxed::Vector(vector)),
        ]
    }

    #[test]
    fn boxed_payloads_keep_the_stored_bytes() {
        for (value, unboxed) in pairs() {
            let bytes = bincode::serialize(&value).unwrap();
            assert_eq!(bytes, bincode::serialize(&unboxed).unwrap(), "{value:?}");
            let back: Value = bincode::deserialize(&bytes).unwrap();
            assert_eq!(back, value);
            let old: Unboxed = bincode::deserialize(&bytes).unwrap();
            assert_eq!(old, unboxed);
        }
    }
}
