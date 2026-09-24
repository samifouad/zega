//! [`GraphStore`] over HashMaps: today's in-memory layout behind the seam.
//! Used to separate what the seam costs from what storage costs.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use crate::model::{index_key, Dir, Interval, Key, Node, NodeId, Op, Rel, RelId};
use crate::store::{GraphStore, Result};

#[derive(Clone, Copy, PartialEq)]
struct F(f64);
impl Eq for F {}
impl PartialOrd for F {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for F {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

type Adjacency = Arc<[(NodeId, RelId)]>;

#[derive(Default)]
struct Range {
    num: BTreeMap<F, BTreeSet<NodeId>>,
    str: BTreeMap<String, BTreeSet<NodeId>>,
}

#[derive(Default)]
pub struct MemStore {
    nodes: HashMap<NodeId, Arc<Node>>,
    rels: HashMap<RelId, Arc<Rel>>,
    labels: HashMap<String, BTreeSet<NodeId>>,
    adj: HashMap<(NodeId, String, Dir), Adjacency>,
    ranges: HashMap<(String, String), Range>,
    next: (NodeId, RelId),
}

impl MemStore {
    pub fn new() -> Self {
        MemStore { next: (1, 1), ..Default::default() }
    }

    pub fn declare_range(&mut self, ty: &str, field: &str) {
        let key = (ty.to_string(), field.to_string());
        if self.ranges.contains_key(&key) {
            return;
        }
        let mut range = Range::default();
        for id in self.labels.get(ty).into_iter().flatten() {
            if let Some(value) = self.nodes[id].props.get(field) {
                add_key(&mut range, value, *id);
            }
        }
        self.ranges.insert(key, range);
    }

    fn index(&mut self, node: &Node, add: bool) {
        for ((ty, field), range) in self.ranges.iter_mut() {
            if !node.labels.contains(ty) {
                continue;
            }
            if let Some(value) = node.props.get(field) {
                if add {
                    add_key(range, value, node.id);
                } else {
                    match index_key(value) {
                        Some(Key::Num(n)) => {
                            if let Some(set) = range.num.get_mut(&F(n)) {
                                set.remove(&node.id);
                            }
                        }
                        Some(Key::Str(s)) => {
                            if let Some(set) = range.str.get_mut(s) {
                                set.remove(&node.id);
                            }
                        }
                        None => {}
                    }
                }
            }
        }
    }

    fn put_node(&mut self, node: Node) {
        if let Some(old) = self.nodes.remove(&node.id) {
            self.index(&old, false);
            for l in &old.labels {
                if let Some(set) = self.labels.get_mut(l) {
                    set.remove(&old.id);
                }
            }
        }
        self.index(&node, true);
        for l in &node.labels {
            self.labels.entry(l.clone()).or_default().insert(node.id);
        }
        self.next.0 = self.next.0.max(node.id + 1);
        self.nodes.insert(node.id, Arc::new(node));
    }

    fn edit_adj(&mut self, key: (NodeId, String, Dir), entry: (NodeId, RelId), add: bool) {
        let mut list: Vec<_> = self.adj.get(&key).map(|l| l.to_vec()).unwrap_or_default();
        if add {
            if let Err(at) = list.binary_search(&entry) {
                list.insert(at, entry);
            }
        } else {
            list.retain(|e| *e != entry);
        }
        self.adj.insert(key, Arc::from(list));
    }

    fn remove_rel(&mut self, id: RelId) {
        if let Some(rel) = self.rels.remove(&id) {
            self.edit_adj((rel.from, rel.kind.clone(), Dir::Out), (rel.to, id), false);
            self.edit_adj((rel.to, rel.kind.clone(), Dir::In), (rel.from, id), false);
        }
    }
}

fn add_key(range: &mut Range, value: &crate::model::Value, id: NodeId) {
    match index_key(value) {
        Some(Key::Num(n)) => {
            range.num.entry(F(n)).or_default().insert(id);
        }
        Some(Key::Str(s)) => {
            range.str.entry(s.to_string()).or_default().insert(id);
        }
        None => {}
    }
}

impl GraphStore for MemStore {
    fn node(&self, id: NodeId) -> Result<Option<Arc<Node>>> {
        Ok(self.nodes.get(&id).cloned())
    }

    fn relationship(&self, id: RelId) -> Result<Option<Arc<Rel>>> {
        Ok(self.rels.get(&id).cloned())
    }

    fn adjacency(&self, id: NodeId, kind: &str, dir: Dir) -> Result<Arc<[(NodeId, RelId)]>> {
        Ok(self.adj.get(&(id, kind.to_string(), dir)).cloned().unwrap_or_else(|| Arc::from(Vec::new())))
    }

    fn scan_label(&self, label: &str, after: NodeId, limit: usize) -> Result<Vec<Arc<Node>>> {
        Ok(self
            .labels
            .get(label)
            .into_iter()
            .flat_map(|set| set.range(after + 1..))
            .take(limit)
            .map(|id| self.nodes[id].clone())
            .collect())
    }

    fn index_range(&self, ty: &str, field: &str, interval: &Interval) -> Result<Option<Vec<NodeId>>> {
        let Some(range) = self.ranges.get(&(ty.to_string(), field.to_string())) else {
            return Ok(None);
        };
        let ids = match interval {
            Interval::Empty => Vec::new(),
            Interval::Num { low, high } => {
                let low = F(low.unwrap_or(f64::NEG_INFINITY));
                let high = F(high.unwrap_or(f64::INFINITY));
                if low > high {
                    Vec::new()
                } else {
                    range.num.range(low..=high).flat_map(|(_, s)| s.iter().copied()).collect()
                }
            }
            Interval::Str { low, high } => {
                let low = low.clone().unwrap_or_default();
                match high {
                    Some(high) if &low > high => Vec::new(),
                    Some(high) => range.str.range(low..=high.clone()).flat_map(|(_, s)| s.iter().copied()).collect(),
                    None => range.str.range(low..).flat_map(|(_, s)| s.iter().copied()).collect(),
                }
            }
        };
        Ok(Some(ids))
    }

    fn has_index(&self, ty: &str, field: &str) -> bool {
        self.ranges.contains_key(&(ty.to_string(), field.to_string()))
    }

    fn next_ids(&self) -> (NodeId, RelId) {
        self.next
    }

    fn apply(&mut self, ops: Vec<Op>) -> Result<()> {
        for op in ops {
            match op {
                Op::InsertNode { id, labels, props } => self.put_node(Node { id, labels, props }),
                Op::UpdateNode { id, props } => {
                    if let Some(node) = self.nodes.get(&id) {
                        let mut node = (**node).clone();
                        node.props.extend(props);
                        self.put_node(node);
                    }
                }
                Op::DeleteNode { id } => {
                    let incident: Vec<RelId> = self
                        .rels
                        .values()
                        .filter(|r| r.from == id || r.to == id)
                        .map(|r| r.id)
                        .collect();
                    for rel in incident {
                        self.remove_rel(rel);
                    }
                    if let Some(old) = self.nodes.remove(&id) {
                        self.index(&old, false);
                        for l in &old.labels {
                            if let Some(set) = self.labels.get_mut(l) {
                                set.remove(&id);
                            }
                        }
                    }
                }
                Op::InsertRel { id, kind, from, to, props } => {
                    self.edit_adj((from, kind.clone(), Dir::Out), (to, id), true);
                    self.edit_adj((to, kind.clone(), Dir::In), (from, id), true);
                    self.next.1 = self.next.1.max(id + 1);
                    self.rels.insert(id, Arc::new(Rel { id, kind, from, to, props }));
                }
                Op::DeleteRel { id } => self.remove_rel(id),
            }
        }
        Ok(())
    }
}
