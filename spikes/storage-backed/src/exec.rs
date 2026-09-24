//! The measured ZQL subset, executed over any [`GraphStore`].
//!
//! Plans are built by hand ([`Sel`]); ZQL parsing is unchanged by a storage
//! backend and is not what this spike measures. Semantics follow
//! `zega/src/v2.rs` (`read`, `candidates`, `retain_matches`, `order_limit`,
//! `project`, `neighbors`, `apply_node`, `lookup_one`, `find_duplicate`), with
//! one deliberate difference in *how*, not *what*: a scan with a limit stops
//! at the limit instead of materializing every id of the type first. Results
//! are identical because both walk ids in ascending order; `tests/parity.rs`
//! checks the JSON against `Zega::run_lang`.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Map, Value as Json};

use crate::model::{Dir, Interval, Node, Op, Value};
use crate::store::{GraphStore, StoreError};

#[derive(Debug)]
pub struct ExecError(pub String);

impl From<StoreError> for ExecError {
    fn from(e: StoreError) -> Self {
        ExecError(e.0)
    }
}

pub type Result<T> = std::result::Result<T, ExecError>;

#[derive(Clone, Copy, Debug)]
pub enum Cmp {
    Gt,
    Gte,
    Lt,
    Lte,
}

#[derive(Clone, Debug)]
pub enum Pred {
    Eq(String, Json),
    Cmp(String, Cmp, Json),
}

/// `Type(cond) limit n { items }`, conditions joined by `&&`.
#[derive(Clone, Debug)]
pub struct Sel {
    pub ty: String,
    pub cond: Vec<Pred>,
    pub limit: Option<usize>,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug)]
pub enum Item {
    Prop(String),
    /// `field -> Target { … }` (read) or `field -> link Target(…) { … }`
    /// (mutation).
    Walk { field: String, rel: String, dir: Dir, many: bool, target: Sel },
}

/// Scan page size: rows fetched per storage call when no index applies.
pub const PAGE: usize = 256;

fn value_to_json(value: &Value) -> Json {
    match value {
        Value::String(v) => Json::String(v.clone()),
        Value::Int(v) => json!(v),
        Value::Float(bits) => json!(f64::from_bits(*bits)),
        Value::Bool(v) => Json::Bool(*v),
        Value::Null => Json::Null,
        Value::List(values) => Json::Array(values.iter().map(value_to_json).collect()),
        Value::Point(point) => point.to_json(),
        Value::Vector(v) => v.to_json(),
        Value::Map(values) => {
            Json::Object(values.iter().map(|(k, v)| (k.clone(), value_to_json(v))).collect())
        }
    }
}

fn json_to_value(value: &Json) -> Result<Value> {
    match value {
        Json::String(v) => Ok(Value::String(v.clone())),
        Json::Number(n) => n
            .as_i64()
            .map(Value::Int)
            .or_else(|| n.as_f64().map(Value::from_f64))
            .ok_or_else(|| ExecError(format!("number {n} is out of range"))),
        Json::Bool(v) => Ok(Value::Bool(*v)),
        Json::Null => Ok(Value::Null),
        _ => Err(ExecError("points and vectors are outside this spike".into())),
    }
}

fn prop_json(node: &Node, name: &str) -> Json {
    if name == "@id" {
        return json!(node.id);
    }
    node.props.get(name).map(value_to_json).unwrap_or(Json::Null)
}

fn cmp_value(left: &Json, right: &Json) -> Option<std::cmp::Ordering> {
    if let (Some(a), Some(b)) = (left.as_i64(), right.as_i64()) {
        return Some(a.cmp(&b));
    }
    if let (Some(a), Some(b)) = (left.as_f64(), right.as_f64()) {
        return a.partial_cmp(&b);
    }
    if let (Some(a), Some(b)) = (left.as_str(), right.as_str()) {
        return Some(a.cmp(b));
    }
    None
}

fn matches(node: &Node, cond: &[Pred]) -> bool {
    cond.iter().all(|pred| match pred {
        Pred::Eq(field, value) => prop_json(node, field) == *value,
        Pred::Cmp(field, op, value) => cmp_value(&prop_json(node, field), value).is_some_and(|o| match op {
            Cmp::Gt => o.is_gt(),
            Cmp::Gte => o.is_ge(),
            Cmp::Lt => o.is_lt(),
            Cmp::Lte => o.is_le(),
        }),
    })
}

fn has_label(node: &Node, ty: &str) -> bool {
    node.labels.iter().any(|l| l == ty)
}

/// `Interval::exactly` / `at_least` / `at_most` from `zega/src/index.rs`.
fn interval(pred: &Pred) -> Option<(&str, Interval)> {
    let bounded = |v: &Json| -> Option<Interval> {
        if let Some(n) = v.as_i64().map(|i| i as f64).or_else(|| v.as_f64()) {
            if n.is_nan() {
                return None;
            }
            let n = if n == 0.0 { 0.0 } else { n };
            return Some(Interval::Num { low: Some(n), high: Some(n) });
        }
        v.as_str().map(|s| Interval::Str { low: Some(s.to_string()), high: Some(s.to_string()) })
    };
    let (field, exact) = match pred {
        Pred::Eq(f, v) | Pred::Cmp(f, _, v) => (f, bounded(v)?),
    };
    if field == "@id" {
        return None;
    }
    let iv = match (pred, exact) {
        (Pred::Eq(..), iv) => iv,
        (Pred::Cmp(_, Cmp::Gt | Cmp::Gte, _), Interval::Num { low, .. }) => Interval::Num { low, high: None },
        (Pred::Cmp(_, Cmp::Lt | Cmp::Lte, _), Interval::Num { high, .. }) => Interval::Num { low: None, high },
        (Pred::Cmp(_, Cmp::Gt | Cmp::Gte, _), Interval::Str { low, .. }) => Interval::Str { low, high: None },
        (Pred::Cmp(_, Cmp::Lt | Cmp::Lte, _), Interval::Str { high, .. }) => Interval::Str { low: None, high },
        (_, Interval::Empty) => Interval::Empty,
    };
    Some((field, iv))
}

/// Ids the declared indexes allow, or None when no condition is indexed.
fn index_candidates<S: GraphStore>(store: &S, sel: &Sel) -> Result<Option<Vec<u64>>> {
    let mut joined: Vec<(&str, Interval)> = Vec::new();
    for pred in &sel.cond {
        let Some((field, iv)) = interval(pred) else { continue };
        if !store.has_index(&sel.ty, field) {
            continue;
        }
        match joined.iter_mut().find(|(f, _)| *f == field) {
            Some((_, j)) => *j = std::mem::replace(j, Interval::Empty).intersect(iv),
            None => joined.push((field, iv)),
        }
    }
    let mut found: Option<HashSet<u64>> = None;
    for (field, iv) in joined {
        let Some(ids) = store.index_range(&sel.ty, field, &iv)? else { continue };
        found = Some(match found {
            None => ids.into_iter().collect(),
            Some(mut set) => {
                let next: HashSet<u64> = ids.into_iter().collect();
                set.retain(|id| next.contains(id));
                set
            }
        });
    }
    Ok(found.map(|set| {
        let mut ids: Vec<u64> = set.into_iter().collect();
        ids.sort_unstable();
        ids
    }))
}

/// The nodes `sel` selects, in id order, stopping at `limit`.
fn select<S: GraphStore>(store: &S, sel: &Sel, limit: Option<usize>) -> Result<Vec<std::sync::Arc<Node>>> {
    let mut out = Vec::new();
    if limit == Some(0) {
        return Ok(out);
    }
    let full = |out: &Vec<_>| limit.is_some_and(|l| out.len() >= l);
    if let Some(ids) = index_candidates(store, sel)? {
        for id in ids {
            let Some(node) = store.node(id)? else { continue };
            if has_label(&node, &sel.ty) && matches(&node, &sel.cond) {
                out.push(node);
                if full(&out) {
                    break;
                }
            }
        }
        return Ok(out);
    }
    let mut after = 0;
    loop {
        let page = store.scan_label(&sel.ty, after, PAGE)?;
        let Some(last) = page.last() else { break };
        after = last.id;
        for node in page {
            if matches(&node, &sel.cond) {
                out.push(node);
                if full(&out) {
                    return Ok(out);
                }
            }
        }
    }
    Ok(out)
}

fn equality_lookup(sel: &Sel) -> bool {
    sel.limit.is_none() && !sel.cond.is_empty() && sel.cond.iter().all(|p| matches!(p, Pred::Eq(..)))
}

/// A query block with one root selection.
pub fn read<S: GraphStore>(store: &S, sel: &Sel) -> Result<Json> {
    if equality_lookup(sel) {
        let found = select(store, sel, None)?;
        return match found.len() {
            0 => Ok(Json::Null),
            1 => project(store, sel, &found[0]),
            n => Err(ExecError(format!("{} matched {n} rows", sel.ty))),
        };
    }
    let found = select(store, sel, sel.limit)?;
    found.iter().map(|node| project(store, sel, node)).collect::<Result<Vec<_>>>().map(Json::Array)
}

fn project<S: GraphStore>(store: &S, sel: &Sel, node: &Node) -> Result<Json> {
    let mut object = Map::new();
    for item in &sel.items {
        match item {
            Item::Prop(name) => {
                object.insert(name.clone(), prop_json(node, name));
            }
            Item::Walk { field, rel, dir, many, target } => {
                let mut reached = Vec::new();
                for (next, _rel) in store.adjacency(node.id, rel, *dir)?.iter() {
                    let Some(n) = store.node(*next)? else { continue };
                    if has_label(&n, &target.ty) && matches(&n, &target.cond) {
                        reached.push(n);
                    }
                }
                if let Some(limit) = target.limit {
                    reached.truncate(limit);
                }
                let rows = reached.iter().map(|n| project(store, target, n)).collect::<Result<Vec<_>>>()?;
                object.insert(
                    field.clone(),
                    if *many { Json::Array(rows) } else { rows.into_iter().next().unwrap_or(Json::Null) },
                );
            }
        }
    }
    Ok(Json::Object(object))
}

/// `mutation { Type(field: value && …) { props } }` inserts one node;
/// `mutation { Type(unique: v) { field -> link Target(unique: w) { props } … } }`
/// finds one node and links it to others (`v2::apply_node`: a selection with
/// a `link` is a lookup, not an insert). Every write of the statement commits
/// as one transaction.
pub fn mutate<S: GraphStore>(store: &mut S, sel: &Sel, uniques: &[(String, String)]) -> Result<Json> {
    let (next_node, mut next_rel) = store.next_ids();
    let mut ops = Vec::new();
    let lookup = sel.items.iter().any(|item| matches!(item, Item::Walk { .. }));
    let node: std::sync::Arc<Node> = if lookup {
        lookup_one(store, sel)?
    } else {
        let mut props = HashMap::new();
        for pred in &sel.cond {
            match pred {
                Pred::Eq(field, value) if field != "@id" => {
                    props.insert(field.clone(), json_to_value(value)?);
                }
                _ => return Err(ExecError(format!("creating a {} only accepts field: value", sel.ty))),
            }
        }
        for (ty, field) in uniques {
            if *ty != sel.ty {
                continue;
            }
            let Some(value) = props.get(field) else { continue };
            if matches!(value, Value::Null) {
                continue;
            }
            let probe = Sel { ty: ty.clone(), cond: vec![Pred::Eq(field.clone(), value_to_json(value))], limit: Some(1), items: vec![] };
            if !select(store, &probe, Some(1))?.is_empty() {
                return Err(ExecError(format!("unique {ty} {{ {field} }} is already used")));
            }
        }
        let node = Node { id: next_node, labels: vec![sel.ty.clone()], props };
        ops.push(Op::InsertNode { id: node.id, labels: node.labels.clone(), props: node.props.clone() });
        std::sync::Arc::new(node)
    };
    let id = node.id;
    let mut object = Map::new();
    let mut lists: HashMap<String, Vec<Json>> = HashMap::new();
    for item in &sel.items {
        match item {
            Item::Prop(name) => {
                object.insert(name.clone(), prop_json(&node, name));
            }
            Item::Walk { field, rel, dir, many, target } => {
                let child = lookup_one(store, target)?;
                let (from, to) = match dir {
                    Dir::Out => (id, child.id),
                    Dir::In => (child.id, id),
                };
                ops.push(Op::InsertRel { id: next_rel, kind: rel.clone(), from, to, props: HashMap::new() });
                next_rel += 1;
                let mut child_object = Map::new();
                for child_item in &target.items {
                    if let Item::Prop(name) = child_item {
                        child_object.insert(name.clone(), prop_json(&child, name));
                    }
                }
                if *many {
                    lists.entry(field.clone()).or_default().push(Json::Object(child_object));
                } else {
                    object.insert(field.clone(), Json::Object(child_object));
                }
            }
        }
    }
    for (key, rows) in lists {
        object.insert(key, Json::Array(rows));
    }
    store.apply(ops)?;
    Ok(Json::Object(object))
}

/// Exactly one node, or an error (`v2::lookup_one`).
fn lookup_one<S: GraphStore>(store: &S, sel: &Sel) -> Result<std::sync::Arc<Node>> {
    let found = select(store, sel, None)?;
    match found.len() {
        1 => Ok(found[0].clone()),
        0 => Err(ExecError(format!("no {} matched", sel.ty))),
        n => Err(ExecError(format!("{} matched {n} rows", sel.ty))),
    }
}
