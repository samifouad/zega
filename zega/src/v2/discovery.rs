//! Discovery reads a fixed set of identities, never the whole graph as scope.
use super::*;
use crate::lang::{DiscoveryExpr, Primitive, Query, TextOp};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
struct Matches {
    nodes: BTreeSet<NodeId>,
    edges: Vec<Json>,
}

impl Matches {
    fn normalize(&mut self) {
        self.edges.retain(|edge| {
            self.nodes.contains(&edge["from"].as_u64().unwrap())
                && self.nodes.contains(&edge["to"].as_u64().unwrap())
        });
        self.edges.sort_by_cached_key(|edge| {
            (
                edge["from"].as_u64().unwrap(),
                edge["to"].as_u64().unwrap(),
                edge["via"].to_string(),
            )
        });
        self.edges.dedup();
    }
    fn pair(&mut self, a: NodeId, b: NodeId, via: Json) {
        self.nodes.extend([a, b]);
        self.edges
            .push(json!({"from": a.min(b), "to": a.max(b), "via": via}));
    }
}

pub(super) fn pipeline(
    graph: &Graph,
    schema: &Schema,
    query: &Query,
    budget: &mut usize,
) -> Result<Json, LangError> {
    let mut context = ReadContext {
        budget,
        trace: Some(ReadTrace::default()),
    };
    if let Some(root) = &query.root {
        read(graph, schema, root, &mut context)?;
    }
    let trace = context.trace.take().unwrap();
    let mut selected = Matches {
        nodes: trace.nodes,
        edges: Vec::new(),
    };
    // `display { skip }` without a then still suppresses the single result.
    if query.then.is_empty() {
        return Ok(Json::Null);
    }
    let mut stages = Vec::new();
    if !query.skip {
        stages.push(stage(graph, 0, "query", &selected, &trace.rels));
    }
    for (i, then) in query.then.iter().enumerate() {
        selected = evaluate(
            graph,
            schema,
            &selected.nodes,
            &then.condition,
            context.budget,
        )?;
        selected.normalize();
        if !then.skip {
            stages.push(stage(graph, i + 1, "then", &selected, &trace.rels));
        }
    }
    Ok(json!({"stages": stages}))
}

fn stage(
    graph: &Graph,
    index: usize,
    kind: &str,
    selected: &Matches,
    rels: &BTreeSet<RelId>,
) -> Json {
    let nodes: Vec<_> = selected
        .nodes
        .iter()
        .filter_map(|id| graph.get_node(*id))
        .map(|node| {
            let props: serde_json::Map<String, Json> = node
                .props
                .iter()
                .map(|(key, value)| (key.clone(), value_to_json(value)))
                .collect();
            json!({"id": node.id, "labels": node.labels, "props": props})
        })
        .collect();
    let edges: Vec<_> = rels
        .iter()
        .filter_map(|id| graph.get_relationship(*id))
        .filter(|rel| selected.nodes.contains(&rel.from) && selected.nodes.contains(&rel.to))
        .map(rel_json)
        .collect();
    let mut result = json!({"index": index, "kind": kind, "nodes": nodes, "edges": edges});
    if kind == "then" {
        result["couldBeEdges"] = json!(selected.edges);
    }
    result
}

fn evaluate(
    graph: &Graph,
    schema: &Schema,
    input: &BTreeSet<NodeId>,
    expr: &DiscoveryExpr,
    budget: &mut usize,
) -> Result<Matches, LangError> {
    match expr {
        DiscoveryExpr::And(a, b) | DiscoveryExpr::Or(a, b) => {
            let mut left = evaluate(graph, schema, input, a, budget)?;
            let right = evaluate(graph, schema, input, b, budget)?;
            if matches!(expr, DiscoveryExpr::And(..)) {
                left.nodes.retain(|id| right.nodes.contains(id));
            } else {
                left.nodes.extend(right.nodes);
            }
            left.edges.extend(right.edges);
            left.normalize();
            Ok(left)
        }
        DiscoveryExpr::Test(primitive) => {
            primitive_matches(graph, schema, input, primitive, budget)
        }
    }
}

fn primitive_matches(
    graph: &Graph,
    schema: &Schema,
    input: &BTreeSet<NodeId>,
    primitive: &Primitive,
    budget: &mut usize,
) -> Result<Matches, LangError> {
    let mut out = Matches::default();
    match primitive {
        Primitive::Text {
            op,
            text,
            fields,
            pattern,
            ..
        } => {
            let mut matched = BTreeSet::new();
            for ty in &schema.types {
                for field in &ty.fields {
                    let crate::lang::Field::Prop {
                        name, ty: field_ty, ..
                    } = field
                    else {
                        continue;
                    };
                    if field_ty != "String"
                        || (!fields.is_empty() && !fields.iter().any(|(f, _)| f == name))
                    {
                        continue;
                    }
                    let pattern_kind = match op {
                        TextOp::FindWith | TextOp::FindWithout => Some(TextPattern::Contains(text)),
                        TextOp::StartsWith => Some(TextPattern::StartsWith(text)),
                        TextOp::EndsWith => Some(TextPattern::EndsWith(text)),
                        TextOp::Regex => None,
                    };
                    let indexed =
                        pattern_kind.and_then(|p| graph.text_candidates(&[&ty.name], name, p));
                    let candidates: Vec<_> = match indexed {
                        Some(ids) => ids.into_iter().filter(|id| input.contains(id)).collect(),
                        None => input.iter().copied().collect(),
                    };
                    for id in candidates {
                        let Some(node) = graph.get_node(id).filter(|n| n.labels.contains(&ty.name))
                        else {
                            continue;
                        };
                        let Some(Value::String(value)) = node.props.get(name) else {
                            continue;
                        };
                        charge(budget, 1)?;
                        graph.note_examined(1);
                        let yes = match op {
                            TextOp::FindWith | TextOp::FindWithout => value.contains(text),
                            TextOp::StartsWith => value.starts_with(text),
                            TextOp::EndsWith => value.ends_with(text),
                            TextOp::Regex => {
                                pattern.as_ref().expect("compiled regex").0.is_match(value)
                            }
                        };
                        if yes {
                            matched.insert(id);
                        }
                    }
                }
            }
            out.nodes = if *op == TextOp::FindWithout {
                input.difference(&matched).copied().collect()
            } else {
                matched
            };
        }
        Primitive::Common { types, .. } => {
            // Tuple position defines equality; field names may differ by type.
            // Null/missing is not a shared value. Each value emits N-1 edges.
            let mut groups: BTreeMap<String, (Json, BTreeSet<NodeId>)> = BTreeMap::new();
            for id in input {
                let Some(node) = graph.get_node(*id) else {
                    continue;
                };
                for (ty, fields, _) in types {
                    if !node.labels.contains(ty) {
                        continue;
                    }
                    charge(budget, fields.len())?;
                    let values: Option<Vec<Json>> = fields
                        .iter()
                        .map(|(field, _)| {
                            node.props
                                .get(field)
                                .filter(|v| !matches!(v, Value::Null))
                                .map(canonical_value)
                        })
                        .collect();
                    let Some(values) = values else { continue };
                    let value = if values.len() == 1 {
                        values[0].clone()
                    } else {
                        json!(values)
                    };
                    groups
                        .entry(value.to_string())
                        .or_insert_with(|| (value, BTreeSet::new()))
                        .1
                        .insert(*id);
                }
            }
            let mut fields: Vec<_> = types
                .iter()
                .flat_map(|(ty, fields, _)| fields.iter().map(move |(f, _)| format!("{ty}.{f}")))
                .collect();
            fields.sort();
            for (value, members) in groups.into_values() {
                let mut ids = members.into_iter();
                let Some(first) = ids.next() else { continue };
                for id in ids {
                    charge(budget, 1)?;
                    out.pair(
                        first,
                        id,
                        json!({"primitive": "common", "fields": fields, "value": value}),
                    );
                }
            }
        }
        Primitive::Similar {
            field,
            threshold,
            inclusive,
            ..
        } => {
            for id in input {
                let Some(Value::Vector(vector)) =
                    graph.get_node(*id).and_then(|n| n.props.get(field))
                else {
                    continue;
                };
                // A threshold is not top-k: ask for the complete allowed set.
                // HNSW traverses its index and falls back to its exact scan;
                // omitted entries are scored directly (also covers no index).
                charge(budget, input.len())?;
                let mut scores: BTreeMap<_, _> = graph
                    .vector_nearest(field, vector, input.len(), false, |other| {
                        input.contains(&other) && other > *id
                    })
                    .into_iter()
                    .collect();
                for other in
                    input.range((std::ops::Bound::Excluded(*id), std::ops::Bound::Unbounded))
                {
                    if scores.contains_key(other) {
                        continue;
                    }
                    if let Some(Value::Vector(v)) =
                        graph.get_node(*other).and_then(|n| n.props.get(field))
                    {
                        if v.metric == vector.metric {
                            if let Some(score) = vector.score(v) {
                                scores.entry(*other).or_insert(score);
                            }
                        }
                    }
                }
                for (other, score) in scores {
                    if score > *threshold || (*inclusive && score == *threshold) {
                        out.pair(
                            *id,
                            other,
                            json!({"primitive": "similar", "fields": [field], "score": score}),
                        );
                    }
                }
            }
        }
        Primitive::Near {
            field,
            metres,
            inclusive,
            ..
        } => {
            for id in input {
                let Some(point) = graph.get_node(*id).and_then(|n| point_prop(n, field)) else {
                    continue;
                };
                // Bounding boxes are conservative; exact portable haversine
                // supplies both selection and the byte-identical host result.
                for other in graph.spatial_candidates(field, Bounds::radius(point, *metres)) {
                    if other <= *id || !input.contains(&other) {
                        continue;
                    }
                    charge(budget, 1)?;
                    let Some(target) = graph.get_node(other).and_then(|n| point_prop(n, field))
                    else {
                        continue;
                    };
                    let distance = point.portable_distance(target);
                    if distance < *metres || (*inclusive && distance == *metres) {
                        out.pair(
                            *id,
                            other,
                            json!({"primitive": "near", "fields": [field], "distance": distance}),
                        );
                    }
                }
            }
        }
    }
    Ok(out)
}

fn canonical_value(value: &Value) -> Json {
    match value {
        Value::Float(bits) if f64::from_bits(*bits) == 0.0 => json!(0.0),
        _ => value_to_json(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalization_filters_sorts_and_deduplicates_edges() {
        let mut result = Matches::default();
        result.pair(9, 2, json!({"primitive":"common"}));
        result.pair(2, 9, json!({"primitive":"common"}));
        result.pair(9, 3, json!({"primitive":"near"}));
        result.nodes.remove(&3);
        result.normalize();
        assert_eq!(
            result.edges,
            vec![json!({"from":2,"to":9,"via":{"primitive":"common"}})]
        );
    }
}
