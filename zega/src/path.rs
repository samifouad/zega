//! Route search for ZQL `*path`: fewest edges (breadth-first), cheapest by an
//! edge weight (Dijkstra), and cheapest with a guess toward the target (A*).
//!
//! The searches know nothing about the store. The caller hands them the
//! neighbours of a node, already sorted by node id and then relationship id,
//! with their weights, and a heuristic. Equal routes are broken the same way
//! every time: nodes are expanded in order of cost (A*: cost plus guess), then
//! node id, and a node keeps the first predecessor that reached it at its best
//! cost. The same data therefore always returns the same route.

use crate::graph::{NodeId, RelId};
use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap};

/// A route from the start to a goal. `nodes` has one more entry than `rels`:
/// `rels[i]` joins `nodes[i]` and `nodes[i + 1]`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Route {
    pub nodes: Vec<NodeId>,
    pub rels: Vec<RelId>,
    pub cost: f64,
}

/// The route, when a goal is reachable within the bound, and the number of
/// nodes whose edges the search read.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Found {
    pub route: Option<Route>,
    pub expanded: usize,
}

/// One edge out of a node: where it leads, which relationship, and its weight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Step {
    pub to: NodeId,
    pub rel: RelId,
    pub weight: f64,
}

/// The most a route may cost. `inclusive` allows a route that costs exactly
/// `limit`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Limit {
    pub limit: f64,
    pub inclusive: bool,
}

impl Limit {
    fn allows(self, cost: f64) -> bool {
        if self.inclusive {
            cost <= self.limit
        } else {
            cost < self.limit
        }
    }
}

/// The route with the fewest edges, at most `max_hops` long. Each level of
/// the breadth-first search is expanded in node-id order, and the search stops
/// at the first level that holds a goal, returning the goal with the lowest id.
pub(crate) fn fewest_edges<E>(
    start: NodeId,
    is_goal: impl Fn(NodeId) -> bool,
    max_hops: Option<usize>,
    mut next: impl FnMut(NodeId) -> Result<Vec<(NodeId, RelId)>, E>,
) -> Result<Found, E> {
    if is_goal(start) {
        return Ok(Found {
            route: Some(Route { nodes: vec![start], rels: Vec::new(), cost: 0.0 }),
            expanded: 0,
        });
    }
    let mut parent: HashMap<NodeId, (NodeId, RelId)> = HashMap::new();
    let mut level = vec![start];
    let mut depth = 0usize;
    let mut expanded = 0usize;
    while !level.is_empty() && max_hops.is_none_or(|max| depth < max) {
        let mut reached = Vec::new();
        for &node in &level {
            expanded += 1;
            for (to, rel) in next(node)? {
                if to != start && !parent.contains_key(&to) {
                    parent.insert(to, (node, rel));
                    reached.push(to);
                }
            }
        }
        depth += 1;
        reached.sort_unstable();
        if let Some(&goal) = reached.iter().find(|&&node| is_goal(node)) {
            let route = walk_back(start, goal, &parent, depth as f64);
            return Ok(Found { route: Some(route), expanded });
        }
        level = reached;
    }
    Ok(Found { route: None, expanded })
}

/// The cheapest route by edge weight. With `guess` returning 0 this is
/// Dijkstra; with a consistent lower bound on the remaining cost it is A*.
/// Weights must be non-negative; the caller rejects the others before they
/// get here. `limit` prunes every node whose cost plus guess exceeds it,
/// which never drops a route inside the limit because the guess never
/// overestimates.
pub(crate) fn cheapest<E>(
    start: NodeId,
    is_goal: impl Fn(NodeId) -> bool,
    limit: Option<Limit>,
    mut next: impl FnMut(NodeId) -> Result<Vec<Step>, E>,
    mut guess: impl FnMut(NodeId) -> Result<f64, E>,
) -> Result<Found, E> {
    let mut guesses: HashMap<NodeId, f64> = HashMap::new();
    let mut guess_of = |node: NodeId| -> Result<f64, E> {
        if let Some(value) = guesses.get(&node) {
            return Ok(*value);
        }
        let value = guess(node)?;
        guesses.insert(node, value);
        Ok(value)
    };
    let mut best: HashMap<NodeId, f64> = HashMap::from([(start, 0.0)]);
    let mut parent: HashMap<NodeId, (NodeId, RelId)> = HashMap::new();
    let mut open = BinaryHeap::new();
    let first = guess_of(start)?;
    if limit.is_some_and(|limit| !limit.allows(first)) {
        return Ok(Found { route: None, expanded: 0 });
    }
    open.push(Reverse(Entry { estimate: first, node: start, cost: 0.0 }));
    let mut expanded = 0usize;
    while let Some(Reverse(Entry { node, cost, .. })) = open.pop() {
        if best.get(&node).is_some_and(|known| cost > *known) {
            continue; // A cheaper route to this node was found after this entry.
        }
        if is_goal(node) {
            let route = walk_back(start, node, &parent, cost);
            return Ok(Found { route: Some(route), expanded });
        }
        expanded += 1;
        for step in next(node)? {
            let reached = cost + step.weight;
            if best.get(&step.to).is_some_and(|known| reached >= *known) {
                continue;
            }
            let estimate = reached + guess_of(step.to)?;
            if limit.is_some_and(|limit| !limit.allows(estimate)) {
                continue;
            }
            best.insert(step.to, reached);
            parent.insert(step.to, (node, step.rel));
            open.push(Reverse(Entry { estimate, node: step.to, cost: reached }));
        }
    }
    Ok(Found { route: None, expanded })
}

fn walk_back(
    start: NodeId,
    goal: NodeId,
    parent: &HashMap<NodeId, (NodeId, RelId)>,
    cost: f64,
) -> Route {
    let mut nodes = vec![goal];
    let mut rels = Vec::new();
    let mut at = goal;
    while at != start {
        let (from, rel) = parent[&at];
        rels.push(rel);
        nodes.push(from);
        at = from;
    }
    nodes.reverse();
    rels.reverse();
    Route { nodes, rels, cost }
}

/// A heap entry, ordered by estimate and then node id.
#[derive(Clone, Copy, Debug)]
struct Entry {
    estimate: f64,
    node: NodeId,
    cost: f64,
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Entry {}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.estimate
            .total_cmp(&other.estimate)
            .then(self.node.cmp(&other.node))
            .then(self.cost.total_cmp(&other.cost))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;

    /// Edges `(from, to, weight)`; relationship ids are positions + 1.
    struct Net {
        edges: Vec<(NodeId, NodeId, f64)>,
    }

    impl Net {
        fn out(&self, node: NodeId) -> Vec<Step> {
            let mut steps: Vec<Step> = self
                .edges
                .iter()
                .enumerate()
                .filter(|(_, (from, _, _))| *from == node)
                .map(|(i, (_, to, weight))| Step { to: *to, rel: i as RelId + 1, weight: *weight })
                .collect();
            steps.sort_by_key(|step| (step.to, step.rel));
            steps
        }

        fn bfs(&self, start: NodeId, goal: NodeId, max: Option<usize>) -> Found {
            fewest_edges(start, |node| node == goal, max, |node| {
                Ok::<_, Infallible>(self.out(node).iter().map(|s| (s.to, s.rel)).collect())
            })
            .unwrap()
        }

        fn dijkstra(&self, start: NodeId, goal: NodeId, limit: Option<Limit>) -> Found {
            cheapest(
                start,
                |node| node == goal,
                limit,
                |node| Ok::<_, Infallible>(self.out(node)),
                |_| Ok(0.0),
            )
            .unwrap()
        }
    }

    #[test]
    fn fewest_edges_prefers_the_lower_id_on_a_tie() {
        // 1 -> 2 -> 4 and 1 -> 3 -> 4: both two edges; 2 is the lower id.
        let net = Net { edges: vec![(1, 3, 1.0), (1, 2, 1.0), (3, 4, 1.0), (2, 4, 1.0)] };
        let found = net.bfs(1, 4, None);
        assert_eq!(found.route.unwrap().nodes, vec![1, 2, 4]);
        assert_eq!(net.dijkstra(1, 4, None).route.unwrap().nodes, vec![1, 2, 4]);
    }

    #[test]
    fn cheapest_takes_more_edges_when_they_cost_less() {
        let net = Net { edges: vec![(1, 4, 10.0), (1, 2, 1.0), (2, 3, 1.0), (3, 4, 1.0)] };
        assert_eq!(net.bfs(1, 4, None).route.unwrap().nodes, vec![1, 4]);
        let route = net.dijkstra(1, 4, None).route.unwrap();
        assert_eq!(route.nodes, vec![1, 2, 3, 4]);
        assert_eq!(route.rels, vec![2, 3, 4]);
        assert_eq!(route.cost, 3.0);
    }

    #[test]
    fn bounds_cut_routes_that_are_too_long() {
        let net = Net { edges: vec![(1, 2, 1.0), (2, 3, 1.0), (3, 4, 1.0)] };
        assert!(net.bfs(1, 4, Some(2)).route.is_none());
        assert_eq!(net.bfs(1, 4, Some(3)).route.unwrap().cost, 3.0);
        let under = Limit { limit: 3.0, inclusive: false };
        let at_most = Limit { limit: 3.0, inclusive: true };
        assert!(net.dijkstra(1, 4, Some(under)).route.is_none());
        assert_eq!(net.dijkstra(1, 4, Some(at_most)).route.unwrap().cost, 3.0);
    }

    #[test]
    fn the_start_is_a_route_of_no_edges_and_unreachable_is_none() {
        let net = Net { edges: vec![(1, 2, 1.0)] };
        assert_eq!(net.bfs(1, 1, None).route.unwrap().rels, Vec::<RelId>::new());
        assert_eq!(net.dijkstra(1, 1, None).route.unwrap().cost, 0.0);
        assert!(net.bfs(2, 1, None).route.is_none());
        assert!(net.dijkstra(2, 1, None).route.is_none());
    }

    #[test]
    fn zero_weight_cycles_end() {
        let net = Net { edges: vec![(1, 2, 0.0), (2, 1, 0.0), (2, 3, 0.0), (3, 2, 0.0), (3, 4, 2.0)] };
        let route = net.dijkstra(1, 4, None).route.unwrap();
        assert_eq!(route.nodes, vec![1, 2, 3, 4]);
        assert_eq!(route.cost, 2.0);
    }
}
