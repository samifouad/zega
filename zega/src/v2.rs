//! Execute the v2 schema language against the graph the engine already stores.
//!
//! A read walks label indexes and adjacency lists and shapes one JSON value.
//! A mutation inserts rows, or looks a row up when the statement says `link`
//! or `set`. Writes go through the same node, relationship, and WAL operations
//! as the existing executor.

use std::collections::{HashMap, HashSet, VecDeque};

use serde_json::{json, Value as Json};
use crate::graph::{Graph, Node, NodeId, RelId};
use crate::lang::{
    BoolExpr, Cmp, Direction, Error as LangError, Item, LoadFormat, Pred, Schema, Selection, Span,
    Statement,
};
use crate::parser::Value;
use crate::wal::Operation;

use crate::{Zega, ZegaError};

/// Which ZQL grammar entry point [`check_zql`] should parse `source` as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZqlEntryPoint {
    /// A full `.zql` file: schema, `unique`, mutations, and an optional query.
    File,
    /// A single query block, standalone.
    Query,
    /// A single statement (schema, mutation, or query), standalone.
    Statement,
}

/// Parse-check `source` as `entry_point` without building or touching a
/// database. On success the source is syntactically valid (and, for
/// [`ZqlEntryPoint::File`], its schema is internally consistent); on failure
/// returns the same rendered diagnostic a parse-stage error from
/// [`Zega::apply_zql`] or [`Zega::run_lang`] would produce.
///
/// This exists for conformance testing of the parser's rejection paths in
/// isolation, without executing anything — the same three entry points
/// `apply_zql` (file) and `run_lang` (statement) already parse internally.
pub fn check_zql(entry_point: ZqlEntryPoint, source: &str) -> std::result::Result<(), String> {
    let result = match entry_point {
        ZqlEntryPoint::File => crate::lang::parse_zql(source).map(|_| ()),
        ZqlEntryPoint::Query => crate::lang::parse_query(source).map(|_| ()),
        ZqlEntryPoint::Statement => crate::lang::parse_statement(source).map(|_| ()),
    };
    result.map_err(|error| crate::lang::render_error("schema", source, &error))
}

impl Zega {
    /// Parse and check the schema, including the explicit display contract.
    pub fn schema(&self, source: &str) -> Result<Schema, ZegaError> {
        crate::lang::parse_schema(source).map_err(|error| explain(error, "schema", source))
    }

    /// Execute ZQL. Native loads resolve relative paths against the process cwd.
    /// HTTP(S) loads require the default `http` feature. Wasm hosts must supply
    /// raw UTF-8 sources with [`Self::run_lang_with_sources`].
    pub fn run_lang(&self, schema_src: &str, source: &str) -> Result<Json, ZegaError> {
        self.run_lang_with_loader(schema_src, source, &|location| read_location(location, self.allow_private_imports))
    }

    /// Execute with host-provided raw text, using the same Rust parsers and WAL
    /// write path. Every named source must be present; there is no I/O fallback.
    pub fn run_lang_with_sources(&self, schema_src: &str, source: &str, sources: &HashMap<String, String>) -> Result<Json, ZegaError> {
        self.run_lang_with_loader(schema_src, source, &|location| supplied_source(location, sources))
    }

    fn run_lang_with_loader(&self, schema_src: &str, source: &str, loader: &dyn Fn(&str) -> Result<String, LangError>) -> Result<Json, ZegaError> {
        let schema = crate::lang::parse_schema(schema_src)
            .map_err(|error| explain(error, "schema", schema_src))?;
        let uniques = crate::lang::parse_uniques(schema_src)
            .map_err(|error| explain(error, "schema", schema_src))?;
        let statement =
            crate::lang::parse_statement(source).map_err(|error| explain(error, "query", source))?;
        self.execute(&schema, &uniques, &statement, "query", source, loader)
    }

    pub fn delete_node(&self, id: u64) -> Result<(), ZegaError> {
        let mut graph = self
            .graph
            .lock()
            .map_err(|_| ZegaError::Execution("lock poisoned".to_string()))?;
        if graph.get_node(id).is_none() {
            return Ok(());
        }
        graph.delete_node(id);
        self.wal
            .append(&Operation::DeleteNode { id })
            .map_err(|error| ZegaError::Execution(error.to_string()))
    }

    pub fn delete_relationship(&self, id: u64) -> Result<(), ZegaError> {
        let mut graph = self
            .graph
            .lock()
            .map_err(|_| ZegaError::Execution("lock poisoned".to_string()))?;
        if graph.get_relationship(id).is_none() {
            return Ok(());
        }
        graph.delete_relationship(id);
        self.wal
            .append(&Operation::DeleteRel { id })
            .map_err(|error| ZegaError::Execution(error.to_string()))
    }

    /// Store one schema relationship from `from_id` to `to_id`.
    /// `field` is the name written on the source type, such as `actedIn`.
    pub fn connect_schema(
        &self,
        schema_src: &str,
        from_id: u64,
        field: &str,
        to_id: u64,
    ) -> Result<(), ZegaError> {
        let schema = crate::lang::parse_schema(schema_src)
            .map_err(|error| explain(error, "schema", schema_src))?;
        let mut graph = self
            .graph
            .lock()
            .map_err(|_| ZegaError::Execution("lock poisoned".to_string()))?;
        let from = graph
            .get_node(from_id)
            .cloned()
            .ok_or_else(|| ZegaError::Execution(format!("missing node {from_id}")))?;
        let to = graph
            .get_node(to_id)
            .cloned()
            .ok_or_else(|| ZegaError::Execution(format!("missing node {to_id}")))?;
        let label = from
            .labels
            .first()
            .cloned()
            .ok_or_else(|| ZegaError::Execution(format!("node {from_id} has no type")))?;
        let edge = schema.edge(&label, field).map_err(|error| {
            ZegaError::Execution(format!("{label} has no relationship {field}: {}", error))
        })?;
        let (_, rel, direction, targets, _) = edge.as_edge().unwrap();
        let target_label = to.labels.first().map(String::as_str).unwrap_or("");
        if !targets.iter().any(|target| target == target_label) {
            return Err(ZegaError::Execution(format!(
                "{label}.{field} does not reach {target_label}"
            )));
        }
        let props = HashMap::new();
        require_edge_props(
            &schema,
            rel,
            &props,
            Span {
                line: 0,
                column: 0,
                end_line: 0,
                end_column: 0,
            },
        )
        .map_err(|error| explain(error, "schema", schema_src))?;
        connect(&mut graph, &self.wal, from_id, to_id, direction, rel, props)
            .map_err(|error| explain(error, "schema", schema_src))?;
        Ok(())
    }

    /// Run a `.zql` file: schema, unique, mutations, then an optional query.
    pub fn apply_zql(&self, source: &str) -> Result<Json, ZegaError> {
        self.apply_zql_with_loader(source, &|location| read_location(location, self.allow_private_imports))
    }

    /// Apply a document using raw text supplied by its host (for example JS fetch).
    pub fn apply_zql_with_sources(&self, source: &str, sources: &HashMap<String, String>) -> Result<Json, ZegaError> {
        self.apply_zql_with_loader(source, &|location| supplied_source(location, sources))
    }

    fn apply_zql_with_loader(&self, source: &str, loader: &dyn Fn(&str) -> Result<String, LangError>) -> Result<Json, ZegaError> {
        let file =
            crate::lang::parse_zql(source).map_err(|error| explain(error, "schema", source))?;
        let mut last = Json::Null;
        for statement in &file.statements {
            last = self.execute(&file.schema, &file.uniques, statement, "schema", source, loader)?;
        }
        Ok(last)
    }

    fn execute(
        &self,
        schema: &Schema,
        uniques: &[(String, String)],
        statement: &Statement,
        source_name: &str,
        source: &str,
        loader: &dyn Fn(&str) -> Result<String, LangError>,
    ) -> Result<Json, ZegaError> {
        if let Statement::Run(query) = statement {
            if query.root.is_none() {
                return Ok(Json::Null);
            }
        }
        prepare(schema, statement).map_err(|error| explain(error, source_name, source))?;
        // I/O and parsing happen before the graph lock. Each complete load is
        // inserted under the same lock as ordinary mutations.
        let rows = if let Statement::Load { format, locations, .. } = statement {
            load_rows(*format, locations, loader).map_err(|error| explain(error, source_name, source))?
        } else { Vec::new() };
        let mut graph = self
            .graph
            .lock()
            .map_err(|_| ZegaError::Execution("lock poisoned".to_string()))?;
        let mut budget = self.traversal_work_budget;
        run_statement(
            &mut graph,
            &self.wal,
            schema,
            uniques,
            statement,
            &mut budget,
            &rows,
        )
        .map_err(|error| explain(error, source_name, source))
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
                let mut props = serde_json::Map::new();
                for (key, value) in &rel.props {
                    props.insert(key.clone(), value_to_json(value));
                }
                json!({
                    "id": rel.id,
                    "type": rel.kind,
                    "from": rel.from,
                    "to": rel.to,
                    "props": props,
                })
            })
            .collect();
        rels.sort_by_key(|rel| rel["id"].as_u64().unwrap_or(0));
        Ok(json!({ "nodes": nodes, "rels": rels }))
    }
}

fn prepare(schema: &Schema, statement: &Statement) -> Result<(), LangError> {
    let (root, mutation) = match statement {
        Statement::Run(query) => (query.root.as_ref(), query.mutation),
        Statement::Load { template, .. } => (template.root.as_ref(), true),
    };
    if let Some(root) = root {
        crate::lang::check(schema, root, mutation)?;
    }
    Ok(())
}

fn run_statement(
    graph: &mut Graph,
    wal: &crate::Wal,
    schema: &Schema,
    uniques: &[(String, String)],
    statement: &Statement,
    budget: &mut usize,
    rows: &[HashMap<String, Json>],
) -> Result<Json, LangError> {
    match statement {
        Statement::Run(query) => {
            let Some(root) = &query.root else {
                return Ok(Json::Null);
            };
            if query.mutation {
                mutate(graph, wal, schema, root, uniques)
            } else {
                read(graph, schema, root, budget)
            }
        }
        Statement::Load { template, .. } => {
            if let Some(error) = crate::lang::missing_columns(template, rows)
                .into_iter()
                .next()
            {
                return Err(error);
            }
            let mut out = Vec::new();
            for row in rows {
                let Some(query) = crate::lang::bind_row(template, row)? else {
                    continue;
                };
                let Some(root) = &query.root else {
                    continue;
                };
                out.push(mutate(graph, wal, schema, root, uniques)?);
            }
            Ok(Json::Array(out))
        }
    }
}

fn load_rows(
    format: LoadFormat,
    locations: &[String],
    loader: &dyn Fn(&str) -> Result<String, LangError>,
) -> Result<Vec<HashMap<String, Json>>, LangError> {
    let mut rows = Vec::new();
    for location in locations {
        let text = loader(location)?;
        rows.extend(parse_load(format, &text, location)?);
    }
    Ok(rows)
}

pub(crate) const MAX_IMPORT_BYTES: usize = 2_000_000;

/// Parse raw UTF-8 import text using the same parser used by load mutations.
/// Hosts can use this to display a preview; insertion should use the raw source.
pub fn parse_import(format: LoadFormat, text: &str) -> Result<Vec<HashMap<String, Json>>, String> {
    parse_load(format, text, "import").map_err(|error| error.to_string())
}

fn parse_load(format: LoadFormat, text: &str, location: &str) -> Result<Vec<HashMap<String, Json>>, LangError> {
    if text.len() > MAX_IMPORT_BYTES {
        return Err(LangError::bare(format!("{location} is larger than 2MB")));
    }
    match format {
        LoadFormat::Csv => crate::lang::csv_rows(text).map_err(LangError::bare),
        LoadFormat::Json => {
            let value = serde_json::from_str(text)
                .map_err(|error| LangError::bare(format!("{location} is not json: {error}")))?;
            crate::lang::json_rows(value).map_err(LangError::bare)
        }
    }
}

fn supplied_source(location: &str, sources: &HashMap<String, String>) -> Result<String, LangError> {
    validate_location(location).map_err(LangError::bare)?;
    sources.get(location).cloned().ok_or_else(|| LangError::bare(format!("cannot read {location}: host did not supply this source")))
}

/// Return the locations a host must fetch for a statement or document. The
/// standard public-network/path policy is checked before any host I/O.
pub fn zql_load_locations(entry_point: ZqlEntryPoint, source: &str) -> Result<Vec<String>, String> {
    let statements = match entry_point {
        ZqlEntryPoint::File => crate::lang::parse_zql(source).map(|file| file.statements),
        ZqlEntryPoint::Statement | ZqlEntryPoint::Query => crate::lang::parse_statement(source).map(|statement| vec![statement]),
    }.map_err(|error| crate::lang::render_error("schema", source, &error))?;
    let mut locations = Vec::new();
    for statement in statements {
        if let Statement::Load { locations: sources, .. } = statement {
            for location in sources {
                validate_location(&location)?;
                if !locations.contains(&location) { locations.push(location); }
            }
        }
    }
    Ok(locations)
}

fn read_location(location: &str, allow_private: bool) -> Result<String, LangError> {
    if allow_private && is_remote(location) {
        validate_remote_syntax(location).map_err(LangError::bare)?;
    } else {
        validate_location(location).map_err(LangError::bare)?;
    }
    read_location_text(location, allow_private)
        .map_err(|error| LangError::bare(format!("cannot read {location}: {error}")))
}

fn is_remote(location: &str) -> bool {
    let lower = location.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// A document may name a relative file or a public http(s) address.
/// It may not climb out of its folder, carry a password, or call a private host.
fn validate_location(location: &str) -> Result<(), String> {
    let location = location.trim();
    if location.is_empty() || location.contains('\0') {
        return Err("invalid location".into());
    }
    let lower = location.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return validate_remote(location);
    }
    if location.contains("://") {
        return Err("only http and https addresses are allowed".into());
    }
    if location.split(['/', '\\']).any(|part| part == "..") {
        return Err("the path cannot contain ..".into());
    }
    Ok(())
}

fn validate_remote_syntax(location: &str) -> Result<(), String> {
    if !is_remote(location) { return Err("only http and https addresses are allowed".into()); }
    let rest = location
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or("");
    if rest.contains('@') {
        return Err("the address cannot include a password".into());
    }
    let hostport = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = if let Some(host) = hostport.strip_prefix('[') {
        host.split(']').next().unwrap_or("")
    } else {
        hostport.split(':').next().unwrap_or("")
    };
    if host.is_empty() {
        return Err("the address has no host".into());
    }
    Ok(())
}

fn validate_remote(location: &str) -> Result<(), String> {
    validate_remote_syntax(location)?;
    let authority = location.split_once("://").unwrap().1.split(['/', '?', '#']).next().unwrap();
    let host = if let Some(host) = authority.strip_prefix('[') { host.split(']').next().unwrap() } else { authority.split(':').next().unwrap() };
    if blocked_host(host) { return Err("that address points at a private network".into()); }
    Ok(())
}

fn blocked_host(host: &str) -> bool {
    let host = host.trim().to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") || host == "metadata.google.internal" {
        return true;
    }
    host.parse::<std::net::IpAddr>().is_ok_and(blocked_ip)
}

fn blocked_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || ip.is_multicast() || ip.is_broadcast() || ip.octets()[0] == 0,
        std::net::IpAddr::V6(ip) => ip.to_ipv4_mapped().map(|ip| blocked_ip(ip.into())).unwrap_or_else(|| ip.is_loopback() || ip.is_unspecified() || ip.is_unique_local() || ip.is_unicast_link_local() || ip.is_multicast()),
    }
}

// Check the actual addresses used by the connector, including DNS responses
// and redirect targets; a textual hostname check alone permits DNS rebinding.
#[cfg(all(not(target_arch = "wasm32"), feature = "http"))]
#[derive(Debug)]
struct ImportResolver { allow_private: bool }

#[cfg(all(not(target_arch = "wasm32"), feature = "http"))]
impl ureq::unversioned::resolver::Resolver for ImportResolver {
    fn resolve(&self, uri: &ureq::http::Uri, config: &ureq::config::Config, timeout: ureq::unversioned::transport::NextTimeout) -> Result<ureq::unversioned::resolver::ResolvedSocketAddrs, ureq::Error> {
        use ureq::unversioned::resolver::DefaultResolver;
        let addresses = DefaultResolver::default().resolve(uri, config, timeout)?;
        if !self.allow_private && addresses.iter().any(|addr| blocked_ip(addr.ip())) {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "that address points at a private network").into());
        }
        Ok(addresses)
    }
}

#[cfg(target_arch = "wasm32")]
fn read_location_text(_location: &str, _allow_private: bool) -> Result<String, String> {
    Err("wasm cannot read files or perform blocking HTTP; supply raw text with run_lang_with_sources or apply_zql_with_sources".into())
}

#[cfg(not(target_arch = "wasm32"))]
fn read_location_text(location: &str, allow_private: bool) -> Result<String, String> {
    if is_remote(location) {
        fetch_location(location, allow_private)
    } else {
        let file = std::fs::File::open(location).map_err(|error| error.to_string())?;
        read_bounded(file)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn read_bounded(reader: impl std::io::Read) -> Result<String, String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    reader.take(MAX_IMPORT_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_IMPORT_BYTES { return Err("larger than 2MB".into()); }
    String::from_utf8(bytes).map_err(|_| "not utf-8".into())
}

#[cfg(all(not(target_arch = "wasm32"), not(feature = "http")))]
fn fetch_location(_location: &str, _allow_private: bool) -> Result<String, String> {
    Err("HTTP loading requires the zega `http` cargo feature".into())
}

#[cfg(all(not(target_arch = "wasm32"), feature = "http"))]
fn fetch_location(location: &str, allow_private: bool) -> Result<String, String> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(20)))
        .max_redirects(0)
        .max_redirects_will_error(false)
        .proxy(None)
        .build();
    let agent = ureq::Agent::with_parts(config, ureq::unversioned::transport::DefaultConnector::default(), ImportResolver { allow_private });
    let mut url = location.to_string();
    for _ in 0..=5 {
        if allow_private { validate_remote_syntax(&url)?; } else { validate_remote(&url)?; }
        let mut response = agent.get(&url).call().map_err(|error| error.to_string())?;
        if response.status().is_redirection() {
            let next = response.headers().get("location").and_then(|value| value.to_str().ok()).ok_or("redirect has no location")?;
            url = url::Url::parse(&url).and_then(|base| base.join(next)).map_err(|error| error.to_string())?.to_string();
            continue;
        }
        if !response.status().is_success() { return Err(format!("status {}", response.status())); }
        return read_bounded(response.body_mut().as_reader());
    }
    Err("too many redirects".into())
}

fn explain(error: LangError, source_name: &str, source: &str) -> ZegaError {
    ZegaError::Execution(crate::lang::render_error(source_name, source, &error))
}

fn read(
    graph: &Graph,
    schema: &Schema,
    root: &Selection,
    budget: &mut usize,
) -> Result<Json, LangError> {
    let mut ids = candidates(graph, root);
    ids.retain(|id| node_matches(graph, *id, root.condition.as_ref()));
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
    uniques: &[(String, String)],
) -> Result<Json, LangError> {
    apply_node(graph, wal, schema, root, None, uniques)
}

/// Writes or finds this selection and returns only the rows this statement touched.
fn apply_node(
    graph: &mut Graph,
    wal: &crate::Wal,
    schema: &Schema,
    sel: &Selection,
    parent: Option<(NodeId, Direction, String)>,
    uniques: &[(String, String)],
) -> Result<Json, LangError> {
    let lookup = !sel.sets.is_empty() || has_link(sel);
    let id = if lookup {
        lookup_one(graph, sel)?
    } else {
        insert_node(graph, wal, sel, uniques)?
    };
    if !sel.sets.is_empty() {
        let props = sel
            .sets
            .iter()
            .map(|(key, value, _)| Ok((key.clone(), json_to_value(value)?)))
            .collect::<Result<HashMap<_, _>, LangError>>()?;
        let labels = graph
            .get_node(id)
            .map(|node| node.labels.clone())
            .unwrap_or_default();
        if let Some((ty, field)) = find_duplicate(graph, &labels, &props, uniques, Some(id)) {
            let span = sel
                .sets
                .iter()
                .find(|(key, _, _)| key == &field)
                .map(|(_, _, span)| *span)
                .unwrap_or(sel.type_span);
            return Err(unique_conflict(&ty, &field, span));
        }
        graph.update_node(id, props.clone());
        wal.append(&Operation::UpdateNode { id, props })
            .map_err(|error| LangError::bare(error.to_string()))?;
    }
    if let Some((parent_id, direction, rel)) = &parent {
        let props = edge_sets(sel)?;
        let span = sel
            .items
            .iter()
            .find_map(|item| match item {
                Item::EdgeSet(_, _, span) => Some(*span),
                _ => None,
            })
            .unwrap_or(sel.type_span);
        require_edge_props(schema, rel, &props, span)?;
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
                    let props = edge_sets(target)?;
                    let span = target
                        .items
                        .iter()
                        .find_map(|item| match item {
                            Item::EdgeSet(_, _, span) => Some(*span),
                            _ => None,
                        })
                        .unwrap_or(target.type_span);
                    require_edge_props(schema, rel, &props, span)?;
                    connect(graph, wal, id, child_id, *direction, rel, props)?;
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
                        uniques,
                    )?
                };
                let key = field.clone();
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
    ids.retain(|id| node_matches(graph, *id, sel.condition.as_ref()));
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

fn insert_node(
    graph: &mut Graph,
    wal: &crate::Wal,
    sel: &Selection,
    uniques: &[(String, String)],
) -> Result<NodeId, LangError> {
    let mut props = HashMap::new();
    if let Some(expr) = &sel.condition {
        assign_props(expr, sel, &mut props)?;
    }
    let labels: Vec<String> = std::iter::once(sel.type_name.clone())
        .chain(sel.also.iter().cloned())
        .collect();
    if let Some((ty, field)) = find_duplicate(graph, &labels, &props, uniques, None) {
        return Err(unique_conflict(&ty, &field, sel.type_span));
    }
    let id = graph.create_node(labels.clone(), props.clone());
    wal.append(&Operation::InsertNode { id, labels, props })
        .map_err(|error| LangError::bare(error.to_string()))?;
    Ok(id)
}

fn unique_conflict(ty: &str, field: &str, span: Span) -> LangError {
    LangError::at(span, format!("unique {ty} {{ {field} }} is already used"))
        .with_help(format!("another {ty} already has this {field}"))
}

/// The first unique field whose value is already stored on a different node.
/// A missing value, or null, does not collide.
fn find_duplicate(
    graph: &Graph,
    labels: &[String],
    props: &HashMap<String, Value>,
    uniques: &[(String, String)],
    except: Option<NodeId>,
) -> Option<(String, String)> {
    for label in labels {
        for (ty, field) in uniques {
            if ty != label {
                continue;
            }
            let Some(value) = props.get(field.as_str()) else {
                continue;
            };
            if matches!(value, Value::Null) {
                continue;
            }
            let Some(ids) = graph.nodes_by_property(field, value) else {
                continue;
            };
            let taken = ids.iter().any(|id| {
                if except == Some(*id) {
                    return false;
                }
                graph
                    .get_node(*id)
                    .is_some_and(|node| node.labels.iter().any(|has| has == label))
            });
            if taken {
                return Some((ty.clone(), field.clone()));
            }
        }
    }
    None
}

fn require_edge_props(
    schema: &Schema,
    rel: &str,
    props: &HashMap<String, Value>,
    span: Span,
) -> Result<(), LangError> {
    let declared = schema.types.iter().find_map(|ty| {
        ty.fields.iter().find_map(|field| match field {
            crate::lang::Field::Edge {
                rel: kind, props, ..
            } if kind == rel && !props.is_empty() => Some(props),
            _ => None,
        })
    });
    let Some(declared) = declared else {
        if let Some(name) = props.keys().next() {
            return Err(
                LangError::at(span, format!("{rel} has no field {name}")).with_help(format!(
                    "declare it on the relationship: {rel} -> Type {{ {name}: Int }}"
                )),
            );
        }
        return Ok(());
    };
    for field in declared {
        if !field.optional && !props.contains_key(&field.name) {
            return Err(
                LangError::at(span, format!("{rel} requires &{}", field.name))
                    .with_help(format!("write `&{}: …` on the edge", field.name)),
            );
        }
        if let Some(value) = props.get(&field.name) {
            if !edge_value_matches(&field.ty, value) {
                return Err(
                    LangError::at(span, format!("&{} is not {}", field.name, field.ty))
                        .with_help(format!("`{}` is {}", field.name, field.ty)),
                );
            }
        }
    }
    for name in props.keys() {
        if !declared.iter().any(|field| &field.name == name) {
            return Err(LangError::at(span, format!("{rel} has no field {name}")));
        }
    }
    Ok(())
}

fn edge_value_matches(ty: &str, value: &Value) -> bool {
    match ty {
        "String" => matches!(value, Value::String(_)),
        "Int" => matches!(value, Value::Int(_)),
        "Float" => matches!(value, Value::Float(_) | Value::Int(_)),
        "Bool" => matches!(value, Value::Bool(_)),
        _ => true,
    }
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
                    if !node_matches(graph, next, target.condition.as_ref()) {
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
                let key = field.clone();
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

fn node_matches(graph: &Graph, id: NodeId, condition: Option<&BoolExpr>) -> bool {
    match condition {
        None => true,
        Some(expr) => eval_expr(graph, id, expr),
    }
}

fn eval_expr(graph: &Graph, id: NodeId, expr: &BoolExpr) -> bool {
    match expr {
        BoolExpr::Test(pred) => pred_matches(graph, id, pred),
        BoolExpr::And(left, right) => eval_expr(graph, id, left) && eval_expr(graph, id, right),
        BoolExpr::Or(left, right) => eval_expr(graph, id, left) || eval_expr(graph, id, right),
    }
}

fn assign_props(
    expr: &BoolExpr,
    sel: &Selection,
    props: &mut HashMap<String, Value>,
) -> Result<(), LangError> {
    match expr {
        BoolExpr::And(left, right) => {
            assign_props(left, sel, props)?;
            assign_props(right, sel, props)
        }
        BoolExpr::Test(Pred::Eq(field, value, _)) if field != "id" => {
            props.insert(field.clone(), json_to_value(value)?);
            Ok(())
        }
        BoolExpr::Test(Pred::Eq(_, _, _)) => Ok(()),
        other => Err(LangError::at(
            other.span(),
            format!("creating a {} only accepts field: value", sel.type_name),
        )
        .with_help("write `name: \"value\"`, and join fields with `&&`")),
    }
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
    sel.condition
        .as_ref()
        .is_some_and(|expr| expr.is_equality_and())
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
          wrote -> Book[] {
            year?: Int
          }
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
                    Author(name: "Le Guin" && died: 2018) {
                      name
                      died
                      wrote -> Book(title: "The Dispossessed" && pages: 387) { title pages }
                      wrote -> Book(title: "A Wizard of Earthsea" && pages: 205) { title pages }
                    }
                }"#,
            )
            .unwrap();
        assert_eq!(created["name"], "Le Guin");
        assert_eq!(created["died"], 2018);
        assert_eq!(created["wrote"].as_array().unwrap().len(), 2);

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
            read["wrote"],
            json!([{ "title": "The Dispossessed", "pages": 387 }])
        );

        zega.run_lang(
            SCHEMA,
            r#"mutation { Book(title: "The Lathe of Heaven" && pages: 175) { title } }"#,
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
            linked["wrote"],
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
    fn empty_query_block_returns_null() {
        let zega = Zega::in_memory().build().unwrap();
        assert_eq!(zega.run_lang(SCHEMA, "query { }").unwrap(), Json::Null);
        assert_eq!(
            zega.run_lang(SCHEMA, "query {\n  Author { name }\n}")
                .unwrap(),
            json!([])
        );
    }

    #[test]
    fn a_condition_can_or_and_exclude() {
        let zega = Zega::in_memory().build().unwrap();
        zega.run_lang(SCHEMA, r#"mutation { Author(name: "Le Guin") { name } }"#)
            .unwrap();
        zega.run_lang(SCHEMA, r#"mutation { Author(name: "Butler") { name } }"#)
            .unwrap();
        let both = zega
            .run_lang(
                SCHEMA,
                r#"{ Author(name = "Le Guin" || name = "Butler") { name } }"#,
            )
            .unwrap();
        let names: Vec<_> = both
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["Le Guin", "Butler"]);

        let one = zega
            .run_lang(
                SCHEMA,
                r#"{ Author(name = "Le Guin" && name = "Le Guin") { name } }"#,
            )
            .unwrap();
        assert_eq!(one["name"], "Le Guin");

        let rest = zega
            .run_lang(SCHEMA, r#"{ Author(name != "Le Guin") { name } }"#)
            .unwrap();
        assert_eq!(rest[0]["name"], "Butler");
    }

    #[test]
    fn connect_schema_then_delete_edge_and_node() {
        let zega = Zega::in_memory().build().unwrap();
        zega.run_lang(SCHEMA, r#"mutation { Author(name: "Le Guin") { name } }"#)
            .unwrap();
        zega.run_lang(
            SCHEMA,
            r#"mutation { Book(title: "The Dispossessed") { title } }"#,
        )
        .unwrap();
        let graph = zega.graph_json().unwrap();
        let author = graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["name"] == "Le Guin")
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        let book = graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["title"] == "The Dispossessed")
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        zega.connect_schema(SCHEMA, author, "wrote", book).unwrap();
        let linked = zega.graph_json().unwrap();
        assert_eq!(linked["rels"].as_array().unwrap().len(), 1);
        let rel_id = linked["rels"][0]["id"].as_u64().unwrap();
        zega.delete_relationship(rel_id).unwrap();
        assert!(zega.graph_json().unwrap()["rels"]
            .as_array()
            .unwrap()
            .is_empty());
        zega.delete_node(author).unwrap();
        let remaining = zega.graph_json().unwrap();
        let names: Vec<_> = remaining["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|node| node["name"].as_str().unwrap_or("").to_string())
            .collect();
        assert!(!names.iter().any(|name| name == "Le Guin"));
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
        let people = read["manages"].as_array().unwrap();
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
                  wrote -> Book(title: "The Dispossessed" && pages: 387) {
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
        assert_eq!(read["wrote"][0]["title"], "The Dispossessed");
        assert_eq!(read["wrote"][0]["year"], 1974);
        let stored = zega.graph_json().unwrap();
        assert_eq!(stored["rels"][0]["props"]["year"], 1974);
    }

    #[test]
    fn required_edge_field_rejects_a_bare_connection() {
        let zega = Zega::in_memory().build().unwrap();
        let schema = r#"
            type Team { name: String playsFor -> Player[] { years: Int } }
            type Player { name: String playsFor <- Team }
        "#;
        let missing = zega
            .run_lang(
                schema,
                r#"mutation { Team(name: "Oilers") { playsFor -> Player(name: "Connor McDavid") { name } } }"#,
            )
            .unwrap_err();
        assert!(missing.to_string().contains("requires &years"), "{missing}");
        zega.run_lang(
            schema,
            r#"mutation { Team(name: "Oilers") { playsFor -> Player(name: "Connor McDavid") { name &years: 10 } } }"#,
        )
        .unwrap();
        let read = zega
            .run_lang(
                schema,
                r#"{ Player(name = "Connor McDavid") { playsFor <- Team { name &years } } }"#,
            )
            .unwrap();
        assert_eq!(read["playsFor"]["name"], "Oilers");
        assert_eq!(read["playsFor"]["years"], 10);
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
                  likes -> Movie(title: "Alien" && runtime: 117) { title }
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

    #[test]
    fn unique_block_rejects_a_second_insert_and_a_rename() {
        let zega = Zega::in_memory().build().unwrap();
        let schema = r#"
            schema {
              type Player { name: String salary: Int }
              type Team { name: String }
            }
            unique { Player { name salary } Team { name } }
        "#;
        zega.run_lang(
            schema,
            r#"mutation { Player(name: "Connor McDavid" && salary: 12500000) { name } }"#,
        )
        .unwrap();
        let dup_name = zega
            .run_lang(
                schema,
                r#"mutation { Player(name: "Connor McDavid" && salary: 1) { name } }"#,
            )
            .unwrap_err();
        assert!(
            dup_name.to_string().contains("unique Player { name }"),
            "{dup_name}"
        );
        let dup_salary = zega
            .run_lang(
                schema,
                r#"mutation { Player(name: "Leon Draisaitl" && salary: 12500000) { name } }"#,
            )
            .unwrap_err();
        assert!(
            dup_salary.to_string().contains("unique Player { salary }"),
            "{dup_salary}"
        );
        zega.run_lang(
            schema,
            r#"mutation { Player(name: "Leon Draisaitl" && salary: 14000000) { name } }"#,
        )
        .unwrap();
        zega.run_lang(schema, r#"mutation { Team(name: "Oilers") { name } }"#)
            .unwrap();
        let renamed = zega
            .run_lang(
                schema,
                r#"mutation { Player(name: "Leon Draisaitl") set name: "Connor McDavid" }"#,
            )
            .unwrap_err();
        assert!(
            renamed.to_string().contains("unique Player { name }"),
            "{renamed}"
        );
        zega.run_lang(
            schema,
            r#"mutation { Player(name: "Leon Draisaitl") set salary: 9000000 }"#,
        )
        .unwrap();
    }

    #[test]
    fn csv_and_json_loads_insert_rows_and_honor_unique() {
        let dir = tempfile::tempdir().unwrap();
        let csv_path = dir.path().join("players.csv");
        let json_path = dir.path().join("more.json");
        std::fs::write(
            &csv_path,
            "Name,Team,Salary\nConnor McDavid,Oilers,12500000\nAuston Matthews,Maple Leafs,13250000\n",
        )
        .unwrap();
        std::fs::write(
            &json_path,
            r#"[{"Name":"Nathan MacKinnon","Team":"Avalanche","Salary":12604000}]"#,
        )
        .unwrap();
        let csv_file = serde_json::to_string(&vec![csv_path.to_str().unwrap()]).unwrap();
        let json_file = serde_json::to_string(&vec![json_path.to_str().unwrap()]).unwrap();
        let zega = Zega::in_memory().build().unwrap();
        let source = format!(
            r#"
                schema {{
                  type Player {{ name: String salary: Int }}
                  type Team {{ name: String playsFor -> Player[] }}
                }}
                unique {{ Player {{ name }} Team {{ name }} }}
                mutation csv {csv_file} {{
                  Team(name: $Team) {{
                    playsFor -> Player(name: $Name && salary: $Salary) {{ name salary }}
                  }}
                }}
                mutation json {json_file} {{
                  Team(name: $Team) {{
                    playsFor -> Player(name: $Name && salary: $Salary) {{ name }}
                  }}
                }}
                query {{ Player {{ name }} }}
                "#
        );
        let loaded = zega.apply_zql(&source).unwrap();
        let names: Vec<&str> = loaded
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec!["Connor McDavid", "Auston Matthews", "Nathan MacKinnon"]
        );
        let again_path = dir.path().join("again.csv");
        std::fs::write(&again_path, "Name,Salary\nConnor McDavid,1\n").unwrap();
        let again_file = serde_json::to_string(&vec![again_path.to_str().unwrap()]).unwrap();
        let again = zega.apply_zql(&format!(
            r#"
            schema {{
              type Player {{ name: String salary: Int }}
              type Team {{ name: String }}
            }}
            unique {{ Player {{ name }} }}
            mutation csv {again_file} {{
              Player(name: $Name && salary: $Salary) {{ name }}
            }}
            "#
        ));
        assert!(again
            .unwrap_err()
            .to_string()
            .contains("unique Player { name }"));
    }

    #[test]
    fn csv_path_reads_dollar_columns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("players.csv");
        std::fs::write(&path, "Name,Salary\nLeon Draisaitl,14000000\n").unwrap();
        let location = serde_json::to_string(&vec![path.to_str().unwrap()]).unwrap();
        let source = format!(
            r#"
            schema {{ type Player {{ name: String salary: Int }} }}
            mutation csv {location} {{
              Player(name: $Name && salary: $Salary) {{ name salary }}
            }}
            "#
        );
        let loaded = Zega::in_memory()
            .build()
            .unwrap()
            .apply_zql(&source)
            .unwrap();
        assert_eq!(loaded[0]["name"], "Leon Draisaitl");
        assert_eq!(loaded[0]["salary"], 14_000_000);
        let missing = Zega::in_memory().build().unwrap().apply_zql(
            r#"
            schema { type Player { name: String } }
            mutation csv ["./no-such-players.csv"] { Player(name: $Name) { name } }
            "#,
        );
        let missing = missing.unwrap_err();
        assert!(missing.to_string().contains("cannot read"), "{missing}");
    }

    #[test]
    fn remote_load_rejects_a_private_address_and_a_parent_path() {
        let zega = Zega::in_memory().build().unwrap();
        let private = zega
            .apply_zql(
                r#"
                schema { type Player { name: String } }
                mutation json ["http://127.0.0.1/players.json"] {
                  Player(name: $Name) { name }
                }
                "#,
            )
            .unwrap_err();
        assert!(private.to_string().contains("private network"), "{private}");
        let parent = zega
            .apply_zql(
                r#"
                schema { type Player { name: String } }
                mutation csv ["../players.csv"] {
                  Player(name: $Name) { name }
                }
                "#,
            )
            .unwrap_err();
        assert!(parent.to_string().contains(".."), "{parent}");
        let password = zega
            .apply_zql(
                r#"
                schema { type Player { name: String } }
                mutation json ["https://user:secret@example.com/players.json"] {
                  Player(name: $Name) { name }
                }
                "#,
            )
            .unwrap_err();
        assert!(password.to_string().contains("password"), "{password}");
    }
}
