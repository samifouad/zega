use super::generator::{GeneratedGraph, GeneratedQuery};

pub fn graph_candidates(graph: &GeneratedGraph) -> Vec<GeneratedGraph> {
    let mut candidates = Vec::new();
    if !graph.relationships.is_empty() {
        let mut candidate = graph.clone();
        candidate
            .relationships
            .truncate(candidate.relationships.len() / 2);
        candidates.push(candidate);
    }
    if graph.nodes.len() > 1 {
        let keep = graph.nodes.len() / 2;
        let mut candidate = graph.clone();
        candidate.nodes.truncate(keep);
        candidate
            .relationships
            .retain(|rel| rel.from < keep && rel.to < keep);
        candidates.push(candidate);
    }
    for removed in &graph.nodes {
        let mut candidate = graph.clone();
        candidate.nodes.retain(|node| node.id != removed.id);
        candidate
            .relationships
            .retain(|rel| rel.from != removed.id && rel.to != removed.id);
        candidates.push(candidate);
    }
    for index in 0..graph.relationships.len() {
        let mut candidate = graph.clone();
        candidate.relationships.remove(index);
        candidates.push(candidate);
    }
    candidates
}

pub fn query_candidates(query: &GeneratedQuery) -> Vec<GeneratedQuery> {
    let mut candidates = Vec::new();
    if query.limit.is_some() {
        let mut candidate = query.clone();
        candidate.limit = None;
        candidates.push(candidate);
    }
    if !query.order.is_empty() {
        let mut candidate = query.clone();
        candidate.order.clear();
        candidate.limit = None;
        candidates.push(candidate);
    }
    if query.order.len() > 1 {
        let mut candidate = query.clone();
        candidate.order.truncate(1);
        candidates.push(candidate);
    }
    if query.predicates.len() > 1 {
        let mut candidate = query.clone();
        candidate.predicates.truncate(1);
        candidates.push(candidate);
    }
    if !query.predicates.is_empty() {
        let mut candidate = query.clone();
        candidate.predicates.clear();
        candidates.push(candidate);
    }
    if query.columns.len() > 1 {
        let mut candidate = query.clone();
        candidate.columns.truncate(1);
        candidate.order.clear();
        candidate.limit = None;
        candidates.push(candidate);
    }
    if query.label.is_some() {
        let mut candidate = query.clone();
        candidate.label = None;
        candidates.push(candidate);
    }
    candidates
}

pub fn shrink_with(
    mut graph: GeneratedGraph,
    mut query: GeneratedQuery,
    mismatch: impl Fn(&GeneratedGraph, &GeneratedQuery) -> bool,
) -> (GeneratedGraph, GeneratedQuery) {
    loop {
        let mut changed = false;
        for candidate in graph_candidates(&graph) {
            if mismatch(&candidate, &query) {
                graph = candidate;
                changed = true;
                break;
            }
        }
        for candidate in query_candidates(&query) {
            if mismatch(&graph, &candidate) {
                query = candidate;
                changed = true;
                break;
            }
        }
        if !changed {
            return (graph, query);
        }
    }
}
