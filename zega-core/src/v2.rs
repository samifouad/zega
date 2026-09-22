//! Execute the v2 schema language against the graph the engine already stores.
//!
//! A read walks label indexes and adjacency lists and shapes one JSON value.
//! A mutation inserts rows, or looks a row up when the statement says `link`
//! or `set`. Writes go through the same node, relationship, and WAL operations
//! as the existing executor.

use std::collections::{HashMap, HashSet, VecDeque};

use serde_json::{json, Value as Json};
use zega_graph::{Graph, Node, NodeId, RelId};
use zega_lang::{Cmp, Direction, Error as LangError, Item, Pred, Schema, Selection};
use zega_parser::Value;
use zega_wal::Operation;

use crate::{Zega, ZegaError};

impl Zega {
    pub fn run_lang(&self, schema_src: &str, source: &str) -> Result<Json, ZegaError> {
        let schema = zega_lang::parse_schema(schema_src).map_err(lang)?;
        let query = zega_lang::parse_query(source).map_err(lang)?;
        zega_lang::check(&schema, &query.root, query.mutation).map_err(lang)?;
        let mut graph = self
            .graph
            .lock()
            .map_err(|_| ZegaError::Execution("lock poisoned".to_string()))?;
        let mut budget = self.traversal_work_budget;
        if query.mutation {
            mutate(&mut graph, &self.wal, &schema, &query.root).map_err(lang)
        } else {
            Ok(read(&graph, &schema, &query.root, &mut budget).map_err(lang)?)
        }
    }

    pub fn graph_json(&self) -> Result<Json, ZegaError> {
        let graph = self
            .graph
            .lock()
            .map_err(|_| ZegaError::Execution("lock poisoned".to_string()))?;
        let mut nodes: Vec<Json> = graph.all_nodes().values().map(node_json).collect();
        nodes.sort_by_key(|node| node["id"].as_u64().unwrap_or(0));
        let mut rels: Vec<Json> = graph
            .all_relationships()
            .values()
            .map(|rel| {
                json!({
                    "id": rel.id,
                    "type": rel.kind,
                    "from": rel.from,
                    "to": rel.to,
                })
            })
            .collect();
        rels.sort_by_key(|rel| rel["id"].as_u64().unwrap_or(0));
        Ok(json!({ "nodes": nodes, "rels": rels }))
    }
}

fn lang(error: LangError) -> ZegaError {
    let mut text = error.message;
    if error.line > 0 {
        text.push_str(&format!(
            "\nat query:{}:{}:{}:{}",
            error.line, error.column, error.end_line, error.end_column
        ));
    }
    if let Some(help) = error.help {
        text.push_str("\nhelp: ");
        text.push_str(&help);
    }
    ZegaError::Execution(text)
}

fn read(
    graph: &Graph,
    schema: &Schema,
    root: &Selection,
    budget: &mut usize,
) -> Result<Json, LangError> {
    let mut ids = candidates(graph, root);
    ids.retain(|id| node_matches(graph, *id, &root.predicates));
    ids.sort_unstable();
    if equality_lookup(root) {
        return match ids.len() {
            0 => Ok(Json::Null),
            1 => project(graph, schema, root, ids[0], 0, None, budget),
            n => Err(LangError::at(
                root.type_span,
                format!("{} matched {n} rows", root.type_name),
            )
            .with_help("an equality filter has to match one row")),
        };
    }
    let mut rows = Vec::new();
    for id in ids {
        rows.push(project(graph, schema, root, id, 0, None, budget)?);
    }
    Ok(Json::Array(rows))
}

fn mutate(
    graph: &mut Graph,
    wal: &crate::Wal,
    schema: &Schema,
    root: &Selection,
) -> Result<Json, LangError> {
    apply_node(graph, wal, schema, root, None)
}

/// Writes or finds this selection and returns only the rows this statement touched.
fn apply_node(
    graph: &mut Graph,
    wal: &crate::Wal,
    schema: &Schema,
    sel: &Selection,
    parent: Option<(NodeId, Direction, String)>,
) -> Result<Json, LangError> {
    let lookup = !sel.sets.is_empty() || has_link(sel);
    let id = if lookup {
        lookup_one(graph, sel)?
    } else {
        insert_node(graph, wal, sel)?
    };
    if !sel.sets.is_empty() {
        let props = sel
            .sets
            .iter()
            .map(|(key, value, _)| Ok((key.clone(), json_to_value(value)?)))
            .collect::<Result<HashMap<_, _>, LangError>>()?;
        graph.update_node(id, props.clone());
        wal.append(&Operation::UpdateNode { id, props })
            .map_err(|error| LangError::bare(error.to_string()))?;
    }
    if let Some((parent_id, direction, rel)) = &parent {
        let props = edge_sets(sel)?;
        connect(graph, wal, *parent_id, id, *direction, rel, props)?;
    }
    let node = graph
        .get_node(id)
        .ok_or_else(|| LangError::bare(format!("missing node {id}")))?
        .clone();
    let mut object = serde_json::Map::new();
    let mut lists: HashMap<String, Vec<Json>> = HashMap::new();
    for item in &sel.items {
        match item {
            Item::Prop(name, _) => {
                object.insert(name.clone(), prop_json(&node, name));
            }
            Item::Hops => {
                object.insert("hops".into(), json!(0));
            }
            Item::EdgeProp(name, _) | Item::EdgeSet(name, _, _) => {
                let value = sel
                    .items
                    .iter()
                    .find_map(|item| match item {
                        Item::EdgeSet(field, value, _) if field == name => Some(value.clone()),
                        _ => None,
                    })
                    .unwrap_or(Json::Null);
                object.insert(name.clone(), value);
            }
            Item::Walk {
                field,
                link,
                direction,
                target,
                range,
                ..
            } => {
                if range.is_some() {
                    return Err(LangError::bare("a mutation cannot use a hop range"));
                }
                let edge = schema.edge(&sel.type_name, field)?;
                let (_, rel, schema_dir, targets, many) = edge.as_edge().unwrap();
                if *direction != schema_dir || !targets.contains(&target.type_name) {
                    return Err(LangError::bare(format!(
                        "{}.{} does not reach {}",
                        sel.type_name, field, target.type_name
                    )));
                }
                let child = if *link {
                    let child_id = lookup_one(graph, target)?;
                    connect(
                        graph,
                        wal,
                        id,
                        child_id,
                        *direction,
                        rel,
                        edge_sets(target)?,
                    )?;
                    let saved = graph.get_node(child_id).unwrap().clone();
                    let mut child_object = serde_json::Map::new();
                    for child_item in &target.items {
                        match child_item {
                            Item::Prop(name, _) => {
                                child_object.insert(name.clone(), prop_json(&saved, name));
                            }
                            Item::EdgeSet(name, value, _) => {
                                child_object.insert(name.clone(), value.clone());
                            }
                            Item::EdgeProp(name, _) => {
                                child_object.insert(name.clone(), Json::Null);
                            }
                            _ => {}
                        }
                    }
                    Json::Object(child_object)
                } else {
                    apply_node(
                        graph,
                        wal,
                        schema,
                        target,
                        Some((id, *direction, rel.to_string())),
                    )?
                };
                let key = if many {
                    format!("{}s", target.type_name)
                } else {
                    target.type_name.clone()
                };
                if many {
                    lists.entry(key).or_default().push(child);
                } else {
                    object.insert(key, child);
                }
            }
        }
    }
    for (key, rows) in lists {
        object.insert(key, Json::Array(rows));
    }
    Ok(Json::Object(object))
}

fn lookup_one(graph: &Graph, sel: &Selection) -> Result<NodeId, LangError> {
    let mut ids = candidates(graph, sel);
    ids.retain(|id| node_matches(graph, *id, &sel.predicates));
    match ids.len() {
        1 => Ok(ids[0]),
        0 => Err(
            LangError::at(sel.type_span, format!("no {} matched", sel.type_name))
                .with_help("`link` and `set` need exactly one matching row"),
        ),
        n => Err(
            LangError::at(sel.type_span, format!("{} matched {n} rows", sel.type_name))
                .with_help("`link` and `set` need exactly one matching row"),
        ),
    }
}

fn has_link(sel: &Selection) -> bool {
    sel.items
        .iter()
        .any(|item| matches!(item, Item::Walk { link: true, .. }))
}

fn insert_node(graph: &mut Graph, wal: &crate::Wal, sel: &Selection) -> Result<NodeId, LangError> {
    let mut props = HashMap::new();
    for pred in &sel.predicates {
        match pred {
            Pred::Eq(field, value, _) if field != "id" => {
                props.insert(field.clone(), json_to_value(value)?);
            }
            Pred::Eq(_, _, _) => {}
            other => {
                return Err(LangError::at(
                    other.span(),
                    format!("creating a {} only accepts field: value", sel.type_name),
                )
                .with_help("write `name: \"value\"`"))
            }
        }
    }
    let labels: Vec<String> = std::iter::once(sel.type_name.clone())
        .chain(sel.also.iter().cloned())
        .collect();
    let id = graph.create_node(labels.clone(), props.clone());
    wal.append(&Operation::InsertNode { id, labels, props })
        .map_err(|error| LangError::bare(error.to_string()))?;
    Ok(id)
}

fn edge_sets(sel: &Selection) -> Result<HashMap<String, Value>, LangError> {
    let mut props = HashMap::new();
    for item in &sel.items {
        if let Item::EdgeSet(name, value, _) = item {
            props.insert(name.clone(), json_to_value(value)?);
        }
    }
    Ok(props)
}

fn connect(
    graph: &mut Graph,
    wal: &crate::Wal,
    parent: NodeId,
    child: NodeId,
    direction: Direction,
    rel: &str,
    props: HashMap<String, Value>,
) -> Result<RelId, LangError> {
    let (from, to) = match direction {
        Direction::Out => (parent, child),
        Direction::In => (child, parent),
    };
    let id = graph.create_relationship(rel.to_string(), from, to, props.clone());
    wal.append(&Operation::InsertRel {
        id,
        kind: rel.to_string(),
        from,
        to,
        props,
    })
    .map_err(|error| LangError::bare(error.to_string()))?;
    Ok(id)
}

fn project(
    graph: &Graph,
    schema: &Schema,
    sel: &Selection,
    id: NodeId,
    hops: usize,
    arrived: Option<RelId>,
    budget: &mut usize,
) -> Result<Json, LangError> {
    let node = graph
        .get_node(id)
        .ok_or_else(|| LangError::bare(format!("missing node {id}")))?;
    let mut object = serde_json::Map::new();
    let mut landing_types = Vec::new();
    for item in &sel.items {
        match item {
            Item::Prop(name, _) => {
                ensure_prop(schema, sel, name)?;
                object.insert(name.clone(), prop_json(node, name));
            }
            Item::Hops => {
                object.insert("hops".into(), json!(hops));
            }
            Item::EdgeSet(name, _, _) => {
                return Err(LangError::bare(format!(
                    "&{name}: value is stored by a mutation"
                )));
            }
            Item::EdgeProp(name, _) => {
                let rel_id = arrived.ok_or_else(|| {
                    LangError::bare(format!("&{name} needs the relationship that arrived here"))
                })?;
                let value = graph
                    .get_relationship(rel_id)
                    .and_then(|rel| rel.props.get(name))
                    .map(value_to_json)
                    .unwrap_or(Json::Null);
                object.insert(name.clone(), value);
            }
            Item::Walk {
                field,
                range,
                direction,
                target,
                ..
            } => {
                let edge = schema.edge(node_type(node, sel)?, field)?;
                let (_, rel, schema_dir, targets, many) = edge.as_edge().unwrap();
                if *direction != schema_dir {
                    return Err(LangError::bare(format!(
                        "{}.{} does not point that way",
                        node_type(node, sel)?,
                        field
                    )));
                }
                let wanted = std::iter::once(target.type_name.as_str())
                    .chain(target.also.iter().map(String::as_str));
                if wanted
                    .clone()
                    .any(|name| !targets.contains(&name.to_string()))
                {
                    return Err(LangError::bare(format!(
                        "{field} does not reach {}",
                        target.type_name
                    )));
                }
                let key_type = if target.also.is_empty() {
                    target.type_name.clone()
                } else {
                    field.clone()
                };
                if target.also.is_empty() {
                    if landing_types.contains(&key_type) {
                        return Err(LangError::bare(format!(
                            "two relationships in one brace land on {key_type}"
                        )));
                    }
                    landing_types.push(key_type.clone());
                }
                let reached = if let Some((min, max)) = range {
                    walk_range(graph, id, rel, *direction, targets, (*min, *max), budget)?
                } else {
                    charge(budget, 1)?;
                    neighbors(graph, id, rel, *direction)
                        .into_iter()
                        .filter(|(next, _)| node_has_any_label(graph, *next, targets))
                        .map(|(next, rel_id)| (next, 1usize, rel_id))
                        .collect()
                };
                let mut rows = Vec::new();
                for (next, depth, rel_id) in reached {
                    if !node_matches(graph, next, &target.predicates) {
                        continue;
                    }
                    rows.push(project(
                        graph,
                        schema,
                        target,
                        next,
                        hops + depth,
                        Some(rel_id),
                        budget,
                    )?);
                }
                let list = many || range.is_some() || !target.also.is_empty();
                let key = if !target.also.is_empty() {
                    field.clone()
                } else if list {
                    format!("{key_type}s")
                } else {
                    key_type
                };
                if list {
                    object.insert(key, Json::Array(rows));
                } else {
                    object.insert(key, rows.into_iter().next().unwrap_or(Json::Null));
                }
            }
        }
    }
    Ok(Json::Object(object))
}

fn node_type<'a>(node: &'a Node, sel: &'a Selection) -> Result<&'a str, LangError> {
    if node.labels.iter().any(|label| label == &sel.type_name) {
        return Ok(sel.type_name.as_str());
    }
    for extra in &sel.also {
        if node.labels.iter().any(|label| label == extra) {
            return Ok(extra.as_str());
        }
    }
    Err(LangError::bare(format!(
        "node {} is not a {}",
        node.id, sel.type_name
    )))
}

fn ensure_prop(schema: &Schema, sel: &Selection, name: &str) -> Result<(), LangError> {
    if name == "id" {
        return Ok(());
    }
    let types = std::iter::once(sel.type_name.as_str()).chain(sel.also.iter().map(String::as_str));
    if types.clone().any(|ty| schema.prop(ty, name).is_ok()) {
        return Ok(());
    }
    Err(LangError::bare(format!(
        "{} has no field {name}",
        sel.type_name
    )))
}

fn walk_range(
    graph: &Graph,
    start: NodeId,
    rel: &str,
    direction: Direction,
    targets: &[String],
    range: (usize, usize),
    budget: &mut usize,
) -> Result<Vec<(NodeId, usize, RelId)>, LangError> {
    let (min, max) = range;
    let mut seen = HashSet::from([start]);
    let mut queue = VecDeque::from([(start, 0usize, 0u64)]);
    let mut found = Vec::new();
    while let Some((node, depth, via)) = queue.pop_front() {
        if depth >= min && depth > 0 && node_has_any_label(graph, node, targets) {
            found.push((node, depth, via));
        }
        if depth == max {
            continue;
        }
        for (next, rel_id) in neighbors(graph, node, rel, direction) {
            charge(budget, 1)?;
            if seen.insert(next) {
                queue.push_back((next, depth + 1, rel_id));
            }
        }
    }
    found.sort_by_key(|(id, depth, _)| (*depth, *id));
    Ok(found)
}

fn neighbors(
    graph: &Graph,
    id: NodeId,
    rel_kind: &str,
    direction: Direction,
) -> Vec<(NodeId, RelId)> {
    let ids = match direction {
        Direction::Out => graph.outgoing_rels(id),
        Direction::In => graph.incoming_rels(id),
    };
    let mut out = Vec::new();
    let Some(ids) = ids else {
        return out;
    };
    for rel_id in ids {
        let Some(rel) = graph.get_relationship(*rel_id) else {
            continue;
        };
        if rel.kind != rel_kind {
            continue;
        }
        let next = match direction {
            Direction::Out if rel.from == id => rel.to,
            Direction::In if rel.to == id => rel.from,
            _ => continue,
        };
        out.push((next, *rel_id));
    }
    out.sort_unstable();
    out
}

fn candidates(graph: &Graph, sel: &Selection) -> Vec<NodeId> {
    let mut labels = vec![sel.type_name.as_str()];
    labels.extend(sel.also.iter().map(String::as_str));
    let mut ids = Vec::new();
    for label in labels {
        if let Some(set) = graph.nodes_by_label(label) {
            ids.extend(set.iter().copied());
        }
    }
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn node_has_any_label(graph: &Graph, id: NodeId, labels: &[String]) -> bool {
    graph.get_node(id).is_some_and(|node| {
        node.labels
            .iter()
            .any(|label| labels.iter().any(|wanted| wanted == label))
    })
}

fn node_matches(graph: &Graph, id: NodeId, preds: &[Pred]) -> bool {
    preds.iter().all(|pred| pred_matches(graph, id, pred))
}

fn pred_matches(graph: &Graph, id: NodeId, pred: &Pred) -> bool {
    let Some(node) = graph.get_node(id) else {
        return false;
    };
    match pred {
        Pred::Eq(field, value, _) => prop_json(node, field) == *value,
        Pred::Ne(field, value, _) => prop_json(node, field) != *value,
        Pred::Cmp(field, op, value, _) => cmp_json(&prop_json(node, field), *op, value),
        Pred::Contains(field, needle, _) => prop_json(node, field)
            .as_str()
            .is_some_and(|text| text.contains(needle)),
        Pred::StartsWith(field, needle, _) => prop_json(node, field)
            .as_str()
            .is_some_and(|text| text.starts_with(needle)),
        Pred::EndsWith(field, needle, _) => prop_json(node, field)
            .as_str()
            .is_some_and(|text| text.ends_with(needle)),
    }
}

fn cmp_json(left: &Json, op: Cmp, right: &Json) -> bool {
    let Some(order) = cmp_value(left, right) else {
        return false;
    };
    match op {
        Cmp::Gt => order.is_gt(),
        Cmp::Lt => order.is_lt(),
        Cmp::Gte => order.is_ge(),
        Cmp::Lte => order.is_le(),
    }
}

fn cmp_value(left: &Json, right: &Json) -> Option<std::cmp::Ordering> {
    if let (Some(a), Some(b)) = (left.as_i64(), right.as_i64()) {
        return Some(a.cmp(&b));
    }
    if let (Some(a), Some(b)) = (left.as_f64(), right.as_f64()) {
        return a.partial_cmp(&b);
    }
    if let (Some(a), Some(b)) = (left.as_str(), right.as_str()) {
        return Some(a.cmp(b));
    }
    None
}

fn equality_lookup(sel: &Selection) -> bool {
    !sel.predicates.is_empty()
        && sel
            .predicates
            .iter()
            .all(|pred| matches!(pred, Pred::Eq(_, _, _)))
}

fn prop_json(node: &Node, name: &str) -> Json {
    if name == "id" {
        return json!(node.id);
    }
    node.props
        .get(name)
        .map(value_to_json)
        .unwrap_or(Json::Null)
}

fn node_json(node: &Node) -> Json {
    let mut object = serde_json::Map::new();
    object.insert("id".into(), json!(node.id));
    object.insert(
        "labels".into(),
        Json::Array(node.labels.iter().cloned().map(Json::String).collect()),
    );
    for (key, value) in &node.props {
        object.insert(key.clone(), value_to_json(value));
    }
    Json::Object(object)
}

fn value_to_json(value: &Value) -> Json {
    match value {
        Value::String(value) => Json::String(value.clone()),
        Value::Int(value) => json!(value),
        Value::Float(bits) => json!(f64::from_bits(*bits)),
        Value::Bool(value) => Json::Bool(*value),
        Value::Null => Json::Null,
        Value::List(values) => Json::Array(values.iter().map(value_to_json).collect()),
        Value::Map(values) => {
            let mut object = serde_json::Map::new();
            for (key, value) in values {
                object.insert(key.clone(), value_to_json(value));
            }
            Json::Object(object)
        }
    }
}

fn json_to_value(value: &Json) -> Result<Value, LangError> {
    match value {
        Json::String(value) => Ok(Value::String(value.clone())),
        Json::Number(value) => {
            if let Some(value) = value.as_i64() {
                Ok(Value::Int(value))
            } else if let Some(value) = value.as_f64() {
                Ok(Value::from_f64(value))
            } else {
                Err(LangError::bare(format!("number {value} is out of range")))
            }
        }
        Json::Bool(value) => Ok(Value::Bool(*value)),
        Json::Null => Ok(Value::Null),
        _ => Err(LangError::bare("only scalar values can be stored")),
    }
}

fn charge(budget: &mut usize, n: usize) -> Result<(), LangError> {
    if *budget < n {
        return Err(LangError::bare(
            "relationship traversal work budget exceeded",
        ));
    }
    *budget -= n;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA: &str = r#"
        type Author {
          name: String
          died?: Int
          wrote -> Book[]
        }
        type Book {
          title: String
          pages: Int
          wrote <- Author
        }
    "#;

    #[test]
    fn create_then_read_filters_pages() {
        let zega = Zega::in_memory().build().unwrap();
        let created = zega
            .run_lang(
                SCHEMA,
                r#"mutation {
                    Author(name: "Le Guin", died: 2018) {
                      name
                      died
                      wrote -> Book(title: "The Dispossessed", pages: 387) { title pages }
                      wrote -> Book(title: "A Wizard of Earthsea", pages: 205) { title pages }
                    }
                }"#,
            )
            .unwrap();
        assert_eq!(created["name"], "Le Guin");
        assert_eq!(created["died"], 2018);
        assert_eq!(created["Books"].as_array().unwrap().len(), 2);

        let read = zega
            .run_lang(
                SCHEMA,
                r#"{
                    Author(name: "Le Guin") {
                      name
                      wrote -> Book(pages > 300) { title pages }
                    }
                }"#,
            )
            .unwrap();
        assert_eq!(read["name"], "Le Guin");
        assert_eq!(
            read["Books"],
            json!([{ "title": "The Dispossessed", "pages": 387 }])
        );

        zega.run_lang(
            SCHEMA,
            r#"mutation { Book(title: "The Lathe of Heaven", pages: 175) { title } }"#,
        )
        .unwrap();
        let linked = zega
            .run_lang(
                SCHEMA,
                r#"mutation {
                    Author(name: "Le Guin") {
                      name
                      wrote -> link Book(title: "The Lathe of Heaven") { title pages }
                    }
                }"#,
            )
            .unwrap();
        assert_eq!(
            linked["Books"],
            json!([{ "title": "The Lathe of Heaven", "pages": 175 }])
        );

        let updated = zega
            .run_lang(
                SCHEMA,
                r#"mutation { Author(name: "Le Guin") set died: 2018 { name died } }"#,
            )
            .unwrap();
        assert_eq!(updated["died"], 2018);
    }

    #[test]
    fn hop_range_counts_from_the_start() {
        let zega = Zega::in_memory().build().unwrap();
        let schema = r#"
            type Person {
              name: String
              manages -> Person[]
            }
        "#;
        zega.run_lang(
            schema,
            r#"mutation {
                Person(name: "Ada") {
                  manages -> Person(name: "Bob") {
                    manages -> Person(name: "Dee") { name }
                  }
                }
            }"#,
        )
        .unwrap();
        let read = zega
            .run_lang(
                schema,
                r#"{
                    Person(name: "Ada") {
                      manages *1..3 -> Person { name &hops }
                    }
                }"#,
            )
            .unwrap();
        let people = read["Persons"].as_array().unwrap();
        assert_eq!(people.len(), 2);
        assert_eq!(people[0]["name"], "Bob");
        assert_eq!(people[0]["hops"], 1);
        assert_eq!(people[1]["name"], "Dee");
        assert_eq!(people[1]["hops"], 2);
    }

    #[test]
    fn edge_field_roundtrips() {
        let zega = Zega::in_memory().build().unwrap();
        zega.run_lang(
            SCHEMA,
            r#"mutation {
                Author(name: "Le Guin") {
                  wrote -> Book(title: "The Dispossessed", pages: 387) {
                    title
                    &year: 1974
                  }
                }
            }"#,
        )
        .unwrap();
        let read = zega
            .run_lang(
                SCHEMA,
                r#"{
                    Author(name: "Le Guin") {
                      wrote -> Book { title &year }
                    }
                }"#,
            )
            .unwrap();
        assert_eq!(read["Books"][0]["title"], "The Dispossessed");
        assert_eq!(read["Books"][0]["year"], 1974);
    }

    #[test]
    fn union_field_missing_on_one_type_is_null() {
        let zega = Zega::in_memory().build().unwrap();
        let schema = r#"
            type User {
              name: String
              likes -> (Book | Movie)[]
            }
            type Book { title: String }
            type Movie { title: String  runtime: Int }
        "#;
        zega.run_lang(
            schema,
            r#"mutation {
                User(name: "Ada") {
                  likes -> Book(title: "Kindred") { title }
                  likes -> Movie(title: "Alien", runtime: 117) { title }
                }
            }"#,
        )
        .unwrap();
        let read = zega
            .run_lang(
                schema,
                r#"{
                    User(name: "Ada") {
                      likes -> (Book | Movie) { title runtime }
                    }
                }"#,
            )
            .unwrap();
        let likes = read["likes"].as_array().unwrap();
        assert_eq!(likes.len(), 2);
        let book = likes.iter().find(|row| row["title"] == "Kindred").unwrap();
        let movie = likes.iter().find(|row| row["title"] == "Alien").unwrap();
        assert_eq!(book["runtime"], Json::Null);
        assert_eq!(movie["runtime"], 117);
    }
}
