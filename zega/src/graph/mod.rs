use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, BuildHasherDefault, Hasher, RandomState};
use std::sync::atomic::{AtomicU64, Ordering};
use crate::idset::IdSet;
use crate::index::{DeclaredIndexes, IndexKind, IndexSpec, Interval, TextPattern};
use crate::value::Value;

mod names;

use names::{Names, Shape, ShapeId, Shapes, Sym};

pub type NodeId = u64;
pub type RelId = u64;

/// A node as it is written down: in the WAL, a snapshot, a rollback journal.
/// The graph itself stores the compact [`NodeRef`] form.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub labels: Vec<String>,
    pub props: HashMap<String, Value>,
}

/// A relationship as it is written down; see [`Node`].
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Relationship {
    pub id: RelId,
    pub kind: String,
    pub from: NodeId,
    pub to: NodeId,
    pub props: HashMap<String, Value>,
}

/// What the query executor reads from a node, whether it is stored in the
/// graph ([`NodeRef`]) or a copy taken before a write ([`Node`]).
pub trait NodeView {
    fn id(&self) -> NodeId;
    fn prop(&self, key: &str) -> Option<&Value>;
    fn has_label(&self, label: &str) -> bool;
    /// The first label written, which ZQL treats as the node's type.
    fn first_label(&self) -> Option<&str>;
}

impl<T: NodeView + ?Sized> NodeView for &T {
    fn id(&self) -> NodeId {
        (**self).id()
    }
    fn prop(&self, key: &str) -> Option<&Value> {
        (**self).prop(key)
    }
    fn has_label(&self, label: &str) -> bool {
        (**self).has_label(label)
    }
    fn first_label(&self) -> Option<&str> {
        (**self).first_label()
    }
}

impl NodeView for Node {
    fn id(&self) -> NodeId {
        self.id
    }
    fn prop(&self, key: &str) -> Option<&Value> {
        self.props.get(key)
    }
    fn has_label(&self, label: &str) -> bool {
        self.labels.iter().any(|has| has == label)
    }
    fn first_label(&self) -> Option<&str> {
        self.labels.first().map(String::as_str)
    }
}

/// A stored node: its shape (labels and property keys) and its property
/// values in the shape's key order.
struct NodeRecord {
    shape: ShapeId,
    values: Box<[Value]>,
}

struct RelRecord {
    kind: Sym,
    /// Property keys only; a relationship's shape has no labels.
    shape: ShapeId,
    from: NodeId,
    to: NodeId,
    values: Box<[Value]>,
}

/// A node in the graph, borrowed.
#[derive(Clone, Copy)]
pub struct NodeRef<'g> {
    pub id: NodeId,
    names: &'g Names,
    shape: &'g Shape,
    values: &'g [Value],
}

/// A relationship in the graph, borrowed.
#[derive(Clone, Copy)]
pub struct RelRef<'g> {
    pub id: RelId,
    pub kind: &'g str,
    pub from: NodeId,
    pub to: NodeId,
    names: &'g Names,
    keys: &'g [Sym],
    values: &'g [Value],
}

/// The value of `key` among `keys` (ascending by symbol) and `values`.
/// A handful of keys is scanned by name, which is cheaper than hashing
/// `key`; more are found by symbol.
fn lookup<'g>(names: &Names, keys: &[Sym], values: &'g [Value], key: &str) -> Option<&'g Value> {
    const SCAN: usize = 8;
    let at = if keys.len() <= SCAN {
        keys.iter().position(|sym| names.name(*sym) == key)?
    } else {
        keys.binary_search(&names.get(key)?).ok()?
    };
    Some(&values[at])
}

impl<'g> NodeRef<'g> {
    pub fn labels(&self) -> impl Iterator<Item = &'g str> + 'g {
        let names = self.names;
        self.shape.labels.iter().map(move |sym| names.name(*sym))
    }

    pub fn prop(&self, key: &str) -> Option<&'g Value> {
        lookup(self.names, &self.shape.keys, self.values, key)
    }

    pub fn props(&self) -> impl Iterator<Item = (&'g str, &'g Value)> + 'g {
        let names = self.names;
        self.shape
            .keys
            .iter()
            .map(move |sym| names.name(*sym))
            .zip(self.values.iter())
    }

    pub fn has_label(&self, label: &str) -> bool {
        self.labels().any(|has| has == label)
    }

    pub fn first_label(&self) -> Option<&'g str> {
        self.labels().next()
    }

    /// The node as it is written down.
    pub fn to_node(&self) -> Node {
        Node {
            id: self.id,
            labels: self.labels().map(str::to_string).collect(),
            props: self.props().map(|(k, v)| (k.to_string(), v.clone())).collect(),
        }
    }
}

impl NodeView for NodeRef<'_> {
    fn id(&self) -> NodeId {
        self.id
    }
    fn prop(&self, key: &str) -> Option<&Value> {
        NodeRef::prop(self, key)
    }
    fn has_label(&self, label: &str) -> bool {
        NodeRef::has_label(self, label)
    }
    fn first_label(&self) -> Option<&str> {
        NodeRef::first_label(self)
    }
}

impl<'g> RelRef<'g> {
    pub fn prop(&self, key: &str) -> Option<&'g Value> {
        lookup(self.names, self.keys, self.values, key)
    }

    pub fn props(&self) -> impl Iterator<Item = (&'g str, &'g Value)> + 'g {
        let names = self.names;
        self.keys
            .iter()
            .map(move |sym| names.name(*sym))
            .zip(self.values.iter())
    }

    /// The relationship as it is written down.
    pub fn to_relationship(&self) -> Relationship {
        Relationship {
            id: self.id,
            kind: self.kind.to_string(),
            from: self.from,
            to: self.to,
            props: self.props().map(|(k, v)| (k.to_string(), v.clone())).collect(),
        }
    }
}

fn node_ref<'g>(names: &'g Names, shapes: &'g Shapes, id: NodeId, node: &'g NodeRecord) -> NodeRef<'g> {
    NodeRef {
        id,
        names,
        shape: shapes.get(node.shape),
        values: &node.values,
    }
}

fn rel_ref<'g>(names: &'g Names, shapes: &'g Shapes, id: RelId, rel: &'g RelRecord) -> RelRef<'g> {
    RelRef {
        id,
        kind: names.name(rel.kind),
        from: rel.from,
        to: rel.to,
        names,
        keys: &shapes.get(rel.shape).keys,
        values: &rel.values,
    }
}

/// Intern `labels` and `props` into a shape and the values in its key order.
fn intern(
    names: &mut Names,
    shapes: &mut Shapes,
    labels: &[String],
    props: impl IntoIterator<Item = (String, Value)>,
) -> (ShapeId, Box<[Value]>) {
    let labels: Box<[Sym]> = labels.iter().map(|label| names.intern(label)).collect();
    let mut pairs: Vec<(Sym, Value)> = props
        .into_iter()
        .map(|(key, value)| (names.intern(&key), value))
        .collect();
    pairs.sort_unstable_by_key(|(sym, _)| *sym);
    let keys = pairs.iter().map(|(sym, _)| *sym).collect();
    let values = pairs.into_iter().map(|(_, value)| value).collect();
    (shapes.intern(Shape { labels, keys }), values)
}

/// A hasher for keys that are already hashes: the property index is keyed
/// by a keyed hash of (property key, value), so hashing it again is waste.
#[derive(Default)]
struct PreHashed(u64);

impl Hasher for PreHashed {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0 << 8) | u64::from(*byte);
        }
    }
    fn write_u64(&mut self, value: u64) {
        self.0 = value;
    }
}

/// The relationships that leave and enter one node. A node has an entry
/// only while it has a relationship.
#[derive(Default)]
struct Adjacency {
    out: IdSet,
    inc: IdSet,
}

pub struct Graph {
    names: Names,
    shapes: Shapes,
    nodes: HashMap<NodeId, NodeRecord>,
    relationships: HashMap<RelId, RelRecord>,
    label_index: HashMap<Sym, IdSet>,
    /// Nodes by a hash of (property key, value), for `unique` checks and
    /// lookups. It stores no key or value: [`Graph::nodes_by_property`]
    /// checks each candidate's stored value, so a collision costs a
    /// comparison, never a wrong answer.
    property_index: HashMap<u64, IdSet, BuildHasherDefault<PreHashed>>,
    /// Keys the property-index hash, per graph, so no one can choose values
    /// that collide.
    property_hasher: RandomState,
    spatial_index: crate::location::SpatialIndex,
    vector_index: crate::vector::VectorIndex,
    /// `index { }` declarations of the schema last run against this graph.
    declared: DeclaredIndexes,
    /// Each node's relationships, both directions in one entry.
    adjacency: HashMap<NodeId, Adjacency>,
    next_node_id: AtomicU64,
    next_rel_id: AtomicU64,
    /// Rows a ZQL filter has been tested on. Indexes lower it.
    examined: AtomicU64,
    /// Nodes a ZQL path search has expanded. A* lowers it.
    expanded: AtomicU64,
}

impl Default for Graph {
    fn default() -> Self {
        Self::new()
    }
}

impl Graph {
    pub fn new() -> Self {
        Graph {
            names: Names::default(),
            shapes: Shapes::default(),
            nodes: HashMap::new(),
            relationships: HashMap::new(),
            label_index: HashMap::new(),
            property_index: HashMap::default(),
            property_hasher: RandomState::new(),
            spatial_index: Default::default(),
            vector_index: Default::default(),
            declared: Default::default(),
            adjacency: HashMap::new(),
            next_node_id: AtomicU64::new(1),
            next_rel_id: AtomicU64::new(1),
            examined: AtomicU64::new(0),
            expanded: AtomicU64::new(0),
        }
    }

    /// Conservative Morton-range candidates. Apply an exact predicate afterwards.
    pub fn spatial_candidates(
        &self,
        field: &str,
        bounds: crate::location::Bounds,
    ) -> HashSet<NodeId> {
        self.spatial_index.candidates(field, bounds)
    }

    pub fn vector_nearest(&self, field: &str, query: &crate::vector::Vector, k: usize, exact: bool, allowed: impl Fn(NodeId) -> bool) -> Vec<(NodeId, f64)> {
        self.vector_index.nearest(field, query, k, exact, allowed)
    }

    /// Make the declared indexes exactly `specs`: drop the rest and build any
    /// new one from the nodes already stored. Writes keep them current after.
    pub fn sync_indexes(&mut self, specs: &[IndexSpec]) {
        self.declared.retain(specs);
        for spec in specs {
            if self.declared.contains(spec) {
                continue;
            }
            let (names, shapes, all) = (&self.names, &self.shapes, &self.nodes);
            let nodes = names
                .get(&spec.type_name)
                .and_then(|label| self.label_index.get(&label))
                .into_iter()
                .flat_map(IdSet::iter)
                .filter_map(|id| all.get(id).map(|node| node_ref(names, shapes, *id, node)));
            self.declared.build(spec, nodes);
        }
    }

    pub fn has_index(&self, kind: IndexKind, types: &[&str], field: &str) -> bool {
        match kind {
            IndexKind::Range => self.declared.has_range(types, field),
            IndexKind::Text => types.iter().all(|ty| {
                self.declared.contains(&IndexSpec {
                    kind,
                    type_name: ty.to_string(),
                    field: field.to_string(),
                })
            }),
        }
    }

    pub fn range_candidates(
        &self,
        types: &[&str],
        field: &str,
        interval: &Interval,
    ) -> Option<HashSet<NodeId>> {
        self.declared.range_candidates(types, field, interval)
    }

    pub fn text_candidates(
        &self,
        types: &[&str],
        field: &str,
        pattern: TextPattern<'_>,
    ) -> Option<HashSet<NodeId>> {
        self.declared.text_candidates(types, field, pattern)
    }

    pub fn note_examined(&self, rows: usize) {
        self.examined.fetch_add(rows as u64, Ordering::Relaxed);
    }

    pub fn examined(&self) -> u64 {
        self.examined.load(Ordering::Relaxed)
    }

    pub fn note_expanded(&self, nodes: usize) {
        self.expanded.fetch_add(nodes as u64, Ordering::Relaxed);
    }

    pub fn expanded(&self) -> u64 {
        self.expanded.load(Ordering::Relaxed)
    }

    /// The ids the next created node and relationship will get.
    pub fn next_ids(&self) -> (NodeId, RelId) {
        (
            self.next_node_id.load(Ordering::SeqCst),
            self.next_rel_id.load(Ordering::SeqCst),
        )
    }

    /// Put the id counters back to an earlier [`Graph::next_ids`], after the
    /// writes that advanced them have been taken back.
    pub fn reset_next_ids(&mut self, (node, rel): (NodeId, RelId)) {
        self.next_node_id.store(node, Ordering::SeqCst);
        self.next_rel_id.store(rel, Ordering::SeqCst);
    }

    pub fn create_node(&mut self, labels: Vec<String>, props: HashMap<String, Value>) -> NodeId {
        let id = self.next_node_id.fetch_add(1, Ordering::SeqCst);
        self.restore_node(id, labels, props);
        id
    }

    pub fn restore_node(&mut self, id: NodeId, labels: Vec<String>, props: HashMap<String, Value>) {
        let (shape, values) = intern(&mut self.names, &mut self.shapes, &labels, props);
        if let Some(previous) = self.nodes.insert(id, NodeRecord { shape, values }) {
            self.remove_node_indexes(id, &previous);
        }
        for label in self.shapes.get(shape).labels.iter() {
            self.label_index.entry(*label).or_default().insert(id);
        }
        self.add_prop_indexes(id);
        self.next_node_id.fetch_max(id + 1, Ordering::SeqCst);
    }

    /// Index every property of stored node `id`, and add it to the declared
    /// indexes. Labels are indexed by the caller.
    fn add_prop_indexes(&mut self, id: NodeId) {
        let Some(record) = self.nodes.get(&id) else {
            return;
        };
        let node = node_ref(&self.names, &self.shapes, id, record);
        for (&key, value) in node.shape.keys.iter().zip(node.values) {
            let name = self.names.name(key);
            if let Value::Point(point) = value {
                self.spatial_index.insert(name, *point, id);
            }
            if let Value::Vector(v) = value {
                self.vector_index.insert(name, v, id);
            }
            let hash = self.property_hasher.hash_one((key, value));
            self.property_index.entry(hash).or_default().insert(id);
        }
        self.declared.insert(&node);
    }

    /// Take stored node `id` (whose record is `record`) out of every property
    /// index and the declared indexes. Labels are left to the caller.
    fn remove_prop_indexes(&mut self, id: NodeId, record: &NodeRecord) {
        let node = node_ref(&self.names, &self.shapes, id, record);
        self.declared.remove(&node);
        for (&key, value) in node.shape.keys.iter().zip(node.values) {
            let name = self.names.name(key);
            if let Value::Point(point) = value {
                self.spatial_index.remove(name, *point, id);
            }
            if let Value::Vector(v) = value {
                self.vector_index.remove(name, v, id);
            }
            let hash = self.property_hasher.hash_one((key, value));
            if let Some(set) = self.property_index.get_mut(&hash) {
                set.remove(id);
                if set.is_empty() {
                    self.property_index.remove(&hash);
                }
            }
        }
    }

    pub fn update_node(&mut self, id: NodeId, props: HashMap<String, Value>) {
        let Some(previous) = self.nodes.remove(&id) else {
            return;
        };
        self.remove_prop_indexes(id, &previous);
        let shape = self.shapes.get(previous.shape).clone();
        let mut merged: HashMap<Sym, Value> = shape
            .keys
            .iter()
            .copied()
            .zip(Vec::from(previous.values))
            .collect();
        for (key, value) in props {
            merged.insert(self.names.intern(&key), value);
        }
        let mut pairs: Vec<(Sym, Value)> = merged.into_iter().collect();
        pairs.sort_unstable_by_key(|(sym, _)| *sym);
        let keys = pairs.iter().map(|(sym, _)| *sym).collect();
        let values = pairs.into_iter().map(|(_, value)| value).collect();
        let shape = self.shapes.intern(Shape { labels: shape.labels, keys });
        self.nodes.insert(id, NodeRecord { shape, values });
        self.add_prop_indexes(id);
    }

    fn remove_node_indexes(&mut self, id: NodeId, node: &NodeRecord) {
        self.remove_prop_indexes(id, node);
        for label in self.shapes.get(node.shape).labels.iter() {
            if let Some(set) = self.label_index.get_mut(label) {
                set.remove(id);
            }
        }
    }

    /// All relationship ids incident to a node (outgoing + incoming). Used by
    /// DETACH DELETE to remove a node's relationships before the node itself.
    pub fn node_relationship_ids(&self, id: NodeId) -> Vec<RelId> {
        let mut ids: Vec<RelId> = Vec::new();
        if let Some(adjacency) = self.adjacency.get(&id) {
            ids.extend(adjacency.out.iter().copied());
            ids.extend(adjacency.inc.iter().copied());
        }
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    pub fn delete_node(&mut self, id: NodeId) {
        if let Some(node) = self.nodes.remove(&id) {
            self.remove_node_indexes(id, &node);
            // Remove connected relationships
            let adjacency = self.adjacency.remove(&id).unwrap_or_default();
            for rid in adjacency.out.iter().chain(adjacency.inc.iter()) {
                self.delete_relationship(*rid);
            }
        }
    }

    pub fn create_relationship(
        &mut self,
        kind: String,
        from: NodeId,
        to: NodeId,
        props: HashMap<String, Value>,
    ) -> RelId {
        let id = self.next_rel_id.fetch_add(1, Ordering::SeqCst);
        self.restore_relationship(id, kind, from, to, props);
        id
    }

    pub fn restore_relationship(
        &mut self,
        id: RelId,
        kind: String,
        from: NodeId,
        to: NodeId,
        props: HashMap<String, Value>,
    ) {
        let (shape, values) = intern(&mut self.names, &mut self.shapes, &[], props);
        let rel = RelRecord {
            kind: self.names.intern(&kind),
            shape,
            from,
            to,
            values,
        };
        if let Some(previous) = self.relationships.insert(id, rel) {
            self.remove_relationship_indexes(id, &previous);
        }
        self.adjacency.entry(from).or_default().out.insert(id);
        self.adjacency.entry(to).or_default().inc.insert(id);
        self.next_rel_id.fetch_max(id + 1, Ordering::SeqCst);
    }

    fn remove_relationship_indexes(&mut self, id: RelId, rel: &RelRecord) {
        for (node, outgoing) in [(rel.from, true), (rel.to, false)] {
            let Some(adjacency) = self.adjacency.get_mut(&node) else {
                continue;
            };
            if outgoing {
                adjacency.out.remove(id);
            } else {
                adjacency.inc.remove(id);
            }
            if adjacency.out.is_empty() && adjacency.inc.is_empty() {
                self.adjacency.remove(&node);
            }
        }
    }

    pub fn delete_relationship(&mut self, id: RelId) {
        if let Some(rel) = self.relationships.remove(&id) {
            self.remove_relationship_indexes(id, &rel);
        }
    }

    pub fn get_node(&self, id: NodeId) -> Option<NodeRef<'_>> {
        let node = self.nodes.get(&id)?;
        Some(node_ref(&self.names, &self.shapes, id, node))
    }

    pub fn get_relationship(&self, id: RelId) -> Option<RelRef<'_>> {
        let rel = self.relationships.get(&id)?;
        Some(rel_ref(&self.names, &self.shapes, id, rel))
    }

    pub fn nodes_by_label(&self, label: &str) -> Option<&IdSet> {
        self.label_index.get(&self.names.get(label)?)
    }

    /// The nodes whose `key` is `value`, ascending by id.
    pub fn nodes_by_property(&self, key: &str, value: &Value) -> Vec<NodeId> {
        let Some(sym) = self.names.get(key) else {
            return Vec::new();
        };
        let hash = self.property_hasher.hash_one((sym, value));
        let Some(set) = self.property_index.get(&hash) else {
            return Vec::new();
        };
        let mut ids: Vec<NodeId> = set
            .iter()
            .copied()
            .filter(|id| self.get_node(*id).and_then(|node| node.prop(key)) == Some(value))
            .collect();
        ids.sort_unstable();
        ids
    }

    /// Every node, in no particular order.
    pub fn nodes(&self) -> impl ExactSizeIterator<Item = NodeRef<'_>> {
        self.nodes
            .iter()
            .map(|(id, node)| node_ref(&self.names, &self.shapes, *id, node))
    }

    /// Every relationship, in no particular order.
    pub fn relationships(&self) -> impl ExactSizeIterator<Item = RelRef<'_>> {
        self.relationships
            .iter()
            .map(|(id, rel)| rel_ref(&self.names, &self.shapes, *id, rel))
    }

    /// The relationships leaving `node_id`; None when there are none.
    pub fn outgoing_rels(&self, node_id: NodeId) -> Option<&IdSet> {
        Some(&self.adjacency.get(&node_id)?.out).filter(|rels| !rels.is_empty())
    }

    /// The relationships entering `node_id`; None when there are none.
    pub fn incoming_rels(&self, node_id: NodeId) -> Option<&IdSet> {
        Some(&self.adjacency.get(&node_id)?.inc).filter(|rels| !rels.is_empty())
    }

    pub fn set_state(&mut self, nodes: HashMap<NodeId, Node>, rels: HashMap<RelId, Relationship>) {
        // Snapshots and WAL replay use the same index-maintenance paths.
        *self = Self::new();
        let mut nodes: Vec<_> = nodes.into_iter().collect();
        nodes.sort_by_key(|(id, _)| *id);
        for (id, node) in nodes {
            self.restore_node(id, node.labels, node.props);
        }
        for (id, rel) in rels {
            self.restore_relationship(id, rel.kind, rel.from, rel.to, rel.props);
        }
    }

    /// Every node as it is written down, by id.
    #[cfg(test)]
    pub fn all_nodes(&self) -> HashMap<NodeId, Node> {
        self.nodes().map(|node| (node.id, node.to_node())).collect()
    }

    /// Every relationship as it is written down, by id.
    #[cfg(test)]
    pub fn all_relationships(&self) -> HashMap<RelId, Relationship> {
        self.relationships()
            .map(|rel| (rel.id, rel.to_relationship()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_node_and_lookup() {
        let mut g = Graph::new();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::from("Alice"));
        let id = g.create_node(vec!["Person".to_string()], props.clone());
        let node = g.get_node(id).unwrap();
        assert_eq!(node.labels().collect::<Vec<_>>(), vec!["Person"]);
        assert_eq!(node.prop("name"), Some(&Value::from("Alice")));
        let by_label = g.nodes_by_label("Person").unwrap();
        assert!(by_label.contains(&id));
        let by_prop = g.nodes_by_property("name", &Value::from("Alice"));
        assert_eq!(by_prop, vec![id]);
    }

    #[test]
    fn test_relationship() {
        let mut g = Graph::new();
        let a = g.create_node(vec!["Person".to_string()], HashMap::new());
        let b = g.create_node(vec!["Person".to_string()], HashMap::new());
        let rid = g.create_relationship("KNOWS".to_string(), a, b, HashMap::new());
        let rel = g.get_relationship(rid).unwrap();
        assert_eq!(rel.kind, "KNOWS");
        assert_eq!(rel.from, a);
        assert_eq!(rel.to, b);
    }

    #[test]
    fn nodes_of_one_type_share_their_names_and_shape() {
        let mut g = Graph::new();
        for n in 0..100 {
            g.create_node(
                vec!["Person".into()],
                HashMap::from([
                    ("name".to_string(), Value::from(format!("p{n}"))),
                    ("age".to_string(), Value::Int(n)),
                ]),
            );
        }
        assert_eq!(g.names.len(), 3);
        assert_eq!(g.shapes.len(), 1);
        let node = g.get_node(50).unwrap();
        assert_eq!(node.prop("age"), Some(&Value::Int(49)));
        assert_eq!(node.prop("name"), Some(&Value::from("p49")));
        assert_eq!(node.prop("missing"), None);
    }

    #[test]
    fn update_moves_a_node_to_the_shape_of_its_new_keys() {
        let mut g = Graph::new();
        let id = g.create_node(vec!["T".into()], HashMap::from([("a".to_string(), Value::Int(1))]));
        g.update_node(id, HashMap::from([("b".to_string(), Value::Int(2)), ("a".to_string(), Value::Int(3))]));
        let node = g.get_node(id).unwrap();
        assert_eq!(node.prop("a"), Some(&Value::Int(3)));
        assert_eq!(node.prop("b"), Some(&Value::Int(2)));
        assert_eq!(node.labels().collect::<Vec<_>>(), vec!["T"]);
        assert!(g.nodes_by_property("a", &Value::Int(1)).is_empty());
        assert_eq!(g.nodes_by_property("a", &Value::Int(3)), vec![id]);
    }

    /// A map hashes by its length, so these two share a property-index
    /// bucket: the lookup must check the stored value, not trust the hash.
    #[test]
    fn a_property_lookup_checks_values_that_share_a_hash() {
        let mut g = Graph::new();
        let map = |k: &str| Value::Map(Box::new(HashMap::from([(k.to_string(), Value::Int(1))])));
        let a = g.create_node(vec!["T".into()], HashMap::from([("m".to_string(), map("a"))]));
        let b = g.create_node(vec!["T".into()], HashMap::from([("m".to_string(), map("b"))]));
        assert_eq!(g.nodes_by_property("m", &map("a")), vec![a]);
        assert_eq!(g.nodes_by_property("m", &map("b")), vec![b]);
        assert!(g.nodes_by_property("m", &map("c")).is_empty());
        assert!(g.nodes_by_property("other", &map("a")).is_empty());
        // Int 1 and Float 1.0 are different values.
        let int = g.create_node(vec!["T".into()], HashMap::from([("n".to_string(), Value::Int(1))]));
        assert_eq!(g.nodes_by_property("n", &Value::Int(1)), vec![int]);
        assert!(g.nodes_by_property("n", &Value::from_f64(1.0)).is_empty());
    }

    #[test]
    fn many_keys_are_found_by_symbol() {
        let mut g = Graph::new();
        let props: HashMap<String, Value> =
            (0..40).map(|k| (format!("k{k}"), Value::Int(k))).collect();
        let id = g.create_node(vec!["Wide".into()], props.clone());
        let node = g.get_node(id).unwrap();
        for (key, value) in &props {
            assert_eq!(node.prop(key), Some(value), "{key}");
        }
        assert_eq!(node.prop("k40"), None);
        assert_eq!(node.to_node().props, props);
    }
}

#[cfg(test)]
mod exhaustive_tests;
