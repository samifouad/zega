use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use zega_parser::Value;

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
    outgoing: HashMap<NodeId, HashSet<RelId>>,
    incoming: HashMap<NodeId, HashSet<RelId>>,
    next_node_id: AtomicU64,
    next_rel_id: AtomicU64,
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
            outgoing: HashMap::new(),
            incoming: HashMap::new(),
            next_node_id: AtomicU64::new(1),
            next_rel_id: AtomicU64::new(1),
        }
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
        self.nodes.insert(id, node);
        for lbl in &labels {
            self.label_index
                .entry(lbl.clone())
                .or_default()
                .insert(id);
        }
        for (k, v) in &props {
            self.property_index
                .entry((k.clone(), v.clone()))
                .or_default()
                .insert(id);
        }
        self.next_node_id.fetch_max(id + 1, Ordering::SeqCst);
    }

    pub fn update_node(&mut self, id: NodeId, props: HashMap<String, Value>) {
        if let Some(node) = self.nodes.get_mut(&id) {
            for (k, v) in &node.props {
                if let Some(set) = self.property_index.get_mut(&(k.clone(), v.clone())) {
                    set.remove(&id);
                }
            }
            node.props.extend(props);
            for (k, v) in &node.props {
                self.property_index
                    .entry((k.clone(), v.clone()))
                    .or_default()
                    .insert(id);
            }
        }
    }

    pub fn delete_node(&mut self, id: NodeId) {
        if let Some(node) = self.nodes.remove(&id) {
            for lbl in &node.labels {
                if let Some(set) = self.label_index.get_mut(lbl) {
                    set.remove(&id);
                }
            }
            for (k, v) in &node.props {
                if let Some(set) = self.property_index.get_mut(&(k.clone(), v.clone())) {
                    set.remove(&id);
                }
            }
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
        self.relationships.insert(id, rel);
        self.outgoing.entry(from).or_default().insert(id);
        self.incoming.entry(to).or_default().insert(id);
        self.next_rel_id.fetch_max(id + 1, Ordering::SeqCst);
    }

    pub fn delete_relationship(&mut self, id: RelId) {
        if let Some(rel) = self.relationships.remove(&id) {
            if let Some(set) = self.outgoing.get_mut(&rel.from) {
                set.remove(&id);
            }
            if let Some(set) = self.incoming.get_mut(&rel.to) {
                set.remove(&id);
            }
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
        self.nodes = nodes;
        self.relationships = rels;
        self.label_index.clear();
        self.property_index.clear();
        self.outgoing.clear();
        self.incoming.clear();
        for (id, node) in &self.nodes {
            for lbl in &node.labels {
                self.label_index.entry(lbl.clone()).or_default().insert(*id);
            }
            for (k, v) in &node.props {
                self.property_index.entry((k.clone(), v.clone())).or_default().insert(*id);
            }
        }
        for (id, rel) in &self.relationships {
            self.outgoing.entry(rel.from).or_default().insert(*id);
            self.incoming.entry(rel.to).or_default().insert(*id);
        }
        // Update next ids
        let max_node = self.nodes.keys().copied().max().unwrap_or(0);
        let max_rel = self.relationships.keys().copied().max().unwrap_or(0);
        self.next_node_id.store(max_node + 1, Ordering::SeqCst);
        self.next_rel_id.store(max_rel + 1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_node_and_lookup() {
        let mut g = Graph::new();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::String("Alice".to_string()));
        let id = g.create_node(vec!["Person".to_string()], props.clone());
        let node = g.get_node(id).unwrap();
        assert_eq!(node.labels, vec!["Person"]);
        assert_eq!(node.props.get("name"), Some(&Value::String("Alice".to_string())));
        let by_label = g.nodes_by_label("Person").unwrap();
        assert!(by_label.contains(&id));
        let by_prop = g.nodes_by_property("name", &Value::String("Alice".to_string())).unwrap();
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
