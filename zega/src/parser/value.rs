use std::collections::HashMap;
use std::fmt;

/// A dynamic value type used throughout Zega for properties and parameters.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Value {
    String(String),
    Int(i64),
    Float(u64), // stored as bits for Eq/Hash; use from_bits/to_bits
    Bool(bool),
    List(Vec<Value>),
    Map(HashMap<String, Value>),
    Null,
}

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
        Value::String(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::String(v.to_string())
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
