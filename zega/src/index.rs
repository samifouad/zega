//! Declared property indexes: `index { range Player { salary } text Player { name } }`.
//!
//! An index answers with candidates. Every node that can match a predicate is
//! in the set it returns, and the executor still tests each candidate with the
//! same predicate it uses on a scan. A query therefore returns the same rows
//! with or without an index; the index only shrinks how many rows are tested.
//!
//! - `range` is an ordered map from value to nodes, per (type, field).
//! - `text` is a trigram index per (type, field). The text is padded with a
//!   start and an end marker, so one structure answers the byte-exact
//!   `findExact`, `startsExact` and `endsExact`: a match has every trigram of
//!   the needle (`findExact`), of start+needle (`startsExact`) or of
//!   needle+end (`endsExact`). A needle too short to form a trigram falls
//!   back to every node with a string in that field. It stores raw bytes, so
//!   it cannot serve the case/accent-folding `…Like` operators
//!   (zegadb/zega#98); those always fall back to a full scan.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Bound;

use serde_json::Value as Json;

use crate::graph::{NodeId, NodeView};
use crate::idset::IdSet;
use crate::value::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IndexKind {
    Range,
    Text,
}

impl IndexKind {
    pub fn as_str(self) -> &'static str {
        match self {
            IndexKind::Range => "range",
            IndexKind::Text => "text",
        }
    }
}

/// One index on one field of one type.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct IndexSpec {
    pub kind: IndexKind,
    pub type_name: String,
    pub field: String,
}

/// A value in a range index. Numbers sort before strings; a number never
/// compares with a string in a filter, so a scan stays within one kind.
#[derive(Clone, Debug)]
enum Key {
    Num(f64),
    Str(String),
}

impl Ord for Key {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Key::Num(a), Key::Num(b)) => a.total_cmp(b),
            (Key::Num(_), Key::Str(_)) => Ordering::Less,
            (Key::Str(_), Key::Num(_)) => Ordering::Greater,
            (Key::Str(a), Key::Str(b)) => a.cmp(b),
        }
    }
}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Key {}

/// `total_cmp` orders -0.0 before 0.0 and a filter treats them as equal, so
/// both are stored as 0.0. NaN never satisfies a comparison and is not stored.
fn number(value: f64) -> Option<f64> {
    if value.is_nan() {
        None
    } else if value == 0.0 {
        Some(0.0)
    } else {
        Some(value)
    }
}

fn key(value: &Value) -> Option<Key> {
    match value {
        Value::Int(value) => number(*value as f64).map(Key::Num),
        Value::Float(bits) => number(f64::from_bits(*bits)).map(Key::Num),
        Value::String(value) => Some(Key::Str(value.to_string())),
        _ => None,
    }
}

/// Inclusive bounds within one kind of value.
///
/// Bounds are always inclusive: `>` widens to `>=`. An Int is keyed by its
/// nearest f64, which is monotonic, so widening keeps every row a filter
/// accepts. The executor re-tests each candidate, so the widening never shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Interval {
    Empty,
    Num { low: Option<f64>, high: Option<f64> },
    Str { low: Option<String>, high: Option<String> },
}

enum Bounded {
    Num(f64),
    Str(String),
}

fn bounded(value: &Json) -> Option<Bounded> {
    if let Some(value) = value.as_i64() {
        return number(value as f64).map(Bounded::Num);
    }
    if let Some(value) = value.as_f64() {
        return number(value).map(Bounded::Num);
    }
    value.as_str().map(|value| Bounded::Str(value.to_string()))
}

impl Interval {
    /// `field = value`. None when the value is not a number or a string.
    pub fn exactly(value: &Json) -> Option<Self> {
        Some(match bounded(value)? {
            Bounded::Num(n) => Interval::Num { low: Some(n), high: Some(n) },
            Bounded::Str(s) => Interval::Str { low: Some(s.clone()), high: Some(s) },
        })
    }

    /// `field > value` and `field >= value`.
    pub fn at_least(value: &Json) -> Option<Self> {
        Some(match bounded(value)? {
            Bounded::Num(n) => Interval::Num { low: Some(n), high: None },
            Bounded::Str(s) => Interval::Str { low: Some(s), high: None },
        })
    }

    /// `field < value` and `field <= value`.
    pub fn at_most(value: &Json) -> Option<Self> {
        Some(match bounded(value)? {
            Bounded::Num(n) => Interval::Num { low: None, high: Some(n) },
            Bounded::Str(s) => Interval::Str { low: None, high: Some(s) },
        })
    }

    /// Both conditions on the same field. A number bound and a string bound
    /// together match nothing: no value compares with both.
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

    fn bounds(&self) -> Option<(Bound<Key>, Bound<Key>)> {
        let (low, high) = match self {
            Interval::Empty => return None,
            Interval::Num { low, high } => (
                Key::Num(low.unwrap_or(f64::NEG_INFINITY)),
                Bound::Included(Key::Num(high.unwrap_or(f64::INFINITY))),
            ),
            Interval::Str { low, high } => (
                Key::Str(low.clone().unwrap_or_default()),
                high.clone()
                    .map_or(Bound::Unbounded, |high| Bound::Included(Key::Str(high))),
            ),
        };
        if let Bound::Included(high) = &high {
            if &low > high {
                return None;
            }
        }
        Some((Bound::Included(low), high))
    }
}

/// What a text predicate asks for.
#[derive(Clone, Copy, Debug)]
pub enum TextPattern<'a> {
    Contains(&'a str),
    StartsWith(&'a str),
    EndsWith(&'a str),
}

const START: char = '\u{2}';
const END: char = '\u{3}';

type Gram = [char; 3];

fn grams(chars: &[char]) -> HashSet<Gram> {
    chars.windows(3).map(|w| [w[0], w[1], w[2]]).collect()
}

fn padded(text: &str) -> Vec<char> {
    std::iter::once(START)
        .chain(text.chars())
        .chain(std::iter::once(END))
        .collect()
}

#[derive(Clone, Debug, Default)]
struct TextIndex {
    grams: HashMap<Gram, IdSet>,
    /// Every node with a string in this field: the answer for a short needle.
    all: HashSet<NodeId>,
}

impl TextIndex {
    fn insert(&mut self, id: NodeId, text: &str) {
        self.all.insert(id);
        for gram in grams(&padded(text)) {
            self.grams.entry(gram).or_default().insert(id);
        }
    }

    fn remove(&mut self, id: NodeId, text: &str) {
        self.all.remove(&id);
        for gram in grams(&padded(text)) {
            if let Some(ids) = self.grams.get_mut(&gram) {
                ids.remove(id);
                if ids.is_empty() {
                    self.grams.remove(&gram);
                }
            }
        }
    }

    fn candidates(&self, pattern: TextPattern<'_>, out: &mut HashSet<NodeId>) {
        let probe: Vec<char> = match pattern {
            TextPattern::Contains(needle) => needle.chars().collect(),
            TextPattern::StartsWith(needle) => std::iter::once(START).chain(needle.chars()).collect(),
            TextPattern::EndsWith(needle) => needle.chars().chain(std::iter::once(END)).collect(),
        };
        if probe.len() < 3 {
            out.extend(&self.all);
            return;
        }
        let mut lists = Vec::new();
        for gram in grams(&probe) {
            match self.grams.get(&gram) {
                Some(ids) => lists.push(ids),
                None => return,
            }
        }
        lists.sort_by_key(|ids| ids.len());
        let (first, rest) = lists.split_first().expect("a probe of 3+ chars has a trigram");
        out.extend(
            first
                .iter()
                .filter(|id| rest.iter().all(|ids| ids.contains(id))),
        );
    }
}

type Slot = (String, String);

/// The indexes the current schema declares, kept in step with every write.
#[derive(Clone, Debug, Default)]
pub(crate) struct DeclaredIndexes {
    range: HashMap<Slot, BTreeMap<Key, IdSet>>,
    text: HashMap<Slot, TextIndex>,
}

impl DeclaredIndexes {
    fn is_empty(&self) -> bool {
        self.range.is_empty() && self.text.is_empty()
    }

    pub fn contains(&self, spec: &IndexSpec) -> bool {
        let slot = (spec.type_name.clone(), spec.field.clone());
        match spec.kind {
            IndexKind::Range => self.range.contains_key(&slot),
            IndexKind::Text => self.text.contains_key(&slot),
        }
    }

    /// Drop every index `specs` does not name.
    pub fn retain(&mut self, specs: &[IndexSpec]) {
        let keep = |kind: IndexKind, (ty, field): &Slot| {
            specs
                .iter()
                .any(|spec| spec.kind == kind && &spec.type_name == ty && &spec.field == field)
        };
        self.range.retain(|slot, _| keep(IndexKind::Range, slot));
        self.text.retain(|slot, _| keep(IndexKind::Text, slot));
    }

    /// Build `spec` from `nodes`, the nodes that carry its type.
    pub fn build<N: NodeView>(&mut self, spec: &IndexSpec, nodes: impl Iterator<Item = N>) {
        let slot = (spec.type_name.clone(), spec.field.clone());
        match spec.kind {
            IndexKind::Range => {
                let tree = self.range.entry(slot).or_default();
                for node in nodes {
                    if let Some(key) = node.prop(&spec.field).and_then(key) {
                        tree.entry(key).or_default().insert(node.id());
                    }
                }
            }
            IndexKind::Text => {
                let index = self.text.entry(slot).or_default();
                for node in nodes {
                    if let Some(Value::String(text)) = node.prop(&spec.field) {
                        index.insert(node.id(), text);
                    }
                }
            }
        }
    }

    pub fn insert(&mut self, node: &impl NodeView) {
        if self.is_empty() {
            return;
        }
        let id = node.id();
        for ((ty, field), tree) in &mut self.range {
            if !node.has_label(ty) {
                continue;
            }
            if let Some(key) = node.prop(field).and_then(key) {
                tree.entry(key).or_default().insert(id);
            }
        }
        for ((ty, field), index) in &mut self.text {
            if !node.has_label(ty) {
                continue;
            }
            if let Some(Value::String(text)) = node.prop(field) {
                index.insert(id, text);
            }
        }
    }

    pub fn remove(&mut self, node: &impl NodeView) {
        if self.is_empty() {
            return;
        }
        let id = node.id();
        for ((ty, field), tree) in &mut self.range {
            if !node.has_label(ty) {
                continue;
            }
            if let Some(key) = node.prop(field).and_then(key) {
                if let Some(ids) = tree.get_mut(&key) {
                    ids.remove(id);
                    if ids.is_empty() {
                        tree.remove(&key);
                    }
                }
            }
        }
        for ((ty, field), index) in &mut self.text {
            if !node.has_label(ty) {
                continue;
            }
            if let Some(Value::String(text)) = node.prop(field) {
                index.remove(id, text);
            }
        }
    }

    /// Candidates for `field` within `interval` over every type in `types`.
    /// None when one of the types has no range index on the field.
    pub fn range_candidates(
        &self,
        types: &[&str],
        field: &str,
        interval: &Interval,
    ) -> Option<HashSet<NodeId>> {
        let trees = types
            .iter()
            .map(|ty| self.range.get(&(ty.to_string(), field.to_string())))
            .collect::<Option<Vec<_>>>()?;
        let mut out = HashSet::new();
        if let Some(bounds) = interval.bounds() {
            for tree in trees {
                for ids in tree.range(bounds.clone()).map(|(_, ids)| ids) {
                    out.extend(ids.iter());
                }
            }
        }
        Some(out)
    }

    pub fn has_range(&self, types: &[&str], field: &str) -> bool {
        types
            .iter()
            .all(|ty| self.range.contains_key(&(ty.to_string(), field.to_string())))
    }

    /// Candidates for a text predicate over every type in `types`. None when
    /// one of the types has no text index on the field.
    pub fn text_candidates(
        &self,
        types: &[&str],
        field: &str,
        pattern: TextPattern<'_>,
    ) -> Option<HashSet<NodeId>> {
        let indexes = types
            .iter()
            .map(|ty| self.text.get(&(ty.to_string(), field.to_string())))
            .collect::<Option<Vec<_>>>()?;
        let mut out = HashSet::new();
        for index in indexes {
            index.candidates(pattern, &mut out);
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::Node;
    use serde_json::json;

    fn node(id: NodeId, field: &str, value: Value) -> Node {
        Node {
            id,
            labels: vec!["T".into()],
            props: HashMap::from([(field.to_string(), value)]),
        }
    }

    #[test]
    fn negative_zero_and_zero_share_a_key_and_nan_is_not_stored() {
        let spec = IndexSpec { kind: IndexKind::Range, type_name: "T".into(), field: "x".into() };
        let nodes = [
            node(1, "x", Value::from_f64(-0.0)),
            node(2, "x", Value::from_f64(0.0)),
            node(3, "x", Value::from_f64(f64::NAN)),
        ];
        let mut indexes = DeclaredIndexes::default();
        indexes.build(&spec, nodes.into_iter());
        let at_zero = indexes
            .range_candidates(&["T"], "x", &Interval::at_least(&json!(0)).unwrap())
            .unwrap();
        assert_eq!(at_zero, HashSet::from([1, 2]));
    }

    #[test]
    fn a_number_bound_and_a_string_bound_match_nothing() {
        let both = Interval::at_least(&json!(1))
            .unwrap()
            .intersect(Interval::at_most(&json!("z")).unwrap());
        assert_eq!(both, Interval::Empty);
        let crossed = Interval::at_least(&json!(5))
            .unwrap()
            .intersect(Interval::at_most(&json!(1)).unwrap());
        assert!(crossed.bounds().is_none());
    }

    #[test]
    fn trigrams_answer_all_three_text_operators() {
        let mut index = TextIndex::default();
        index.insert(1, "Connor McDavid");
        index.insert(2, "Leon Draisaitl");
        let find = |pattern| {
            let mut out = HashSet::new();
            index.candidates(pattern, &mut out);
            out
        };
        assert_eq!(find(TextPattern::Contains("McD")), HashSet::from([1]));
        assert_eq!(find(TextPattern::StartsWith("Le")), HashSet::from([2]));
        assert_eq!(find(TextPattern::EndsWith("tl")), HashSet::from([2]));
        assert_eq!(find(TextPattern::Contains("zz")), HashSet::from([1, 2]));
        assert!(find(TextPattern::Contains("xyz")).is_empty());
    }
}
