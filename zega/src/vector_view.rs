//! Engine-owned PCA and full-vector explanations for an explicit result set.
use crate::{
    lang::{Field, ViewKind},
    vector::{pca, Vector, VectorSpec},
    Value, Zega, ZegaError,
};
use serde_json::{json, Value as Json};
use std::collections::{BTreeSet, HashMap, HashSet};

fn result_ids(value: &Json, ids: &mut BTreeSet<u64>) {
    match value {
        Json::Array(rows) => {
            for row in rows {
                result_ids(row, ids);
            }
        }
        Json::Object(row) => {
            if let Some(id) = row.get("id").and_then(Json::as_u64) {
                ids.insert(id);
            }
            for value in row.values() {
                if value.is_object() || value.is_array() {
                    result_ids(value, ids);
                }
            }
        }
        _ => {}
    }
}
impl Zega {
    /// Project precisely the IDs selected by a query. Hosts attach this metadata
    /// to its result; they never compute PCA or scores. The first Vector field
    /// in schema order is used. Incompatible dimensions/metrics get separate groups.
    pub fn vector_view(
        &self,
        schema_src: &str,
        result: &Json,
        kind: ViewKind,
        selected: Option<u64>,
        k: usize,
        threshold: f64,
    ) -> Result<Json, ZegaError> {
        let schema = self.schema(schema_src)?;
        let view = schema
            .display
            .views
            .iter()
            .find(|v| v.kind == kind && matches!(kind, ViewKind::Vector2d | ViewKind::Vector3d))
            .ok_or_else(|| {
                ZegaError::Execution("vector view is not declared in display {}".into())
            })?;
        if !threshold.is_finite() {
            return Err(ZegaError::Execution(
                "similarity threshold must be finite".into(),
            ));
        }
        let mut ids = BTreeSet::new();
        result_ids(result, &mut ids);
        let graph = self
            .graph
            .lock()
            .map_err(|_| ZegaError::Execution("lock poisoned".into()))?;
        let mut nodes: Vec<(u64, String, Vector)> = Vec::new();
        for id in ids {
            let Some(node) = graph.get_node(id) else {
                continue;
            };
            let Some(ty) = schema.types.iter().find(|ty| {
                node.has_label(&ty.name)
                    && view.types.as_ref().is_none_or(|ts| ts.contains(&ty.name))
            }) else {
                continue;
            };
            let field = ty.fields.iter().find_map(|f| match f {
                Field::Prop { name, ty, .. } if VectorSpec::parse(ty).is_some() => Some(name),
                _ => None,
            });
            if let Some(field) = field {
                if let Some(Value::Vector(v)) = node.prop(field) {
                    nodes.push((id, field.clone(), (**v).clone()));
                }
            }
        }
        let mut positions: HashMap<u64, [f64; 3]> = HashMap::new();
        let mut groups: Vec<Vec<usize>> = Vec::new();
        for (i, (_, _, v)) in nodes.iter().enumerate() {
            if let Some(group) = groups.iter_mut().find(|g| {
                nodes[g[0]].2.dimensions() == v.dimensions() && nodes[g[0]].2.metric == v.metric
            }) {
                group.push(i);
            } else {
                groups.push(vec![i]);
            }
        }
        for group in &groups {
            let vectors: Vec<_> = group.iter().map(|&i| nodes[i].2.clone()).collect();
            for (&i, p) in group.iter().zip(pca(&vectors)) {
                positions.insert(nodes[i].0, p);
            }
        }
        let points: Vec<_> = nodes.iter().map(|(id,field,v)| json!({"id":id,"field":field,"metric":v.metric,"dimensions":v.dimensions(),"position":positions[id],"group":groups.iter().position(|g| nodes[g[0]].2.dimensions()==v.dimensions() && nodes[g[0]].2.metric==v.metric).unwrap()})).collect();
        let plotted: HashSet<_> = nodes.iter().map(|n| n.0).collect();
        let mut linked = HashSet::new();
        let mut links = Vec::new();
        for &(id, _, _) in &nodes {
            for rid in graph.node_relationship_ids(id) {
                let rel = graph.get_relationship(rid).unwrap();
                if rel.from == id && plotted.contains(&rel.to) {
                    links.push(json!({"from":rel.from,"to":rel.to,"type":rel.kind}));
                    linked.insert((rel.from.min(rel.to), rel.from.max(rel.to)));
                }
            }
        }
        links.sort_by_key(|r| {
            (
                r["from"].as_u64(),
                r["to"].as_u64(),
                r["type"].as_str().unwrap_or("").to_owned(),
            )
        });
        let mut flags = Vec::new();
        for (i, (from, _, a)) in nodes.iter().enumerate() {
            for (to, _, b) in &nodes[i + 1..] {
                if a.dimensions() != b.dimensions() || a.metric != b.metric {
                    continue;
                }
                let score = a.score(b).unwrap();
                let is_linked = linked.contains(&(*from, *to));
                let kind = if is_linked && score < threshold {
                    "linked-but-far"
                } else if !is_linked && score >= threshold {
                    "near-but-unlinked"
                } else {
                    continue;
                };
                flags.push(json!({"from":from,"to":to,"score":score,"kind":kind}));
            }
        }
        let mut nearest = Vec::new();
        if let Some((id, _, v)) = selected.and_then(|id| nodes.iter().find(|n| n.0 == id)) {
            nearest = nodes
                .iter()
                .filter(|n| n.0 != *id && n.2.metric == v.metric)
                .filter_map(|n| v.score(&n.2).map(|score| (n.0, score)))
                .collect();
            nearest.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            nearest.truncate(k);
        }
        Ok(
            json!({"results":result,"points":points,"links":links,"flags":flags,"nearest":nearest.into_iter().map(|(id,score)| json!({"id":id,"score":score})).collect::<Vec<_>>(),"threshold":threshold,"projection":"PCA","approximate_positions":true}),
        )
    }
}
