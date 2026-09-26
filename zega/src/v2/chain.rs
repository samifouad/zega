//! Chains in a condition (zegadb/zega#86): `has country(iso = "CA")`,
//! `!have flights in country(iso = "US")`, `same team in arena`,
//! `… in same country`, and `flights within 3 hops`.
//!
//! A chain holds for a node when some walk along its hops, from that node,
//! passes every hop's test. Hops are followed a frontier at a time, so a
//! chain costs the nodes it reaches, not the paths to them. Where a later
//! chain in the same `&&` group says `same team`, the walk splits at `team`
//! and each team is tried on its own, so both chains agree on the one team.
use super::*;
use crate::lang::{Chain, Hop, Repeat};
use std::rc::Rc;

/// The stored kind, direction and target types of `field` on the node's type.
fn edge_of<'s>(schema: &'s Schema, node: NodeRef<'_>, field: &str) -> Option<(&'s str, Direction, &'s [String])> {
    node.labels().find_map(|label| {
        let (_, kind, direction, targets, _) = schema.edge(label, field).ok()?.as_edge()?;
        Some((kind, direction, targets))
    })
}

/// The nodes one `field` hop from `id`, of a type the relationship reaches.
/// Each relationship read is charged to the statement.
fn step(graph: &Graph, schema: &Schema, id: NodeId, field: &str, work: &mut Work) -> Result<Vec<NodeId>, LangError> {
    let Some(node) = graph.get_node(id) else {
        return Ok(Vec::new());
    };
    let Some((kind, direction, targets)) = edge_of(schema, node, field) else {
        return Ok(Vec::new());
    };
    let next = neighbors(graph, id, kind, direction);
    work.charge(next.len())?;
    Ok(next
        .into_iter()
        .map(|(to, _)| to)
        .filter(|to| node_has_any_label(graph, *to, targets))
        .collect())
}

/// `field N hops` or `field within N hops` from one node: the nodes whose
/// shortest distance along `field` is in `min..=max`, as `*min..max` reads
/// them. The start is never among them, and no node is reached twice.
fn repeat(graph: &Graph, schema: &Schema, from: NodeId, field: &str, (min, max): (usize, usize), work: &mut Work) -> Result<Vec<NodeId>, LangError> {
    let mut seen = HashSet::from([from]);
    let mut layer = vec![from];
    let mut out = Vec::new();
    for depth in 1..=max {
        let mut next = Vec::new();
        for id in layer {
            for to in step(graph, schema, id, field, work)? {
                if seen.insert(to) {
                    next.push(to);
                }
            }
        }
        if depth >= min {
            out.extend(next.iter().copied());
        }
        if next.is_empty() {
            break;
        }
        layer = next;
    }
    Ok(out)
}

/// The nodes `hop` reaches from any of `frontier` that pass its test.
fn advance(graph: &Graph, schema: &Schema, frontier: &[NodeId], hop: &Hop, work: &mut Work) -> Result<Vec<NodeId>, LangError> {
    let mut reached = Vec::new();
    for &from in frontier {
        match hop.repeat {
            None => reached.extend(step(graph, schema, from, &hop.field, work)?),
            Some(r) => reached.extend(repeat(graph, schema, from, &hop.field, r.range(), work)?),
        }
    }
    reached.sort_unstable();
    reached.dedup();
    let mut kept = Vec::with_capacity(reached.len());
    for id in reached {
        if node_matches(graph, schema, id, hop.test.as_ref(), work)? {
            kept.push(id);
        }
    }
    Ok(kept)
}

/// A plain chain, `has …` or `!have …`, with no `same` in it.
pub(super) fn holds(graph: &Graph, schema: &Schema, id: NodeId, chain: &Chain, work: &mut Work) -> Result<bool, LangError> {
    let found = !walks(graph, schema, id, chain, &HashMap::new(), &[], true, work)?.is_empty();
    Ok(found != chain.negated)
}

/// The nodes a later `same` names, bound by a walk: one per named hop.
type Binding = Vec<(String, NodeId)>;

/// The walks of `chain` from `id` that pass every test, as the nodes they
/// bind at the hops named in `wanted`. `bound` holds what earlier chains
/// bound, for this chain's own `same`. With `any`, the first walk is enough.
#[allow(clippy::too_many_arguments)] // one walk's whole state; split, it would only travel as a struct
fn walks(
    graph: &Graph,
    schema: &Schema,
    id: NodeId,
    chain: &Chain,
    bound: &HashMap<String, NodeId>,
    wanted: &[&str],
    any: bool,
    work: &mut Work,
) -> Result<Vec<Binding>, LangError> {
    let start = match &chain.from {
        None => id,
        Some(same) => {
            let Some(&node) = bound.get(&same.name) else {
                return Ok(Vec::new());
            };
            if !node_matches(graph, schema, node, same.test.as_ref(), work)? {
                return Ok(Vec::new());
            }
            node
        }
    };
    let mut out = Vec::new();
    let mut stack: Vec<(usize, Vec<NodeId>, Binding)> = vec![(0, vec![start], Vec::new())];
    while let Some((k, frontier, binding)) = stack.pop() {
        if frontier.is_empty() {
            continue;
        }
        let Some(hop) = chain.hops.get(k) else {
            if !out.contains(&binding) {
                out.push(binding);
            }
            if any {
                break;
            }
            continue;
        };
        // Nothing after here is named or joined: reaching the pinned end set
        // is the whole answer, found from both ends at once.
        if let Some(pinned) = pinned(graph, schema, id, chain, k, wanted, work)? {
            if frontier.iter().try_fold(false, |hit, &from| -> Result<bool, LangError> {
                Ok(hit || meet(graph, schema, from, &hop.field, hop.repeat.map_or((1, 1), Repeat::range).1, &pinned, work)?)
            })? {
                out.push(binding);
                if any {
                    break;
                }
            }
            continue;
        }
        let mut next = advance(graph, schema, &frontier, hop, work)?;
        if hop.same {
            let Some(&node) = bound.get(&hop.field) else {
                continue;
            };
            next.retain(|reached| *reached == node);
        }
        if !hop.same && wanted.contains(&hop.field.as_str()) {
            // Each node here may bind a different later walk: try each alone.
            for &node in next.iter().rev() {
                let mut binding = binding.clone();
                binding.push((hop.field.clone(), node));
                stack.push((k + 1, vec![node], binding));
            }
        } else {
            stack.push((k + 1, next, binding));
        }
    }
    Ok(out)
}

/// Whether `id` passes an `&&` group in which some chain says `same`: the
/// other tests as usual, and the chains all at once, agreeing on every node
/// a `same` names. The checker has made each name refer to one earlier hop.
pub(super) fn and_group(graph: &Graph, schema: &Schema, id: NodeId, terms: &[BoolExpr], work: &mut Work) -> Result<bool, LangError> {
    let mut chains: Vec<&Chain> = Vec::new();
    for term in terms {
        match term {
            BoolExpr::Test(Pred::Chain(chain)) if !chain.negated => chains.push(chain),
            other => {
                if !eval_expr(graph, schema, id, other, work)? {
                    return Ok(false);
                }
            }
        }
    }
    // What each chain's later chains name with `same`.
    let wanted: Vec<Vec<&str>> = (0..chains.len())
        .map(|i| {
            chains[i + 1..]
                .iter()
                .flat_map(|later| later.from.as_ref().map(|same| same.name.as_str()).into_iter().chain(later.hops.iter().filter(|hop| hop.same).map(|hop| hop.field.as_str())))
                .filter(|name| chains[i].names().any(|own| own == *name))
                .collect()
        })
        .collect();
    solve(graph, schema, id, &chains, &wanted, &mut HashMap::new(), work)
}

fn solve(
    graph: &Graph,
    schema: &Schema,
    id: NodeId,
    chains: &[&Chain],
    wanted: &[Vec<&str>],
    bound: &mut HashMap<String, NodeId>,
    work: &mut Work,
) -> Result<bool, LangError> {
    let Some((chain, rest)) = chains.split_first() else {
        return Ok(true);
    };
    let any = wanted[0].is_empty();
    for binding in walks(graph, schema, id, chain, bound, &wanted[0], any, work)? {
        for (name, node) in &binding {
            bound.insert(name.clone(), *node);
        }
        if solve(graph, schema, id, rest, &wanted[1..], bound, work)? {
            return Ok(true);
        }
        for (name, _) in &binding {
            bound.remove(name);
        }
    }
    Ok(false)
}

/// For a `within N hops` hop `k` whose end the index pins: the nodes it has
/// to reach for the rest of the chain to pass. None when that does not
/// apply; computed once per statement and kept in `work`.
fn pinned(
    graph: &Graph,
    schema: &Schema,
    id: NodeId,
    chain: &Chain,
    k: usize,
    wanted: &[&str],
    work: &mut Work,
) -> Result<Option<Rc<HashSet<NodeId>>>, LangError> {
    let hop = &chain.hops[k];
    if !matches!(hop.repeat, Some(Repeat::Within(_))) || chain.from.is_some() {
        return Ok(None);
    }
    let rest = &chain.hops[k + 1..];
    if chain.hops[k..].iter().any(|hop| hop.same || wanted.contains(&hop.field.as_str()))
        || rest.iter().any(|hop| hop.repeat.is_some())
    {
        return Ok(None);
    }
    let Some(start) = graph.get_node(id).and_then(|node| node.first_label().map(str::to_string)) else {
        return Ok(None);
    };
    let key = (chain as *const Chain as usize, k, start.clone());
    if let Some(found) = work.pins.get(&key) {
        return Ok(found.clone());
    }
    let found = pin(graph, schema, &start, chain, k, work)?.map(Rc::new);
    work.pins.insert(key, found.clone());
    Ok(found)
}

/// The types each hop lands on, following the schema from `start`.
fn landings(schema: &Schema, start: &[String], hops: &[Hop]) -> Option<Vec<Vec<String>>> {
    let mut here = start.to_vec();
    let mut out = Vec::with_capacity(hops.len());
    for hop in hops {
        let mut next: Vec<String> = Vec::new();
        for ty in &here {
            if let Ok(edge) = schema.edge(ty, &hop.field) {
                for target in edge.as_edge()?.3 {
                    if !next.contains(target) {
                        next.push(target.clone());
                    }
                }
            }
        }
        if next.is_empty() {
            return None;
        }
        out.push(next.clone());
        here = next;
    }
    Some(out)
}

/// The nodes that pass the last hop's test, found through an index; None
/// when no index answers it. Exact: each candidate is tested.
fn indexed_end(graph: &Graph, schema: &Schema, types: &[String], hop: &Hop, work: &mut Work) -> Result<Option<Vec<NodeId>>, LangError> {
    let Some(test) = &hop.test else {
        return Ok(None);
    };
    let names: Vec<&str> = types.iter().map(String::as_str).collect();
    let Some(found) = index_filter(graph, schema, &names, test, work)? else {
        return Ok(None);
    };
    let mut ids: Vec<NodeId> = found.into_iter().filter(|id| node_has_any_label(graph, *id, types)).collect();
    ids.sort_unstable();
    retain_matches(graph, schema, &mut ids, Some(test), work)?;
    Ok(Some(ids))
}

/// The nodes one `hop` back from `ends`: those of `types` whose `hop` reaches
/// one of them. With a repeat, every node within its most hops (a superset).
fn back(graph: &Graph, schema: &Schema, ends: &HashSet<NodeId>, types: &[String], hop: &Hop, work: &mut Work) -> Result<HashSet<NodeId>, LangError> {
    let kinds = kinds_of(schema, types, &hop.field);
    let depth = hop.repeat.map_or(1, |r| r.range().1);
    let mut out = HashSet::new();
    let mut layer: Vec<NodeId> = ends.iter().copied().collect();
    let mut seen: HashSet<NodeId> = ends.clone();
    for _ in 0..depth {
        let mut next = Vec::new();
        for id in layer {
            for (kind, direction) in &kinds {
                let reverse = match direction {
                    Direction::Out => Direction::In,
                    Direction::In => Direction::Out,
                };
                let from = neighbors(graph, id, kind, reverse);
                work.charge(from.len())?;
                for (node, _) in from {
                    if node_has_any_label(graph, node, types) {
                        out.insert(node);
                    }
                    if seen.insert(node) {
                        next.push(node);
                    }
                }
            }
        }
        layer = next;
    }
    Ok(out)
}

/// The stored (kind, direction) of `field` on each of `types`.
fn kinds_of(schema: &Schema, types: &[String], field: &str) -> Vec<(String, Direction)> {
    let mut kinds: Vec<(String, Direction)> = Vec::new();
    for ty in types {
        if let Some((_, kind, direction, ..)) = schema.edge(ty, field).ok().and_then(|edge| edge.as_edge()) {
            if !kinds.iter().any(|(k, d)| k == kind && *d == direction) {
                kinds.push((kind.to_string(), direction));
            }
        }
    }
    kinds
}

/// Backwards from the indexed end to hop `k`: the nodes hop `k` must reach.
/// Only single hops follow `k`, so each step back is exact.
fn pin(graph: &Graph, schema: &Schema, start: &str, chain: &Chain, k: usize, work: &mut Work) -> Result<Option<HashSet<NodeId>>, LangError> {
    let Some(types) = landings(schema, &[start.to_string()], &chain.hops) else {
        return Ok(None);
    };
    let last = chain.hops.len() - 1;
    let Some(ends) = indexed_end(graph, schema, &types[last], &chain.hops[last], work)? else {
        return Ok(None);
    };
    let mut set: HashSet<NodeId> = ends.into_iter().collect();
    for j in (k + 1..=last).rev() {
        let before = &types[j - 1];
        let mut nodes: Vec<NodeId> = back(graph, schema, &set, before, &chain.hops[j], work)?.into_iter().collect();
        nodes.sort_unstable();
        retain_matches(graph, schema, &mut nodes, chain.hops[j - 1].test.as_ref(), work)?;
        set = nodes.into_iter().collect();
    }
    Ok(Some(set))
}

/// Whether some node of `ends`, other than `from`, is at most `most` hops of
/// `field` from `from`: a breadth-first search from both sides, each step
/// expanding the smaller frontier, so two searches of half the depth meet
/// in the middle instead of one searching the whole depth.
fn meet(graph: &Graph, schema: &Schema, from: NodeId, field: &str, most: usize, ends: &HashSet<NodeId>, work: &mut Work) -> Result<bool, LangError> {
    let targets: HashSet<NodeId> = ends.iter().copied().filter(|id| *id != from).collect();
    if targets.is_empty() {
        return Ok(false);
    }
    let types: Vec<String> = graph
        .get_node(from)
        .map(|node| node.labels().map(str::to_string).collect())
        .unwrap_or_default();
    let mut all_types = types.clone();
    for ty in &types {
        if let Ok(edge) = schema.edge(ty, field) {
            for target in edge.as_edge().map(|edge| edge.3).unwrap_or(&[]) {
                if !all_types.contains(target) {
                    all_types.push(target.clone());
                }
            }
        }
    }
    let kinds = kinds_of(schema, &all_types, field);
    let mut ahead: HashSet<NodeId> = HashSet::from([from]);
    let mut behind: HashSet<NodeId> = targets.clone();
    let mut front = vec![from];
    let mut rear: Vec<NodeId> = targets.into_iter().collect();
    let (mut depth_ahead, mut depth_behind) = (0, 0);
    while depth_ahead + depth_behind < most && !front.is_empty() && !rear.is_empty() {
        let forward = front.len() <= rear.len();
        let (layer, mine, theirs, depth) = if forward {
            (&mut front, &mut ahead, &behind, &mut depth_ahead)
        } else {
            (&mut rear, &mut behind, &ahead, &mut depth_behind)
        };
        *depth += 1;
        let mut next = Vec::new();
        for id in layer.drain(..) {
            for (kind, direction) in &kinds {
                let direction = match (forward, direction) {
                    (true, d) => *d,
                    (false, Direction::Out) => Direction::In,
                    (false, Direction::In) => Direction::Out,
                };
                let reached = neighbors(graph, id, kind, direction);
                work.charge(reached.len())?;
                for (node, _) in reached {
                    if theirs.contains(&node) {
                        return Ok(true);
                    }
                    if mine.insert(node) {
                        next.push(node);
                    }
                }
            }
        }
        *layer = next;
    }
    Ok(false)
}

/// Candidates for a `has` chain whose last hop's test an index answers: that
/// hop's nodes through the index, then back along each hop to the start.
/// None when it does not apply; the caller then walks forward from each row.
pub(super) fn candidates(graph: &Graph, schema: &Schema, types: &[&str], chain: &Chain, work: &mut Work) -> Result<Option<HashSet<NodeId>>, LangError> {
    if chain.negated || chain.uses_same() {
        return Ok(None);
    }
    let start: Vec<String> = types.iter().map(|ty| ty.to_string()).collect();
    let Some(landed) = landings(schema, &start, &chain.hops) else {
        return Ok(None);
    };
    let last = chain.hops.len() - 1;
    let Some(ends) = indexed_end(graph, schema, &landed[last], &chain.hops[last], work)? else {
        return Ok(None);
    };
    let mut set: HashSet<NodeId> = ends.into_iter().collect();
    for j in (0..=last).rev() {
        let before = if j == 0 { &start } else { &landed[j - 1] };
        let nodes = back(graph, schema, &set, before, &chain.hops[j], work)?;
        set = if j == 0 {
            nodes
        } else {
            let mut nodes: Vec<NodeId> = nodes.into_iter().collect();
            nodes.sort_unstable();
            retain_matches(graph, schema, &mut nodes, chain.hops[j - 1].test.as_ref(), work)?;
            nodes.into_iter().collect()
        };
    }
    Ok(Some(set))
}

/// Whether an `&&` group needs [`and_group`]: some chain in it says `same`.
pub(super) fn correlated(terms: &[BoolExpr]) -> bool {
    terms.iter().any(|term| matches!(term, BoolExpr::Test(Pred::Chain(chain)) if chain.uses_same()))
}

