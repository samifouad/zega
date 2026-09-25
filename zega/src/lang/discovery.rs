//! APS 7: discovery expressions have their own atoms, with filter precedence.
use super::*;
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq)]
pub struct ThenStage {
    pub condition: DiscoveryExpr,
    pub skip: bool,
}

/// Like [`BoolExpr`](super::BoolExpr), `&&` and `||` are n-ary so a flat
/// chain does not nest; only parentheses do, bounded by
/// [`MAX_NESTING`](super::MAX_NESTING).
#[derive(Clone, Debug, PartialEq)]
pub enum DiscoveryExpr {
    Test(Primitive),
    And(Vec<Self>),
    Or(Vec<Self>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextOp {
    FindExact,
    FindWithout,
    StartsExact,
    EndsExact,
    /// Case- and accent-insensitive (zegadb/zega#98): see [`crate::text_fold`].
    FindLike,
    StartsLike,
    EndsLike,
    Regex,
}

#[derive(Clone, Debug)]
pub struct Pattern(pub regex::Regex);
impl PartialEq for Pattern {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_str() == other.0.as_str()
    }
}

pub type CommonType = (String, Vec<(String, Span)>, Span);

#[derive(Clone, Debug, PartialEq)]
pub enum Primitive {
    Text {
        op: TextOp,
        text: String,
        fields: Vec<(String, Span)>,
        pattern: Option<Pattern>,
        span: Span,
    },
    Common {
        types: Vec<CommonType>,
        span: Span,
    },
    Similar {
        field: String,
        threshold: f64,
        inclusive: bool,
        span: Span,
    },
    Near {
        field: String,
        metres: f64,
        inclusive: bool,
        span: Span,
    },
}

const PRIMITIVES: &[&str] = &[
    "findExact",
    "findWithout",
    "startsExact",
    "endsExact",
    "findLike",
    "startsLike",
    "endsLike",
    "regex",
    "common",
    "similar",
    "near",
];

impl Parser<'_> {
    pub(super) fn reject_detached_discovery(&mut self) -> Result<()> {
        if self.starts_word("then") {
            return Err(self.err("then requires a preceding query stage"));
        }
        if self.starts_word("display") {
            return Err(self.err("display { skip } must immediately follow a query or then stage"));
        }
        self.reject_discovery_block()
    }

    pub(super) fn reject_discovery_block(&self) -> Result<()> {
        if PRIMITIVES.iter().any(|name| self.starts_call(name, "{")) {
            return Err(self.err("discovery sub-blocks are only allowed inside then")
                .with_help("write `query { ... } then { findExact { \"text\" } }`; a filter uses `field findExact \"text\"`"));
        }
        Ok(())
    }

    // A type can still be called `findExact`. Only a literal in its field list
    // identifies an accidentally placed text primitive, without reserving names.
    pub(super) fn reject_discovery_literal(&self) -> Result<()> {
        for name in PRIMITIVES {
            let mut p = self.fork();
            if p.eat_word(name) && p.eat("{") {
                p.skip();
                if p.src[p.i..].starts_with(['"', '&']) {
                    return self.reject_discovery_block();
                }
            }
        }
        let mut p = self.fork();
        if p.discovery_atom().is_ok() {
            return self.reject_discovery_block();
        }
        Ok(())
    }

    pub(super) fn take_pipeline(&mut self, query: &mut Query) -> Result<()> {
        if query.mutation && (self.starts_word("then") || self.starts_word("display")) {
            return Err(self.err("then and display { skip } require a query, not a mutation"));
        }
        query.skip = self.stage_display()?;
        while self.eat_word("then") {
            self.expect("{")?;
            let condition = self.discovery_or()?;
            if !self.eat("}") {
                return Err(self.err("then has one condition; join sub-blocks with && or ||"));
            }
            query.then.push(ThenStage {
                condition,
                skip: self.stage_display()?,
            });
        }
        Ok(())
    }

    fn stage_display(&mut self) -> Result<bool> {
        if !self.eat_word("display") {
            return Ok(false);
        }
        self.expect("{")?;
        if !self.eat_word("skip") {
            return Err(self
                .err("a stage display block is `display { skip }`")
                .with_help("view declarations belong inside schema { display { ... } }"));
        }
        self.expect("}")?;
        Ok(true)
    }

    fn discovery_or(&mut self) -> Result<DiscoveryExpr> {
        let mut terms = vec![self.discovery_and()?];
        while self.eat("||") {
            terms.push(self.discovery_and()?);
        }
        Ok(if terms.len() == 1 { terms.remove(0) } else { DiscoveryExpr::Or(terms) })
    }
    fn discovery_and(&mut self) -> Result<DiscoveryExpr> {
        let mut terms = vec![self.discovery_atom()?];
        while self.eat("&&") {
            terms.push(self.discovery_atom()?);
        }
        Ok(if terms.len() == 1 { terms.remove(0) } else { DiscoveryExpr::And(terms) })
    }
    pub(super) fn discovery_atom(&mut self) -> Result<DiscoveryExpr> {
        self.skip();
        let start = self.i;
        if self.eat("(") {
            return self.nested(start, |p| {
                let inner = p.discovery_or()?;
                p.expect(")")?;
                Ok(inner)
            });
        }
        self.discovery_primitive()
    }

    /// One sub-block. Kept out of [`Self::discovery_atom`] so the frame that
    /// recurses per parenthesis stays small (zegadb/zega#48).
    #[inline(never)]
    fn discovery_primitive(&mut self) -> Result<DiscoveryExpr> {
        let (name, span) = self.ident()?;
        // zegadb/zega#98: same rename as the infix filter operators, only
        // caught here too because a discovery sub-block has its own name set.
        for (old, exact, like) in [
            ("findWith", "findExact", "findLike"),
            ("startsWith", "startsExact", "startsLike"),
            ("endsWith", "endsExact", "endsLike"),
        ] {
            if name == old {
                return Err(Error::at(span, format!("`{old}` was renamed `{exact}`"))
                    .with_help(format!("use `{exact}` for byte-exact matching, or `{like}` to ignore case and accents")));
            }
        }
        if !PRIMITIVES.contains(&name.as_str()) || !self.eat("{") {
            return Err(Error::at(span, "then requires discovery sub-blocks, such as `findExact { \"text\" }`")
                .with_help("infix filters need a field and belong in query, e.g. `Player(name findExact \"text\")`"));
        }
        let primitive = match name.as_str() {
            "common" => {
                let mut types = Vec::new();
                while !self.eat("}") {
                    let (ty, type_span) = self.ident()?;
                    let fields = self.discovery_fields()?;
                    types.push((ty, fields, type_span));
                }
                if types.is_empty() {
                    return Err(Error::at(span, "common needs at least one type and field"));
                }
                return Ok(DiscoveryExpr::Test(Primitive::Common { types, span }));
            }
            "similar" | "near" => {
                self.expect("&")?;
                let (field, span) = self.ident()?;
                let inclusive = if name == "similar" {
                    if self.eat(">=") {
                        true
                    } else {
                        self.expect(">")?;
                        false
                    }
                } else if self.eat("<=") {
                    true
                } else {
                    self.expect("<")?;
                    false
                };
                let threshold = self
                    .parse_value()?
                    .as_f64()
                    .filter(|n| n.is_finite())
                    .ok_or_else(|| Error::at(span, "discovery threshold must be finite"))?;
                if name == "similar" {
                    Primitive::Similar {
                        field,
                        threshold,
                        inclusive,
                        span,
                    }
                } else {
                    let (unit, unit_span) = self.ident()?;
                    let unit = DistanceUnit::parse(&unit)
                        .ok_or_else(|| Error::at(unit_span, "near unit must be m, km, or mi"))?;
                    let metres = threshold * unit.metres();
                    if metres < 0.0 || !metres.is_finite() {
                        return Err(Error::at(
                            span,
                            "near distance must be finite and non-negative",
                        ));
                    }
                    Primitive::Near {
                        field,
                        metres,
                        inclusive,
                        span,
                    }
                }
            }
            _ => {
                let text = self.string()?;
                let fields = if self.eat_word("in") {
                    self.discovery_fields()?
                } else {
                    Vec::new()
                };
                let op = match name.as_str() {
                    "findExact" => TextOp::FindExact,
                    "findWithout" => TextOp::FindWithout,
                    "startsExact" => TextOp::StartsExact,
                    "endsExact" => TextOp::EndsExact,
                    "findLike" => TextOp::FindLike,
                    "startsLike" => TextOp::StartsLike,
                    "endsLike" => TextOp::EndsLike,
                    _ => TextOp::Regex,
                };
                let pattern = if op == TextOp::Regex {
                    Some(Pattern(regex::Regex::new(&text).map_err(|error| Error::at(span,
                        format!("invalid regex: {error}"))
                        .with_help("Rust regex is linear-time; lookaround and backreferences are unsupported"))?))
                } else {
                    None
                };
                Primitive::Text {
                    op,
                    text,
                    fields,
                    pattern,
                    span,
                }
            }
        };
        self.expect("}")?;
        Ok(DiscoveryExpr::Test(primitive))
    }
    fn discovery_fields(&mut self) -> Result<Vec<(String, Span)>> {
        self.expect("{")?;
        let mut fields = Vec::new();
        while !self.eat("}") {
            let field = self.ident()?;
            if fields.iter().any(|(name, _)| name == &field.0) {
                return Err(Error::at(field.1, "a discovery field is listed twice"));
            }
            fields.push(field);
        }
        if fields.is_empty() {
            return Err(self.err("discovery field scope is empty"));
        }
        Ok(fields)
    }
}

fn selection_types<'a>(sel: &'a Selection, types: &mut BTreeSet<&'a str>) {
    types.insert(&sel.type_name);
    types.extend(sel.also.iter().map(String::as_str));
    for item in &sel.items {
        if let Item::Walk { target, .. } = item {
            selection_types(target, types);
        }
    }
}

pub(crate) fn check_pipeline(schema: &Schema, query: &Query) -> Result<()> {
    let mut types = BTreeSet::new();
    if let Some(root) = &query.root {
        selection_types(root, &mut types);
    }
    for stage in &query.then {
        check_expr(schema, &types, &stage.condition)?;
    }
    Ok(())
}

fn check_expr(schema: &Schema, types: &BTreeSet<&str>, expr: &DiscoveryExpr) -> Result<()> {
    match expr {
        DiscoveryExpr::And(terms) | DiscoveryExpr::Or(terms) => {
            for term in terms {
                check_expr(schema, types, term)?;
            }
            Ok(())
        }
        DiscoveryExpr::Test(Primitive::Text { fields, span, .. }) => {
            if fields.is_empty() {
                if !types.iter().any(|ty| {
                    schema.get(ty).is_ok_and(|t| {
                        t.fields
                            .iter()
                            .any(|f| matches!(f, Field::Prop { ty, .. } if ty == "String"))
                    })
                }) {
                    return Err(Error::at(
                        *span,
                        "text discovery needs a String field in the query result types",
                    ));
                }
            } else {
                for (field, span) in fields {
                    check_field(schema, types, field, *span, "String")?;
                }
            }
            Ok(())
        }
        DiscoveryExpr::Test(Primitive::Similar { field, span, .. }) => {
            check_field(schema, types, field, *span, "Vector")
        }
        DiscoveryExpr::Test(Primitive::Near { field, span, .. }) => {
            check_field(schema, types, field, *span, "Point")
        }
        DiscoveryExpr::Test(Primitive::Common {
            types: groups,
            span,
        }) => {
            let mut tuple_types: Option<Vec<String>> = None;
            let mut seen = BTreeSet::new();
            for (ty, fields, type_span) in groups {
                schema
                    .get(ty)
                    .map_err(|e| Error::at(*type_span, e.message))?;
                if !types.contains(ty.as_str()) {
                    return Err(Error::at(
                        *type_span,
                        format!("common type {ty} is not in the query result types"),
                    ));
                }
                if !seen.insert(ty) {
                    return Err(Error::at(
                        *type_span,
                        format!("common lists type {ty} twice"),
                    ));
                }
                let mut shape = Vec::new();
                for (field, field_span) in fields {
                    let Field::Prop { ty: field_ty, .. } = schema
                        .prop(ty, field)
                        .map_err(|e| Error::at(*field_span, e.message))?
                    else {
                        unreachable!()
                    };
                    shape.push(field_ty.clone());
                }
                if tuple_types
                    .as_ref()
                    .is_some_and(|expected| expected != &shape)
                {
                    return Err(Error::at(
                        *span,
                        "common types need the same number and types of fields, in the same order",
                    ));
                }
                tuple_types = Some(shape);
            }
            Ok(())
        }
    }
}

fn check_field(
    schema: &Schema,
    types: &BTreeSet<&str>,
    field: &str,
    span: Span,
    required: &str,
) -> Result<()> {
    let mut found = false;
    let mut vector = None;
    for ty in types {
        let definition = schema.get(ty)?;
        if let Some(property) = definition.fields.iter().find(|f| match f {
            Field::Prop { name, .. } => name == field,
            Field::Edge { field: name, .. } => name == field,
        }) {
            found = true;
            let Field::Prop { ty: actual, .. } = property else {
                return Err(Error::at(
                    span,
                    format!("{ty}.{field} is a relationship; discovery requires {required}"),
                ));
            };
            if required == "Vector" {
                let spec = VectorSpec::parse(actual).ok_or_else(|| {
                    Error::at(span, format!("{ty}.{field} must be Vector, found {actual}"))
                })?;
                if vector.is_some_and(|previous| previous != spec) {
                    return Err(Error::at(span, "similar requires matching Vector dimensions and metrics across query result types"));
                }
                vector = Some(spec);
            } else if actual != required {
                return Err(Error::at(
                    span,
                    format!("{ty}.{field} must be {required}, found {actual}"),
                ));
            }
        }
    }
    if !found {
        return Err(Error::at(
            span,
            format!("no field {field} in the query result types"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_and_parentheses_match_filters() {
        let q = parse_query(
            r#"query { A } then { findExact { "a" } || startsExact { "b" } && endsExact { "c" } }"#,
        )
        .unwrap();
        assert!(
            matches!(&q.then[0].condition, DiscoveryExpr::Or(terms) if matches!(terms.as_slice(), [_, DiscoveryExpr::And(..)]))
        );
        let q = parse_query(
            r#"query { A } then { (findExact { "a" } || startsExact { "b" }) && endsExact { "c" } }"#,
        )
        .unwrap();
        assert!(
            matches!(&q.then[0].condition, DiscoveryExpr::And(terms) if matches!(terms.as_slice(), [DiscoveryExpr::Or(..), _]))
        );
    }

    #[test]
    fn every_primitive_parses_and_checks() {
        let schema = parse_schema("type A { name: String vector: Vector<2> at: Point }").unwrap();
        for atom in [
            r#"findExact { "a" in { name } }"#,
            r#"findWithout { "a" }"#,
            r#"startsExact { "a" }"#,
            r#"endsExact { "a" }"#,
            r#"findLike { "a" in { name } }"#,
            r#"startsLike { "a" }"#,
            r#"endsLike { "a" }"#,
            r#"regex { "^a+$" }"#,
            "common { A { name } }",
            "similar { &vector > 0.9 }",
            "near { &at < 1 km }",
        ] {
            let q = parse_query(&format!("query {{ A }} then {{ {atom} }}")).unwrap();
            check_pipeline(&schema, &q).unwrap();
        }
    }

    #[test]
    fn infix_names_and_type_names_are_not_reserved() {
        for op in [
            "findExact",
            "startsExact",
            "endsExact",
            "findLike",
            "startsLike",
            "endsLike",
        ] {
            let query =
                parse_query(&format!("query {{ {op}(name {op} \"x\") {{ name }} }}")).unwrap();
            assert_eq!(query.root.unwrap().type_name, op);
        }
        // The retired name is still a fine type name; it just no longer works
        // as an operator (zegadb/zega#98).
        let query = parse_query(r#"query { findWith(name findExact "x") { name } }"#).unwrap();
        assert_eq!(query.root.unwrap().type_name, "findWith");
    }

    #[test]
    fn regex_is_compiled_and_unsupported_syntax_has_help() {
        let q = parse_query(r#"query { A } then { regex { "a+b" } }"#).unwrap();
        let DiscoveryExpr::Test(Primitive::Text {
            pattern: Some(pattern),
            ..
        }) = &q.then[0].condition
        else {
            panic!()
        };
        assert!(pattern.0.is_match("aaab"));
        for text in [r"(?=a)", r"(a)\1", "["] {
            let error = parse_query(&format!(
                "query {{ A }} then {{ regex {{ {} }} }}",
                serde_json::json!(text)
            ))
            .unwrap_err();
            assert!(error
                .help
                .unwrap()
                .contains("lookaround and backreferences"));
        }
    }

    #[test]
    fn stages_and_skip_bind_to_the_immediate_stage() {
        let query = parse_query(r#"query { A } display { skip } then { findExact { "a" } } then { findWithout { "b" } } display { skip }"#).unwrap();
        assert!(query.skip);
        assert!(!query.then[0].skip);
        assert!(query.then[1].skip);
        for source in [
            "then {}",
            "mutation { A } then {}",
            "display { skip } query { A }",
            r#"findExact { "a" }"#,
            r#"query { A(findExact { "a" }) }"#,
            r#"query { findExact { "a" } }"#,
            "query { common { A { name } } }",
            r#"query { A { findExact { "a" } } }"#,
            r#"query { A } then { findExact "a" }"#,
            r#"query { A } then { findExact { "a" } findExact { "b" } }"#,
        ] {
            assert!(parse_statement(source).is_err(), "{source}");
        }
    }
}
