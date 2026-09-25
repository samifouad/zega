use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use crate::index::{DeclaredIndexes, IndexKind, IndexSpec, Interval, TextPattern};
use crate::value::Value;

pub type NodeId = u64;
pub type RelId = u64;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub labels: Vec<String>,
    pub props: HashMap<String, Value>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Relationship {
    pub id: RelId,
    pub kind: String,
    pub from: NodeId,
    pub to: NodeId,
    pub props: HashMap<String, Value>,
}

pub struct Graph {
    nodes: HashMap<NodeId, Node>,
    relationships: HashMap<RelId, Relationship>,
    label_index: HashMap<String, HashSet<NodeId>>,
    property_index: HashMap<(String, Value), HashSet<NodeId>>,
    spatial_index: crate::location::SpatialIndex,
    vector_index: crate::vector::VectorIndex,
    /// `index { }` declarations of the schema last run against this graph.
    declared: DeclaredIndexes,
    outgoing: HashMap<NodeId, HashSet<RelId>>,
    incoming: HashMap<NodeId, HashSet<RelId>>,
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
            nodes: HashMap::new(),
            relationships: HashMap::new(),
            label_index: HashMap::new(),
            property_index: HashMap::new(),
            spatial_index: Default::default(),
            vector_index: Default::default(),
            declared: Default::default(),
            outgoing: HashMap::new(),
            incoming: HashMap::new(),
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
            let nodes = self
                .label_index
                .get(&spec.type_name)
                .into_iter()
                .flatten()
                .filter_map(|id| self.nodes.get(id));
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
        let node = Node {
            id,
            labels: labels.clone(),
            props: props.clone(),
        };
        if let Some(previous) = self.nodes.insert(id, node) {
            self.remove_node_indexes(&previous);
        }
        for lbl in &labels {
            self.label_index
                .entry(lbl.clone())
                .or_default()
                .insert(id);
        }
        for (k, v) in &props {
            if let Value::Point(point) = v {
                self.spatial_index.insert(k, *point, id);
            }
            if let Value::Vector(v) = v { self.vector_index.insert(k, v, id); }
            self.property_index
                .entry((k.clone(), v.clone()))
                .or_default()
                .insert(id);
        }
        self.declared.insert(id, &labels, &props);
        self.next_node_id.fetch_max(id + 1, Ordering::SeqCst);
    }

    pub fn update_node(&mut self, id: NodeId, props: HashMap<String, Value>) {
        if let Some(node) = self.nodes.get_mut(&id) {
            self.declared.remove(id, &node.labels, &node.props);
            for (k, v) in &node.props {
                if let Value::Point(point) = v {
                    self.spatial_index.remove(k, *point, id);
                }
                if let Value::Vector(v) = v { self.vector_index.remove(k, v, id); }
                if let Some(set) = self.property_index.get_mut(&(k.clone(), v.clone())) {
                    set.remove(&id);
                }
            }
            node.props.extend(props);
            for (k, v) in &node.props {
                if let Value::Point(point) = v {
                    self.spatial_index.insert(k, *point, id);
                }
                if let Value::Vector(v) = v { self.vector_index.insert(k, v, id); }
                self.property_index
                    .entry((k.clone(), v.clone()))
                    .or_default()
                    .insert(id);
            }
            self.declared.insert(id, &node.labels, &node.props);
        }
    }

    fn remove_node_indexes(&mut self, node: &Node) {
        self.declared.remove(node.id, &node.labels, &node.props);
        for lbl in &node.labels {
            if let Some(set) = self.label_index.get_mut(lbl) {
                set.remove(&node.id);
            }
        }
        for (k, v) in &node.props {
            if let Value::Point(point) = v {
                self.spatial_index.remove(k, *point, node.id);
            }
            if let Value::Vector(v) = v { self.vector_index.remove(k, v, node.id); }
            if let Some(set) = self.property_index.get_mut(&(k.clone(), v.clone())) {
                set.remove(&node.id);
            }
        }
    }

    /// All relationship ids incident to a node (outgoing + incoming). Used by
    /// DETACH DELETE to remove a node's relationships before the node itself.
    pub fn node_relationship_ids(&self, id: NodeId) -> Vec<RelId> {
        let mut ids: Vec<RelId> = Vec::new();
        if let Some(out) = self.outgoing.get(&id) {
            ids.extend(out.iter().copied());
        }
        if let Some(inc) = self.incoming.get(&id) {
            ids.extend(inc.iter().copied());
        }
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    pub fn delete_node(&mut self, id: NodeId) {
        if let Some(node) = self.nodes.remove(&id) {
            self.remove_node_indexes(&node);
            // Remove connected relationships
            let out_rels: Vec<RelId> = self.outgoing.remove(&id).unwrap_or_default().into_iter().collect();
            let in_rels: Vec<RelId> = self.incoming.remove(&id).unwrap_or_default().into_iter().collect();
            for rid in out_rels {
                self.delete_relationship(rid);
            }
            for rid in in_rels {
                self.delete_relationship(rid);
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
        let rel = Relationship {
            id,
            kind,
            from,
            to,
            props,
        };
        if let Some(previous) = self.relationships.insert(id, rel) {
            self.remove_relationship_indexes(&previous);
        }
        self.outgoing.entry(from).or_default().insert(id);
        self.incoming.entry(to).or_default().insert(id);
        self.next_rel_id.fetch_max(id + 1, Ordering::SeqCst);
    }

    fn remove_relationship_indexes(&mut self, rel: &Relationship) {
        if let Some(set) = self.outgoing.get_mut(&rel.from) {
            set.remove(&rel.id);
        }
        if let Some(set) = self.incoming.get_mut(&rel.to) {
            set.remove(&rel.id);
        }
    }

    pub fn delete_relationship(&mut self, id: RelId) {
        if let Some(rel) = self.relationships.remove(&id) {
            self.remove_relationship_indexes(&rel);
        }
    }

    pub fn get_node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    pub fn get_relationship(&self, id: RelId) -> Option<&Relationship> {
        self.relationships.get(&id)
    }

    pub fn nodes_by_label(&self, label: &str) -> Option<&HashSet<NodeId>> {
        self.label_index.get(label)
    }

    pub fn nodes_by_property(&self, key: &str, value: &Value) -> Option<&HashSet<NodeId>> {
        self.property_index.get(&(key.to_string(), value.clone()))
    }

    pub fn all_nodes(&self) -> &HashMap<NodeId, Node> {
        &self.nodes
    }

    pub fn all_relationships(&self) -> &HashMap<RelId, Relationship> {
        &self.relationships
    }

    pub fn outgoing_rels(&self, node_id: NodeId) -> Option<&HashSet<RelId>> {
        self.outgoing.get(&node_id)
    }

    pub fn incoming_rels(&self, node_id: NodeId) -> Option<&HashSet<RelId>> {
        self.incoming.get(&node_id)
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
        assert_eq!(node.labels, vec!["Person"]);
        assert_eq!(node.props.get("name"), Some(&Value::from("Alice")));
        let by_label = g.nodes_by_label("Person").unwrap();
        assert!(by_label.contains(&id));
        let by_prop = g.nodes_by_property("name", &Value::from("Alice")).unwrap();
        assert!(by_prop.contains(&id));
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
}

#[cfg(test)]
mod exhaustive_tests;
