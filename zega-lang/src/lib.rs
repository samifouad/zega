//! The v2 schema and query language. Users write this. The engine walks the
//! graph it already stores; this crate does not parse ZQL.

use serde_json::Value as Json;
use thiserror::Error;
use zega_validation::{closest, render};

pub use zega_validation::{Diagnostic, Pane, Report, Severity};

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
    },
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
    let mut p = Parser::new(source);
    let mut types = Vec::new();
    p.skip();
    while !p.eof() {
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
        p.skip();
    }
    if types.is_empty() {
        return Err(p
            .err("schema has no types")
            .with_help("start with `type Name { }`"));
    }
    let schema = Schema { types };
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
    Ok(schema)
}

pub fn parse_query(source: &str) -> Result<Query> {
    let mut p = Parser::new(source);
    p.skip();
    let mutation = p.eat_word("mutation");
    if mutation {
        if p.eat_word("query") {
            return Err(p
                .err("a statement is a query or a mutation")
                .with_help("drop one of the words"));
        }
    } else {
        let _ = p.eat_word("query");
    }
    p.expect("{")?;
    p.skip();
    if p.eat("}") {
        p.skip();
        if !p.eof() {
            return Err(p.err("unexpected input"));
        }
        return Ok(Query {
            mutation,
            root: None,
        });
    }
    let root = p.parse_selection()?;
    p.expect("}")?;
    p.skip();
    if !p.eof() {
        return Err(p.err("unexpected input"));
    }
    Ok(Query {
        mutation,
        root: Some(root),
    })
}

struct Parser<'a> {
    src: &'a str,
    i: usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Self { src, i: 0 }
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
                return Ok(Field::Edge {
                    field: name,
                    rel,
                    direction,
                    targets,
                    target_spans,
                    many,
                });
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
        Ok(Field::Edge {
            field: name.clone(),
            rel: name,
            direction,
            targets,
            target_spans,
            many,
        })
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

    fn parse_value(&mut self) -> Result<Json> {
        self.skip();
        if self.src[self.i..].starts_with('"') {
            return Ok(Json::String(self.string()?));
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

/// Parse and type-check. An empty query reports nothing: the page is idle.
/// `text` is the report a terminal prints unchanged.
pub fn diagnose(schema_src: &str, query_src: &str) -> Report {
    let mut out = Vec::new();
    let schema = match parse_schema(schema_src) {
        Ok(schema) => Some(schema),
        Err(error) => {
            out.push(from_error(Pane::Schema, error));
            None
        }
    };
    if !query_src.trim().is_empty() {
        match parse_query(query_src) {
            Err(error) => out.push(from_error(Pane::Query, error)),
            Ok(query) => {
                if let (Some(schema), Some(root)) = (&schema, query.root.as_ref()) {
                    Check {
                        schema,
                        mutation: query.mutation,
                        out: &mut out,
                    }
                    .selection(root, true);
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
    out: &'a mut Vec<Diagnostic>,
}

impl Check<'_> {
    fn push(&mut self, span: Span, message: impl Into<String>, help: Option<String>) {
        self.out.push(Diagnostic::at(
            Pane::Query,
            span.line,
            span.column,
            span.end_line,
            span.end_column,
            message,
            help,
        ));
    }

    fn selection(&mut self, sel: &Selection, root: bool) {
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
                    if root {
                        self.push(
                            *span,
                            format!(
                                "&{name} is an edge field, and this value was not reached by an edge"
                            ),
                            Some(format!(
                                "read `&{name}` inside the type the edge lands on"
                            )),
                        );
                    }
                }
                Item::EdgeSet(name, _, span) => {
                    if !self.mutation {
                        self.push(
                            *span,
                            format!("&{name}: value is stored by a mutation"),
                            Some(format!(
                                "drop the value to read `&{name}`, or wrap the query in `mutation`"
                            )),
                        );
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
                    self.selection(target, false);
                }
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
    fn reports_every_type_error() {
        let schema = "type Player {\n  name: String\n  playsFor -> Team\n}\ntype Team {\n  name: String\n}\n";
        let query = "{\n  Player {\n    slary\n    playsFor <- Team { name }\n  }\n}\n";
        let report = diagnose(schema, query);
        assert!(report.diagnostics.len() >= 2, "{:?}", report.diagnostics);
        assert!(report.text.contains("\n\n"));
    }
}
