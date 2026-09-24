//! The v2 schema and query language. Users write this. The engine walks the
//! graph it already stores; this crate does not parse ZQL.

mod discovery;
pub(crate) use discovery::{check_pipeline, DiscoveryExpr, Primitive, TextOp};
use discovery::ThenStage;

pub use crate::index::{IndexKind, IndexSpec};
use crate::location::{Bounds, Point};
use crate::vector::{Vector, VectorSpec, Metric};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use thiserror::Error;
use crate::validation::{closest, render};

pub use crate::validation::{Diagnostic, Pane, Report};

/// A source range. Columns are 1-based and count UTF-16 code units, which is
/// what the editor uses. `end_column` is exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Span {
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
}

#[derive(Debug, Error, PartialEq)]
#[error("{message}")]
pub struct Error {
    pub message: String,
    pub help: Option<String>,
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
}

impl Error {
    /// An error that is not tied to a source location. `line == 0`.
    pub fn bare(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            help: None,
            line: 0,
            column: 0,
            end_line: 0,
            end_column: 0,
        }
    }

    pub fn at(span: Span, message: impl Into<String>) -> Self {
        let end_column = if span.end_line == span.line && span.end_column <= span.column {
            span.column.saturating_add(1)
        } else {
            span.end_column
        };
        Self {
            message: message.into(),
            help: None,
            line: span.line,
            column: span.column,
            end_line: span.end_line,
            end_column,
        }
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Schema {
    pub types: Vec<TypeDef>,
    pub display: DisplayConfig,
}

/// The ordered, explicit view contract. Absence of a block means graph only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayConfig {
    pub views: Vec<DisplayView>,
    pub default: ViewKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayView {
    pub kind: ViewKind,
    /// None means all types; a declared list is always nonempty.
    pub types: Option<Vec<String>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ViewKind {
    Graph,
    Table,
    Map,
    Timeline,
    Vector2d,
    Vector3d,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self { views: vec![DisplayView { kind: ViewKind::Graph, types: None }], default: ViewKind::Graph }
    }
}

struct DisplayEntry {
    view: DisplayView,
    span: Span,
    type_spans: Vec<Span>,
    default_span: Option<Span>,
}

struct DisplayBlock {
    entries: Vec<DisplayEntry>,
    span: Span,
}

fn check_display(schema: &Schema, block: DisplayBlock) -> Result<DisplayConfig> {
    if block.entries.is_empty() {
        return Err(Error::at(block.span, "display block is empty")
            .with_help("list at least one view, e.g. `display { graph }`"));
    }
    let mut default = None;
    let mut views: Vec<DisplayView> = Vec::new();
    for entry in block.entries {
        let kind = entry.view.kind;
        if views.iter().any(|view| view.kind == kind) {
            return Err(Error::at(entry.span, "duplicate display view")
                .with_help("list each view once"));
        }
        if let Some(span) = entry.default_span {
            if default.is_some() {
                return Err(Error::at(span, "display has more than one Default")
                    .with_help("mark only one view `: Default`, or omit it to start on the first view"));
            }
            default = Some(kind);
        }
        let requirement = match kind {
            ViewKind::Map => Some(("map", "coordinates", "`lat: Float` and `lon: Float`")),
            ViewKind::Timeline => Some(("timeline", "a year/date field", "`year: Int` or `date: String`")),
            ViewKind::Vector2d => Some(("vector2d", "a Vector field", "`embedding: Vector<384>`")),
            ViewKind::Vector3d => Some(("vector3d", "a Vector field", "`embedding: Vector<384>`")),
            ViewKind::Graph | ViewKind::Table => None,
        };
        let eligible = |ty: &TypeDef| match kind {
            ViewKind::Map => ty.fields.iter().any(|field| {
                matches!(field, Field::Prop { ty, .. } if ty == "Point")
            }) || ["lat", "lon"].iter().all(|coordinate| {
                ty.fields.iter().any(|field| {
                    matches!(field, Field::Prop { name, ty, .. } if name == coordinate && ty == "Float")
                })
            }),
            ViewKind::Timeline => ty.timeline_field.is_some(),
            ViewKind::Vector2d | ViewKind::Vector3d => ty.fields.iter().any(|f| matches!(f, Field::Prop { ty, .. } if VectorSpec::parse(ty).is_some())),
            ViewKind::Graph | ViewKind::Table => true,
        };
        if let Some(names) = &entry.view.types {
            for (name, span) in names.iter().zip(&entry.type_spans) {
                let ty = schema.types.iter().find(|ty| ty.name == *name).ok_or_else(||
                    Error::at(*span, format!("display refers to unknown type {name}"))
                        .with_help(type_help(schema, name)))?;
                if !eligible(ty) {
                    let (view, needs, fields) = requirement.unwrap();
                    return Err(Error::at(*span, format!("display `{view}` needs {needs} on type {name}"))
                        .with_help(format!("add {fields} to {name}, or remove {name} from this view")));
                }
            }
        } else if matches!(kind, ViewKind::Vector2d | ViewKind::Vector3d) {
            if let Some(ty) = schema.types.iter().find(|ty| !eligible(ty)) {
                let (view, needs, fields) = requirement.unwrap();
                return Err(Error::at(ty.span, format!("display `{view}` needs {needs} on type {}", ty.name))
                    .with_help(format!("add {fields} to {}, or list only vector types in this view", ty.name)));
            }
        } else if !schema.types.iter().any(eligible) {
            let (view, needs, fields) = requirement.unwrap();
            return Err(Error::at(entry.span, format!("display `{view}` needs {needs}, and no type has them"))
                .with_help(format!("add {fields} to a type, e.g. {}", schema.types[0].name)));
        }
        views.push(entry.view);
    }
    Ok(DisplayConfig { default: default.unwrap_or(views[0].kind), views })
}

/// Timeline conventions use existing scalar types; dates are ISO date strings.
fn timeline_field(fields: &[Field]) -> Option<&str> {
    fields.iter().find_map(|field| match field {
        Field::Prop { name, ty, .. } if (name == "year" && ty == "Int") || (name == "date" && ty == "String") => Some(name.as_str()),
        _ => None,
    })
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TypeDef {
    pub name: String,
    pub span: Span,
    pub fields: Vec<Field>,
    pub timeline_field: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Field {
    Prop {
        name: String,
        ty: String,
        optional: bool,
        /// Explicit source column names, latitude then longitude, for loads.
        from: Option<Vec<String>>,
        /// `Float<km>`: the distance unit a number is measured in.
        unit: Option<DistanceUnit>,
    },
    Edge {
        /// Name used in a query.
        field: String,
        /// Stored relationship kind.
        rel: String,
        direction: Direction,
        targets: Vec<String>,
        target_spans: Vec<Span>,
        many: bool,
        /// Fields of the relationship record, such as `years: Int`.
        props: Vec<EdgeField>,
        /// Set when this side wrote the `{ ... }` block.
        props_span: Option<Span>,
    },
}

/// A field stored on the relationship, not on either node.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EdgeField {
    pub name: String,
    pub ty: String,
    pub optional: bool,
    pub span: Span,
    /// `km: Float<km>`: the distance unit, which A* reads.
    pub unit: Option<DistanceUnit>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Out,
    In,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    pub mutation: bool,
    pub skip: bool,
    pub then: Vec<ThenStage>,
    /// None when the block is empty: `query { }`.
    pub root: Option<Selection>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    pub type_name: String,
    pub type_span: Span,
    /// Extra types when the query wrote `(Book | Movie)`.
    pub also: Vec<String>,
    pub also_spans: Vec<Span>,
    /// The condition in parentheses. `&&` is and, `||` is or, `!=` is not equal.
    pub condition: Option<BoolExpr>,
    pub sets: Vec<(String, Json, Span)>,
    pub near: Option<Near>,
    pub order: Option<Distance>,
    pub limit: Option<usize>,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Similarity { pub field: String, pub query: Vector, pub span: Span }
#[derive(Clone, Debug, PartialEq)]
pub struct Near { pub similarity: Similarity, pub k: usize, pub exact: bool }

#[derive(Clone, Debug, PartialEq)]
pub struct Distance {
    pub field: String,
    pub origin: Point,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Score(String, Span),
    Similarity(String, Similarity),
    Distance(String, Distance),
    Prop(String, Span),
    Hops(String),
    Id(String),
    EdgeProp(String, Span),
    /// `&year: 1974` on a mutation stores `year` on the edge that arrived here.
    EdgeSet(String, Json, Span),
    Walk {
        field: String,
        span: Span,
        range: Option<(usize, usize)>,
        /// `*path`: one route to the target instead of every node in reach.
        path: Option<PathSpec>,
        link: bool,
        direction: Direction,
        target: Box<Selection>,
    },
}

/// `road *path(@cost <= 50) by &km toward at in km -> Junction(...)`.
#[derive(Clone, Debug, PartialEq)]
pub struct PathSpec {
    /// The `*path` word.
    pub span: Span,
    pub bound: Option<(PathBound, Span)>,
    /// `by &km`: the edge field summed along the route. None counts edges.
    pub weight: Option<(String, Span)>,
    /// `toward at in km`: A*, guided by this Point field.
    pub toward: Option<Toward>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathBound {
    /// At most this many edges.
    Hops(usize),
    /// A route costing at most (or, not inclusive, under) this much.
    Cost { limit: f64, inclusive: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Toward {
    pub field: String,
    pub span: Span,
}

/// The distance unit of an `Int` or `Float` field, declared in its type:
/// `Float<km>`. A* reads it from the weight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum DistanceUnit {
    #[serde(rename = "m")]
    Metres,
    #[serde(rename = "km")]
    Kilometres,
    #[serde(rename = "mi")]
    Miles,
}

impl DistanceUnit {
    pub fn metres(self) -> f64 {
        match self {
            DistanceUnit::Metres => 1.0,
            DistanceUnit::Kilometres => 1000.0,
            DistanceUnit::Miles => 1609.344,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            DistanceUnit::Metres => "m",
            DistanceUnit::Kilometres => "km",
            DistanceUnit::Miles => "mi",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        match name {
            "m" => Some(DistanceUnit::Metres),
            "km" => Some(DistanceUnit::Kilometres),
            "mi" => Some(DistanceUnit::Miles),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Pred {
    Similarity(Similarity, Cmp, f64),
    Distance(Distance, Cmp, f64),
    Box(String, Bounds, Span),
    Eq(String, Json, Span),
    Ne(String, Json, Span),
    Cmp(String, Cmp, Json, Span),
    Contains(String, String, Span),
    StartsWith(String, String, Span),
    EndsWith(String, String, Span),
}

/// A condition, read like the test in an `if`.
#[derive(Clone, Debug, PartialEq)]
pub enum BoolExpr {
    Test(Pred),
    And(Box<BoolExpr>, Box<BoolExpr>),
    Or(Box<BoolExpr>, Box<BoolExpr>),
}

impl BoolExpr {
    pub fn tests(&self) -> Vec<&Pred> {
        let mut out = Vec::new();
        self.collect_tests(&mut out);
        out
    }

    fn collect_tests<'a>(&'a self, out: &mut Vec<&'a Pred>) {
        match self {
            BoolExpr::Test(pred) => out.push(pred),
            BoolExpr::And(left, right) | BoolExpr::Or(left, right) => {
                left.collect_tests(out);
                right.collect_tests(out);
            }
        }
    }

    /// True when the condition is only `&&` of equalities, so a root read
    /// still returns one object.
    pub fn is_equality_and(&self) -> bool {
        match self {
            BoolExpr::Test(Pred::Eq(_, _, _)) => true,
            BoolExpr::And(left, right) => left.is_equality_and() && right.is_equality_and(),
            BoolExpr::Or(_, _) | BoolExpr::Test(_) => false,
        }
    }

    pub fn span(&self) -> Span {
        match self {
            BoolExpr::Test(pred) => pred.span(),
            BoolExpr::And(left, _) | BoolExpr::Or(left, _) => left.span(),
        }
    }
}

impl Pred {
    pub fn field(&self) -> &str {
        match self {
            Pred::Similarity(sim, ..) => &sim.field,
            Pred::Distance(distance, ..) => &distance.field,
            Pred::Box(field, ..) => field,
            Pred::Eq(field, _, _)
            | Pred::Ne(field, _, _)
            | Pred::Cmp(field, _, _, _)
            | Pred::Contains(field, _, _)
            | Pred::StartsWith(field, _, _)
            | Pred::EndsWith(field, _, _) => field,
        }
    }

    pub fn span(&self) -> Span {
        match self {
            Pred::Similarity(sim, ..) => sim.span,
            Pred::Distance(distance, ..) => distance.span,
            Pred::Box(_, _, span) => *span,
            Pred::Eq(_, _, span)
            | Pred::Ne(_, _, span)
            | Pred::Cmp(_, _, _, span)
            | Pred::Contains(_, _, span)
            | Pred::StartsWith(_, _, span)
            | Pred::EndsWith(_, _, span) => *span,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmp {
    Gt,
    Lt,
    Gte,
    Lte,
}

impl Schema {
    pub fn get(&self, name: &str) -> Result<&TypeDef> {
        self.types
            .iter()
            .find(|ty| ty.name == name)
            .ok_or_else(|| Error::bare(format!("unknown type {name}")))
    }

    pub fn edge<'a>(&'a self, type_name: &str, field: &str) -> Result<&'a Field> {
        let ty = self.get(type_name)?;
        ty.fields
            .iter()
            .find(|f| matches!(f, Field::Edge { field: name, .. } if name == field))
            .ok_or_else(|| Error::bare(format!("{type_name} has no relationship {field}")))
    }

    pub fn prop<'a>(&'a self, type_name: &str, field: &str) -> Result<&'a Field> {
        let ty = self.get(type_name)?;
        ty.fields
            .iter()
            .find(|f| matches!(f, Field::Prop { name, .. } if name == field))
            .ok_or_else(|| Error::bare(format!("{type_name} has no field {field}")))
    }
}

impl Field {
    pub fn as_edge(&self) -> Option<(&str, &str, Direction, &[String], bool)> {
        match self {
            Field::Edge {
                field,
                rel,
                direction,
                targets,
                many,
                ..
            } => Some((field, rel, *direction, targets, *many)),
            Field::Prop { .. } => None,
        }
    }
}

pub fn parse_schema(source: &str) -> Result<Schema> {
    Ok(parse_schema_at(source)?.0)
}

/// Types, and the byte offset just after them. A following `unique`, `mutation`,
/// or `query` block is left for the caller.
fn parse_schema_at(source: &str) -> Result<(Schema, usize)> {
    let mut p = Parser::new(source);
    let mut types = Vec::new();
    let mut display = None;
    p.skip();
    let wrapped = p.eat_word("schema");
    if wrapped {
        p.expect("{")?;
    }
    loop {
        p.skip();
        if wrapped {
            if p.eat("}") {
                break;
            }
        } else if p.eof()
            || p.starts_word("unique")
            || p.starts_word("index")
            || p.starts_word("mutation")
            || p.starts_word("query")
        {
            break;
        }
        if p.starts_word("display") {
            if display.is_some() {
                return Err(p.err("duplicate display block").with_help("a schema has one display block"));
            }
            display = Some(p.parse_display()?);
            continue;
        }
        p.expect_word("type")?;
        let (name, span) = p.ident()?;
        p.expect("{")?;
        let mut fields = Vec::new();
        while !p.eat("}") {
            fields.push(p.parse_field()?);
            p.skip();
        }
        if types.iter().any(|ty: &TypeDef| ty.name == name) {
            return Err(p
                .err_at(span, format!("duplicate type {name}"))
                .with_help("a schema names each type once"));
        }
        let timeline_field = timeline_field(&fields).map(str::to_owned);
        types.push(TypeDef { name, span, fields, timeline_field });
    }
    if types.is_empty() {
        return Err(p
            .err("schema has no types")
            .with_help("start with `type Name { }`"));
    }
    let mut schema = Schema { types, display: DisplayConfig::default() };
    for ty in &schema.types {
        for field in &ty.fields {
            let Field::Edge {
                targets,
                target_spans,
                field,
                ..
            } = field
            else {
                continue;
            };
            for (target, span) in targets.iter().zip(target_spans) {
                if schema.types.iter().all(|other| other.name != *target) {
                    return Err(Error::at(
                        *span,
                        format!("{}.{} points at unknown type {target}", ty.name, field),
                    )
                    .with_help(type_help(&schema, target)));
                }
            }
        }
    }
    unify_edge_props(&mut schema.types)?;
    if let Some(block) = display {
        schema.display = check_display(&schema, block)?;
    }
    Ok((schema, p.i))
}

/// One relationship record has one set of fields. Either side may declare
/// them. Declaring them on both sides means the two lists have to match.
fn unify_edge_props(types: &mut [TypeDef]) -> Result<()> {
    let mut rels = Vec::new();
    for ty in types.iter() {
        for field in &ty.fields {
            if let Field::Edge { rel, .. } = field {
                if !rels.contains(rel) {
                    rels.push(rel.clone());
                }
            }
        }
    }
    for rel in rels {
        let mut canon: Option<(String, Vec<EdgeField>)> = None;
        for ty in types.iter() {
            for field in &ty.fields {
                let Field::Edge {
                    rel: kind,
                    props,
                    props_span,
                    field: field_name,
                    ..
                } = field
                else {
                    continue;
                };
                if kind != &rel || props_span.is_none() {
                    continue;
                }
                if let Some((owner, existing)) = &canon {
                    if !same_edge_props(existing, props) {
                        let span = props_span.unwrap_or(ty.span);
                        return Err(Error::at(span, format!("{rel} fields do not match"))
                            .with_help(format!(
                                "{owner} already declares `{field_name}`. Declare the fields once"
                            )));
                    }
                } else {
                    canon = Some((format!("{}.{}", ty.name, field_name), props.clone()));
                }
            }
        }
        let Some((_, canon)) = canon else {
            continue;
        };
        for ty in types.iter_mut() {
            for field in &mut ty.fields {
                if let Field::Edge {
                    rel: kind, props, ..
                } = field
                {
                    if kind == &rel {
                        *props = canon.clone();
                    }
                }
            }
        }
    }
    Ok(())
}

fn json_matches(ty: &str, value: &Json) -> bool {
    if value
        .as_object()
        .is_some_and(|object| object.contains_key("$column"))
    {
        return true;
    }
    match ty {
        "String" => value.is_string(),
        "Int" => value.as_i64().is_some(),
        "Float" => value.is_number(),
        "Bool" => value.is_boolean(),
        "Point" => Point::from_json(value).is_ok(),
        _ => VectorSpec::parse(ty).is_none_or(|spec| spec.value(value).is_ok()),
    }
}

fn same_edge_props(left: &[EdgeField], right: &[EdgeField]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut left: Vec<_> = left.iter().collect();
    let mut right: Vec<_> = right.iter().collect();
    left.sort_by(|a, b| a.name.cmp(&b.name));
    right.sort_by(|a, b| a.name.cmp(&b.name));
    left.iter()
        .zip(right)
        .all(|(a, b)| a.name == b.name && a.ty == b.ty && a.optional == b.optional && a.unit == b.unit)
}

pub fn parse_query(source: &str) -> Result<Query> {
    match parse_statement(source)? {
        Statement::Run(query) => Ok(query),
        Statement::Load { .. } => {
            let p = Parser::new(source);
            Err(p
                .err("a load runs as its own mutation")
                .with_help("`mutation csv` and `mutation json` are one statement"))
        }
    }
}

/// The single statement in a query pane, including a `csv` or `json` load.
pub fn parse_statement(source: &str) -> Result<Statement> {
    let mut p = Parser::new(source);
    let statement = p.parse_statement()?;
    p.skip();
    if !p.eof() {
        return Err(p.err("unexpected input"));
    }
    Ok(statement)
}

/// A `.zql` file: `schema`, then `unique` and `index`, then `mutation` and
/// `query` blocks.
#[derive(Clone, Debug, PartialEq)]
pub struct ZqlFile {
    pub schema: Schema,
    /// `(type, field)` pairs. Each field is unique on its own.
    pub uniques: Vec<(String, String)>,
    /// The `index { }` block, in the order written.
    pub indexes: Vec<IndexSpec>,
    pub statements: Vec<Statement>,
}

/// One block after the schema. A load keeps its template until something runs
/// it, so checking a file does not read a path or a url.
#[derive(Clone, Debug, PartialEq)]
pub enum Statement {
    Run(Query),
    Load {
        format: LoadFormat,
        /// Paths or `http(s)` urls. Read when the mutation runs.
        locations: Vec<String>,
        template: Query,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadFormat {
    Csv,
    Json,
}

pub fn parse_zql(source: &str) -> Result<ZqlFile> {
    let mut p = Parser::new(source);
    p.skip();
    if !p.eat_word("schema") {
        return Err(p
            .err("expected schema")
            .with_help("a file starts with `schema { }`"));
    }
    let (schema, end) = parse_schema_at(source)?;
    p.i = end;
    let (uniques, indexes) = p.take_blocks(&schema)?;
    let mut statements = Vec::new();
    while !p.eof() {
        p.skip();
        if p.eof() {
            break;
        }
        statements.push(p.parse_statement()?);
    }
    Ok(ZqlFile {
        schema,
        uniques,
        indexes,
        statements,
    })
}

/// Unique fields declared in `source`, or an empty list when the text has no
/// `unique` block. Each name inside a type's braces is unique on its own.
pub fn parse_uniques(source: &str) -> Result<Vec<(String, String)>> {
    Ok(parse_blocks(source)?.0)
}

/// Indexes declared in `source`, or an empty list when the text has no
/// `index` block.
pub fn parse_indexes(source: &str) -> Result<Vec<IndexSpec>> {
    Ok(parse_blocks(source)?.1)
}

/// The `unique` pairs and the `index` block of one text.
type Blocks = (Vec<(String, String)>, Vec<IndexSpec>);

fn parse_blocks(source: &str) -> Result<Blocks> {
    let (schema, end) = parse_schema_at(source)?;
    let mut p = Parser::new(source);
    p.i = end;
    p.take_blocks(&schema)
}

/// Every index the engine keeps for a schema: the `index` block, plus a range
/// index on each unique field whose type can be ordered.
pub fn effective_indexes(
    schema: &Schema,
    uniques: &[(String, String)],
    indexes: &[IndexSpec],
) -> Vec<IndexSpec> {
    let mut out = indexes.to_vec();
    for (type_name, field) in uniques {
        let orderable = matches!(
            schema.prop(type_name, field),
            Ok(Field::Prop { ty, .. }) if orderable(ty)
        );
        let spec = IndexSpec {
            kind: IndexKind::Range,
            type_name: type_name.clone(),
            field: field.clone(),
        };
        if orderable && !out.contains(&spec) {
            out.push(spec);
        }
    }
    out
}

/// Types a range index can order.
fn orderable(ty: &str) -> bool {
    matches!(ty, "Int" | "Float" | "String")
}

struct Parser<'a> {
    src: &'a str,
    i: usize,
    /// `$Name` in a load template reads a column or a JSON key.
    columns: bool,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src,
            i: 0,
            columns: false,
        }
    }

    fn starts_word(&self, word: &str) -> bool {
        let rest = self.src[self.i..].trim_start();
        rest.starts_with(word)
            && rest[word.len()..]
                .chars()
                .next()
                .is_none_or(|ch| !ch.is_ascii_alphanumeric() && ch != '_')
    }

    fn parse_statement(&mut self) -> Result<Statement> {
        self.skip();
        self.reject_detached_discovery()?;
        let mutation = self.eat_word("mutation");
        if mutation && self.eat_word("csv") {
            return self.parse_load(LoadFormat::Csv);
        }
        if mutation && self.eat_word("json") {
            return self.parse_load(LoadFormat::Json);
        }
        if mutation && self.eat_word("query") {
            return Err(self
                .err("a statement is a query or a mutation")
                .with_help("drop one of the words"));
        }
        if !mutation {
            let _ = self.eat_word("query");
        }
        self.columns = false;
        let mut query = self.parse_braced(mutation)?;
        self.take_pipeline(&mut query)?;
        Ok(Statement::Run(query))
    }

    fn parse_load(&mut self, format: LoadFormat) -> Result<Statement> {
        self.skip();
        if self.src[self.i..].starts_with("\"\"\"") {
            return Err(self
                .err("a load reads a file")
                .with_help("write `[\"./data.csv\"]` or `[\"https://...\"]`"));
        }
        if !self.src[self.i..].starts_with(['"', '[']) {
            return Err(self
                .err("a load reads a file")
                .with_help("write `[\"./data.csv\"]` or `[\"https://...\"]`"));
        }
        let value = self.embedded_json()?;
        let locations = file_locations(value).map_err(|message| {
            self.err(message)
                .with_help("write `[\"./data.csv\"]` or `[\"https://...\"]`")
        })?;
        self.columns = true;
        let template = self.parse_braced(true)?;
        self.columns = false;
        Ok(Statement::Load {
            format,
            locations,
            template,
        })
    }

    fn parse_braced(&mut self, mutation: bool) -> Result<Query> {
        self.expect("{")?;
        self.skip();
        if self.eat("}") {
            return Ok(Query {
                mutation,
                skip: false,
                then: Vec::new(),
                root: None,
            });
        }
        let root = self.parse_selection()?;
        self.expect("}")?;
        Ok(Query {
            mutation,
            skip: false,
            then: Vec::new(),
            root: Some(root),
        })
    }

    /// The `unique` and `index` blocks after the types, in either order, at
    /// most one of each.
    fn take_blocks(&mut self, schema: &Schema) -> Result<Blocks> {
        let mut uniques = None;
        let mut indexes = None;
        loop {
            self.skip();
            if self.starts_word("unique") {
                if uniques.is_some() {
                    return Err(self
                        .err("duplicate unique block")
                        .with_help("a file has one unique block"));
                }
                uniques = Some(self.take_uniques(schema)?);
            } else if self.starts_word("index") {
                if indexes.is_some() {
                    return Err(self
                        .err("duplicate index block")
                        .with_help("a file has one index block"));
                }
                indexes = Some(self.take_indexes(schema)?);
            } else {
                break;
            }
        }
        let uniques = uniques.unwrap_or_default();
        let indexes = indexes.unwrap_or_default();
        let mut specs: Vec<IndexSpec> = Vec::new();
        for (spec, span) in indexes {
            if specs.contains(&spec) {
                return Err(self
                    .err_at(
                        span,
                        format!(
                            "{}.{} already has a {} index",
                            spec.type_name,
                            spec.field,
                            spec.kind.as_str()
                        ),
                    )
                    .with_help("name each field once per index kind"));
            }
            if spec.kind == IndexKind::Range
                && uniques
                    .iter()
                    .any(|(ty, field)| ty == &spec.type_name && field == &spec.field)
            {
                return Err(self
                    .err_at(
                        span,
                        format!("{}.{} is already indexed by unique", spec.type_name, spec.field),
                    )
                    .with_help("a unique field already has a range index; remove it from `range`"));
            }
            specs.push(spec);
        }
        Ok((uniques, specs))
    }

    /// `index { range Player { salary } text Player { name } }`. Each field in
    /// the braces gets its own index. Checked against the schema here;
    /// duplicates and unique fields are checked by the caller.
    fn take_indexes(&mut self, schema: &Schema) -> Result<Vec<(IndexSpec, Span)>> {
        self.expect_word("index")?;
        self.expect("{")?;
        let mut specs = Vec::new();
        loop {
            self.skip();
            if self.eat("}") {
                break;
            }
            let (kind_name, kind_span) = self.ident()?;
            let kind = match kind_name.as_str() {
                "range" => IndexKind::Range,
                "text" => IndexKind::Text,
                _ => {
                    return Err(self
                        .err_at(kind_span, format!("unknown index kind {kind_name}"))
                        .with_help("write `range Type { field }` or `text Type { field }`"))
                }
            };
            let (type_name, type_span) = self.ident()?;
            if schema.types.iter().all(|ty| ty.name != type_name) {
                return Err(self
                    .err_at(type_span, format!("unknown type {type_name}"))
                    .with_help(type_help(schema, &type_name)));
            }
            self.expect("{")?;
            loop {
                self.skip();
                if self.eat("}") {
                    break;
                }
                let (field, field_span) = self.ident()?;
                let ty = schema.get(&type_name)?;
                let is_edge = ty
                    .fields
                    .iter()
                    .any(|item| matches!(item, Field::Edge { field: name, .. } if name == &field));
                if is_edge {
                    return Err(self
                        .err_at(field_span, format!("{type_name}.{field} is a relationship"))
                        .with_help("an index applies to a field, such as `name`"));
                }
                let Some(field_ty) = ty.fields.iter().find_map(|item| match item {
                    Field::Prop { name, ty, .. } if name == &field => Some(ty.as_str()),
                    _ => None,
                }) else {
                    return Err(self
                        .err_at(field_span, format!("{type_name} has no field {field}"))
                        .with_help(prop_help(schema, &type_name, &field)));
                };
                match kind {
                    IndexKind::Text if field_ty != "String" => {
                        return Err(self
                            .err_at(
                                field_span,
                                format!("text index needs a String field; {type_name}.{field} is {field_ty}"),
                            )
                            .with_help("`text` speeds findWith, startsWith and endsWith on a String"));
                    }
                    IndexKind::Range if !orderable(field_ty) => {
                        return Err(self
                            .err_at(
                                field_span,
                                format!("range index needs an Int, Float or String field; {type_name}.{field} is {field_ty}"),
                            )
                            .with_help("`range` orders values; Point and Vector fields are indexed already"));
                    }
                    _ => {}
                }
                specs.push((
                    IndexSpec {
                        kind,
                        type_name: type_name.clone(),
                        field,
                    },
                    field_span,
                ));
            }
        }
        Ok(specs)
    }

    /// `(type, field)` pairs. Each field is unique by itself, not as a group.
    fn take_uniques(&mut self, schema: &Schema) -> Result<Vec<(String, String)>> {
        self.skip();
        if !self.eat_word("unique") {
            return Ok(Vec::new());
        }
        self.expect("{")?;
        let mut rules = Vec::new();
        loop {
            self.skip();
            if self.eat("}") {
                break;
            }
            let (type_name, type_span) = self.ident()?;
            if schema.types.iter().all(|ty| ty.name != type_name) {
                return Err(self
                    .err_at(type_span, format!("unknown type {type_name}"))
                    .with_help(type_help(schema, &type_name)));
            }
            self.expect("{")?;
            loop {
                self.skip();
                if self.eat("}") {
                    break;
                }
                let (field, field_span) = self.ident()?;
                let ty = schema.get(&type_name)?;
                let is_edge = ty
                    .fields
                    .iter()
                    .any(|item| matches!(item, Field::Edge { field: name, .. } if name == &field));
                if is_edge {
                    return Err(self
                        .err_at(field_span, format!("{type_name}.{field} is a relationship"))
                        .with_help("unique applies to a field, such as `name`"));
                }
                let is_prop = ty
                    .fields
                    .iter()
                    .any(|item| matches!(item, Field::Prop { name, .. } if name == &field));
                if !is_prop {
                    return Err(self
                        .err_at(field_span, format!("{type_name} has no field {field}"))
                        .with_help(prop_help(schema, &type_name, &field)));
                }
                rules.push((type_name.clone(), field));
            }
        }
        Ok(rules)
    }

    fn embedded_json(&mut self) -> Result<Json> {
        self.skip();
        let rest = &self.src[self.i..];
        let mut stream = serde_json::Deserializer::from_str(rest).into_iter::<Json>();
        let value = stream
            .next()
            .ok_or_else(|| self.err("expected json"))?
            .map_err(|error| self.err(format!("bad json: {error}")))?;
        self.i += stream.byte_offset();
        Ok(value)
    }

    fn eof(&self) -> bool {
        self.i >= self.src.len()
    }

    /// Line and column of a byte offset. The column counts UTF-16 code units.
    fn loc(&self, byte: usize) -> (u32, u32) {
        let byte = byte.min(self.src.len());
        let mut line = 1u32;
        let mut column = 1u32;
        for (i, ch) in self.src.char_indices() {
            if i >= byte {
                break;
            }
            if ch == '\n' {
                line += 1;
                column = 1;
            } else {
                column += ch.len_utf16() as u32;
            }
        }
        (line, column)
    }

    fn span_bytes(&self, start: usize, end: usize) -> Span {
        let start = start.min(self.src.len());
        let end = end.max(start).min(self.src.len());
        let (line, column) = self.loc(start);
        let (end_line, mut end_column) = self.loc(end);
        if end_line == line && end_column <= column {
            end_column = column.saturating_add(1);
        }
        Span {
            line,
            column,
            end_line,
            end_column,
        }
    }

    fn peek_token_end(&self, start: usize) -> usize {
        if start >= self.src.len() {
            return start;
        }
        let rest = &self.src[start..];
        if rest.starts_with("->")
            || rest.starts_with("<-")
            || rest.starts_with(">=")
            || rest.starts_with("<=")
            || rest.starts_with("<>")
            || rest.starts_with("..")
        {
            return start + 2;
        }
        let ch = rest.chars().next().unwrap();
        if ch.is_ascii_alphanumeric() || ch == '_' {
            let mut end = start;
            for next in rest.chars() {
                if next.is_ascii_alphanumeric() || next == '_' {
                    end += next.len_utf8();
                } else {
                    break;
                }
            }
            return end;
        }
        start + ch.len_utf8()
    }

    fn err(&self, message: impl Into<String>) -> Error {
        let start = self.i.min(self.src.len());
        Error::at(self.span_bytes(start, self.peek_token_end(start)), message)
    }

    fn err_at(&self, span: Span, message: impl Into<String>) -> Error {
        Error::at(span, message)
    }

    fn peek_digit(&self) -> bool {
        self.src[self.i..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
    }

    fn skip(&mut self) {
        loop {
            let bytes = &self.src.as_bytes()[self.i..];
            if bytes.is_empty() {
                return;
            }
            if bytes[0].is_ascii_whitespace() {
                self.i += 1;
                continue;
            }
            if bytes.starts_with(b"//") {
                self.i += 2;
                while self.i < self.src.len() && self.src.as_bytes()[self.i] != b'\n' {
                    self.i += 1;
                }
                continue;
            }
            return;
        }
    }

    fn eat(&mut self, token: &str) -> bool {
        self.skip();
        if self.src[self.i..].starts_with(token) {
            let next = self.src[self.i + token.len()..].chars().next();
            if token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && next.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                return false;
            }
            self.i += token.len();
            true
        } else {
            false
        }
    }

    fn eat_word(&mut self, word: &str) -> bool {
        self.eat(word)
    }

    fn expect(&mut self, token: &str) -> Result<()> {
        if self.eat(token) {
            Ok(())
        } else {
            Err(self.err(format!("expected {token}")))
        }
    }

    fn expect_word(&mut self, word: &str) -> Result<()> {
        self.expect(word)
    }

    fn ident(&mut self) -> Result<(String, Span)> {
        self.skip();
        let start = self.i;
        let mut chars = self.src[self.i..].chars();
        let Some(first) = chars.next() else {
            return Err(self.err("expected a name"));
        };
        if !first.is_ascii_alphabetic() && first != '_' {
            return Err(self.err("expected a name"));
        }
        self.i += first.len_utf8();
        while let Some(c) = self.src[self.i..].chars().next() {
            if c.is_ascii_alphanumeric() || c == '_' {
                self.i += c.len_utf8();
            } else {
                break;
            }
        }
        let span = self.span_bytes(start, self.i);
        Ok((self.src[start..self.i].to_string(), span))
    }

    fn starts_call(&self, name: &str, delimiter: &str) -> bool {
        let mut lookahead = Self { src: self.src, i: self.i, columns: self.columns };
        let _ = lookahead.eat("@");
        lookahead.eat_word(name) && lookahead.eat(delimiter)
    }

    fn expect_builtin(&mut self, name: &str) -> Result<()> {
        self.skip();
        if self.starts_word(name) {
            let (_, span) = self.ident()?;
            return Err(self.err_at(span, format!("`{name}` is built in: write `@{name}`")));
        }
        self.expect_word(&format!("@{name}"))
    }

    fn builtin_item(&mut self, alias: Option<String>) -> Result<Item> {
        self.skip();
        let start = self.i;
        let prefixed = self.eat("@");
        let (name, _) = self.ident()?;
        let span = self.span_bytes(start, self.i);
        if !prefixed {
            return Err(self.err_at(span, format!("`{name}` is built in: write `@{name}`")));
        }
        let alias = alias.unwrap_or_else(|| name.clone());
        match name.as_str() {
            "hops" => {
                if self.eat(":") {
                    return Err(self.err_at(span, "@hops is measured, not stored")
                        .with_help("`@hops` counts edges from the start of the query"));
                }
                Ok(Item::Hops(alias))
            }
            "id" => Ok(Item::Id(alias)),
            "score" => Ok(Item::Score(alias, span)),
            "distance" => { self.i = start; Ok(Item::Distance(alias, self.distance()?)) }
            "similarity" => { self.i = start; Ok(Item::Similarity(alias, self.similarity()?)) }
            _ => Err(self.err_at(span, format!("unknown built-in @{name}"))),
        }
    }

    fn parse_display(&mut self) -> Result<DisplayBlock> {
        let (_, span) = self.ident()?;
        self.expect("{")?;
        let mut entries = Vec::new();
        while !self.eat("}") {
            let (name, view_span) = self.ident()?;
            let kind = match name.as_str() {
                "graph" => ViewKind::Graph,
                "table" => ViewKind::Table,
                "map" => ViewKind::Map,
                "timeline" => ViewKind::Timeline,
                "vector2d" => ViewKind::Vector2d,
                "vector3d" => ViewKind::Vector3d,
                _ => return Err(Error::at(view_span, format!("unknown display view {name}"))
                    .with_help("use `graph`, `table`, `map`, `timeline`, `vector2d`, or `vector3d`")),
            };
            let mut type_spans = Vec::new();
            let types = if self.eat("{") {
                if self.eat("}") {
                    return Err(Error::at(view_span, "display type list is empty")
                        .with_help("name at least one type, or omit braces to show all types"));
                }
                let mut names = Vec::new();
                loop {
                    let (name, span) = self.ident()?;
                    names.push(name);
                    type_spans.push(span);
                    if self.eat("}") { break; }
                    self.expect(",")?;
                }
                Some(names)
            } else { None };
            let default_span = if self.eat(":") {
                let (marker, span) = self.ident()?;
                if marker != "Default" {
                    return Err(Error::at(span, "expected Default").with_help("write `: Default` after the view and its type list"));
                }
                Some(span)
            } else { None };
            entries.push(DisplayEntry { view: DisplayView { kind, types }, span: view_span, type_spans, default_span });
        }
        Ok(DisplayBlock { entries, span })
    }

    fn parse_field(&mut self) -> Result<Field> {
        let (name, name_span) = self.ident()?;
        let optional = self.eat("?");
        if self.eat(":") {
            self.skip();
            if self.looks_like_rel_name() {
                let (rel, _) = self.ident()?;
                let (direction, targets, target_spans, many) = self.parse_arrow()?;
                if optional {
                    return Err(self
                        .err_at(
                            name_span,
                            format!("{name} cannot be optional and a relationship"),
                        )
                        .with_help("drop the `?`; a relationship is one record or a list"));
                }
                return self.finish_edge(name, rel, direction, targets, target_spans, many);
            }
            let (mut ty, ty_span) = self.ident()?;
            if ty == "Vector" {
                self.expect("<")?;
                self.skip(); let start = self.i;
                let n = self.integer()?;
                if !(1..=4096).contains(&n) { return Err(self.err_at(self.span_bytes(start,self.i), "Vector dimension must be in 1..=4096")); }
                let metric = if self.eat(",") {
                    let (metric, span) = self.ident()?;
                    if !matches!(metric.as_str(), "cosine" | "dot" | "l2") { return Err(self.err_at(span, "Vector metric must be cosine, dot, or l2")); }
                    metric
                } else { "cosine".into() };
                self.expect(">")?;
                ty = format!("Vector<{n},{metric}>");
            }
            let unit = self.parse_unit(&ty, ty_span)?;
            let from = if self.starts_call("from", "(") {
                self.expect_word("from")?;
                if ty != "Point" && VectorSpec::parse(&ty).is_none() {
                    return Err(self.err_at(name_span, "from (...) is only valid on a Point or Vector field"));
                }
                self.expect("(")?;
                let mut columns = vec![self.ident()?.0];
                while self.eat(",") { columns.push(self.ident()?.0); }
                self.expect(")")?;
                let count = VectorSpec::parse(&ty).map_or(2, |s| s.dimensions);
                if columns.len() != count { return Err(self.err_at(ty_span, format!("{ty} from mapping needs {count} columns"))); }
                Some(columns)
            } else {
                None
            };
            return Ok(Field::Prop {
                name,
                ty,
                optional,
                from,
                unit,
            });
        }
        if optional {
            return Err(self
                .err_at(name_span, format!("{name}? needs a type"))
                .with_help(format!("write `{name}?: String`")));
        }
        let (direction, targets, target_spans, many) = self.parse_arrow()?;
        self.finish_edge(name.clone(), name, direction, targets, target_spans, many)
    }

    fn finish_edge(
        &mut self,
        field: String,
        rel: String,
        direction: Direction,
        targets: Vec<String>,
        target_spans: Vec<Span>,
        many: bool,
    ) -> Result<Field> {
        let (props, props_span) = self.parse_edge_props()?;
        Ok(Field::Edge {
            field,
            rel,
            direction,
            targets,
            target_spans,
            many,
            props,
            props_span,
        })
    }

    /// `<km>` after a type: the distance unit of an `Int` or `Float`.
    fn parse_unit(&mut self, ty: &str, ty_span: Span) -> Result<Option<DistanceUnit>> {
        if !self.eat("<") {
            return Ok(None);
        }
        if ty != "Int" && ty != "Float" {
            return Err(self
                .err_at(ty_span, format!("a unit needs an Int or Float; {ty} has none"))
                .with_help("declare a distance as `km: Float<km>` or `length: Int<m>`"));
        }
        let (name, span) = self.ident()?;
        let unit = DistanceUnit::parse(&name).ok_or_else(|| {
            self.err_at(span, format!("unknown distance unit {name}"))
                .with_help("the unit is `m`, `km` or `mi`, as in `Float<km>`")
        })?;
        self.expect(">")?;
        Ok(Some(unit))
    }

    fn parse_edge_props(&mut self) -> Result<(Vec<EdgeField>, Option<Span>)> {
        self.skip();
        if !self.src[self.i..].starts_with('{') {
            return Ok((Vec::new(), None));
        }
        let start = self.i;
        self.eat("{");
        let mut props: Vec<EdgeField> = Vec::new();
        loop {
            self.skip();
            if self.eat("}") {
                break;
            }
            let (name, span) = self.ident()?;
            let optional = self.eat("?");
            self.expect(":")?;
            let (ty, ty_span) = self.ident()?;
            let unit = self.parse_unit(&ty, ty_span)?;
            if props.iter().any(|field| field.name == name) {
                return Err(self
                    .err_at(span, format!("duplicate edge field {name}"))
                    .with_help("each field of a relationship is named once"));
            }
            props.push(EdgeField {
                name,
                ty,
                optional,
                span,
                unit,
            });
        }
        Ok((props, Some(self.span_bytes(start, self.i))))
    }

    fn looks_like_rel_name(&self) -> bool {
        let mut j = self.i;
        let bytes = &self.src.as_bytes()[j..];
        if bytes.is_empty() || !bytes[0].is_ascii_alphabetic() {
            return false;
        }
        while j < self.src.len() {
            let c = self.src[j..].chars().next().unwrap();
            if c.is_ascii_alphanumeric() || c == '_' {
                j += c.len_utf8();
            } else {
                break;
            }
        }
        let rest = self.src[j..].trim_start();
        rest.starts_with("->") || rest.starts_with("<-")
    }

    fn parse_arrow(&mut self) -> Result<(Direction, Vec<String>, Vec<Span>, bool)> {
        let direction = if self.eat("->") {
            Direction::Out
        } else if self.eat("<-") {
            Direction::In
        } else {
            return Err(self
                .err("expected -> or <-")
                .with_help("a relationship names its direction, `->` or `<-`"));
        };
        let (targets, spans, many) = self.parse_type_ref()?;
        Ok((direction, targets, spans, many))
    }

    fn parse_type_ref(&mut self) -> Result<(Vec<String>, Vec<Span>, bool)> {
        self.skip();
        if self.eat("(") {
            let mut targets = Vec::new();
            let mut spans = Vec::new();
            let (name, span) = self.ident()?;
            targets.push(name);
            spans.push(span);
            while self.eat("|") {
                let (name, span) = self.ident()?;
                targets.push(name);
                spans.push(span);
            }
            self.expect(")")?;
            let many = self.eat("[]");
            return Ok((targets, spans, many));
        }
        let (name, span) = self.ident()?;
        let many = self.eat("[]");
        Ok((vec![name], vec![span], many))
    }

    fn parse_selection(&mut self) -> Result<Selection> {
        self.skip();
        self.reject_discovery_literal()?;
        let mut also = Vec::new();
        let mut also_spans = Vec::new();
        let (type_name, type_span) = if self.eat("(") {
            let (type_name, type_span) = self.ident()?;
            while self.eat("|") {
                let (name, span) = self.ident()?;
                also.push(name);
                also_spans.push(span);
            }
            self.expect(")")?;
            (type_name, type_span)
        } else {
            self.ident()?
        };
        let condition = if self.eat("(") {
            self.skip();
            if self.eat(")") {
                None
            } else {
                let expr = self.parse_or()?;
                self.expect(")")?;
                Some(expr)
            }
        } else {
            None
        };
        let near = if self.starts_call("near", "(") {
            self.expect_builtin("near")?;
            self.expect("(")?;
            let (field, span) = self.ident()?; self.expect(",")?;
            let query = self.vector()?; self.expect(",")?;
            let k = self.integer()?;
            let k = usize::try_from(k).map_err(|_| self.err_at(span, "near k must be non-negative"))?;
            let exact = if self.eat(",") { self.expect_word("exact")?; true } else { false };
            self.expect(")")?;
            Some(Near { similarity: Similarity { field, query, span }, k, exact })
        } else { None };
        let order = if self.eat_word("order") {
            self.expect_word("by")?;
            Some(self.distance()?)
        } else {
            None
        };
        let limit = if self.eat_word("limit") {
            self.skip();
            let start = self.i;
            let n = self.integer()?;
            Some(usize::try_from(n).map_err(|_| {
                self.err_at(
                    self.span_bytes(start, self.i),
                    "limit must be a non-negative integer",
                )
            })?)
        } else {
            None
        };
        let mut sets = Vec::new();
        if self.eat_word("set") {
            loop {
                let (field, span) = self.ident()?;
                self.expect(":")?;
                sets.push((field, self.parse_value()?, span));
                self.skip();
                if !self.eat(",") {
                    break;
                }
            }
        }
        let mut items = Vec::new();
        if self.eat("{") {
            while !self.eat("}") {
                items.push(self.parse_item()?);
                self.skip();
            }
        }
        Ok(Selection {
            type_name,
            type_span,
            also,
            also_spans,
            condition,
            sets,
            near,
            order,
            limit,
            items,
        })
    }

    fn parse_item(&mut self) -> Result<Item> {
        self.skip();
        self.reject_discovery_block()?;
        if self.src[self.i..].starts_with('@') {
            return self.builtin_item(None);
        }
        if self.eat("&") {
            let amp = self.i - 1;
            let (name, _) = self.ident()?;
            let span = self.span_bytes(amp, self.i);
            if self.eat(":") {
                return Ok(Item::EdgeSet(name, self.parse_value()?, span));
            }
            return Ok(Item::EdgeProp(name, span));
        }
        for name in ["similarity", "distance"] {
            if self.starts_call(name, "(") {
                return self.builtin_item(None);
            }
        }
        let (field, mut span) = self.ident()?;
        if self.eat(":") {
            return self.builtin_item(Some(field));
        }
        let mut path = None;
        let star = self.eat("*");
        let range = if star && self.starts_word("path") {
            path = Some(self.parse_path()?);
            None
        } else if star {
            let min = self.integer()? as usize;
            self.expect("..")?;
            let max = self.integer()? as usize;
            let (end_line, end_column) = self.loc(self.i);
            span.end_line = end_line;
            span.end_column = end_column;
            if min < 1 || max < min {
                return Err(self
                    .err_at(span, format!("bad range *{min}..{max}"))
                    .with_help("write `*min..max`, with min at least 1 and max at least min"));
            }
            Some((min, max))
        } else {
            None
        };
        if self.starts_with_arrow() {
            let direction = if self.eat("->") {
                Direction::Out
            } else {
                self.expect("<-")?;
                Direction::In
            };
            let link = self.eat_word("link");
            let target = self.parse_selection()?;
            return Ok(Item::Walk {
                field,
                span,
                range,
                path,
                link,
                direction,
                target: Box::new(target),
            });
        }
        if path.is_some() {
            return Err(self
                .err(format!("{field} *path needs an arrow and a target"))
                .with_help(format!("write `{field} *path -> Type(name: \"B\")`")));
        }
        if range.is_some() {
            return Err(self
                .err_at(span, format!("{field} has a range but no arrow"))
                .with_help(format!("write `{field} *1..3 -> Type`")));
        }
        Ok(Item::Prop(field, span))
    }

    /// `path` after the `*`, then an optional bound `(@hops <= 20)` or
    /// `(@cost <= 50)`, a weight `by &km`, and an A* guide `toward at in km`,
    /// in that order.
    fn parse_path(&mut self) -> Result<PathSpec> {
        self.skip();
        let start = self.i;
        self.expect_word("path")?;
        let span = self.span_bytes(start, self.i);
        let bound = if self.eat("(") {
            self.skip();
            let bound_start = self.i;
            let builtin = self.eat("@");
            let (name, _) = self.ident()?;
            let name_span = self.span_bytes(bound_start, self.i);
            if !builtin && matches!(name.as_str(), "hops" | "cost") {
                return Err(self.err_at(name_span, format!("`{name}` is a path bound: write `@{name}`")));
            }
            let inclusive = if self.eat("<=") {
                true
            } else if self.eat("<") {
                false
            } else {
                return Err(self
                    .err("a path bound is `<=` or `<`")
                    .with_help("write `*path(@hops <= 20)` or `*path(@cost <= 50)`"));
            };
            self.skip();
            let value_start = self.i;
            let value = self.number_token()?;
            let value_span = self.span_bytes(value_start, self.i);
            let bound = match name.as_str() {
                "hops" => {
                    let n = value
                        .as_i64()
                        .and_then(|n| usize::try_from(n).ok())
                        .ok_or_else(|| {
                            self.err_at(value_span, "a hops bound is a non-negative integer")
                        })?;
                    match (inclusive, n) {
                        (true, n) => PathBound::Hops(n),
                        (false, 0) => {
                            return Err(self
                                .err_at(value_span, "`@hops < 0` allows no route")
                                .with_help("write `@hops <= 0` for a route of no edges"))
                        }
                        (false, n) => PathBound::Hops(n - 1),
                    }
                }
                "cost" => {
                    let limit = value
                        .as_f64()
                        .filter(|n| n.is_finite() && *n >= 0.0)
                        .ok_or_else(|| {
                            self.err_at(value_span, "a cost bound is a non-negative number")
                        })?;
                    PathBound::Cost { limit, inclusive }
                }
                _ => {
                    return Err(self
                        .err_at(name_span, format!("unknown path bound {name}"))
                        .with_help("bound a path by `@hops` or `@cost`, e.g. `*path(@cost <= 50)`"))
                }
            };
            let bound_span = self.span_bytes(bound_start, self.i);
            self.expect(")")?;
            Some((bound, bound_span))
        } else {
            None
        };
        let weight = if self.eat_word("by") {
            self.skip();
            let amp = self.i;
            if !self.eat("&") {
                return Err(self
                    .err("a path weight is an edge field")
                    .with_help("write `by &km`, where `km` is a field of the relationship"));
            }
            let (name, _) = self.ident()?;
            Some((name, self.span_bytes(amp, self.i)))
        } else {
            None
        };
        let toward = if self.eat_word("toward") {
            let (field, span) = self.ident()?;
            self.skip();
            if self.starts_word("in") {
                return Err(self
                    .err("the unit is declared on the weight, not here")
                    .with_help("declare `km: Float<km>` on the relationship and write `toward at`"));
            }
            Some(Toward { field, span })
        } else {
            None
        };
        Ok(PathSpec {
            span,
            bound,
            weight,
            toward,
        })
    }

    fn starts_with_arrow(&mut self) -> bool {
        self.skip();
        self.src[self.i..].starts_with("->") || self.src[self.i..].starts_with("<-")
    }

    fn parse_or(&mut self) -> Result<BoolExpr> {
        let mut left = self.parse_and()?;
        loop {
            if self.eat("||") {
                let right = self.parse_and()?;
                left = BoolExpr::Or(Box::new(left), Box::new(right));
                continue;
            }
            self.skip();
            if self.src[self.i..].starts_with('|') {
                return Err(self
                    .err("or is `||`")
                    .with_help("one `|` joins types, as in `(Book | Movie)`"));
            }
            return Ok(left);
        }
    }

    fn parse_and(&mut self) -> Result<BoolExpr> {
        let mut left = self.parse_atom()?;
        loop {
            if self.eat("&&") {
                let right = self.parse_atom()?;
                left = BoolExpr::And(Box::new(left), Box::new(right));
                continue;
            }
            self.skip();
            if self.src[self.i..].starts_with('&') {
                return Err(self.err("and is `&&`"));
            }
            if self.src[self.i..].starts_with(',') {
                return Err(self
                    .err("and is `&&`")
                    .with_help("a comma separates writes in `set`"));
            }
            return Ok(left);
        }
    }

    fn parse_atom(&mut self) -> Result<BoolExpr> {
        if self.eat("(") {
            let inner = self.parse_or()?;
            self.expect(")")?;
            return Ok(inner);
        }
        Ok(BoolExpr::Test(self.parse_pred()?))
    }

    fn parse_pred(&mut self) -> Result<Pred> {
        self.skip();
        self.reject_discovery_block()?;
        if self.starts_call("similarity", "(") {
            let sim = self.similarity()?;
            let op = if self.eat(">=") { Cmp::Gte } else if self.eat("<=") { Cmp::Lte } else if self.eat(">") { Cmp::Gt } else if self.eat("<") { Cmp::Lt } else { return Err(self.err("similarity needs <, <=, >, or >= and a score")); };
            let value = self.parse_value()?.as_f64().filter(|v| v.is_finite()).ok_or_else(|| self.err("similarity threshold must be finite"))?;
            return Ok(Pred::Similarity(sim, op, value));
        }
        if self.starts_call("distance", "(") {
            let distance = self.distance()?;
            let op = if self.eat("<=") {
                Cmp::Lte
            } else if self.eat("<") {
                Cmp::Lt
            } else if self.eat(">=") {
                Cmp::Gte
            } else if self.eat(">") {
                Cmp::Gt
            } else {
                return Err(self.err("distance needs <, <=, >, or >= and metres"));
            };
            self.skip();
            let start = self.i;
            let metres = self
                .parse_value()?
                .as_f64()
                .filter(|n| n.is_finite() && *n >= 0.0)
                .ok_or_else(|| {
                    self.err_at(
                        self.span_bytes(start, self.i),
                        "distance must be a non-negative number of metres",
                    )
                })?;
            return Ok(Pred::Distance(distance, op, metres));
        }
        if self.starts_call("within_box", "(") {
            self.expect_builtin("within_box")?;
            self.expect("(")?;
            let (field, span) = self.ident()?;
            self.expect(",")?;
            let southwest = self.point()?;
            self.expect(",")?;
            let northeast = self.point()?;
            self.expect(")")?;
            return Ok(Pred::Box(
                field,
                Bounds::new(southwest, northeast).map_err(|message| self.err_at(span, message))?,
                span,
            ));
        }
        self.skip();
        let start = self.i;
        let builtin = self.eat("@");
        let (mut field, _) = self.ident()?;
        let span = self.span_bytes(start, self.i);
        if builtin {
            if field != "id" {
                return Err(self.err_at(span, format!("unknown filter built-in @{field}")));
            }
            field = "@id".into();
        }
        self.skip();
        if self.eat(":") || self.eat("=") {
            return Ok(Pred::Eq(field, self.parse_value()?, span));
        }
        if self.eat("!=") || self.eat("<>") {
            return Ok(Pred::Ne(field, self.parse_value()?, span));
        }
        if self.src[self.i..].starts_with('!') {
            return Err(self
                .err("not-equal is `!=`")
                .with_help("a condition uses `=`, `!=`, `&&`, and `||`"));
        }
        if self.eat(">=") {
            return Ok(Pred::Cmp(field, Cmp::Gte, self.parse_value()?, span));
        }
        if self.eat("<=") {
            return Ok(Pred::Cmp(field, Cmp::Lte, self.parse_value()?, span));
        }
        if self.eat(">") {
            return Ok(Pred::Cmp(field, Cmp::Gt, self.parse_value()?, span));
        }
        if self.eat("<") {
            return Ok(Pred::Cmp(field, Cmp::Lt, self.parse_value()?, span));
        }
        for (old, new) in [("CONTAINS", "findWith"), ("STARTS", "startsWith"), ("ENDS", "endsWith")] {
            let start = self.i;
            if self.eat_word(old) {
                let mut end = self.i;
                if old != "CONTAINS" && self.eat_word("WITH") { end = self.i; }
                let spelling = if old == "CONTAINS" { old.to_string() } else { format!("{old} WITH") };
                return Err(self.err_at(self.span_bytes(start, end), format!("`{spelling}` was renamed: write `{new}`")));
            }
        }
        if self.eat_word("findWith") {
            return Ok(Pred::Contains(field, self.string()?, span));
        }
        if self.eat_word("startsWith") {
            return Ok(Pred::StartsWith(field, self.string()?, span));
        }
        if self.eat_word("endsWith") {
            return Ok(Pred::EndsWith(field, self.string()?, span));
        }
        Err(self
            .err(format!("expected a comparison after {field}"))
            .with_help(
            "use `=`, `!=`, `>`, `<`, `>=`, `<=`, `<>`, `findWith`, `startsWith`, or `endsWith`",
        ))
    }

    fn looking_at_ident(&self) -> bool {
        self.src[self.i..]
            .trim_start()
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
    }

    fn point(&mut self) -> Result<Point> {
        self.skip();
        let start = self.i;
        self.expect_builtin("point")?;
        self.expect("(")?;
        let arity = |p: &Self| {
            p.err_at(
                p.span_bytes(start, p.i),
                "@point() needs exactly two numbers: latitude, longitude",
            )
        };
        if self.eat(")") {
            return Err(arity(self));
        }
        self.skip();
        let lat_start = self.i;
        let lat = self.parse_value()?;
        let lat_span = self.span_bytes(lat_start, self.i);
        if !self.eat(",") {
            return Err(arity(self));
        }
        self.skip();
        if self.eat(")") {
            return Err(arity(self));
        }
        let lon_start = self.i;
        let lon = self.parse_value()?;
        let lon_span = self.span_bytes(lon_start, self.i);
        if !self.eat(")") {
            return Err(arity(self));
        }
        let latitude = lat
            .as_f64()
            .filter(|n| n.is_finite() && (-90.0..=90.0).contains(n))
            .ok_or_else(|| self.err_at(lat_span, "Point latitude must be a number in [-90, 90]"))?;
        let longitude = lon
            .as_f64()
            .filter(|n| n.is_finite() && (-180.0..=180.0).contains(n))
            .ok_or_else(|| {
                self.err_at(lon_span, "Point longitude must be a number in [-180, 180]")
            })?;
        Ok(Point::new(latitude, longitude).expect("validated coordinates"))
    }

    fn distance(&mut self) -> Result<Distance> {
        self.expect_builtin("distance")?;
        self.expect("(")?;
        let (field, span) = self.ident()?;
        self.expect(",")?;
        let origin = self.point()?;
        self.expect(")")?;
        Ok(Distance {
            field,
            origin,
            span,
        })
    }

    fn vector(&mut self) -> Result<Vector> {
        self.expect_builtin("vector")?; self.expect("[")?;
        let mut values = Vec::new();
        if !self.eat("]") { loop {
            self.skip(); let start = self.i;
            let value = self.parse_value()?;
            let number = value.as_f64().filter(|v| v.is_finite() && (*v as f32).is_finite())
                .ok_or_else(|| self.err_at(self.span_bytes(start,self.i), "Vector components must be finite float32 numbers"))?;
            values.push(number as f32);
            if self.eat("]") { break; } self.expect(",")?;
        } }
        Vector::new(&values, Metric::Cosine).map_err(|m| self.err(m))
    }
    fn similarity(&mut self) -> Result<Similarity> {
        self.expect_builtin("similarity")?; self.expect("(")?;
        let (field, span) = self.ident()?; self.expect(",")?;
        let query = self.vector()?; self.expect(")")?;
        Ok(Similarity { field, query, span })
    }

    fn parse_value(&mut self) -> Result<Json> {
        self.skip();
        if self.starts_call("vector", "[") { return Ok(self.vector()?.to_json()); }
        if self.starts_call("point", "(") {
            return Ok(self.point()?.to_json());
        }
        if self.eat("$") {
            if !self.columns {
                return Err(self
                    .err("`$` names a column or a key")
                    .with_help("use `$Name` inside `mutation csv` or `mutation json`"));
            }
            let name = if self.src[self.i..].starts_with('"') {
                self.string()?
            } else {
                self.ident().map(|(name, _)| name)?
            };
            return Ok(column_ref(&name));
        }
        if self.src[self.i..].starts_with('"') {
            return Ok(Json::String(self.string()?));
        }
        if self.columns && self.looking_at_ident() {
            return Err(self
                .err("a column needs `$`")
                .with_help("write `$Team`, or `$\"Type 1\"` when the name has a space"));
        }
        if self.eat_word("true") {
            return Ok(Json::Bool(true));
        }
        if self.eat_word("false") {
            return Ok(Json::Bool(false));
        }
        if self.eat_word("null") {
            return Ok(Json::Null);
        }
        let n = self.number_token()?;
        Ok(n)
    }

    fn string(&mut self) -> Result<String> {
        self.skip();
        if !self.src[self.i..].starts_with('"') {
            return Err(self.err("expected a string"));
        }
        let start = self.i;
        self.i += 1;
        let mut out = String::new();
        while self.i < self.src.len() {
            let c = self.src[self.i..].chars().next().unwrap();
            self.i += c.len_utf8();
            if c == '"' {
                return Ok(out);
            }
            if c == '\\' {
                let e = self.src[self.i..]
                    .chars()
                    .next()
                    .ok_or_else(|| self.err_at(self.span_bytes(start, self.i), "bad string"))?;
                self.i += e.len_utf8();
                out.push(match e {
                    'n' => '\n',
                    't' => '\t',
                    '"' => '"',
                    '\\' => '\\',
                    other => other,
                });
            } else {
                out.push(c);
            }
        }
        Err(self.err_at(self.span_bytes(start, self.i), "unterminated string"))
    }

    fn integer(&mut self) -> Result<i64> {
        let start = self.i;
        match self.number_token()? {
            Json::Number(n) => n
                .as_i64()
                .ok_or_else(|| self.err_at(self.span_bytes(start, self.i), "expected an integer")),
            _ => Err(self.err_at(self.span_bytes(start, self.i), "expected an integer")),
        }
    }

    fn number_token(&mut self) -> Result<Json> {
        self.skip();
        let start = self.i;
        if self.src[self.i..].starts_with('-') {
            self.i += 1;
        }
        if !self.peek_digit() {
            return Err(self.err("expected a number"));
        }
        while self.peek_digit() {
            self.i += 1;
        }
        let mut float = false;
        if self.src[self.i..].starts_with('.')
            && self.src[self.i + 1..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit())
        {
            float = true;
            self.i += 1;
            while self.peek_digit() {
                self.i += 1;
            }
        }
        if self.src[self.i..].starts_with(['e', 'E']) {
            float = true; self.i += 1;
            if self.src[self.i..].starts_with(['+', '-']) { self.i += 1; }
            if !self.peek_digit() { return Err(self.err("exponent needs digits")); }
            while self.peek_digit() { self.i += 1; }
        }
        let text = &self.src[start..self.i];
        if float {
            let n: f64 = text.parse().map_err(|_| {
                self.err_at(self.span_bytes(start, self.i), format!("bad number {text}"))
            })?;
            if !n.is_finite() { return Err(self.err_at(self.span_bytes(start, self.i), "number must be finite")); }
            Ok(Json::from(n))
        } else {
            let n: i64 = text.parse().map_err(|_| {
                self.err_at(self.span_bytes(start, self.i), format!("bad number {text}"))
            })?;
            Ok(Json::from(n))
        }
    }
}

fn from_error(pane: Pane, error: Error) -> Diagnostic {
    Diagnostic::at(
        pane,
        error.line,
        error.column,
        error.end_line,
        error.end_column,
        error.message,
        error.help,
    )
}

/// The rendered text a CLI prints and the browser shows.
pub fn render_error(source_name: &str, source: &str, error: &Error) -> String {
    let pane = if source_name == "schema" {
        Pane::Schema
    } else {
        Pane::Query
    };
    render(
        source_name,
        source,
        &Diagnostic::at(
            pane,
            error.line,
            error.column,
            error.end_line,
            error.end_column,
            error.message.clone(),
            error.help.clone(),
        ),
    )
}

fn column_ref(name: &str) -> Json {
    serde_json::json!({ "$column": name })
}

fn column_name(value: &Json) -> Option<&str> {
    let object = value.as_object()?;
    if object.len() != 1 {
        return None;
    }
    object.get("$column").and_then(Json::as_str)
}

fn bind_query(
    query: &Query,
    row: &std::collections::HashMap<String, Json>,
) -> Result<Option<Query>> {
    let Some(root) = &query.root else {
        return Ok(Some(query.clone()));
    };
    Ok(bind_selection(root, row)?.map(|root| Query {
        mutation: query.mutation,
        skip: query.skip,
        then: query.then.clone(),
        root: Some(root),
    }))
}

fn bind_selection(
    sel: &Selection,
    row: &std::collections::HashMap<String, Json>,
) -> Result<Option<Selection>> {
    let condition = match &sel.condition {
        Some(expr) => bind_expr(expr, row)?,
        None => None,
    };
    if sel.condition.is_some() && condition.is_none() {
        return Ok(None);
    }
    let mut sets = Vec::new();
    for (name, value, span) in &sel.sets {
        if let Some(value) = bind_json(value, row)? {
            sets.push((name.clone(), value, *span));
        }
    }
    let mut items = Vec::new();
    for item in &sel.items {
        match item {
            Item::Walk {
                field,
                span,
                range,
                path,
                link,
                direction,
                target,
            } => {
                let Some(target) = bind_selection(target, row)? else {
                    continue;
                };
                items.push(Item::Walk {
                    field: field.clone(),
                    span: *span,
                    range: *range,
                    path: path.clone(),
                    link: *link,
                    direction: *direction,
                    target: Box::new(target),
                });
            }
            Item::EdgeSet(name, value, span) => {
                if let Some(value) = bind_json(value, row)? {
                    items.push(Item::EdgeSet(name.clone(), value, *span));
                }
            }
            other => items.push(other.clone()),
        }
    }
    Ok(Some(Selection {
        type_name: sel.type_name.clone(),
        type_span: sel.type_span,
        also: sel.also.clone(),
        also_spans: sel.also_spans.clone(),
        condition,
        sets,
        near: sel.near.clone(),
        order: sel.order.clone(),
        limit: sel.limit,
        items,
    }))
}

fn bind_expr(
    expr: &BoolExpr,
    row: &std::collections::HashMap<String, Json>,
) -> Result<Option<BoolExpr>> {
    match expr {
        BoolExpr::Test(pred) => Ok(bind_pred(pred, row)?.map(BoolExpr::Test)),
        BoolExpr::And(left, right) | BoolExpr::Or(left, right) => {
            let bound_left = bind_expr(left, row)?;
            let bound_right = bind_expr(right, row)?;
            match (bound_left, bound_right) {
                (Some(left), Some(right)) => Ok(Some(if matches!(expr, BoolExpr::And(_, _)) {
                    BoolExpr::And(Box::new(left), Box::new(right))
                } else {
                    BoolExpr::Or(Box::new(left), Box::new(right))
                })),
                (Some(only), None) | (None, Some(only)) => Ok(Some(only)),
                (None, None) => Ok(None),
            }
        }
    }
}

fn bind_pred(pred: &Pred, row: &std::collections::HashMap<String, Json>) -> Result<Option<Pred>> {
    match pred {
        Pred::Eq(field, value, span) => {
            Ok(bind_json(value, row)?.map(|value| Pred::Eq(field.clone(), value, *span)))
        }
        Pred::Ne(field, value, span) => {
            Ok(bind_json(value, row)?.map(|value| Pred::Ne(field.clone(), value, *span)))
        }
        Pred::Cmp(field, op, value, span) => {
            Ok(bind_json(value, row)?.map(|value| Pred::Cmp(field.clone(), *op, value, *span)))
        }
        other => Ok(Some(other.clone())),
    }
}

fn bind_json(value: &Json, row: &std::collections::HashMap<String, Json>) -> Result<Option<Json>> {
    let Some(name) = column_name(value) else {
        return Ok(Some(value.clone()));
    };
    match row.get(name) {
        None | Some(Json::Null) => Ok(None),
        Some(other) => Ok(Some(other.clone())),
    }
}

fn file_locations(value: Json) -> std::result::Result<Vec<String>, String> {
    match value {
        Json::String(location) if !location.is_empty() => Ok(vec![location]),
        Json::Array(items) if !items.is_empty() && items.iter().all(Json::is_string) => {
            let locations = string_list(items);
            if locations.iter().any(String::is_empty) {
                return Err("a load reads a file".into());
            }
            Ok(locations)
        }
        _ => Err("a load reads a file".into()),
    }
}

fn string_list(items: Vec<Json>) -> Vec<String> {
    items
        .into_iter()
        .filter_map(|item| item.as_str().map(str::to_string))
        .collect()
}

/// `$` names used by a load template, in source order.
pub fn column_refs(query: &Query) -> Vec<(String, Span)> {
    let Some(root) = &query.root else {
        return Vec::new();
    };
    let mut out = Vec::new();
    collect_refs(root, &mut out);
    out
}

fn collect_refs(sel: &Selection, out: &mut Vec<(String, Span)>) {
    if let Some(expr) = &sel.condition {
        collect_expr_refs(expr, out);
    }
    for (_, value, span) in &sel.sets {
        if let Some(name) = column_name(value) {
            out.push((name.to_string(), *span));
        }
    }
    for item in &sel.items {
        match item {
            Item::EdgeSet(_, value, span) => {
                if let Some(name) = column_name(value) {
                    out.push((name.to_string(), *span));
                }
            }
            Item::Walk { target, .. } => collect_refs(target, out),
            _ => {}
        }
    }
}

fn collect_expr_refs(expr: &BoolExpr, out: &mut Vec<(String, Span)>) {
    match expr {
        BoolExpr::Test(pred) => {
            let (value, span) = match pred {
                Pred::Eq(_, value, span)
                | Pred::Ne(_, value, span)
                | Pred::Cmp(_, _, value, span) => (value, *span),
                _ => return,
            };
            if let Some(name) = column_name(value) {
                out.push((name.to_string(), span));
            }
        }
        BoolExpr::And(left, right) | BoolExpr::Or(left, right) => {
            collect_expr_refs(left, out);
            collect_expr_refs(right, out);
        }
    }
}

/// `$` names that are not a column of `rows`. An empty row list reports nothing.
pub fn missing_columns(
    template: &Query,
    rows: &[std::collections::HashMap<String, Json>],
) -> Vec<Error> {
    if rows.is_empty() {
        return Vec::new();
    }
    let known: std::collections::HashSet<&str> = rows
        .iter()
        .flat_map(|row| row.keys().map(String::as_str))
        .collect();
    let mut names: Vec<&str> = known.iter().copied().collect();
    names.sort_unstable();
    let help = format!("columns are {}", names.join(", "));
    column_refs(template)
        .into_iter()
        .filter(|(name, _)| !known.contains(name.as_str()))
        .map(|(name, span)| Error::at(span, format!("no column {name}")).with_help(help.clone()))
        .collect()
}

pub fn bind_row(
    template: &Query,
    row: &std::collections::HashMap<String, Json>,
) -> Result<Option<Query>> {
    bind_query(template, row)
}

/// Bind the explicit load template, then populate Point fields from their named
/// object or the schema's explicit `from` mapping. Validate before any writes.
pub fn bind_location_row(
    schema: &Schema,
    template: &Query,
    row: &std::collections::HashMap<String, Json>,
) -> Result<Option<Query>> {
    let Some(mut query) = bind_row(template, row)? else {
        return Ok(None);
    };
    if let Some(root) = &mut query.root {
        bind_points(schema, root, row, false)?;
        check(schema, root, true)?;
    }
    Ok(Some(query))
}
fn bind_points(
    schema: &Schema,
    sel: &mut Selection,
    row: &std::collections::HashMap<String, Json>,
    link: bool,
) -> Result<()> {
    let lookup = link || !sel.sets.is_empty() || sel.items.iter().any(|item| matches!(item, Item::Walk { link: true, .. }));
    for field in &schema.get(&sel.type_name)?.fields {
        let Field::Prop {
            name,
            ty,
            optional,
            from,
            ..
        } = field
        else {
            continue;
        };
        if (ty != "Point" && VectorSpec::parse(ty).is_none()) || lookup {
            continue;
        }
        if sel.sets.iter().any(|(key, ..)| key == name)
            || sel
                .condition
                .as_ref()
                .is_some_and(|expr| expr.tests().iter().any(|pred| pred.field() == name))
        {
            continue;
        }
        let value = if let Some(value) = row.get(name) {
            value.clone()
        } else if let Some(columns) = from {
            let lat = &columns[0];
            let lon = columns.get(1).unwrap_or(lat);
            let coordinate = |column: &str| {
                row.get(column)
                    .filter(|value| value.is_number())
                    .ok_or_else(|| {
                        Error::at(
                            sel.type_span,
                            if row.contains_key(column) {
                                format!("Point source column {column} must be numeric")
                            } else {
                                format!("no column {column} for Point field {name}")
                            },
                        )
                    })
            };
            if VectorSpec::parse(ty).is_some() {
                Json::Array(columns.iter().map(|c| coordinate(c).cloned()).collect::<Result<Vec<_>>>()?)
            } else { serde_json::json!({"lat": coordinate(lat)?, "lon": coordinate(lon)?}) }
        } else if *optional {
            continue;
        } else {
            return Err(Error::at(sel.type_span, format!("{name} requires a {ty} value or an explicit from (...) mapping")));
        };
        if !(value.is_null() && *optional) {
            if let Some(spec) = VectorSpec::parse(ty) { spec.value(&value).map_err(|m| Error::at(sel.type_span, m))?; }
            else { Point::from_json(&value).map_err(|message| Error::at(sel.type_span, message))?; }
        }
        let pred = BoolExpr::Test(Pred::Eq(name.clone(), value, sel.type_span));
        sel.condition = Some(match sel.condition.take() {
            Some(expr) => BoolExpr::And(Box::new(expr), Box::new(pred)),
            None => pred,
        });
    }
    for item in &mut sel.items {
        if let Item::Walk { target, link, .. } = item {
            bind_points(schema, target, row, *link)?;
        }
    }
    Ok(())
}

pub fn json_rows(
    value: Json,
) -> std::result::Result<Vec<std::collections::HashMap<String, Json>>, String> {
    let items = match value {
        Json::Array(items) => items,
        Json::Object(_) => vec![value],
        _ => return Err("json load expects an object or an array of objects".into()),
    };
    let mut rows = Vec::new();
    for item in items {
        let Some(object) = item.as_object() else {
            return Err("json load expects an object or an array of objects".into());
        };
        rows.push(
            object
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        );
    }
    Ok(rows)
}

pub fn csv_rows(
    text: &str,
) -> std::result::Result<Vec<std::collections::HashMap<String, Json>>, String> {
    let mut reader = csv::ReaderBuilder::new().trim(csv::Trim::All).from_reader(text.as_bytes());
    let headers = reader.headers().map_err(|error| format!("invalid csv: {error}"))?.clone();
    if headers.is_empty() {
        return Err("csv has no header".into());
    }
    let mut seen = std::collections::HashSet::new();
    if headers.iter().any(|header| header.is_empty() || !seen.insert(header)) {
        return Err("csv headers must be nonempty and unique".into());
    }
    reader.records().map(|record| {
        let record = record.map_err(|error| format!("invalid csv: {error}"))?;
        Ok(headers.iter().zip(record.iter()).map(|(header, cell)| {
            (header.to_string(), if cell.is_empty() { Json::Null } else { csv_cell(cell) })
        }).collect())
    }).collect()
}

fn csv_cell(cell: &str) -> Json {
    if cell == "true" || cell == "false" {
        return Json::Bool(cell == "true");
    }
    if let Ok(number) = cell.parse::<i64>() {
        return Json::from(number);
    }
    if cell.contains('.') {
        if let Ok(number) = cell.parse::<f64>() {
            return Json::from(number);
        }
    }
    Json::String(cell.to_string())
}

fn note_statement(schema: &Schema, statement: &Statement, pane: Pane, out: &mut Vec<Diagnostic>) {
    if let Statement::Run(query) = statement {
        if let Err(error) = check_pipeline(schema, query) {
            out.push(from_error(pane, error));
        }
    }
    let (root, mutation) = match statement {
        Statement::Run(query) => (query.root.as_ref(), query.mutation),
        Statement::Load { template, .. } => (template.root.as_ref(), true),
    };
    if let Some(root) = root {
        Check {
            schema,
            mutation,
            pane,
            out,
        }
        .selection(root, true);
    }
}

/// Type-check the `unique` and `index` blocks and any `mutation` or `query` that follows the
/// types in the same text. The schema editor holds the whole file.
fn document_diagnostics(schema: &Schema, source: &str, out: &mut Vec<Diagnostic>) {
    let Ok((_, end)) = parse_schema_at(source) else {
        return;
    };
    let mut p = Parser::new(source);
    p.i = end;
    if let Err(error) = p.take_blocks(schema) {
        out.push(from_error(Pane::Schema, error));
        return;
    }
    loop {
        p.skip();
        if p.eof() {
            break;
        }
        match p.parse_statement() {
            Err(error) => {
                out.push(from_error(Pane::Schema, error));
                return;
            }
            Ok(statement) => note_statement(schema, &statement, Pane::Schema, out),
        }
    }
}

/// Parse and type-check. An empty query reports nothing: the page is idle.
/// `text` is the report a terminal prints unchanged.
pub fn diagnose(schema_src: &str, query_src: &str) -> Report {
    let mut out = Vec::new();
    let schema = match parse_schema(schema_src) {
        Ok(schema) => {
            document_diagnostics(&schema, schema_src, &mut out);
            Some(schema)
        }
        Err(error) => {
            out.push(from_error(Pane::Schema, error));
            None
        }
    };
    if !query_src.trim().is_empty() {
        match parse_statement(query_src) {
            Err(error) => out.push(from_error(Pane::Query, error)),
            Ok(statement) => {
                if let Some(schema) = &schema {
                    note_statement(schema, &statement, Pane::Query, &mut out);
                }
            }
        }
    }
    Report::new(schema_src, query_src, out)
}

/// The same check the editor uses. Execution stops on the first problem.
pub fn check(schema: &Schema, sel: &Selection, mutation: bool) -> Result<()> {
    let mut out = Vec::new();
    Check {
        schema,
        mutation,
        pane: Pane::Query,
        out: &mut out,
    }
    .selection(sel, true);
    match out.into_iter().next() {
        Some(diag) => Err(Error {
            message: diag.message,
            help: diag.help,
            line: diag.line,
            column: diag.column,
            end_line: diag.end_line,
            end_column: diag.end_column,
        }),
        None => Ok(()),
    }
}

struct Check<'a> {
    schema: &'a Schema,
    mutation: bool,
    pane: Pane,
    out: &'a mut Vec<Diagnostic>,
}

impl Check<'_> {
    fn push(&mut self, span: Span, message: impl Into<String>, help: Option<String>) {
        self.out.push(Diagnostic::at(
            self.pane,
            span.line,
            span.column,
            span.end_line,
            span.end_column,
            message,
            help,
        ));
    }

    fn selection(&mut self, sel: &Selection, root: bool) {
        self.visit(sel, root, None);
    }

    fn visit(&mut self, sel: &Selection, _root: bool, arrived: Option<(&str, &str)>) {
        let known = self.schema.types.iter().any(|ty| ty.name == sel.type_name);
        if !known {
            self.push(
                sel.type_span,
                format!("unknown type {}", sel.type_name),
                Some(type_help(self.schema, &sel.type_name)),
            );
            return;
        }
        for (extra, span) in sel.also.iter().zip(&sel.also_spans) {
            if self.schema.types.iter().all(|ty| ty.name != *extra) {
                self.push(
                    *span,
                    format!("unknown type {extra}"),
                    Some(type_help(self.schema, extra)),
                );
            }
        }
        if let Some(expr) = &sel.condition {
            for pred in expr.tests() {
                if pred.field() != "@id" {
                    self.ensure_prop(sel, pred.field(), pred.span());
                }
                match pred {
                    Pred::Similarity(sim, ..) => self.ensure_vector(sel, sim),
                    Pred::Distance(distance, ..) => {
                        self.ensure_point(sel, &distance.field, distance.span)
                    }
                    Pred::Box(field, _, span) => self.ensure_point(sel, field, *span),
                    Pred::Eq(field, value, span)
                    | Pred::Ne(field, value, span)
                    | Pred::Cmp(field, _, value, span) => {
                        self.check_point_value(sel, field, value, *span)
                    }
                    _ => {}
                }
            }
        }
        if let Some(near) = &sel.near {
            self.ensure_vector(sel, &near.similarity);
            if sel.order.is_some() { self.push(sel.type_span, "near already orders by similarity", None); }
        }
        if let Some(order) = &sel.order {
            self.ensure_point(sel, &order.field, order.span);
        }
        if self.mutation && (sel.near.is_some() || sel.order.is_some() || sel.limit.is_some()) {
            self.push(
                sel.type_span,
                "order and limit are only valid in queries",
                None,
            );
        }
        for (name, value, span) in &sel.sets {
            self.check_point_value(sel, name, value, *span);
            if self.mutation {
                self.ensure_prop(sel, name, *span);
            } else {
                self.push(
                    *span,
                    format!("`set {name}` writes a row"),
                    Some("wrap the query in `mutation { }`".into()),
                );
            }
        }
        for item in &sel.items {
            match item {
                Item::Score(_, span) => { if sel.near.is_none() { self.push(*span, "@score requires an @near(...) selection", None); } }
                Item::Similarity(_, sim) => self.ensure_vector(sel, sim),
                Item::Prop(name, span) => self.ensure_prop(sel, name, *span),
                Item::Distance(_, distance) => {
                    self.ensure_point(sel, &distance.field, distance.span)
                }
                Item::Hops(_) | Item::Id(_) => {}
                Item::EdgeProp(name, span) => {
                    self.edge_field(arrived, name, *span, None);
                }
                Item::EdgeSet(name, value, span) => {
                    if !self.mutation {
                        self.push(
                            *span,
                            format!("&{name}: value is stored by a mutation"),
                            Some(format!(
                                "drop the value to read `&{name}`, or wrap the query in `mutation`"
                            )),
                        );
                    } else {
                        self.edge_field(arrived, name, *span, Some(value));
                    }
                }
                Item::Walk {
                    field,
                    span,
                    range,
                    path,
                    direction,
                    target,
                    ..
                } => {
                    if let Some(path) = path {
                        self.path(sel, field, *span, *direction, path, target);
                    }
                    if self.mutation && range.is_some() {
                        self.push(
                            *span,
                            "a mutation cannot use a hop range",
                            Some("a range walks rows that are already stored".into()),
                        );
                    }
                    self.walk(sel, field, *span, *direction, target);
                    if self.mutation {
                        self.require_edge_fields(sel, field, *span, target);
                    }
                    self.visit(target, false, Some((sel.type_name.as_str(), field)));
                }
            }
        }
    }

    fn edge_field(
        &mut self,
        arrived: Option<(&str, &str)>,
        name: &str,
        span: Span,
        value: Option<&Json>,
    ) {
        // A declared user edge property wins. Only the removed implicit value
        // gets migration help; no user property name is reserved.
        if name == "hops" && arrived
            .and_then(|(ty, field)| find_edge(self.schema, ty, field))
            .is_none_or(|edge| !matches!(edge, Field::Edge { props, .. } if props.iter().any(|prop| prop.name == name)))
        {
            self.push(span, "`&hops` is built in: write `@hops`", None);
            return;
        }
        let Some((type_name, field)) = arrived else {
            self.push(
                span,
                format!("&{name} is an edge field, and this value was not reached by an edge"),
                Some(format!("read `&{name}` inside the type the edge lands on")),
            );
            return;
        };
        let Some(edge) = find_edge(self.schema, type_name, field) else {
            return;
        };
        let Field::Edge { props, .. } = edge else {
            return;
        };
        let Some(declared) = props.iter().find(|prop| prop.name == name) else {
            let known: Vec<&str> = props.iter().map(|prop| prop.name.as_str()).collect();
            let help = if let Some(hit) = closest(name, known.iter().copied()) {
                format!("did you mean `&{hit}`?")
            } else if known.is_empty() {
                format!("declare it on the relationship: {field} -> Type {{ {name}: Int }}")
            } else {
                format!(
                    "{field} has {}",
                    known
                        .iter()
                        .map(|item| format!("`{item}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            self.push(span, format!("{field} has no field {name}"), Some(help));
            return;
        };
        if let Some(value) = value {
            if !json_matches(&declared.ty, value) {
                self.push(
                    span,
                    format!("&{name} is not {}", declared.ty),
                    Some(format!("`{name}` is {}", declared.ty)),
                );
            }
        }
    }

    fn require_edge_fields(
        &mut self,
        sel: &Selection,
        field: &str,
        span: Span,
        target: &Selection,
    ) {
        let Some(edge) = find_edge(self.schema, &sel.type_name, field) else {
            return;
        };
        let Field::Edge { props, .. } = edge else {
            return;
        };
        for prop in props {
            if prop.optional {
                continue;
            }
            let present = target
                .items
                .iter()
                .any(|item| matches!(item, Item::EdgeSet(name, _, _) if name == &prop.name));
            if !present {
                self.push(
                    span,
                    format!("{field} requires &{}", prop.name),
                    Some(format!(
                        "write `&{}: …` inside {}",
                        prop.name, target.type_name
                    )),
                );
            }
        }
    }

    fn walk(
        &mut self,
        sel: &Selection,
        field: &str,
        span: Span,
        direction: Direction,
        target: &Selection,
    ) {
        let Some(edge) = find_edge(self.schema, &sel.type_name, field) else {
            self.push(
                span,
                format!("{} has no relationship {field}", sel.type_name),
                Some(field_help(self.schema, &[sel.type_name.as_str()], field)),
            );
            return;
        };
        let Some((_, _, schema_dir, targets, many)) = edge.as_edge() else {
            return;
        };
        if direction != schema_dir {
            self.push(
                span,
                format!(
                    "{}.{} does not point {}",
                    sel.type_name,
                    field,
                    arrow(direction)
                ),
                Some(format!(
                    "`{field}` points {} {}",
                    arrow(schema_dir),
                    show_targets(targets, many)
                )),
            );
        }
        let names = std::iter::once((target.type_name.as_str(), target.type_span)).chain(
            target
                .also
                .iter()
                .zip(&target.also_spans)
                .map(|(name, span)| (name.as_str(), *span)),
        );
        for (name, name_span) in names {
            if !targets.iter().any(|candidate| candidate == name) {
                self.push(
                    name_span,
                    format!("{}.{} does not reach {name}", sel.type_name, field),
                    Some(format!("`{field}` reaches {}", targets.join(", "))),
                );
            }
        }
    }

    /// `field *path ... -> Target`: the route keeps following `field` from each
    /// target, the weight is a number on the relationship, and A* has a Point
    /// on every type it can reach.
    fn path(
        &mut self,
        sel: &Selection,
        field: &str,
        span: Span,
        direction: Direction,
        path: &PathSpec,
        target: &Selection,
    ) {
        if self.mutation {
            self.push(
                path.span,
                "a mutation cannot find a path",
                Some("a path reads rows that are already stored; use `query`".into()),
            );
        }
        if target.near.is_some() || target.order.is_some() || target.limit.is_some() {
            self.push(
                target.type_span,
                "a path target takes a condition, not near, order or limit",
                Some("the condition names where the route ends, e.g. `Junction(name: \"B\")`".into()),
            );
        }
        let Some(edge) = find_edge(self.schema, &sel.type_name, field) else {
            // `walk` reports the missing relationship.
            return;
        };
        let Field::Edge { rel, props, .. } = edge else {
            return;
        };
        for name in selection_types(target) {
            let continues = find_edge(self.schema, name, field).is_some_and(|next| {
                matches!(next, Field::Edge { rel: next_rel, direction: next_dir, .. }
                    if next_rel == rel && *next_dir == direction)
            });
            if !continues && self.schema.types.iter().any(|ty| ty.name == name) {
                self.push(
                    span,
                    format!("a path keeps following {field}, and {name} has no `{field} {}`", arrow(direction)),
                    Some(format!("declare `{field} {} …` on {name}, or walk one hop without `*path`", arrow(direction))),
                );
            }
        }
        if let Some((name, weight_span)) = &path.weight {
            match props.iter().find(|prop| &prop.name == name) {
                Some(prop) if prop.ty == "Int" || prop.ty == "Float" => {}
                Some(prop) => self.push(
                    *weight_span,
                    format!("a path weight is a number; {field}.{name} is {}", prop.ty),
                    Some("weigh a path by an Int or Float field of the relationship".into()),
                ),
                None => {
                    let known: Vec<&str> = props.iter().map(|prop| prop.name.as_str()).collect();
                    let help = if let Some(hit) = closest(name, known.iter().copied()) {
                        format!("did you mean `&{hit}`?")
                    } else {
                        format!("declare it on the relationship: {field} -> Type {{ {name}: Float }}")
                    };
                    self.push(*weight_span, format!("{field} has no field {name}"), Some(help));
                }
            }
        }
        if let Some((PathBound::Hops(_), bound_span)) = &path.bound {
            if path.weight.is_some() {
                self.push(
                    *bound_span,
                    "a weighted path is bounded by cost",
                    Some("write `*path(@cost <= 50) by &km`; hops bound a path without a weight".into()),
                );
            }
        }
        if let Some(toward) = &path.toward {
            match &path.weight {
                None => self.push(
                    toward.span,
                    "toward needs a weight measured in a distance",
                    Some(format!(
                        "A* guesses the rest of the route as a distance; write `by &km toward {}` with `km: Float<km>`",
                        toward.field
                    )),
                ),
                Some((name, _)) => {
                    // A* compares the weight with metres, so it needs the unit.
                    if let Some(prop) = props.iter().find(|prop| &prop.name == name) {
                        if (prop.ty == "Int" || prop.ty == "Float") && prop.unit.is_none() {
                            self.push(
                                toward.span,
                                format!("toward needs a unit on the weight: declare {name}: {}<km>", prop.ty),
                                Some(format!(
                                    "A* compares {field}.{name} with straight-line distances; the unit is `m`, `km` or `mi`"
                                )),
                            );
                        }
                    }
                }
            }
            // The start's roads are checked against the straight line too,
            // so the start needs the Point as much as every node after it.
            // A start type that is also a target type is checked below.
            let targets = selection_types(target);
            for name in selection_types(sel) {
                if targets.contains(&name) {
                    continue;
                }
                match self.schema.prop(name, &toward.field) {
                    Ok(Field::Prop { ty, .. }) if ty == "Point" => {}
                    Ok(Field::Prop { ty, .. }) => self.push(
                        toward.span,
                        format!("toward needs a Point; {name}.{} is {ty}", toward.field),
                        Some("A* measures the straight line to the target from a Point field".into()),
                    ),
                    _ => self.push(
                        toward.span,
                        format!(
                            "{name} has no {}, and `toward {}` needs a location on every node it reaches",
                            toward.field, toward.field
                        ),
                        Some(format!(
                            "declare `{}: Point` on {name}, or drop `toward` to search without a guess",
                            toward.field
                        )),
                    ),
                }
            }
            for name in selection_types(target) {
                if self.schema.types.iter().all(|ty| ty.name != name) {
                    continue;
                }
                match self.schema.prop(name, &toward.field) {
                    Ok(Field::Prop { ty, .. }) if ty == "Point" => {}
                    Ok(Field::Prop { ty, .. }) => self.push(
                        toward.span,
                        format!("toward needs a Point; {name}.{} is {ty}", toward.field),
                        Some("A* measures the straight line to the target from a Point field".into()),
                    ),
                    _ => self.push(
                        toward.span,
                        format!("{name} has no field {}", toward.field),
                        Some(prop_help(self.schema, name, &toward.field)),
                    ),
                }
            }
        }
    }

    fn ensure_vector(&mut self, sel: &Selection, sim: &Similarity) {
        for name in selection_types(sel) {
            let spec = self.schema.prop(name, &sim.field).ok().and_then(|f| match f { Field::Prop {ty,..} => VectorSpec::parse(ty), _ => None });
            match spec {
                Some(spec) if spec.dimensions == sim.query.dimensions() => {},
                Some(spec) => self.push(sim.span, format!("Vector<{}> query needs exactly {} numbers, got {}", spec.dimensions, spec.dimensions, sim.query.dimensions()), None),
                None => self.push(sim.span, format!("{name}.{} must be Vector for a similarity query", sim.field), None),
            }
        }
    }

    fn ensure_point(&mut self, sel: &Selection, name: &str, span: Span) {
        if !selection_types(sel).iter().any(
            |ty| matches!(self.schema.prop(ty, name), Ok(Field::Prop { ty, .. }) if ty == "Point"),
        ) {
            self.push(
                span,
                format!(
                    "{}.{} must be Point for a spatial query",
                    sel.type_name, name
                ),
                Some(format!("declare `{name}: Point`")),
            );
        }
    }

    fn check_point_value(&mut self, sel: &Selection, name: &str, value: &Json, span: Span) {
        if column_name(value).is_some() {
            return;
        }
        for type_name in selection_types(sel) {
            if let Ok(Field::Prop { ty, optional, .. }) = self.schema.prop(type_name, name) {
                if let Some(spec) = VectorSpec::parse(ty) {
                    if !(value.is_null() && *optional) { if let Err(m) = spec.value(value) { self.push(span, m, Some("write `@vector[0.1, 0.2, ...]` with the declared dimension".into())); } }
                } else if value.is_array() { self.push(span, format!("{type_name}.{name} is {ty}, not Vector"), None);
                } else if ty == "Point" && !(value.is_null() && *optional) {
                    if let Err(message) = Point::from_json(value) {
                        self.push(
                            span,
                            message,
                            Some("write `@point(latitude, longitude)`".into()),
                        );
                    }
                } else if ty != "Point" && Point::from_json(value).is_ok() {
                    self.push(span, format!("{type_name}.{name} is {ty}, not Point"), None);
                }
            }
        }
    }

    fn ensure_prop(&mut self, sel: &Selection, name: &str, span: Span) {
        if name == "@id" {
            return;
        }
        let types = selection_types(sel);
        if types.iter().any(|ty| self.schema.prop(ty, name).is_ok()) {
            return;
        }
        if matches!(name, "id" | "score") {
            self.push(span, format!("`{name}` is built in: write `@{name}`"), None);
            return;
        }
        for ty in &types {
            if let Some(edge) = find_edge(self.schema, ty, name) {
                if let Some((_, _, dir, targets, many)) = edge.as_edge() {
                    self.push(
                        span,
                        format!("{ty} has no field {name}"),
                        Some(format!(
                            "`{name}` is a relationship: `{name} {} {}`",
                            arrow(dir),
                            show_targets(targets, many)
                        )),
                    );
                    return;
                }
            }
        }
        self.push(
            span,
            format!("{} has no field {name}", sel.type_name),
            Some(field_help(self.schema, &types, name)),
        );
    }
}

fn selection_types(sel: &Selection) -> Vec<&str> {
    std::iter::once(sel.type_name.as_str())
        .chain(sel.also.iter().map(String::as_str))
        .collect()
}

fn find_edge<'a>(schema: &'a Schema, type_name: &str, field: &str) -> Option<&'a Field> {
    let ty = schema.types.iter().find(|ty| ty.name == type_name)?;
    ty.fields
        .iter()
        .find(|candidate| matches!(candidate, Field::Edge { field: name, .. } if name == field))
}

fn arrow(direction: Direction) -> &'static str {
    match direction {
        Direction::Out => "->",
        Direction::In => "<-",
    }
}

fn show_targets(targets: &[String], many: bool) -> String {
    let body = if targets.len() == 1 {
        targets[0].clone()
    } else {
        format!("({})", targets.join(" | "))
    };
    if many {
        format!("{body}[]")
    } else {
        body
    }
}

fn prop_help(schema: &Schema, type_name: &str, wanted: &str) -> String {
    let names: Vec<&str> = schema
        .types
        .iter()
        .find(|ty| ty.name == type_name)
        .map(|ty| {
            ty.fields
                .iter()
                .filter_map(|field| match field {
                    Field::Prop { name, .. } => Some(name.as_str()),
                    Field::Edge { .. } => None,
                })
                .collect()
        })
        .unwrap_or_default();
    if let Some(hit) = closest(wanted, names.iter().copied()) {
        format!("did you mean `{hit}`?")
    } else if names.is_empty() {
        format!("{type_name} has no fields")
    } else {
        format!("{type_name} has {}", names.join(", "))
    }
}

fn type_help(schema: &Schema, wanted: &str) -> String {
    let names: Vec<&str> = schema.types.iter().map(|ty| ty.name.as_str()).collect();
    if let Some(hit) = closest(wanted, names.iter().copied()) {
        format!("did you mean `{hit}`?")
    } else if names.is_empty() {
        "declare the type in the schema".to_string()
    } else {
        format!("types are {}", names.join(", "))
    }
}

fn field_help(schema: &Schema, type_names: &[&str], wanted: &str) -> String {
    let mut options: Vec<(&str, String)> = Vec::new();
    for ty_name in type_names {
        let Some(ty) = schema.types.iter().find(|ty| ty.name == *ty_name) else {
            continue;
        };
        for field in &ty.fields {
            match field {
                Field::Prop { name, .. } => options.push((name, format!("`{name}`"))),
                Field::Edge {
                    field,
                    direction,
                    targets,
                    many,
                    ..
                } => options.push((
                    field,
                    format!(
                        "`{field} {} {}`",
                        arrow(*direction),
                        show_targets(targets, *many)
                    ),
                )),
            }
        }
    }
    if let Some(hit) = closest(wanted, options.iter().map(|(name, _)| *name)) {
        let how = options
            .iter()
            .find(|(name, _)| *name == hit)
            .map(|(_, how)| how.as_str())
            .unwrap_or(hit);
        return format!("did you mean {how}?");
    }
    if options.is_empty() {
        return format!(
            "{} has no fields",
            type_names.first().copied().unwrap_or("this type")
        );
    }
    let list = options
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(", ");
    format!("{} has {list}", type_names[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marked<'a>(source: &'a str, diag: &Diagnostic) -> &'a str {
        let line = source.lines().nth(diag.line as usize - 1).unwrap();
        let start = diag.column as usize - 1;
        let end = start + diag.underline_length as usize;
        &line[start..end]
    }

    #[test]
    fn parses_schema_and_query() {
        let schema = parse_schema(
            r#"
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
            "#,
        )
        .unwrap();
        assert_eq!(schema.types.len(), 2);
        let query = parse_query(
            r#"
            {
              Author(name: "Le Guin") {
                name
                wrote -> Book(pages > 300) { title pages }
              }
            }
            "#,
        )
        .unwrap();
        assert!(!query.mutation);
        assert_eq!(query.root.unwrap().type_name, "Author");
    }

    #[test]
    fn query_keyword_wraps_a_read_and_an_empty_block_is_valid() {
        let wrapped = parse_query("query {\n  Author { name }\n}").unwrap();
        assert!(!wrapped.mutation);
        assert_eq!(wrapped.root.unwrap().type_name, "Author");
        let empty = parse_query("query { }").unwrap();
        assert!(empty.root.is_none());
        assert!(parse_query("mutation { }").unwrap().root.is_none());
        let comma = diagnose(
            "type Author {\n  name: String\n}\n",
            "{ Author(name: \"A\", name: \"B\") { name } }",
        );
        assert!(
            comma.diagnostics[0].message.contains("and is `&&`"),
            "{:?}",
            comma.diagnostics
        );
        let report = diagnose("type Author {\n  name: String\n}\n", "query { }");
        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    }

    #[test]
    fn unknown_field_underlines_the_name_and_suggests() {
        let schema = "type Player {\n  name: String\n  salary: Int\n}\n";
        let query = "{\n  Player {\n    slary\n  }\n}\n";
        let report = diagnose(schema, query);
        let diags = &report.diagnostics;
        assert!(report.text.contains("did you mean `salary`?"));
        assert!(report.text.contains("^^^^^"));
        assert_eq!(diags.len(), 1, "{diags:?}");
        let diag = &diags[0];
        assert_eq!(diag.pane, Pane::Query);
        assert_eq!(diag.message, "Player has no field slary");
        assert_eq!(diag.help.as_deref(), Some("did you mean `salary`?"));
        assert_eq!(marked(query, diag), "slary");
    }

    #[test]
    fn wrong_arrow_names_the_schema_direction() {
        let schema = "type Player {\n  playsFor -> Team\n}\ntype Team {\n  name: String\n}\n";
        let query = "{\n  Player {\n    playsFor <- Team { name }\n  }\n}\n";
        let report = diagnose(schema, query);
        let diag = report
            .diagnostics
            .iter()
            .find(|diag| diag.message.contains("does not point"))
            .unwrap();
        assert_eq!(diag.help.as_deref(), Some("`playsFor` points -> Team"));
        assert_eq!(marked(query, diag), "playsFor");
    }

    #[test]
    fn schema_unknown_type_is_a_schema_diagnostic() {
        let schema = "type Player {\n  playsFor -> Tea\n}\ntype Team {\n  name: String\n}\n";
        let report = diagnose(schema, "");
        let diags = &report.diagnostics;
        assert!(report.text.contains("schema:"));
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].pane, Pane::Schema);
        assert_eq!(
            diags[0].message,
            "Player.playsFor points at unknown type Tea"
        );
        assert_eq!(diags[0].help.as_deref(), Some("did you mean `Team`?"));
        assert_eq!(marked(schema, &diags[0]), "Tea");
    }

    #[test]
    fn unique_block_makes_each_field_unique_on_its_own() {
        let file = parse_zql(
            r#"
            schema {
              type Player { name: String salary: Int playsFor -> Team }
              type Team { name: String }
            }
            unique { Player { name salary } Team { name } }
            mutation { Player(name: "Connor McDavid") { name } }
            "#,
        )
        .unwrap();
        assert_eq!(
            file.uniques,
            vec![
                ("Player".into(), "name".into()),
                ("Player".into(), "salary".into()),
                ("Team".into(), "name".into()),
            ]
        );
        assert_eq!(file.statements.len(), 1);
        assert!(matches!(
            &file.statements[0],
            Statement::Run(query) if query.mutation
        ));
    }

    #[test]
    fn index_block_names_one_index_per_field_and_kind() {
        let file = parse_zql(
            r#"
            schema {
              type Player { name: String salary: Int rating?: Float }
            }
            index {
              range Player { salary rating }
              text Player { name }
            }
            unique { Player { name } }
            query { Player { name } }
            "#,
        )
        .unwrap();
        let spec = |kind, field: &str| IndexSpec {
            kind,
            type_name: "Player".into(),
            field: field.into(),
        };
        assert_eq!(
            file.indexes,
            vec![
                spec(IndexKind::Range, "salary"),
                spec(IndexKind::Range, "rating"),
                spec(IndexKind::Text, "name"),
            ]
        );
        assert_eq!(file.uniques, vec![("Player".into(), "name".into())]);
        assert_eq!(file.statements.len(), 1);
        // A unique String field brings its own range index.
        assert_eq!(
            effective_indexes(&file.schema, &file.uniques, &file.indexes).last(),
            Some(&spec(IndexKind::Range, "name"))
        );
        // Bare types stop at `index` the way they stop at `unique`.
        let bare = "type Player { name: String }\nindex { text Player { name } }\n";
        assert_eq!(parse_schema(bare).unwrap().types.len(), 1);
        assert_eq!(parse_indexes(bare).unwrap(), vec![spec(IndexKind::Text, "name")]);
    }

    #[test]
    fn bare_types_still_parse_beside_a_schema_wrapper() {
        let bare = parse_schema("type Player {\n  name: String\n}\n").unwrap();
        assert_eq!(bare.types.len(), 1);
        let source = "schema {\n  type Player { name: String }\n  type Team { name: String }\n}\nunique { Player { name } }\n";
        let wrapped = parse_schema(source).unwrap();
        assert_eq!(wrapped.types.len(), 2);
        assert_eq!(
            parse_uniques(source).unwrap(),
            vec![("Player".into(), "name".into())]
        );
    }

    #[test]
    fn unique_unknown_field_suggests_a_field() {
        let source =
            "schema {\n  type Player {\n    name: String\n  }\n}\nunique {\n  Player { nme }\n}\n";
        let report = diagnose(source, "");
        let diag = &report.diagnostics[0];
        assert_eq!(diag.pane, Pane::Schema);
        assert_eq!(diag.message, "Player has no field nme");
        assert_eq!(diag.help.as_deref(), Some("did you mean `name`?"));
        assert_eq!(marked(source, diag), "nme");
    }

    #[test]
    fn unique_rejects_a_relationship() {
        let err = parse_zql(
            "schema {\n  type Player { name: String playsFor -> Team }\n  type Team { name: String }\n}\nunique { Player { playsFor } }\n",
        )
        .unwrap_err();
        assert_eq!(err.message, "Player.playsFor is a relationship");
    }

    #[test]
    fn dollar_names_a_column_and_a_load_is_a_file() {
        let file = parse_zql(
            r#"
            schema { type Player { name: String salary: Int } }
            mutation csv ["./players.csv"] {
              Player(name: $Name && salary: $Salary) { name }
            }
            mutation json ["https://example.com/players.json"] {
              Player(name: $Name) { name }
            }
            "#,
        )
        .unwrap();
        let Statement::Load {
            format: LoadFormat::Csv,
            locations,
            template,
        } = &file.statements[0]
        else {
            panic!("csv file");
        };
        assert_eq!(locations, &["./players.csv".to_string()]);
        let mut row = std::collections::HashMap::new();
        row.insert("Name".into(), Json::from("Connor McDavid"));
        row.insert("Salary".into(), Json::from(12_500_000));
        let bound = bind_row(template, &row).unwrap().unwrap();
        let BoolExpr::And(left, right) = bound.root.unwrap().condition.unwrap() else {
            panic!("expected name && salary");
        };
        assert!(matches!(
            left.as_ref(),
            BoolExpr::Test(Pred::Eq(field, Json::String(text), _))
                if field == "name" && text == "Connor McDavid"
        ));
        assert!(matches!(
            right.as_ref(),
            BoolExpr::Test(Pred::Eq(field, value, _))
                if field == "salary" && value.as_i64() == Some(12_500_000)
        ));
        match &file.statements[1] {
            Statement::Load {
                format: LoadFormat::Json,
                locations,
                ..
            } => assert_eq!(locations[0], "https://example.com/players.json"),
            other => panic!("{other:?}"),
        }

        let pasted = parse_zql(
            r#"
            schema { type Player { name: String } }
            mutation json [{"Name": "Mitch Marner"}] { Player(name: $Name) { name } }
            "#,
        )
        .unwrap_err();
        assert!(pasted.message.contains("file"), "{pasted:?}");

        let inline = parse_zql(
            "schema { type Player { name: String } }\nmutation csv \"\"\"\nName\nA\n\"\"\" { Player(name: $Name) { name } }\n",
        )
        .unwrap_err();
        assert!(inline.message.contains("file"), "{inline:?}");

        let bare = parse_zql(
            r#"
            schema { type Player { name: String } }
            mutation csv ["./players.csv"] { Player(name: Name) { name } }
            "#,
        )
        .unwrap_err();
        assert!(bare.message.contains("`$`"), "{bare:?}");
    }

    #[test]
    fn edge_fields_are_declared_once_on_the_relationship() {
        let source = "type Team {\n  name: String\n  playsFor -> Player[] {\n    years: Int\n  }\n}\ntype Player {\n  name: String\n  playsFor <- Team\n}\n";
        let schema = parse_schema(source).unwrap();
        let team = schema.edge("Team", "playsFor").unwrap();
        let player = schema.edge("Player", "playsFor").unwrap();
        let names = |edge: &Field| match edge {
            Field::Edge { props, .. } => props
                .iter()
                .map(|field| field.name.clone())
                .collect::<Vec<_>>(),
            Field::Prop { .. } => Vec::new(),
        };
        assert_eq!(names(team), vec!["years".to_string()]);
        assert_eq!(names(player), vec!["years".to_string()]);
        let mismatch = parse_schema(
            "type Team {\n  playsFor -> Player[] { years: Int }\n}\ntype Player {\n  name: String\n  playsFor <- Team { years: String }\n}\n",
        )
        .unwrap_err();
        assert!(mismatch.message.contains("do not match"), "{mismatch:?}");
        let report = diagnose(
            source,
            "mutation {\n  Team(name: \"Oilers\") {\n    playsFor -> Player(name: \"Connor McDavid\") { name }\n  }\n}\n",
        );
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diag| diag.message.contains("requires &years")),
            "{:?}",
            report.diagnostics
        );
    }

    #[test]
    fn reports_every_type_error() {
        let schema = "type Player {\n  name: String\n  playsFor -> Team\n}\ntype Team {\n  name: String\n}\n";
        let query = "{\n  Player {\n    slary\n    playsFor <- Team { name }\n  }\n}\n";
        let report = diagnose(schema, query);
        assert!(report.diagnostics.len() >= 2, "{:?}", report.diagnostics);
        assert!(report.text.contains("\n\n"));
    }
}

#[cfg(test)]
mod display_tests;
