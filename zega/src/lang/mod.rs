//! The v2 schema and query language. Users write this. The engine walks the
//! graph it already stores; this crate does not parse ZQL.

use serde_json::Value as Json;
use thiserror::Error;
use crate::validation::{closest, render};

pub use crate::validation::{Diagnostic, Pane, Report};

/// A source range. Columns are 1-based and count UTF-16 code units, which is
/// what the editor uses. `end_column` is exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug, PartialEq)]
pub struct Schema {
    pub types: Vec<TypeDef>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeDef {
    pub name: String,
    pub span: Span,
    pub fields: Vec<Field>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Field {
    Prop {
        name: String,
        ty: String,
        optional: bool,
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
#[derive(Clone, Debug, PartialEq)]
pub struct EdgeField {
    pub name: String,
    pub ty: String,
    pub optional: bool,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Out,
    In,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    pub mutation: bool,
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
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Prop(String, Span),
    Hops,
    EdgeProp(String, Span),
    /// `&year: 1974` on a mutation stores `year` on the edge that arrived here.
    EdgeSet(String, Json, Span),
    Walk {
        field: String,
        span: Span,
        range: Option<(usize, usize)>,
        link: bool,
        direction: Direction,
        target: Box<Selection>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Pred {
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
            || p.starts_word("mutation")
            || p.starts_word("query")
        {
            break;
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
        types.push(TypeDef { name, span, fields });
    }
    if types.is_empty() {
        return Err(p
            .err("schema has no types")
            .with_help("start with `type Name { }`"));
    }
    let mut schema = Schema { types };
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
        _ => true,
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
        .all(|(a, b)| a.name == b.name && a.ty == b.ty && a.optional == b.optional)
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

/// A `.zql` file: `schema`, then `unique`, then `mutation` and `query` blocks.
#[derive(Clone, Debug, PartialEq)]
pub struct ZqlFile {
    pub schema: Schema,
    /// `(type, field)` pairs. Each field is unique on its own.
    pub uniques: Vec<(String, String)>,
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
    let uniques = p.take_uniques(&schema)?;
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
        statements,
    })
}

/// Unique fields declared in `source`, or an empty list when the text has no
/// `unique` block. Each name inside a type's braces is unique on its own.
pub fn parse_uniques(source: &str) -> Result<Vec<(String, String)>> {
    let (schema, end) = parse_schema_at(source)?;
    let mut p = Parser::new(source);
    p.i = end;
    p.take_uniques(&schema)
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
        Ok(Statement::Run(self.parse_braced(mutation)?))
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
                root: None,
            });
        }
        let root = self.parse_selection()?;
        self.expect("}")?;
        Ok(Query {
            mutation,
            root: Some(root),
        })
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
                while self.i < self.src.len() && !self.src[self.i..].starts_with('\n') {
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
            let (ty, _) = self.ident()?;
            return Ok(Field::Prop { name, ty, optional });
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
            let (ty, _) = self.ident()?;
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
            items,
        })
    }

    fn parse_item(&mut self) -> Result<Item> {
        self.skip();
        if self.eat("&") {
            let amp = self.i - 1;
            let (name, _) = self.ident()?;
            let span = self.span_bytes(amp, self.i);
            if self.eat(":") {
                if name == "hops" {
                    return Err(self
                        .err_at(span, "&hops is measured, not stored")
                        .with_help("`&hops` counts edges from the start of the query"));
                }
                return Ok(Item::EdgeSet(name, self.parse_value()?, span));
            }
            return Ok(if name == "hops" {
                Item::Hops
            } else {
                Item::EdgeProp(name, span)
            });
        }
        let (field, mut span) = self.ident()?;
        let range = if self.eat("*") {
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
                link,
                direction,
                target: Box::new(target),
            });
        }
        if range.is_some() {
            return Err(self
                .err_at(span, format!("{field} has a range but no arrow"))
                .with_help(format!("write `{field} *1..3 -> Type`")));
        }
        Ok(Item::Prop(field, span))
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
        let (field, span) = self.ident()?;
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
        if self.eat_word("CONTAINS") {
            return Ok(Pred::Contains(field, self.string()?, span));
        }
        if self.eat_word("STARTS") {
            self.expect_word("WITH")?;
            return Ok(Pred::StartsWith(field, self.string()?, span));
        }
        if self.eat_word("ENDS") {
            self.expect_word("WITH")?;
            return Ok(Pred::EndsWith(field, self.string()?, span));
        }
        Err(self
            .err(format!("expected a comparison after {field}"))
            .with_help(
            "use `=`, `!=`, `>`, `<`, `>=`, `<=`, `<>`, `CONTAINS`, `STARTS WITH`, or `ENDS WITH`",
        ))
    }

    fn looking_at_ident(&self) -> bool {
        self.src[self.i..]
            .trim_start()
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
    }

    fn parse_value(&mut self) -> Result<Json> {
        self.skip();
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
        let text = &self.src[start..self.i];
        if float {
            let n: f64 = text.parse().map_err(|_| {
                self.err_at(self.span_bytes(start, self.i), format!("bad number {text}"))
            })?;
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

/// Type-check a `unique` block and any `mutation` or `query` that follows the
/// types in the same text. The schema editor holds the whole file.
fn document_diagnostics(schema: &Schema, source: &str, out: &mut Vec<Diagnostic>) {
    let Ok((_, end)) = parse_schema_at(source) else {
        return;
    };
    let mut p = Parser::new(source);
    p.i = end;
    if let Err(error) = p.take_uniques(schema) {
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
                if pred.field() != "id" {
                    self.ensure_prop(sel, pred.field(), pred.span());
                }
            }
        }
        for (name, _, span) in &sel.sets {
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
                Item::Prop(name, span) => self.ensure_prop(sel, name, *span),
                Item::Hops => {}
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
                    direction,
                    target,
                    ..
                } => {
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

    fn ensure_prop(&mut self, sel: &Selection, name: &str, span: Span) {
        if name == "id" {
            return;
        }
        let types = selection_types(sel);
        if types.iter().any(|ty| self.schema.prop(ty, name).is_ok()) {
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
