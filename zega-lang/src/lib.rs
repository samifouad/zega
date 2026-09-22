//! The v2 schema and query language. Users write this. The engine walks the
//! graph it already stores; this crate does not parse ZQL.

use serde_json::Value as Json;
use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
#[error("{0}")]
pub struct Error(pub String);

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq)]
pub struct Schema {
    pub types: Vec<TypeDef>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeDef {
    pub name: String,
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
    pub root: Selection,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    pub type_name: String,
    /// Extra types when the query wrote `(Book | Movie)`.
    pub also: Vec<String>,
    pub predicates: Vec<Pred>,
    pub sets: Vec<(String, Json)>,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Prop(String),
    Hops,
    EdgeProp(String),
    /// `&year: 1974` on a mutation stores `year` on the edge that arrived here.
    EdgeSet(String, Json),
    Walk {
        field: String,
        range: Option<(usize, usize)>,
        link: bool,
        direction: Direction,
        target: Selection,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Pred {
    Eq(String, Json),
    Ne(String, Json),
    Cmp(String, Cmp, Json),
    Contains(String, String),
    StartsWith(String, String),
    EndsWith(String, String),
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
            .ok_or_else(|| Error(format!("unknown type {name}")))
    }

    pub fn edge<'a>(&'a self, type_name: &str, field: &str) -> Result<&'a Field> {
        let ty = self.get(type_name)?;
        ty.fields
            .iter()
            .find(|f| matches!(f, Field::Edge { field: name, .. } if name == field))
            .ok_or_else(|| Error(format!("{type_name} has no relationship {field}")))
    }

    pub fn prop<'a>(&'a self, type_name: &str, field: &str) -> Result<&'a Field> {
        let ty = self.get(type_name)?;
        ty.fields
            .iter()
            .find(|f| matches!(f, Field::Prop { name, .. } if name == field))
            .ok_or_else(|| Error(format!("{type_name} has no field {field}")))
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
        let name = p.ident()?;
        p.expect("{")?;
        let mut fields = Vec::new();
        while !p.eat("}") {
            fields.push(p.parse_field()?);
            p.skip();
        }
        if types.iter().any(|ty: &TypeDef| ty.name == name) {
            return Err(Error(format!("duplicate type {name}")));
        }
        types.push(TypeDef { name, fields });
        p.skip();
    }
    if types.is_empty() {
        return Err(Error("schema has no types".into()));
    }
    let schema = Schema { types };
    for ty in &schema.types {
        for field in &ty.fields {
            if let Field::Edge { targets, field, .. } = field {
                for target in targets {
                    schema.get(target).map_err(|_| {
                        Error(format!("{}.{} points at unknown type {target}", ty.name, field))
                    })?;
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
    p.expect("{")?;
    let root = p.parse_selection()?;
    p.expect("}")?;
    p.skip();
    if !p.eof() {
        return Err(Error(format!("unexpected {:?}", p.rest())));
    }
    Ok(Query { mutation, root })
}

struct Parser<'a> {
    src: &'a str,
    i: usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Self { src, i: 0 }
    }

    fn rest(&self) -> &str {
        &self.src[self.i..]
    }

    fn eof(&self) -> bool {
        self.i >= self.src.len()
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
            Err(Error(format!("expected {token}")))
        }
    }

    fn expect_word(&mut self, word: &str) -> Result<()> {
        self.expect(word)
    }

    fn ident(&mut self) -> Result<String> {
        self.skip();
        let start = self.i;
        let mut chars = self.src[self.i..].chars();
        let Some(first) = chars.next() else {
            return Err(Error("expected a name".into()));
        };
        if !first.is_ascii_alphabetic() && first != '_' {
            return Err(Error(format!("expected a name, found {}", self.rest())));
        }
        self.i += first.len_utf8();
        while let Some(c) = self.src[self.i..].chars().next() {
            if c.is_ascii_alphanumeric() || c == '_' {
                self.i += c.len_utf8();
            } else {
                break;
            }
        }
        Ok(self.src[start..self.i].to_string())
    }

    fn parse_field(&mut self) -> Result<Field> {
        let name = self.ident()?;
        let optional = self.eat("?");
        if self.eat(":") {
            self.skip();
            if self.looks_like_rel_name() {
                let rel = self.ident()?;
                let (direction, targets, many) = self.parse_arrow()?;
                if optional {
                    return Err(Error(format!("{name} cannot be optional and a relationship")));
                }
                return Ok(Field::Edge {
                    field: name,
                    rel,
                    direction,
                    targets,
                    many,
                });
            }
            let ty = self.ident()?;
            return Ok(Field::Prop {
                name,
                ty,
                optional,
            });
        }
        if optional {
            return Err(Error(format!("{name}? needs a type")));
        }
        let (direction, targets, many) = self.parse_arrow()?;
        Ok(Field::Edge {
            field: name.clone(),
            rel: name,
            direction,
            targets,
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

    fn parse_arrow(&mut self) -> Result<(Direction, Vec<String>, bool)> {
        let direction = if self.eat("->") {
            Direction::Out
        } else if self.eat("<-") {
            Direction::In
        } else {
            return Err(Error("expected -> or <-".into()));
        };
        let (targets, many) = self.parse_type_ref()?;
        Ok((direction, targets, many))
    }

    fn parse_type_ref(&mut self) -> Result<(Vec<String>, bool)> {
        self.skip();
        if self.eat("(") {
            let mut targets = vec![self.ident()?];
            while self.eat("|") {
                targets.push(self.ident()?);
            }
            self.expect(")")?;
            let many = self.eat("[]");
            return Ok((targets, many));
        }
        let name = self.ident()?;
        let many = self.eat("[]");
        Ok((vec![name], many))
    }

    fn parse_selection(&mut self) -> Result<Selection> {
        self.skip();
        let mut also = Vec::new();
        let type_name = if self.eat("(") {
            let type_name = self.ident()?;
            while self.eat("|") {
                also.push(self.ident()?);
            }
            self.expect(")")?;
            type_name
        } else {
            self.ident()?
        };
        let mut predicates = Vec::new();
        if self.eat("(") {
            self.skip();
            if !self.eat(")") {
                loop {
                    predicates.push(self.parse_pred()?);
                    self.skip();
                    if self.eat(")") {
                        break;
                    }
                    self.expect(",")?;
                }
            }
        }
        let mut sets = Vec::new();
        if self.eat_word("set") {
            loop {
                let field = self.ident()?;
                self.expect(":")?;
                sets.push((field, self.parse_value()?));
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
            also,
            predicates,
            sets,
            items,
        })
    }

    fn parse_item(&mut self) -> Result<Item> {
        self.skip();
        if self.eat("&") {
            let name = self.ident()?;
            if self.eat(":") {
                if name == "hops" {
                    return Err(Error("&hops is measured, not stored".into()));
                }
                return Ok(Item::EdgeSet(name, self.parse_value()?));
            }
            return Ok(if name == "hops" {
                Item::Hops
            } else {
                Item::EdgeProp(name)
            });
        }
        let field = self.ident()?;
        let range = if self.eat("*") {
            let min = self.integer()? as usize;
            self.expect("..")?;
            let max = self.integer()? as usize;
            if min < 1 || max < min {
                return Err(Error(format!("bad range *{min}..{max}")));
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
                range,
                link,
                direction,
                target,
            });
        }
        if range.is_some() {
            return Err(Error(format!("{field} has a range but no arrow")));
        }
        Ok(Item::Prop(field))
    }

    fn starts_with_arrow(&mut self) -> bool {
        self.skip();
        self.src[self.i..].starts_with("->") || self.src[self.i..].starts_with("<-")
    }

    fn parse_pred(&mut self) -> Result<Pred> {
        let field = self.ident()?;
        self.skip();
        if self.eat(":") || self.eat("=") {
            return Ok(Pred::Eq(field, self.parse_value()?));
        }
        if self.eat("<>") {
            return Ok(Pred::Ne(field, self.parse_value()?));
        }
        if self.eat(">=") {
            return Ok(Pred::Cmp(field, Cmp::Gte, self.parse_value()?));
        }
        if self.eat("<=") {
            return Ok(Pred::Cmp(field, Cmp::Lte, self.parse_value()?));
        }
        if self.eat(">") {
            return Ok(Pred::Cmp(field, Cmp::Gt, self.parse_value()?));
        }
        if self.eat("<") {
            return Ok(Pred::Cmp(field, Cmp::Lt, self.parse_value()?));
        }
        if self.eat_word("CONTAINS") {
            return Ok(Pred::Contains(field, self.string()?));
        }
        if self.eat_word("STARTS") {
            self.expect_word("WITH")?;
            return Ok(Pred::StartsWith(field, self.string()?));
        }
        if self.eat_word("ENDS") {
            self.expect_word("WITH")?;
            return Ok(Pred::EndsWith(field, self.string()?));
        }
        Err(Error(format!("expected a comparison after {field}")))
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
            return Err(Error("expected a string".into()));
        }
        self.i += 1;
        let mut out = String::new();
        while self.i < self.src.len() {
            let c = self.src[self.i..].chars().next().unwrap();
            self.i += c.len_utf8();
            if c == '"' {
                return Ok(out);
            }
            if c == '\\' {
                let e = self.src[self.i..].chars().next().ok_or_else(|| Error("bad string".into()))?;
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
        Err(Error("unterminated string".into()))
    }

    fn integer(&mut self) -> Result<i64> {
        match self.number_token()? {
            Json::Number(n) => n
                .as_i64()
                .ok_or_else(|| Error("expected an integer".into())),
            _ => Err(Error("expected an integer".into())),
        }
    }

    fn number_token(&mut self) -> Result<Json> {
        self.skip();
        let start = self.i;
        if self.src[self.i..].starts_with('-') {
            self.i += 1;
        }
        if !self.peek_digit() {
            return Err(Error("expected a number".into()));
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
            let n: f64 = text.parse().map_err(|_| Error(format!("bad number {text}")))?;
            Ok(Json::from(n))
        } else {
            let n: i64 = text.parse().map_err(|_| Error(format!("bad number {text}")))?;
            Ok(Json::from(n))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(query.root.type_name, "Author");
    }
}
