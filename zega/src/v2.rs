//! Execute the v2 schema language against the graph the engine already stores.
//!
//! A read walks label indexes and adjacency lists and shapes one JSON value.
//! A mutation inserts rows, or looks a row up when the statement says `link`
//! or `set`. Writes go through the same node, relationship, and WAL operations
//! as the existing executor.

mod discovery;

use crate::location::{Bounds, Point, EARTH_RADIUS};
use crate::vector::{Vector, VectorSpec, Metric};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use crate::graph::{Graph, Node, NodeId, RelId};
use crate::index::{IndexKind, Interval, TextPattern};
use crate::lang::{
    BoolExpr, Cmp, Direction, Error as LangError, Item, LoadFormat, Pred, Schema, Selection, Span,
    Statement,
};
use crate::value::Value;
use crate::journal::{atomically, Journal};
use serde_json::{json, Value as Json};

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
        self.run_lang_with_loader(schema_src, source, &|location| {
            read_location(location, self.allow_private_imports)
        })
    }

    /// Execute with host-provided raw text, using the same Rust parsers and WAL
    /// write path. Every named source must be present; there is no I/O fallback.
    pub fn run_lang_with_sources(
        &self,
        schema_src: &str,
        source: &str,
        sources: &HashMap<String, String>,
    ) -> Result<Json, ZegaError> {
        self.run_lang_with_loader(schema_src, source, &|location| {
            supplied_source(location, sources)
        })
    }

    fn run_lang_with_loader(
        &self,
        schema_src: &str,
        source: &str,
        loader: &dyn Fn(&str) -> Result<String, LangError>,
    ) -> Result<Json, ZegaError> {
        let schema = crate::lang::parse_schema(schema_src)
            .map_err(|error| explain(error, "schema", schema_src))?;
        let uniques = crate::lang::parse_uniques(schema_src)
            .map_err(|error| explain(error, "schema", schema_src))?;
        let indexes = crate::lang::parse_indexes(schema_src)
            .map_err(|error| explain(error, "schema", schema_src))?;
        let statement = crate::lang::parse_statement(source)
            .map_err(|error| explain(error, "query", source))?;
        let declared = Declared { uniques: &uniques, indexes: &indexes };
        self.execute(&schema, declared, &statement, "query", source, loader)
    }

    /// Rows a ZQL filter has been tested on since this database opened. An
    /// index lowers this number and never changes a result; tests use it to
    /// show an index was used.
    pub fn rows_examined(&self) -> Result<u64, ZegaError> {
        let graph = self
            .graph
            .lock()
            .map_err(|_| ZegaError::Execution("lock poisoned".to_string()))?;
        Ok(graph.examined())
    }

    /// Nodes whose edges a ZQL `*path` search has read since this database
    /// opened. `toward` (A*) lowers this number and never changes a route's
    /// cost; tests use it to show the guess was used.
    pub fn nodes_expanded(&self) -> Result<u64, ZegaError> {
        let graph = self
            .graph
            .lock()
            .map_err(|_| ZegaError::Execution("lock poisoned".to_string()))?;
        Ok(graph.expanded())
    }

    pub fn delete_node(&self, id: u64) -> Result<(), ZegaError> {
        let mut graph = self
            .graph
            .lock()
            .map_err(|_| ZegaError::Execution("lock poisoned".to_string()))?;
        atomically(&mut graph, &self.wal, |graph, journal| {
            journal.delete_node(graph, id);
            Ok(())
        })
    }

    pub fn delete_relationship(&self, id: u64) -> Result<(), ZegaError> {
        let mut graph = self
            .graph
            .lock()
            .map_err(|_| ZegaError::Execution("lock poisoned".to_string()))?;
        atomically(&mut graph, &self.wal, |graph, journal| {
            journal.delete_relationship(graph, id);
            Ok(())
        })
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
        let (_, rel, direction, targets, many) = edge.as_edge().unwrap();
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
        atomically(&mut graph, &self.wal, |graph, journal| {
            connect(
                graph,
                journal,
                from_id,
                to_id,
                direction,
                RelationshipSpec {
                    field,
                    kind: rel,
                    many,
                    span: Span {
                        line: 0,
                        column: 0,
                        end_line: 0,
                        end_column: 0,
                    },
                },
                props,
            )
            .map_err(|error| explain(error, "schema", schema_src))
        })?;
        Ok(())
    }

    /// Run a `.zql` file: schema, unique, mutations, then an optional query.
    pub fn apply_zql(&self, source: &str) -> Result<Json, ZegaError> {
        self.apply_zql_with_loader(source, &|location| {
            read_location(location, self.allow_private_imports)
        })
    }

    /// Apply a document using raw text supplied by its host (for example JS fetch).
    pub fn apply_zql_with_sources(
        &self,
        source: &str,
        sources: &HashMap<String, String>,
    ) -> Result<Json, ZegaError> {
        self.apply_zql_with_loader(source, &|location| supplied_source(location, sources))
    }

    fn apply_zql_with_loader(
        &self,
        source: &str,
        loader: &dyn Fn(&str) -> Result<String, LangError>,
    ) -> Result<Json, ZegaError> {
        let file =
            crate::lang::parse_zql(source).map_err(|error| explain(error, "schema", source))?;
        let mut last = Json::Null;
        for statement in &file.statements {
            last = self.execute(
                &file.schema,
                Declared { uniques: &file.uniques, indexes: &file.indexes },
                statement,
                "schema",
                source,
                loader,
            )?;
        }
        Ok(last)
    }

    fn execute(
        &self,
        schema: &Schema,
        declared: Declared<'_>,
        statement: &Statement,
        source_name: &str,
        source: &str,
        loader: &dyn Fn(&str) -> Result<String, LangError>,
    ) -> Result<Json, ZegaError> {
        prepare(schema, statement).map_err(|error| explain(error, source_name, source))?;
        // I/O and parsing happen before the graph lock. Each complete load is
        // inserted under the same lock as ordinary mutations.
        let rows = if let Statement::Load {
            format, locations, ..
        } = statement
        {
            load_rows(*format, locations, loader)
                .map_err(|error| explain(error, source_name, source))?
        } else {
            Vec::new()
        };
        let uniques = declared.uniques;
        let mut graph = self
            .graph
            .lock()
            .map_err(|_| ZegaError::Execution("lock poisoned".to_string()))?;
        // The schema of this statement says which indexes exist; the writes
        // below keep them current, and a rollback restores them with the rows.
        graph.sync_indexes(&crate::lang::effective_indexes(
            schema,
            declared.uniques,
            declared.indexes,
        ));
        let mut work = Work::new(self.traversal_work_budget, self.query_time_limit);
        // A read logs nothing. A mutation or load is one statement: every row
        // and nested selection is applied, or (on a validation error, a
        // refused WAL append, or the time limit) none is, in memory and in the
        // WAL alike.
        let result = atomically(&mut graph, &self.wal, |graph, journal| {
            run_statement(graph, journal, schema, uniques, statement, &mut work, &rows)
                .map_err(|error| explain(error, source_name, source))
        });
        match (result, self.query_time_limit) {
            (Err(_), Some(limit)) if work.expired() => Err(ZegaError::QueryTimeLimit { limit }),
            (result, _) => result,
        }
    }

    pub fn graph_json(&self) -> Result<Json, ZegaError> {
        let graph = self
            .graph
            .lock()
            .map_err(|_| ZegaError::Execution("lock poisoned".to_string()))?;
        let mut nodes: Vec<Json> = graph.all_nodes().values().map(node_json).collect();
        nodes.sort_by_key(|node| node["id"].as_u64().unwrap_or(0));
        let mut rels: Vec<Json> = graph.all_relationships().values().map(rel_json).collect();
        rels.sort_by_key(|rel| rel["id"].as_u64().unwrap_or(0));
        Ok(json!({ "nodes": nodes, "rels": rels }))
    }
}

/// The `unique` and `index` blocks that travel with a schema.
#[derive(Clone, Copy)]
struct Declared<'a> {
    uniques: &'a [(String, String)],
    indexes: &'a [crate::lang::IndexSpec],
}

fn prepare(schema: &Schema, statement: &Statement) -> Result<(), LangError> {
    if let Statement::Run(query) = statement {
        crate::lang::check_pipeline(schema, query)?;
    }
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
    journal: &mut Journal,
    schema: &Schema,
    uniques: &[(String, String)],
    statement: &Statement,
    work: &mut Work,
    rows: &[HashMap<String, Json>],
) -> Result<Json, LangError> {
    match statement {
        Statement::Run(query) => {
            if !query.mutation && (!query.then.is_empty() || query.skip) {
                return discovery::pipeline(graph, schema, query, work);
            }
            let Some(root) = &query.root else {
                return Ok(Json::Null);
            };
            if query.mutation {
                mutate(graph, journal, schema, root, uniques, work)
            } else {
                read(graph, schema, root, &mut ReadContext { work, trace: None })
            }
        }
        Statement::Load { template, .. } => {
            if let Some(error) = crate::lang::missing_columns(template, rows)
                .into_iter()
                .next()
            {
                return Err(error);
            }
            let bound = rows
                .iter()
                .map(|row| crate::lang::bind_location_row(schema, template, row))
                .collect::<Result<Vec<_>, _>>()?;
            let mut out = Vec::new();
            for query in bound.into_iter().flatten() {
                let Some(root) = &query.root else {
                    continue;
                };
                work.step()?;
                out.push(mutate(graph, journal, schema, root, uniques, work)?);
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

fn parse_load(
    format: LoadFormat,
    text: &str,
    location: &str,
) -> Result<Vec<HashMap<String, Json>>, LangError> {
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
    sources.get(location).cloned().ok_or_else(|| {
        LangError::bare(format!(
            "cannot read {location}: host did not supply this source"
        ))
    })
}

/// Return the locations a host must fetch for a statement or document. The
/// standard public-network/path policy is checked before any host I/O.
pub fn zql_load_locations(entry_point: ZqlEntryPoint, source: &str) -> Result<Vec<String>, String> {
    let statements = match entry_point {
        ZqlEntryPoint::File => crate::lang::parse_zql(source).map(|file| file.statements),
        ZqlEntryPoint::Statement | ZqlEntryPoint::Query => {
            crate::lang::parse_statement(source).map(|statement| vec![statement])
        }
    }
    .map_err(|error| crate::lang::render_error("schema", source, &error))?;
    let mut locations = Vec::new();
    for statement in statements {
        if let Statement::Load {
            locations: sources, ..
        } = statement
        {
            for location in sources {
                validate_location(&location)?;
                if !locations.contains(&location) {
                    locations.push(location);
                }
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
    if !is_remote(location) {
        return Err("only http and https addresses are allowed".into());
    }
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
    let authority = location
        .split_once("://")
        .unwrap()
        .1
        .split(['/', '?', '#'])
        .next()
        .unwrap();
    let host = if let Some(host) = authority.strip_prefix('[') {
        host.split(']').next().unwrap()
    } else {
        authority.split(':').next().unwrap()
    };
    if blocked_host(host) {
        return Err("that address points at a private network".into());
    }
    Ok(())
}

fn blocked_host(host: &str) -> bool {
    let host = host.trim().to_ascii_lowercase();
    if host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host == "metadata.google.internal"
    {
        return true;
    }
    host.parse::<std::net::IpAddr>().is_ok_and(blocked_ip)
}

fn blocked_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => {
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip.is_broadcast()
                || ip.octets()[0] == 0
        }
        std::net::IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map(|ip| blocked_ip(ip.into()))
            .unwrap_or_else(|| {
                ip.is_loopback()
                    || ip.is_unspecified()
                    || ip.is_unique_local()
                    || ip.is_unicast_link_local()
                    || ip.is_multicast()
            }),
    }
}

// Check the actual addresses used by the connector, including DNS responses
// and redirect targets; a textual hostname check alone permits DNS rebinding.
#[cfg(all(not(target_arch = "wasm32"), feature = "http"))]
#[derive(Debug)]
struct ImportResolver {
    allow_private: bool,
}

#[cfg(all(not(target_arch = "wasm32"), feature = "http"))]
impl ureq::unversioned::resolver::Resolver for ImportResolver {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        config: &ureq::config::Config,
        timeout: ureq::unversioned::transport::NextTimeout,
    ) -> Result<ureq::unversioned::resolver::ResolvedSocketAddrs, ureq::Error> {
        use ureq::unversioned::resolver::DefaultResolver;
        let addresses = DefaultResolver::default().resolve(uri, config, timeout)?;
        if !self.allow_private && addresses.iter().any(|addr| blocked_ip(addr.ip())) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "that address points at a private network",
            )
            .into());
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
    reader
        .take(MAX_IMPORT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > MAX_IMPORT_BYTES {
        return Err("larger than 2MB".into());
    }
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
    let agent = ureq::Agent::with_parts(
        config,
        ureq::unversioned::transport::DefaultConnector::default(),
        ImportResolver { allow_private },
    );
    let mut url = location.to_string();
    for _ in 0..=5 {
        if allow_private {
            validate_remote_syntax(&url)?;
        } else {
            validate_remote(&url)?;
        }
        let mut response = agent.get(&url).call().map_err(|error| error.to_string())?;
        if response.status().is_redirection() {
            let next = response
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok())
                .ok_or("redirect has no location")?;
            url = url::Url::parse(&url)
                .and_then(|base| base.join(next))
                .map_err(|error| error.to_string())?
                .to_string();
            continue;
        }
        if !response.status().is_success() {
            return Err(format!("status {}", response.status()));
        }
        return read_bounded(response.body_mut().as_reader());
    }
    Err("too many redirects".into())
}

fn explain(error: LangError, source_name: &str, source: &str) -> ZegaError {
    ZegaError::Execution(crate::lang::render_error(source_name, source, &error))
}

#[derive(Default)]
struct ReadTrace {
    nodes: std::collections::BTreeSet<NodeId>,
    rels: std::collections::BTreeSet<RelId>,
}

struct ReadContext<'a> {
    work: &'a mut Work,
    trace: Option<ReadTrace>,
}

fn read(
    graph: &Graph,
    schema: &Schema,
    root: &Selection,
    context: &mut ReadContext<'_>,
) -> Result<Json, LangError> {
    let mut ids = candidates(graph, root, context.work)?;
    // Candidates are in ascending id order, which is also the result order,
    // so without a ranking (`near`, `order by`) the first `limit` matches are
    // the answer and the rest need not be tested (zegadb/zega#82).
    let enough = root.limit.filter(|_| root.near.is_none() && root.order.is_none());
    retain_first_matches(graph, &mut ids, root.condition.as_ref(), enough, context.work)?;
    order_limit(graph, root, &mut ids, |id| *id, context.work)?;
    if equality_lookup(root) {
        return match ids.len() {
            0 => Ok(Json::Null),
            1 => project(graph, schema, root, ids[0], 0, None, context),
            n => Err(LangError::at(
                root.type_span,
                format!("{} matched {n} rows", root.type_name),
            )
            .with_help("an equality filter has to match one row")),
        };
    }
    let mut rows = Vec::new();
    for id in ids {
        rows.push(project(graph, schema, root, id, 0, None, context)?);
    }
    Ok(Json::Array(rows))
}

fn mutate(
    graph: &mut Graph,
    journal: &mut Journal,
    schema: &Schema,
    root: &Selection,
    uniques: &[(String, String)],
    work: &mut Work,
) -> Result<Json, LangError> {
    apply_node(graph, journal, schema, root, None, uniques, work)
}

/// Writes or finds this selection and returns only the rows this statement touched.
fn apply_node(
    graph: &mut Graph,
    journal: &mut Journal,
    schema: &Schema,
    sel: &Selection,
    parent: Option<(NodeId, Direction, String, String, bool, Span)>,
    uniques: &[(String, String)],
    work: &mut Work,
) -> Result<Json, LangError> {
    work.step()?;
    let lookup = !sel.sets.is_empty() || has_link(sel);
    let id = if lookup {
        lookup_one(graph, sel, uniques, work)?
    } else {
        require_points(schema, sel)?;
        insert_node(graph, journal, schema, sel, uniques)?
    };
    if !sel.sets.is_empty() {
        let props = sel
            .sets
            .iter()
            .map(|(key, value, _)| Ok((key.clone(), json_to_prop(schema, sel, key, value)?)))
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
        journal.update_node(graph, id, props);
    }
    if let Some((parent_id, direction, field, rel, many, edge_span)) = &parent {
        let props = edge_sets(sel)?;
        let props_span = sel
            .items
            .iter()
            .find_map(|item| match item {
                Item::EdgeSet(_, _, span) => Some(*span),
                _ => None,
            })
            .unwrap_or(sel.type_span);
        require_edge_props(schema, rel, &props, props_span)?;
        connect(
            graph,
            journal,
            *parent_id,
            id,
            *direction,
            RelationshipSpec { field, kind: rel, many: *many, span: *edge_span },
            props,
        )?;
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
            Item::Id(alias) => { object.insert(alias.clone(), json!(node.id)); }
            Item::Score(alias, _) => { object.insert(alias.clone(), score_json(&node, sel)); }
            Item::Similarity(alias, sim) => { object.insert(alias.clone(), similarity_json(&node, sim)); }
            Item::Distance(alias, distance) => {
                object.insert(
                    alias.clone(),
                    point_prop(&node, &distance.field)
                        .map(|point| json!(point.distance(distance.origin)))
                        .unwrap_or(Json::Null),
                );
            }
            Item::Hops(alias) => {
                object.insert(alias.clone(), json!(0));
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
                span,
                link,
                direction,
                target,
                range,
                path,
            } => {
                if range.is_some() {
                    return Err(LangError::bare("a mutation cannot use a hop range"));
                }
                if path.is_some() {
                    return Err(LangError::bare("a mutation cannot find a path"));
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
                    let child_id = lookup_one(graph, target, uniques, work)?;
                    let props = edge_sets(target)?;
                    let props_span = target
                        .items
                        .iter()
                        .find_map(|item| match item {
                            Item::EdgeSet(_, _, span) => Some(*span),
                            _ => None,
                        })
                        .unwrap_or(target.type_span);
                    require_edge_props(schema, rel, &props, props_span)?;
                    connect(
                        graph,
                        journal,
                        id,
                        child_id,
                        *direction,
                        RelationshipSpec { field, kind: rel, many, span: *span },
                        props,
                    )?;
                    let saved = graph.get_node(child_id).unwrap().clone();
                    let mut child_object = serde_json::Map::new();
                    for child_item in &target.items {
                        match child_item {
                            Item::Prop(name, _) => {
                                child_object.insert(name.clone(), prop_json(&saved, name));
                            }
                            Item::Id(alias) => { child_object.insert(alias.clone(), json!(saved.id)); }
                            Item::Score(alias, _) => { child_object.insert(alias.clone(), score_json(&saved, target)); }
                            Item::Similarity(alias, sim) => { child_object.insert(alias.clone(), similarity_json(&saved, sim)); }
                            Item::Distance(alias, distance) => {
                                child_object.insert(
                                    alias.clone(),
                                    point_prop(&saved, &distance.field)
                                        .map(|point| json!(point.distance(distance.origin)))
                                        .unwrap_or(Json::Null),
                                );
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
                        journal,
                        schema,
                        target,
                        Some((id, *direction, field.clone(), rel.to_string(), many, *span)),
                        uniques,
                        work,
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

fn lookup_one(
    graph: &Graph,
    sel: &Selection,
    uniques: &[(String, String)],
    work: &mut Work,
) -> Result<NodeId, LangError> {
    let mut ids = match unique_candidates(graph, sel, uniques) {
        Some(ids) => ids,
        None => candidates(graph, sel, work)?,
    };
    retain_matches(graph, &mut ids, sel.condition.as_ref(), work)?;
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

fn unique_candidates(
    graph: &Graph,
    sel: &Selection,
    uniques: &[(String, String)],
) -> Option<Vec<NodeId>> {
    for (ty, field) in uniques {
        if ty != &sel.type_name || !sel.also.is_empty() {
            continue;
        }
        if let Some(value) = sel
            .condition
            .as_ref()
            .and_then(|expr| guaranteed_eq(expr, field))
        {
            if value.is_array() { return None; }
            let value = json_to_value(value).ok()?;
            let mut ids: Vec<_> = graph
                .nodes_by_property(field, &value)
                .into_iter()
                .flat_map(|ids| ids.iter().copied())
                .filter(|id| {
                    graph
                        .get_node(*id)
                        .is_some_and(|node| node.labels.iter().any(|label| label == ty))
                })
                .collect();
            ids.sort_unstable();
            return Some(ids);
        }
    }
    None
}

fn guaranteed_eq<'a>(expr: &'a BoolExpr, field: &str) -> Option<&'a Json> {
    match expr {
        BoolExpr::Test(Pred::Eq(name, value, _)) if name == field => Some(value),
        BoolExpr::And(terms) => terms.iter().find_map(|term| guaranteed_eq(term, field)),
        BoolExpr::Test(_) | BoolExpr::Or(_) => None,
    }
}

fn has_link(sel: &Selection) -> bool {
    sel.items
        .iter()
        .any(|item| matches!(item, Item::Walk { link: true, .. }))
}

fn insert_node(
    graph: &mut Graph,
    journal: &mut Journal,
    schema: &Schema,
    sel: &Selection,
    uniques: &[(String, String)],
) -> Result<NodeId, LangError> {
    let mut props = HashMap::new();
    if let Some(expr) = &sel.condition {
        assign_props(expr, sel, schema, &mut props)?;
    }
    let labels: Vec<String> = std::iter::once(sel.type_name.clone())
        .chain(sel.also.iter().cloned())
        .collect();
    if let Some((ty, field)) = find_duplicate(graph, &labels, &props, uniques, None) {
        return Err(unique_conflict(&ty, &field, sel.type_span));
    }
    Ok(journal.create_node(graph, labels, props))
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
                        .with_help(if crate::lang::is_unit_string(&field.ty) {
                            crate::lang::unit_string_help(&field.ty).into()
                        } else {
                            format!("`{}` is {}", field.name, field.ty)
                        }),
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
        "String<url>" | "String<iso2>" => {
            matches!(value, Value::String(text) if crate::lang::valid_unit_string(ty, text))
        }
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

struct RelationshipSpec<'a> {
    field: &'a str,
    kind: &'a str,
    many: bool,
    span: Span,
}

fn connect(
    graph: &mut Graph,
    journal: &mut Journal,
    parent: NodeId,
    child: NodeId,
    direction: Direction,
    relationship: RelationshipSpec<'_>,
    props: HashMap<String, Value>,
) -> Result<RelId, LangError> {
    let RelationshipSpec { field, kind, many, span } = relationship;
    if !many {
        let mut existing = neighbors(graph, parent, kind, direction);
        existing.sort_unstable();
        if let Some((current, _)) = existing.first() {
            let source = graph
                .get_node(parent)
                .map(node_description)
                .unwrap_or_else(|| format!("node {parent}"));
            let source_type = graph
                .get_node(parent)
                .and_then(|node| node.labels.first())
                .map(String::as_str)
                .unwrap_or("node");
            let first = graph
                .get_node(*current)
                .map(node_description)
                .unwrap_or_else(|| format!("node {current}"));
            let second = graph
                .get_node(child)
                .map(node_description)
                .unwrap_or_else(|| format!("node {child}"));
            return Err(LangError::at(
                span,
                format!("single-valued relationship {source_type}.{field} on {source} already connects {first}; cannot also connect {second}"),
            )
            .with_help("declare it `Book[]` if many are intended, or unlink the current one first"));
        }
    }
    let (from, to) = match direction {
        Direction::Out => (parent, child),
        Direction::In => (child, parent),
    };
    Ok(journal.create_relationship(graph, kind.to_string(), from, to, props))
}

fn node_description(node: &Node) -> String {
    let ty = node.labels.first().map(String::as_str).unwrap_or("node");
    format!("{ty}#{}", node.id)
}

fn ensure_single_valued(
    graph: &Graph,
    id: NodeId,
    rel: &str,
    field: &str,
    direction: Direction,
    span: Span,
) -> Result<(), LangError> {
    let mut edges = neighbors(graph, id, rel, direction);
    if edges.len() <= 1 {
        return Ok(());
    }
    edges.sort_unstable();
    let node = graph
        .get_node(id)
        .ok_or_else(|| LangError::bare(format!("missing node {id}")))?;
    let first = graph
        .get_node(edges[0].0)
        .map(node_description)
        .unwrap_or_else(|| format!("node {}", edges[0].0));
    let second = graph
        .get_node(edges[1].0)
        .map(node_description)
        .unwrap_or_else(|| format!("node {}", edges[1].0));
    Err(LangError::at(
        span,
        format!(
            "single-valued relationship {}.{field} on {} connects both {first} and {second}",
            node.labels.first().map(String::as_str).unwrap_or("node"),
            node_description(node)
        ),
    )
    .with_help("declare it `Book[]` if many are intended, or unlink the current one first"))
}

fn project(
    graph: &Graph,
    schema: &Schema,
    sel: &Selection,
    id: NodeId,
    hops: usize,
    arrived: Option<RelId>,
    context: &mut ReadContext<'_>,
) -> Result<Json, LangError> {
    context.work.step()?;
    let node = graph
        .get_node(id)
        .ok_or_else(|| LangError::bare(format!("missing node {id}")))?;
    if let Some(trace) = &mut context.trace {
        trace.nodes.insert(id);
        if let Some(rel) = arrived { trace.rels.insert(rel); }
    }
    let mut object = serde_json::Map::new();
    for item in &sel.items {
        match item {
            Item::Prop(name, _) => {
                ensure_prop(schema, sel, name)?;
                object.insert(name.clone(), prop_json(node, name));
            }
            Item::Id(alias) => { object.insert(alias.clone(), json!(node.id)); }
            Item::Score(alias, _) => { object.insert(alias.clone(), score_json(node, sel)); }
            Item::Similarity(alias, sim) => { object.insert(alias.clone(), similarity_json(node, sim)); }
            Item::Distance(alias, distance) => {
                object.insert(
                    alias.clone(),
                    point_prop(node, &distance.field)
                        .map(|point| json!(point.distance(distance.origin)))
                        .unwrap_or(Json::Null),
                );
            }
            Item::Hops(alias) => {
                object.insert(alias.clone(), json!(hops));
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
                path,
                direction,
                target,
                span,
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
                if let Some(path) = path {
                    let weight_ty = match edge {
                        crate::lang::Field::Edge { props, .. } => path
                            .weight
                            .as_ref()
                            .and_then(|(name, _)| props.iter().find(|prop| &prop.name == name)),
                        crate::lang::Field::Prop { .. } => None,
                    };
                    let walk = PathWalk {
                        field,
                        rel,
                        direction: *direction,
                        targets,
                        span: *span,
                        path,
                        target,
                        weight_ty: weight_ty.map(|prop| prop.ty.as_str()),
                        weight_unit: weight_ty.and_then(|prop| prop.unit),
                    };
                    let value = route(graph, schema, id, hops, walk, context)?;
                    object.insert(field.clone(), value);
                    continue;
                }
                let reached = if let Some((min, max)) = range {
                    walk_range(
                        graph,
                        id,
                        WalkSpec {
                            rel,
                            field,
                            direction: *direction,
                            targets,
                            range: (*min, *max),
                            single_valued: !many,
                            span: *span,
                        },
                        context.work,
                    )?
                } else {
                    if !many {
                        ensure_single_valued(graph, id, rel, field, *direction, *span)?;
                    }
                    context.work.charge(1)?;
                    neighbors(graph, id, rel, *direction)
                        .into_iter()
                        .filter(|(next, _)| node_has_any_label(graph, *next, targets))
                        .map(|(next, rel_id)| (next, 1usize, rel_id))
                        .collect()
                };
                let mut reached = reached;
                reached.retain(|(next, ..)| node_matches(graph, *next, target.condition.as_ref()));
                order_limit(graph, target, &mut reached, |(id, ..)| *id, context.work)?;
                let mut rows = Vec::new();
                for (next, depth, rel_id) in reached {
                    rows.push(project(
                        graph,
                        schema,
                        target,
                        next,
                        hops + depth,
                        Some(rel_id),
                        context,
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
    if name == "@id" {
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

/// A relationship as the explorer draws it: the shape of `graph_json`'s rels.
fn rel_json(rel: &crate::graph::Relationship) -> Json {
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
}

/// A* guesses this share of the straight line to the target. Distances here
/// are on a sphere; a road measured on the WGS84 ellipsoid can be up to 0.56%
/// shorter than the sphere's straight line, and the guess must stay below it.
const STRAIGHT_LINE_SHARE: f64 = 0.99;

struct PathWalk<'a> {
    field: &'a str,
    rel: &'a str,
    direction: Direction,
    targets: &'a [String],
    span: Span,
    path: &'a crate::lang::PathSpec,
    target: &'a Selection,
    /// The declared type of the weight field, `Int` or `Float`.
    weight_ty: Option<&'a str>,
    /// Its declared distance unit, `Float<km>`, which A* needs.
    weight_unit: Option<crate::lang::DistanceUnit>,
}

/// `field *path ... -> Target`: one route from `start` to the nearest node
/// the target selects, or null when there is none within the bound.
fn route(
    graph: &Graph,
    schema: &Schema,
    start: NodeId,
    hops: usize,
    walk: PathWalk<'_>,
    context: &mut ReadContext<'_>,
) -> Result<Json, LangError> {
    use crate::lang::PathBound;
    use crate::path::{cheapest, fewest_edges, Limit, Step};

    let PathWalk { field, rel, direction, targets, span, path, target, weight_ty, weight_unit } = walk;
    let mut goals = candidates(graph, target, context.work)?;
    retain_matches(graph, &mut goals, target.condition.as_ref(), context.work)?;
    let goal_set: HashSet<NodeId> = goals.iter().copied().collect();
    let is_goal = |id: NodeId| goal_set.contains(&id);
    let describe = |id: NodeId| {
        graph.get_node(id).map_or_else(|| format!("node#{id}"), node_description)
    };
    // Every node after the start is one the target selection can read.
    let next = |node: NodeId, work: &mut Work| -> Result<Vec<(NodeId, RelId)>, LangError> {
        let out: Vec<_> = neighbors(graph, node, rel, direction)
            .into_iter()
            .filter(|(to, _)| node_has_any_label(graph, *to, targets))
            .collect();
        work.charge(out.len())?;
        Ok(out)
    };
    let found = match &path.weight {
        None => {
            let max_hops = match path.bound {
                None => None,
                Some((PathBound::Hops(n), _)) => Some(n),
                Some((PathBound::Cost { limit, inclusive }, _)) => {
                    // A route of n edges costs n.
                    let whole = limit.floor();
                    let most = if inclusive || whole < limit { whole } else { whole - 1.0 };
                    if most < 0.0 {
                        return Ok(Json::Null);
                    }
                    Some(most as usize)
                }
            };
            fewest_edges(start, is_goal, max_hops, |node| next(node, context.work))?
        }
        Some((weight, _)) => {
            let limit = match path.bound {
                None => None,
                Some((PathBound::Cost { limit, inclusive }, _)) => Some(Limit { limit, inclusive }),
                Some((PathBound::Hops(_), bound_span)) => {
                    return Err(LangError::at(bound_span, "a weighted path is bounded by cost"))
                }
            };
            let toward = path.toward.as_ref();
            let point_of = |id: NodeId| -> Result<Option<Point>, LangError> {
                let Some(toward) = toward else { return Ok(None) };
                let node = graph
                    .get_node(id)
                    .ok_or_else(|| LangError::bare(format!("missing node {id}")))?;
                match point_prop(node, &toward.field) {
                    Some(point) => Ok(Some(point)),
                    None => Err(LangError::at(
                        toward.span,
                        format!(
                            "{} has no {}, and `toward {}` needs a location on every node it reaches",
                            node_description(node),
                            toward.field,
                            toward.field
                        ),
                    )
                    .with_help("store the Point, or drop `toward` to search without a guess")),
                }
            };
            let goal_points = goals
                .iter()
                .map(|goal| point_of(*goal))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect::<Vec<Point>>();
            // The checker requires the unit for `toward`; this is the backstop.
            let unit = match (toward, weight_unit) {
                (None, _) => crate::lang::DistanceUnit::Metres,
                (Some(_), Some(unit)) => unit,
                (Some(toward), None) => {
                    return Err(LangError::at(
                        toward.span,
                        format!("toward needs a unit on the weight: declare {weight}: {}<km>", weight_ty.unwrap_or("Float")),
                    ))
                }
            };
            let unit_name = unit.as_str();
            let unit = unit.metres();
            let straight = |from: Point, to: Point| from.portable_distance(to) / unit;
            let guess = |node: NodeId| -> Result<f64, LangError> {
                if toward.is_none() || goal_points.is_empty() {
                    return Ok(0.0);
                }
                let here = point_of(node)?.expect("toward is set");
                let nearest = goal_points
                    .iter()
                    .map(|goal| straight(here, *goal))
                    .fold(f64::INFINITY, f64::min);
                Ok(STRAIGHT_LINE_SHARE * nearest)
            };
            let steps = |node: NodeId, work: &mut Work| -> Result<Vec<Step>, LangError> {
                let mut out = Vec::new();
                for (to, rel_id) in next(node, work)? {
                    let edge = || format!("{field}#{rel_id} from {} to {}", describe(node), describe(to));
                    let stored = graph.get_relationship(rel_id).and_then(|r| r.props.get(weight));
                    let weight_value = match stored {
                        Some(Value::Int(n)) => *n as f64,
                        Some(Value::Float(bits)) => f64::from_bits(*bits),
                        None | Some(Value::Null) => {
                            return Err(LangError::at(span, format!("{} has no {weight}", edge()))
                                .with_help(format!(
                                    "a path does not guess a missing weight; store `&{weight}` on every {field}"
                                )))
                        }
                        Some(other) => {
                            return Err(LangError::at(
                                span,
                                format!("{} has {weight} {}, not a number", edge(), value_to_json(other)),
                            ))
                        }
                    };
                    if weight_value < 0.0 {
                        return Err(LangError::at(
                            span,
                            format!("{} has {weight} {weight_value}, and a path weight cannot be negative", edge()),
                        )
                        .with_help("the cheapest route is only defined for weights of 0 or more"));
                    }
                    if let Some(toward) = toward {
                        // Every node, the start included, has its Point here:
                        // a missing one is an error, never a skipped check.
                        if let (Some(from), Some(to_point)) = (point_of(node)?, point_of(to)?) {
                            let line = straight(from, to_point);
                            if weight_value < STRAIGHT_LINE_SHARE * line {
                                return Err(LangError::at(
                                    toward.span,
                                    format!(
                                        "{} has {weight} {weight_value}, shorter than the {line:.3} {} straight line between its ends",
                                        edge(),
                                        unit_name
                                    ),
                                )
                                .with_help(format!(
                                    "`toward` needs every {weight} to be at least the straight-line distance; {weight} is declared in {unit_name}, so check that unit, or drop `toward`"
                                )));
                            }
                        }
                    }
                    out.push(Step { to, rel: rel_id, weight: weight_value });
                }
                Ok(out)
            };
            cheapest(start, is_goal, limit, |node| steps(node, context.work), guess)?
        }
    };
    graph.note_expanded(found.expanded);
    let Some(route) = found.route else {
        return Ok(Json::Null);
    };
    let steps = route.rels.len();
    let cost = match weight_ty {
        None => json!(steps),
        Some("Int") => json!(route.cost as i64),
        Some(_) => json!(route.cost),
    };
    // The start has no edge that arrived at it: its edge fields read null.
    let mut first = target.clone();
    let edge_fields: Vec<String> = first
        .items
        .iter()
        .filter_map(|item| match item {
            Item::EdgeProp(name, _) => Some(name.clone()),
            _ => None,
        })
        .collect();
    first.items.retain(|item| !matches!(item, Item::EdgeProp(..)));
    let mut nodes = Vec::with_capacity(route.nodes.len());
    for (i, node) in route.nodes.iter().enumerate() {
        if i == 0 {
            let mut row = project(graph, schema, &first, *node, hops, None, context)?;
            if let Json::Object(object) = &mut row {
                for name in &edge_fields {
                    object.insert(name.clone(), Json::Null);
                }
            }
            nodes.push(row);
        } else {
            let arrived = Some(route.rels[i - 1]);
            nodes.push(project(graph, schema, target, *node, hops + i, arrived, context)?);
        }
    }
    let edges: Vec<Json> = route
        .rels
        .iter()
        .filter_map(|id| graph.get_relationship(*id))
        .map(rel_json)
        .collect();
    Ok(json!({ "cost": cost, "hops": steps, "nodes": nodes, "edges": edges }))
}

struct WalkSpec<'a> {
    rel: &'a str,
    field: &'a str,
    direction: Direction,
    targets: &'a [String],
    range: (usize, usize),
    single_valued: bool,
    span: Span,
}

fn walk_range(
    graph: &Graph,
    start: NodeId,
    spec: WalkSpec<'_>,
    work: &mut Work,
) -> Result<Vec<(NodeId, usize, RelId)>, LangError> {
    let WalkSpec {
        rel,
        field,
        direction,
        targets,
        range,
        single_valued,
        span,
    } = spec;
    let (min, max) = range;
    let mut seen = HashSet::from([start]);
    let mut queue = VecDeque::from([(start, 0usize, 0u64)]);
    let mut found = Vec::new();
    while let Some((node, depth, via)) = queue.pop_front() {
        if single_valued {
            ensure_single_valued(graph, node, rel, field, direction, span)?;
        }
        if depth >= min && depth > 0 && node_has_any_label(graph, node, targets) {
            found.push((node, depth, via));
        }
        if depth == max {
            continue;
        }
        for (next, rel_id) in neighbors(graph, node, rel, direction) {
            work.charge(1)?;
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

/// Keep the rows whose condition holds, and count each row tested. A scan
/// is where a query spends its time, so each row tested is a step of work.
fn retain_matches(
    graph: &Graph,
    ids: &mut Vec<NodeId>,
    condition: Option<&BoolExpr>,
    work: &mut Work,
) -> Result<(), LangError> {
    retain_first_matches(graph, ids, condition, None, work)
}

/// Like `retain_matches`, but stop once `enough` rows match: the rows after
/// that are neither tested nor kept, nor counted as examined.
fn retain_first_matches(
    graph: &Graph,
    ids: &mut Vec<NodeId>,
    condition: Option<&BoolExpr>,
    enough: Option<usize>,
    work: &mut Work,
) -> Result<(), LangError> {
    let enough = enough.unwrap_or(usize::MAX);
    if condition.is_none() {
        ids.truncate(enough);
        return Ok(());
    }
    let mut kept = 0;
    let mut tested = 0;
    let mut stopped = Ok(());
    for i in 0..ids.len() {
        if kept == enough {
            break;
        }
        if let Err(error) = work.step() {
            stopped = Err(error);
            break;
        }
        tested += 1;
        if node_matches(graph, ids[i], condition) {
            ids[kept] = ids[i];
            kept += 1;
        }
    }
    graph.note_examined(tested);
    ids.truncate(kept);
    stopped
}

/// A condition on one field that a range index can answer.
fn range_interval(pred: &Pred) -> Option<(&str, Interval)> {
    let (field, interval) = match pred {
        Pred::Eq(field, value, _) => (field, Interval::exactly(value)?),
        Pred::Cmp(field, Cmp::Gt | Cmp::Gte, value, _) => (field, Interval::at_least(value)?),
        Pred::Cmp(field, Cmp::Lt | Cmp::Lte, value, _) => (field, Interval::at_most(value)?),
        _ => return None,
    };
    // `id` reads the node id, not a stored field.
    (field != "@id").then_some((field.as_str(), interval))
}

fn and_terms<'a>(expr: &'a BoolExpr, out: &mut Vec<&'a BoolExpr>) {
    match expr {
        BoolExpr::And(terms) => {
            for term in terms {
                and_terms(term, out);
            }
        }
        other => out.push(other),
    }
}

fn intersect(found: Option<HashSet<NodeId>>, next: HashSet<NodeId>) -> Option<HashSet<NodeId>> {
    Some(match found {
        None => next,
        Some(mut found) => {
            found.retain(|id| next.contains(id));
            found
        }
    })
}

/// Candidates from the spatial index and the declared `index { }` block.
/// Every row the condition accepts is in the set; the caller still tests each
/// one. None means no index applies and the caller scans the types.
fn index_filter(graph: &Graph, types: &[&str], expr: &BoolExpr) -> Option<HashSet<NodeId>> {
    match expr {
        BoolExpr::Test(Pred::Box(field, bounds, _)) => {
            Some(graph.spatial_candidates(field, *bounds))
        }
        BoolExpr::Test(Pred::Distance(distance, Cmp::Lt | Cmp::Lte, metres)) => Some(
            graph.spatial_candidates(&distance.field, Bounds::radius(distance.origin, *metres)),
        ),
        BoolExpr::Test(Pred::Contains(field, needle, _)) => {
            graph.text_candidates(types, field, TextPattern::Contains(needle))
        }
        BoolExpr::Test(Pred::StartsWith(field, needle, _)) => {
            graph.text_candidates(types, field, TextPattern::StartsWith(needle))
        }
        BoolExpr::Test(Pred::EndsWith(field, needle, _)) => {
            graph.text_candidates(types, field, TextPattern::EndsWith(needle))
        }
        BoolExpr::Test(pred) => {
            let (field, interval) = range_interval(pred)?;
            graph.range_candidates(types, field, &interval)
        }
        BoolExpr::And(_) => {
            // Bounds on one field join into one range scan: `a > 1 && a < 9`.
            let mut terms = Vec::new();
            and_terms(expr, &mut terms);
            let mut ranges: Vec<(&str, Interval)> = Vec::new();
            let mut found = None;
            for term in terms {
                if let BoolExpr::Test(pred) = term {
                    if let Some((field, interval)) = range_interval(pred) {
                        if graph.has_index(IndexKind::Range, types, field) {
                            match ranges.iter_mut().find(|(name, _)| *name == field) {
                                Some((_, joined)) => {
                                    *joined = std::mem::replace(joined, Interval::Empty)
                                        .intersect(interval)
                                }
                                None => ranges.push((field, interval)),
                            }
                            continue;
                        }
                    }
                }
                if let Some(set) = index_filter(graph, types, term) {
                    found = intersect(found, set);
                }
            }
            for (field, interval) in ranges {
                if let Some(set) = graph.range_candidates(types, field, &interval) {
                    found = intersect(found, set);
                }
            }
            found
        }
        BoolExpr::Or(terms) => {
            // A branch with no index may match anywhere.
            let mut found = HashSet::new();
            for term in terms {
                found.extend(index_filter(graph, types, term)?);
            }
            Some(found)
        }
    }
}

fn candidates(graph: &Graph, sel: &Selection, work: &mut Work) -> Result<Vec<NodeId>, LangError> {
    let has_label = |id: &NodeId| {
        graph.get_node(*id).is_some_and(|node| {
            node.labels
                .iter()
                .any(|label| label == &sel.type_name || sel.also.contains(label))
        })
    };
    let types: Vec<&str> = std::iter::once(sel.type_name.as_str())
        .chain(sel.also.iter().map(String::as_str))
        .collect();
    let indexed = sel
        .condition
        .as_ref()
        .and_then(|expr| index_filter(graph, &types, expr));
    // Expand a geodesic circle until k qualifying points are inside. Every
    // point outside is farther than the kth match, so early stopping is exact.
    if let Some(order) = &sel.order {
        if sel.limit == Some(0) {
            return Ok(Vec::new());
        }
        let maximum = std::f64::consts::PI * EARTH_RADIUS;
        let mut radius = if sel.limit.is_some() { 1000.0 } else { maximum };
        loop {
            let mut ids: Vec<_> = graph
                .spatial_candidates(&order.field, Bounds::radius(order.origin, radius))
                .into_iter()
                .filter(has_label)
                .filter(|id| indexed.as_ref().is_none_or(|set| set.contains(id)))
                .collect();
            retain_matches(graph, &mut ids, sel.condition.as_ref(), work)?;
            let mut ids: Vec<_> = ids
                .into_iter()
                .filter(|id| node_distance(graph, *id, order).is_some_and(|d| d <= radius))
                .collect();
            if radius >= maximum || sel.limit.is_some_and(|k| ids.len() >= k) {
                ids.sort_unstable();
                return Ok(ids);
            }
            radius = (radius * 2.0).min(maximum);
        }
    }
    let mut ids: Vec<_> = if let Some(indexed) = indexed {
        indexed.into_iter().filter(has_label).collect()
    } else {
        std::iter::once(&sel.type_name)
            .chain(&sel.also)
            .filter_map(|label| graph.nodes_by_label(label))
            .flat_map(|set| set.iter().copied())
            .collect()
    };
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}
fn point_prop(node: &Node, field: &str) -> Option<Point> {
    match node.props.get(field) {
        Some(Value::Point(point)) => Some(*point),
        _ => None,
    }
}
fn node_distance(graph: &Graph, id: NodeId, distance: &crate::lang::Distance) -> Option<f64> {
    Some(point_prop(graph.get_node(id)?, &distance.field)?.distance(distance.origin))
}
fn order_limit<T>(
    graph: &Graph,
    sel: &Selection,
    ids: &mut Vec<T>,
    id: impl Fn(&T) -> NodeId,
    work: &mut Work,
) -> Result<(), LangError> {
    if let Some(near) = &sel.near {
        let allowed: HashSet<_> = ids.iter().map(&id).collect();
        // A union may contain different metrics. Search each compatible index,
        // then rank the candidates by their actual stored field's metric.
        let mut ranked = Vec::new();
        for metric in [Metric::Cosine, Metric::Dot, Metric::L2] {
            let mut q = near.similarity.query.clone(); q.metric = metric;
            ranked.extend(graph.vector_nearest(&near.similarity.field, &q, near.k, near.exact, |n| allowed.contains(&n)));
        }
        ranked.sort_by(|a,b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0))); ranked.truncate(near.k);
        let ranks: HashMap<_,_> = ranked.iter().enumerate().map(|(i,(id,_))| (*id,i)).collect();
        ids.retain(|row| ranks.contains_key(&id(row)));
        ids.sort_by_key(|row| ranks[&id(row)]);
    }
    if let Some(order) = &sel.order {
        // Each distance is computed once, as a step of work, rather than twice
        // per comparison inside a sort that cannot be stopped part way.
        let mut keyed = Vec::with_capacity(ids.len());
        for row in ids.drain(..) {
            work.step()?;
            if let Some(distance) = node_distance(graph, id(&row), order) {
                keyed.push((distance, row));
            }
        }
        keyed.sort_by(|(a, x), (b, y)| a.total_cmp(b).then(id(x).cmp(&id(y))));
        ids.extend(keyed.into_iter().map(|(_, row)| row));
    }
    if let Some(limit) = sel.limit {
        ids.truncate(limit);
    }
    Ok(())
}
fn require_points(schema: &Schema, sel: &Selection) -> Result<(), LangError> {
    let tests = sel
        .condition
        .as_ref()
        .map(BoolExpr::tests)
        .unwrap_or_default();
    for name in std::iter::once(&sel.type_name).chain(&sel.also) {
        for field in &schema.get(name)?.fields {
            if let crate::lang::Field::Prop {
                name,
                ty,
                optional: false,
                ..
            } = field
            {
                if (ty == "Point" || VectorSpec::parse(ty).is_some())
                    && !tests
                        .iter()
                        .any(|pred| matches!(pred, Pred::Eq(field, _, _) if field == name))
                {
                    return Err(LangError::at(
                        sel.type_span,
                        format!("{} requires {ty} field {name}", sel.type_name),
                    ));
                }
            }
        }
    }
    Ok(())
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
        BoolExpr::And(terms) => terms.iter().all(|term| eval_expr(graph, id, term)),
        BoolExpr::Or(terms) => terms.iter().any(|term| eval_expr(graph, id, term)),
    }
}

fn assign_props(
    expr: &BoolExpr,
    sel: &Selection,
    schema: &Schema,
    props: &mut HashMap<String, Value>,
) -> Result<(), LangError> {
    match expr {
        BoolExpr::And(terms) => {
            for term in terms {
                assign_props(term, sel, schema, props)?;
            }
            Ok(())
        }
        BoolExpr::Test(Pred::Eq(field, value, _)) if field != "@id" => {
            props.insert(field.clone(), json_to_prop(schema, sel, field, value)?);
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
        Pred::Similarity(sim, op, threshold) => node_similarity(node, sim).is_some_and(|s| cmp_json(&json!(s), *op, &json!(threshold))),
        Pred::Distance(distance, op, metres) => {
            point_prop(node, &distance.field).is_some_and(|point| {
                cmp_json(&json!(point.distance(distance.origin)), *op, &json!(metres))
            })
        }
        Pred::Box(field, bounds, _) => {
            point_prop(node, field).is_some_and(|point| bounds.contains(point))
        }
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
    sel.near.is_none() && sel.order.is_none()
        && sel.limit.is_none()
        && sel
            .condition
            .as_ref()
            .is_some_and(|expr| expr.is_equality_and())
}

fn prop_json(node: &Node, name: &str) -> Json {
    if name == "@id" {
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
        Value::Point(point) => point.to_json(),
        Value::Vector(v) => v.to_json(),
        Value::Map(values) => {
            let mut object = serde_json::Map::new();
            for (key, value) in values {
                object.insert(key.clone(), value_to_json(value));
            }
            Json::Object(object)
        }
    }
}

fn node_similarity(node: &Node, sim: &crate::lang::Similarity) -> Option<f64> {
    match node.props.get(&sim.field) { Some(Value::Vector(v)) => v.score(&sim.query), _ => None }
}
fn similarity_json(node: &Node, sim: &crate::lang::Similarity) -> Json { node_similarity(node,sim).map_or(Json::Null, |s| json!(s)) }
fn score_json(node: &Node, sel: &Selection) -> Json { sel.near.as_ref().map_or(Json::Null, |n| similarity_json(node,&n.similarity)) }
fn json_to_prop(schema: &Schema, sel: &Selection, field: &str, value: &Json) -> Result<Value, LangError> {
    if !value.is_null() {
        if let Ok(crate::lang::Field::Prop { ty, .. }) = schema.prop(&sel.type_name, field) {
            if let Some(spec) = VectorSpec::parse(ty) { return spec.value(value).map(Value::Vector).map_err(|m| LangError::at(sel.type_span,m)); }
        }
    }
    json_to_value(value)
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
        Json::Object(_) => Point::from_json(value)
            .map(Value::Point)
            .map_err(LangError::bare),
        Json::Array(_) => Vector::from_json(value, Metric::Cosine).map(Value::Vector).map_err(LangError::bare),
    }
}

/// What one statement may still spend: relationships read, against the
/// traversal budget, and, when the host set a query time limit, time. The
/// clock is read as the work is done, so a statement over its time stops
/// where it is and its writes roll back; it does not run on unseen after its
/// caller has given up. With no limit the clock is never read, which keeps
/// wasm32 (where there is no clock to read) out of it.
pub(crate) struct Work {
    left: usize,
    deadline: Option<Deadline>,
}

struct Deadline {
    at: Instant,
    steps: u32,
    expired: bool,
}

/// Steps between readings of the clock. A step is one row tested, projected,
/// sorted or written, or one relationship read: far under a millisecond.
const STEPS_PER_CLOCK_READ: u32 = 256;

impl Work {
    pub(crate) fn new(relationships: usize, limit: Option<Duration>) -> Self {
        Work {
            left: relationships,
            deadline: limit.map(|limit| Deadline {
                at: Instant::now() + limit,
                steps: 0,
                expired: false,
            }),
        }
    }

    /// Read `n` relationships: spend them from the budget, and take a step.
    pub(crate) fn charge(&mut self, n: usize) -> Result<(), LangError> {
        if self.left < n {
            return Err(LangError::bare(
                "relationship traversal work budget exceeded",
            ));
        }
        self.left -= n;
        self.step()
    }

    /// One unit of work. Fails once the deadline has passed.
    pub(crate) fn step(&mut self) -> Result<(), LangError> {
        let Some(deadline) = &mut self.deadline else {
            return Ok(());
        };
        deadline.steps += 1;
        if deadline.steps >= STEPS_PER_CLOCK_READ {
            deadline.steps = 0;
            deadline.expired = Instant::now() >= deadline.at;
        }
        if deadline.expired {
            return Err(LangError::bare("query time limit exceeded"));
        }
        Ok(())
    }

    /// Whether this statement stopped because it ran out of time.
    pub(crate) fn expired(&self) -> bool {
        self.deadline.as_ref().is_some_and(|deadline| deadline.expired)
    }
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

    const UNIQUE_SCHEMA: &str = r#"
        schema {
          type Author {
            name: String
            wrote -> Book[]
          }
          type Book { title: String pages: Int }
        }
        unique { Author { name } Book { title } }
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
                      manages *1..3 -> Person { name @hops }
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
    fn lookup_one_unique_index_matches_scan_for_seeded_data() {
        let uniques = vec![("Person".to_string(), "name".to_string())];
        for seed in 1..=16_u64 {
            let mut graph = Graph::new();
            let mut state = seed;
            let mut selected = Vec::new();
            for index in 0..100_u64 {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                let name = format!("person-{seed}-{index}");
                let age = ((state >> 32) % 100) as i64;
                let id = graph.create_node(
                    vec!["Person".into()],
                    HashMap::from([
                        ("name".into(), Value::String(name.clone())),
                        ("age".into(), Value::Int(age)),
                    ]),
                );
                if index % 3 == (seed % 3) {
                    selected.push((name, id));
                }
            }
            for (name, expected) in selected {
                let source = format!(
                    "mutation {{ Person(name = {name:?} && age >= 0) set age: 0 {{ name }} }}"
                );
                let Statement::Run(query) = crate::lang::parse_statement(&source).unwrap() else {
                    panic!("expected mutation statement");
                };
                let selection = query.root.unwrap();
                let mut work = Work::new(usize::MAX, None);
                let mut scan = candidates(&graph, &selection, &mut work).unwrap();
                scan.retain(|id| node_matches(&graph, *id, selection.condition.as_ref()));
                assert_eq!(scan, vec![expected]);
                assert_eq!(
                    unique_candidates(&graph, &selection, &uniques),
                    Some(scan.clone())
                );
                assert_eq!(
                    lookup_one(&graph, &selection, &uniques, &mut work).unwrap(),
                    scan[0]
                );
            }
        }
    }

    #[test]
    fn link_and_set_reject_ambiguous_lookups() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        let zega = Zega::open(path).wal_flush_every_write().build().unwrap();
        zega.run_lang(SCHEMA, r#"mutation { Book(title: "Twin") { title } }"#)
            .unwrap();
        zega.run_lang(SCHEMA, r#"mutation { Book(title: "Twin") { title } }"#)
            .unwrap();
        let missing_author = zega.run_lang(
            SCHEMA,
            r#"mutation { Author(name: "A") { wrote -> link Book(title: "Twin") { title } } }"#,
        );
        assert!(missing_author
            .unwrap_err()
            .to_string()
            .contains("no Author matched"));

        zega.run_lang(SCHEMA, r#"mutation { Author(name: "A") { name } }"#)
            .unwrap();
        let link = zega
            .run_lang(
                SCHEMA,
                r#"mutation { Author(name: "A") { wrote -> link Book(title: "Twin") { title } } }"#,
            )
            .unwrap_err();
        let set = zega
            .run_lang(
                SCHEMA,
                r#"mutation { Book(title: "Twin") set pages: 1 { title } }"#,
            )
            .unwrap_err();
        let unique_link = zega
            .run_lang(
                UNIQUE_SCHEMA,
                r#"mutation { Author(name: "A") { wrote -> link Book(title: "Twin") { title } } }"#,
            )
            .unwrap_err();
        let unique_set = zega
            .run_lang(
                UNIQUE_SCHEMA,
                r#"mutation { Book(title: "Twin") set pages: 1 { title } }"#,
            )
            .unwrap_err();
        assert!(link.to_string().contains("Book matched 2 rows"), "{link}");
        assert!(set.to_string().contains("Book matched 2 rows"), "{set}");
        assert!(
            unique_link.to_string().contains("Book matched 2 rows"),
            "{unique_link}"
        );
        assert!(
            unique_set.to_string().contains("Book matched 2 rows"),
            "{unique_set}"
        );
        assert!(zega.graph_json().unwrap()["rels"]
            .as_array()
            .unwrap()
            .is_empty());
        let graph = zega.graph_json().unwrap();
        let books: Vec<_> = graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|node| node["title"] == "Twin")
            .collect();
        assert_eq!(books.len(), 2);
        assert!(books.iter().all(|book| book["pages"].is_null()));
        let before = zega.graph_json().unwrap();
        drop(zega);
        let reopened = Zega::open(path).wal_flush_every_write().build().unwrap();
        assert_eq!(reopened.graph_json().unwrap(), before);
    }

    #[test]
    fn link_and_set_zero_and_one_match_with_and_without_unique() {
        let plain = Zega::in_memory().build().unwrap();
        let missing_link = plain.run_lang(
            SCHEMA,
            r#"mutation { Author(name: "A") { wrote -> link Book(title: "Twin") { title } } }"#,
        );
        assert!(missing_link
            .unwrap_err()
            .to_string()
            .contains("no Author matched"));
        plain
            .run_lang(SCHEMA, r#"mutation { Author(name: "A") { name } }"#)
            .unwrap();
        let missing_target = plain.run_lang(
            SCHEMA,
            r#"mutation { Author(name: "A") { wrote -> link Book(title: "Twin") { title } } }"#,
        );
        assert!(missing_target
            .unwrap_err()
            .to_string()
            .contains("no Book matched"));
        let missing_set = plain.run_lang(
            SCHEMA,
            r#"mutation { Book(title: "Twin") set pages: 1 { title } }"#,
        );
        assert!(missing_set
            .unwrap_err()
            .to_string()
            .contains("no Book matched"));
        plain
            .run_lang(SCHEMA, r#"mutation { Book(title: "Twin") { title } }"#)
            .unwrap();
        assert_eq!(
            plain
                .run_lang(
                    SCHEMA,
                    r#"mutation { Author(name: "A") { wrote -> link Book(title: "Twin") { title } } }"#,
                )
                .unwrap()["wrote"][0]["title"],
            "Twin"
        );
        assert_eq!(
            plain
                .run_lang(
                    SCHEMA,
                    r#"mutation { Book(title: "Twin") set pages: 1 { title pages } }"#,
                )
                .unwrap()["pages"],
            1
        );

        let unique = Zega::in_memory().build().unwrap();
        let missing_unique_link = unique.run_lang(
            UNIQUE_SCHEMA,
            r#"mutation { Author(name: "A") { wrote -> link Book(title: "Twin") { title } } }"#,
        );
        assert!(missing_unique_link
            .unwrap_err()
            .to_string()
            .contains("no Author matched"));
        let missing_unique_set = unique.run_lang(
            UNIQUE_SCHEMA,
            r#"mutation { Book(title: "Twin") set pages: 1 { title } }"#,
        );
        assert!(missing_unique_set
            .unwrap_err()
            .to_string()
            .contains("no Book matched"));
        unique
            .run_lang(UNIQUE_SCHEMA, r#"mutation { Author(name: "A") { name } }"#)
            .unwrap();
        unique
            .run_lang(
                UNIQUE_SCHEMA,
                r#"mutation { Book(title: "Twin") { title } }"#,
            )
            .unwrap();
        assert_eq!(
            unique
                .run_lang(
                    UNIQUE_SCHEMA,
                    r#"mutation { Author(name: "A") { wrote -> link Book(title: "Twin") { title } } }"#,
                )
                .unwrap()["wrote"][0]["title"],
            "Twin"
        );
        assert_eq!(
            unique
                .run_lang(
                    UNIQUE_SCHEMA,
                    r#"mutation { Book(title: "Twin") set pages: 1 { title pages } }"#,
                )
                .unwrap()["pages"],
            1
        );
    }

    #[test]
    fn single_valued_relationship_rejects_second_target_on_every_write_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        let zega = Zega::open(path).wal_flush_every_write().build().unwrap();
        let schema = r#"
            schema {
            type Author { name: String favorite -> Book }
            type Book { title: String }
            }
        "#;
        zega.apply_zql(schema).unwrap();
        zega.run_lang(schema, r#"mutation { Author(name: "A") { name } }"#)
            .unwrap();
        zega.run_lang(schema, r#"mutation { Book(title: "First") { title } }"#)
            .unwrap();
        zega.run_lang(schema, r#"mutation { Book(title: "Second") { title } }"#)
            .unwrap();
        let graph = zega.graph_json().unwrap();
        let author = graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["name"] == "A")
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        let books: Vec<_> = graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|node| node["title"].is_string())
            .map(|node| node["id"].as_u64().unwrap())
            .collect();
        zega.connect_schema(schema, author, "favorite", books[0])
            .unwrap();
        let before = zega.graph_json().unwrap();

        let direct = zega
            .connect_schema(schema, author, "favorite", books[1])
            .unwrap_err();
        assert!(direct.to_string().contains("Author.favorite"), "{direct}");
        for target in &books {
            assert!(direct.to_string().contains(&format!("Book#{target}")), "{direct}");
        }
        assert!(
            direct.to_string().contains("unlink the current one first"),
            "{direct}"
        );

        let link = zega
            .run_lang(
                schema,
                r#"mutation {
            Author(name: "A") { favorite -> link Book(title: "First") { title } }
        }"#,
            )
            .unwrap_err();
        assert!(link.to_string().contains("Author.favorite"), "{link}");
        assert!(link.to_string().contains("query:2:"), "{link}");
        assert!(
            link.to_string().contains("unlink the current one first"),
            "{link}"
        );
        assert_eq!(zega.graph_json().unwrap(), before);

        let created = zega
            .run_lang(
                schema,
                r#"mutation {
            Author(name: "New") {
              favorite -> Book(title: "Third") { title }
              favorite -> Book(title: "Fourth") { title }
            }
        }"#,
            )
            .unwrap_err();
        assert!(created.to_string().contains("Author.favorite"), "{created}");
        assert_eq!(zega.graph_json().unwrap(), before);
        drop(zega);
        let reopened = Zega::open(path).wal_flush_every_write().build().unwrap();
        assert_eq!(reopened.graph_json().unwrap(), before);
    }

    #[test]
    fn single_valued_relationship_loads_are_atomic_for_json_and_csv() {
        let schema = r#"schema {
          type Author { name: String favorite -> Book }
          type Book { title: String }
        }"#;
        for (format, data) in [
            (
                "json",
                r#"[{"author":"A","title":"First"},{"author":"A","title":"Second"}]"#,
            ),
            ("csv", "author,title\nA,First\nA,Second\n"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().to_str().unwrap();
            let zega = Zega::open(path).wal_flush_every_write().build().unwrap();
            zega.apply_zql(schema).unwrap();
            let schema_body = r#"type Author { name: String favorite -> Book } type Book { title: String }"#;
            zega.run_lang(schema_body, r#"mutation { Author(name: "A") { name } }"#)
                .unwrap();
            let before = zega.graph_json().unwrap();
            let document = format!(
                "{schema}\nmutation {format} [\"rows.{format}\"] {{ Author(name: $author) set name: $author {{ favorite -> Book(title: $title) {{ title }} }} }}"
            );
            let err = zega
                .apply_zql_with_sources(
                    &document,
                    &HashMap::from([(format!("rows.{format}"), data.to_string())]),
                )
            .unwrap_err();
            assert!(err.to_string().contains("Author.favorite"), "{err}");
            assert_eq!(zega.graph_json().unwrap(), before);
            drop(zega);
            let reopened = Zega::open(path).wal_flush_every_write().build().unwrap();
            assert_eq!(reopened.graph_json().unwrap(), before);
        }
    }

    #[test]
    fn single_valued_read_rejects_legacy_raw_graph_state() {
        let zega = Zega::in_memory().build().unwrap();
        let schema = r#"type Author { name: String favorite -> Book } type Book { title: String }"#;
        zega.run_lang(schema, r#"mutation { Author(name: "A") { name } }"#)
            .unwrap();
        zega.run_lang(schema, r#"mutation { Book(title: "One") { title } }"#)
            .unwrap();
        zega.run_lang(schema, r#"mutation { Book(title: "Two") { title } }"#)
            .unwrap();
        let graph = zega.graph_json().unwrap();
        let author = graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["name"] == "A")
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        let books: Vec<_> = graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|n| n["title"].is_string())
            .map(|n| n["id"].as_u64().unwrap())
            .collect();
        {
            let mut raw = zega.graph.lock().unwrap();
            raw.create_relationship("favorite".into(), author, books[0], HashMap::new());
            raw.create_relationship("favorite".into(), author, books[1], HashMap::new());
        }
        let error = zega
            .run_lang(
                schema,
                r#"{ Author(name = "A") { favorite -> Book { title } } }"#,
            )
            .unwrap_err();
        assert!(error.to_string().contains("Author.favorite"), "{error}");
        assert!(error.to_string().contains("Book#"), "{error}");

        let ranged_error = zega
            .run_lang(
                schema,
                r#"{ Author(name = "A") { favorite *1..1 -> Book { title } } }"#,
            )
            .unwrap_err();
        assert!(
            ranged_error.to_string().contains("Author.favorite"),
            "{ranged_error}"
        );
        assert!(
            ranged_error
                .to_string()
                .contains(&format!("Author#{author}")),
            "{ranged_error}"
        );
        for target in &books {
            assert!(
                ranged_error
                    .to_string()
                    .contains(&format!("Book#{target}")),
                "{ranged_error}"
            );
        }
        assert!(
            ranged_error
                .to_string()
                .contains("unlink the current one first"),
            "{ranged_error}"
        );
    }

    #[test]
    fn ranged_single_valued_walk_checks_every_node_in_the_walk() {
        let zega = Zega::in_memory().build().unwrap();
        let schema = r#"type Person { name: String parent -> Person }"#;
        zega.run_lang(
            schema,
            r#"mutation {
                Person(name: "A") {
                  parent -> Person(name: "B") {
                    parent -> Person(name: "C") {
                      parent -> Person(name: "D") { name }
                    }
                  }
                }
            }"#,
        )
        .unwrap();
        let clean = zega
            .run_lang(
                schema,
                r#"{ Person(name = "A") { parent *1..3 -> Person { name } } }"#,
            )
            .unwrap();
        let chain = clean["parent"].as_array().unwrap();
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0]["name"], "B");
        assert_eq!(chain[1]["name"], "C");
        assert_eq!(chain[2]["name"], "D");

        zega.run_lang(schema, r#"mutation { Person(name: "E") { name } }"#)
            .unwrap();
        let graph = zega.graph_json().unwrap();
        let nodes = graph["nodes"].as_array().unwrap();
        let b_id = nodes.iter().find(|node| node["name"] == "B").unwrap()["id"]
            .as_u64()
            .unwrap();
        let c_id = nodes.iter().find(|node| node["name"] == "C").unwrap()["id"]
            .as_u64()
            .unwrap();
        let e_id = nodes.iter().find(|node| node["name"] == "E").unwrap()["id"]
            .as_u64()
            .unwrap();
        {
            let mut raw = zega.graph.lock().unwrap();
            raw.create_relationship("parent".into(), b_id, e_id, HashMap::new());
        }

        let error = zega
            .run_lang(
                schema,
                r#"{ Person(name = "A") { parent *1..3 -> Person { name } } }"#,
            )
            .unwrap_err();
        assert!(
            error.to_string().contains(&format!("Person#{b_id}")),
            "{error}"
        );
        assert!(
            error.to_string().contains(&format!("Person#{c_id}")),
            "{error}"
        );
        assert!(
            error.to_string().contains(&format!("Person#{e_id}")),
            "{error}"
        );
        assert!(error.to_string().contains("Person.parent"), "{error}");
        assert!(
            error.to_string().contains("unlink the current one first"),
            "{error}"
        );
    }

    #[test]
    fn many_relationship_still_accepts_multiple_targets() {
        let zega = Zega::in_memory().build().unwrap();
        let schema =
            r#"type Author { name: String favorites -> Book[] } type Book { title: String }"#;
        let result = zega
            .run_lang(
                schema,
                r#"mutation {
            Author(name: "A") {
              favorites -> Book(title: "One") { title }
              favorites -> Book(title: "Two") { title }
            }
        }"#,
            )
            .unwrap();
        assert_eq!(result["favorites"].as_array().unwrap().len(), 2);
        assert_eq!(zega.graph_json().unwrap()["rels"].as_array().unwrap().len(), 2);
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
