//! The canonical ZQL formatter (APS 12), shared by the CLI and WASM.
//!
//! Like deka-fmt, parse first and leave incomplete input alone. The real parser
//! supplies the AST and item boundaries. Leaves retain original literal tokens;
//! comments are reattached positionally, never reconstructed from the AST.
use super::*;
mod json;
mod layout;
pub use json::format_json;
use layout::{display_fragment, fragment, join, tokens, Doc, Token};

/// Format a file, schema pane, or query pane with two-space indentation and an
/// 80-column soft target. Parse errors return the source byte-for-byte unchanged.
pub fn format_zql(source: &str) -> std::result::Result<String, String> {
    let parsed = match Parsed::parse(source) {
        Ok(parsed) => parsed,
        Err(_) => return Ok(source.to_owned()),
    };
    let mut printer = Printer {
        source,
        tokens: tokens(source),
        cursor: 0,
    };
    let doc = printer.document(&parsed).map_err(|e| e.to_string())?;
    Ok(doc.render())
}

#[derive(Debug, PartialEq)]
enum Parsed {
    Schema(Schema, Blocks),
    File(ZqlFile),
    Statements(Vec<Statement>),
    Empty,
}
impl Parsed {
    fn parse(source: &str) -> Result<Self> {
        let mut p = Parser::new(source);
        p.skip();
        if p.eof() {
            return Ok(Self::Empty);
        }
        if p.starts_word("schema") {
            return parse_zql(source).map(Self::File);
        }
        if p.starts_word("type") || p.starts_word("display") {
            let (schema, end) = parse_schema_at(source)?;
            p.i = end;
            let blocks = p.take_blocks(&schema)?;
            p.skip();
            if !p.eof() {
                return Err(p.err("unexpected input"));
            }
            return Ok(Self::Schema(schema, blocks));
        }
        let mut statements = Vec::new();
        while !p.eof() {
            statements.push(p.parse_statement()?);
            p.skip();
        }
        Ok(Self::Statements(statements))
    }
}

struct Printer<'a> {
    source: &'a str,
    tokens: Vec<Token<'a>>,
    cursor: usize,
}
impl<'a> Printer<'a> {
    fn parser(&self) -> Parser<'a> {
        let mut p = Parser::new(self.source);
        p.i = self
            .tokens
            .get(self.cursor)
            .map_or(self.source.len(), |t| t.start);
        p.skip();
        p
    }
    fn peek(&self) -> &str {
        self.tokens[self.cursor..]
            .iter()
            .find(|t| !t.text.starts_with("//"))
            .map_or("", |t| t.text)
    }
    fn until(&mut self, end: usize, types: bool) -> Doc {
        fragment(self.until_tokens(end), types)
    }
    fn until_tokens(&mut self, end: usize) -> &[Token<'a>] {
        let start = self.cursor;
        while self.cursor < self.tokens.len() && self.tokens[self.cursor].start < end {
            self.cursor += 1;
        }
        while self
            .tokens
            .get(self.cursor)
            .is_some_and(|t| t.inline_comment)
        {
            self.cursor += 1;
        }
        &self.tokens[start..self.cursor]
    }
    fn token(&mut self) -> Doc {
        let mut end = self.cursor;
        while end < self.tokens.len() && self.tokens[end].text.starts_with("//") {
            end += 1;
        }
        self.until(
            self.tokens.get(end).map_or(self.source.len(), |t| t.end),
            false,
        )
    }
    fn block(&mut self, header: Doc, items: Vec<Node>, style: Block) -> Node {
        // Closing-brace trivia stays inside the block, including an empty one.
        let start = self.cursor;
        while self.cursor < self.tokens.len() && self.tokens[self.cursor].text.starts_with("//") {
            self.cursor += 1;
        }
        let comments = fragment(&self.tokens[start..self.cursor], false);
        let has_comments = start != self.cursor;
        let close = self.token();
        if items.is_empty() && !has_comments {
            return Node {
                doc: Doc::seq([
                    header,
                    if style == Block::Top {
                        Doc::Hard
                    } else {
                        Doc::text("")
                    },
                    close,
                ]),
                block: true,
            };
        }
        let multiline = matches!(style, Block::Top | Block::Schema)
            || items.len() > if style == Block::Fields { 1 } else { 2 }
            || (style == Block::Selection && items.iter().any(|item| item.block))
            || has_comments
            || header.width() == usize::MAX;
        let sep = if multiline { Doc::Hard } else { Doc::Line(" ") };
        let body = join(
            items.into_iter().map(|n| n.doc).collect(),
            if style == Block::Schema {
                Doc::Blank
            } else {
                sep.clone()
            },
        );
        let mut parts = Vec::new();
        if has_comments {
            parts.push(Doc::seq([sep.clone(), body, Doc::Hard, comments]).nest());
            parts.push(close);
        } else {
            parts.push(Doc::seq([sep.clone(), body]).nest());
            parts.push(sep);
            parts.push(close);
        }
        let doc = Doc::seq(parts);
        Node {
            doc: Doc::seq([header, if multiline { doc } else { doc.group() }]),
            block: true,
        }
    }
    fn document(&mut self, parsed: &Parsed) -> Result<Doc> {
        let mut blocks = Vec::new();
        match parsed {
            Parsed::Schema(schema, _) => self.schema(schema, false, &mut blocks)?,
            Parsed::File(ZqlFile {
                schema,
                uniques: _,
                indexes: _,
                statements,
            }) => {
                self.schema(schema, true, &mut blocks)?;
                for statement in statements {
                    blocks.push(self.statement(statement)?.doc);
                }
            }
            Parsed::Statements(statements) => {
                for statement in statements {
                    blocks.push(self.statement(statement)?.doc);
                }
            }
            Parsed::Empty => {}
        }
        if self.tokens[self.cursor..]
            .iter()
            .any(|t| !t.text.starts_with("//"))
        {
            return Err(Error::bare("formatter did not consume input"));
        }
        let tail = self.until(self.source.len(), false);
        if self.tokens.last().is_some_and(|t| t.text.starts_with("//")) {
            blocks.push(tail);
        }
        Ok(join(blocks, Doc::Blank))
    }
    fn schema(&mut self, schema: &Schema, wrapped: bool, blocks: &mut Vec<Doc>) -> Result<()> {
        let header = if wrapped {
            let mut p = self.parser();
            p.expect_word("schema")?;
            p.expect("{")?;
            Some(self.until(p.i, false))
        } else {
            None
        };
        let Schema {
            types: definitions,
            display:
                DisplayConfig {
                    views: _,
                    default: _,
                },
        } = schema;
        let mut items = Vec::new();
        let mut types = definitions.iter();
        while matches!(self.peek(), "type" | "display") {
            let node = if self.peek() == "display" {
                self.display()?
            } else {
                self.type_def(
                    types
                        .next()
                        .ok_or_else(|| Error::bare("missing type AST"))?,
                )?
            };
            if wrapped {
                items.push(node);
            } else {
                blocks.push(node.doc);
            }
        }
        if let Some(header) = header {
            blocks.push(self.block(header, items, Block::Schema).doc);
        }
        while matches!(self.peek(), "unique" | "index") {
            blocks.push(self.constraints(schema)?.doc);
        }
        Ok(())
    }
    fn type_def(&mut self, ty: &TypeDef) -> Result<Node> {
        let TypeDef {
            name: _,
            span: _,
            fields,
            timeline_field: _,
        } = ty;
        let mut p = self.parser();
        p.expect_word("type")?;
        p.ident()?;
        p.expect("{")?;
        let header = self.until(p.i, false);
        let mut items = Vec::new();
        for field in fields {
            items.push(self.field(field)?);
        }
        Ok(self.block(header, items, Block::Fields))
    }
    fn field(&mut self, field: &Field) -> Result<Node> {
        let mut p = self.parser();
        let parsed = p.parse_field()?;
        let end = p.i;
        match field {
            Field::Prop {
                name: _,
                ty: _,
                optional: _,
                from: _,
                unit,
            } => {
                if let Some(unit) = unit {
                    unit_text(*unit);
                }
                Ok(Node::leaf(self.until(end, true)))
            }
            Field::Edge {
                field: _,
                rel: _,
                direction,
                targets: _,
                target_spans: _,
                many: _,
                props: _,
                props_span: _,
            } => {
                direction_text(*direction);
                // Use the written fields, not the engine's inherited inverse
                // relationship fields; inserting the latter changes the AST.
                let Field::Edge {
                    props, props_span, ..
                } = parsed
                else {
                    return Err(Error::bare("expected edge"));
                };
                if props_span.is_none() {
                    return Ok(Node::leaf(self.until(end, true)));
                }
                let open = self.tokens[self.cursor..]
                    .iter()
                    .find(|t| t.text == "{")
                    .unwrap()
                    .end;
                let header = self.until(open, true);
                let mut items = Vec::new();
                for (i, prop) in props.iter().enumerate() {
                    let EdgeField {
                        name: _,
                        ty: _,
                        optional: _,
                        span: _,
                        unit,
                    } = prop;
                    if let Some(unit) = unit {
                        unit_text(*unit);
                    }
                    let end = props.get(i + 1).map_or_else(
                        || self.tokens.iter().find(|t| t.end == end).unwrap().start,
                        |next| self.byte(next.span),
                    );
                    // Don't attach next field's leading comments to this leaf.
                    let end = self.before_comments(end);
                    items.push(Node::leaf(self.until(end, true)));
                }
                Ok(self.block(header, items, Block::Fields))
            }
        }
    }
    fn before_comments(&self, end: usize) -> usize {
        let mut i = self.tokens.partition_point(|t| t.start < end);
        while i > self.cursor && self.tokens[i - 1].text.starts_with("//") {
            i -= 1;
        }
        self.tokens.get(i).map_or(end, |t| t.start)
    }
    fn byte(&self, span: Span) -> usize {
        let mut line = 1;
        let mut column = 1;
        for (i, c) in self.source.char_indices() {
            if line == span.line && column == span.column {
                return i;
            }
            if c == '\n' {
                line += 1;
                column = 1;
            } else {
                column += c.len_utf16() as u32;
            }
        }
        self.source.len()
    }
    fn display(&mut self) -> Result<Node> {
        let mut p = self.parser();
        let DisplayBlock { entries, span: _ } = p.parse_display()?;
        let mut head = self.parser();
        head.ident()?;
        head.expect("{")?;
        let header = self.until(head.i, false);
        let mut items = Vec::new();
        for entry in entries {
            let DisplayEntry {
                view:
                    DisplayView {
                        kind,
                        types,
                        nodes: _,
                        globe: _,
                    },
                span: _,
                settings,
                type_spans: _,
                attributes,
                default_span,
            } = entry;
            let mut p = self.parser();
            p.expect_word(globe::view_name(kind))?;
            // `globe(@zoom: …)` stays one leaf with the view name.
            if settings.is_some() {
                p.parse_view_settings()?;
            }
            let mut node = if let Some(types) = types {
                p.expect("{")?;
                let h = self.until(p.i, false);
                let mut names = Vec::new();
                for (_, attributes) in types.iter().zip(&attributes) {
                    let mut p = self.parser();
                    p.ident()?;
                    p.parse_display_attributes()?;
                    p.eat(",");
                    names.push(Node::leaf(display_fragment(
                        self.until_tokens(p.i),
                        attributes.len() > 2,
                    )));
                }
                self.block(h, names, Block::Subblock)
            } else {
                Node::leaf(self.until(p.i, false))
            };
            if default_span.is_some() {
                let mut p = self.parser();
                p.expect(":")?;
                p.ident()?;
                node.doc = Doc::seq([node.doc, Doc::text(" "), self.until(p.i, false)]);
            }
            items.push(node);
        }
        Ok(self.block(header, items, Block::Top))
    }
    fn constraints(&mut self, schema: &Schema) -> Result<Node> {
        let index = self.peek() == "index";
        let mut check = self.parser();
        if index {
            for (
                IndexSpec {
                    kind,
                    type_name: _,
                    field: _,
                },
                _,
            ) in check.take_indexes(schema)?
            {
                match kind {
                    IndexKind::Range | IndexKind::Text => {}
                }
            }
        } else {
            check.take_uniques(schema)?;
        }
        let mut p = self.parser();
        p.ident()?;
        p.expect("{")?;
        let header = self.until(p.i, false);
        let mut groups = Vec::new();
        while self.peek() != "}" {
            let mut p = self.parser();
            if index {
                p.ident()?;
            }
            p.ident()?;
            p.expect("{")?;
            let h = self.until(p.i, false);
            let mut fields = Vec::new();
            while self.peek() != "}" {
                let mut p = self.parser();
                p.ident()?;
                fields.push(Node::leaf(self.until(p.i, false)));
            }
            groups.push(self.block(h, fields, Block::Selection));
        }
        Ok(self.block(header, groups, Block::Top))
    }
    fn statement(&mut self, statement: &Statement) -> Result<Node> {
        let (query, columns) = match statement {
            Statement::Run(query) => (query, false),
            Statement::Load {
                format,
                locations: _,
                template,
            } => {
                match format {
                    LoadFormat::Json | LoadFormat::Csv => {}
                }
                (template, true)
            }
        };
        let Query {
            mutation: _,
            root,
            skip,
            then,
        } = query;
        let open = self.tokens[self.cursor..]
            .iter()
            .find(|t| t.text == "{")
            .ok_or_else(|| Error::bare("missing query brace"))?
            .end;
        let header = self.until(open, false);
        let items = if let Some(root) = root {
            vec![self.selection(root, columns)?]
        } else {
            Vec::new()
        };
        let mut stages = vec![self.block(header, items, Block::Top).doc];
        if *skip {
            stages.push(self.stage_display()?.doc);
        }
        for ThenStage { condition, skip } in then {
            let header = Doc::seq([self.token(), Doc::text(" "), self.token()]);
            let mut condition = self.discovery(condition)?;
            condition.doc = condition.doc.group();
            stages.push(self.block(header, vec![condition], Block::Top).doc);
            if *skip {
                stages.push(self.stage_display()?.doc);
            }
        }
        Ok(Node {
            doc: join(stages, Doc::Blank),
            block: true,
        })
    }
    fn stage_display(&mut self) -> Result<Node> {
        let header = Doc::seq([self.token(), Doc::text(" "), self.token()]);
        let skip = Node::leaf(self.token());
        Ok(self.block(header, vec![skip], Block::Top))
    }
    fn discovery(&mut self, expr: &DiscoveryExpr) -> Result<Node> {
        // The parser has already checked precedence; retain written parentheses.
        if self.peek() == "(" && self.parser().discovery_atom()? == *expr {
            let open = self.token();
            let inner = self.discovery(expr)?;
            let close = self.token();
            return Ok(Node {
                doc: Doc::seq([
                    open,
                    Doc::seq([Doc::Line(""), inner.doc]).nest(),
                    Doc::Line(""),
                    close,
                ])
                .group(),
                block: inner.block,
            });
        }
        match expr {
            DiscoveryExpr::And(terms) | DiscoveryExpr::Or(terms) => {
                // One group at the stage/parenthesis boundary breaks every
                // operand together. A chain is one node, walked in a loop.
                let mut parts = Vec::with_capacity(terms.len() * 4);
                for (n, term) in terms.iter().enumerate() {
                    if n > 0 {
                        parts.extend([Doc::text(" "), self.token(), Doc::Line(" ")]);
                    }
                    parts.push(self.discovery(term)?.doc);
                }
                Ok(Node {
                    doc: Doc::seq(parts),
                    block: true,
                })
            }
            DiscoveryExpr::Test(primitive) => {
                let header = Doc::seq([self.token(), Doc::text(" "), self.token()]);
                let mut items = Vec::new();
                match primitive {
                    Primitive::Common { types, span: _ } => {
                        for (_, fields, _) in types {
                            let h = Doc::seq([self.token(), Doc::text(" "), self.token()]);
                            let mut names = Vec::new();
                            for _ in fields {
                                let mut p = self.parser();
                                p.ident()?;
                                names.push(Node::leaf(self.until(p.i, false)));
                            }
                            items.push(self.block(h, names, Block::Selection));
                        }
                    }
                    Primitive::Text {
                        op,
                        text: _,
                        fields,
                        pattern: _,
                        span: _,
                    } => {
                        match op {
                            TextOp::FindExact
                            | TextOp::FindWithout
                            | TextOp::StartsExact
                            | TextOp::EndsExact
                            | TextOp::FindLike
                            | TextOp::StartsLike
                            | TextOp::EndsLike
                            | TextOp::Regex => {}
                        }
                        if fields.is_empty() {
                            items.push(self.discovery_leaf());
                        } else {
                            let mut p = self.parser();
                            p.string()?;
                            p.expect_word("in")?;
                            p.expect("{")?;
                            let h = self.until(p.i, false);
                            let mut names = Vec::new();
                            for _ in fields {
                                let mut p = self.parser();
                                p.ident()?;
                                names.push(Node::leaf(self.until(p.i, false)));
                            }
                            items.push(self.block(h, names, Block::Selection));
                        }
                    }
                    Primitive::Similar {
                        field: _,
                        threshold: _,
                        inclusive: _,
                        span: _,
                    }
                    | Primitive::Near {
                        field: _,
                        metres: _,
                        inclusive: _,
                        span: _,
                    } => items.push(self.discovery_leaf()),
                }
                Ok(self.block(header, items, Block::Subblock))
            }
        }
    }
    fn discovery_leaf(&mut self) -> Node {
        let end = self.tokens[self.cursor..]
            .iter()
            .find(|t| t.text == "}")
            .unwrap()
            .start;
        Node::leaf(self.until(self.before_comments(end), false))
    }
    fn selection(&mut self, selection: &Selection, columns: bool) -> Result<Node> {
        let Selection {
            type_name: _,
            type_span: _,
            also: _,
            also_spans: _,
            condition,
            sets: _,
            near,
            order,
            limit: _,
            items,
            delete: _,
        } = selection;
        if let Some(condition) = condition {
            condition_forms(condition);
        }
        if let Some(Near {
            similarity,
            k: _,
            exact: _,
        }) = near
        {
            similarity_form(similarity);
        }
        for OrderKey { by, desc: _, span: _ } in order {
            match by {
                OrderBy::Field(_) => {}
                OrderBy::Distance(distance) => distance_form(distance),
            }
        }
        let mut p = self.parser();
        p.columns = columns;
        p.parse_selection()?;
        let end = p.i;
        if let Some(open) = self.tokens[self.cursor..]
            .iter()
            .take_while(|t| t.start < end)
            .find(|t| t.text == "{")
            .map(|t| t.end)
        {
            let header = self.until(open, false);
            let mut children = Vec::new();
            for item in items {
                children.push(self.item(item, columns)?);
            }
            Ok(self.block(header, children, Block::Selection))
        } else {
            Ok(Node::leaf(self.until(self.before_comments(end), false)))
        }
    }
    fn item(&mut self, item: &Item, columns: bool) -> Result<Node> {
        let mut p = self.parser();
        p.columns = columns;
        p.parse_item()?;
        let end = p.i;
        match item {
            Item::Walk {
                field: _,
                span: _,
                range: _,
                path,
                link: _,
                direction,
                target,
            } => {
                direction_text(*direction);
                if let Some(PathSpec {
                    span: _,
                    bound,
                    weight: _,
                    toward,
                }) = path
                {
                    if let Some((bound, _)) = bound {
                        match bound {
                            PathBound::Hops(_)
                            | PathBound::Cost {
                                limit: _,
                                inclusive: _,
                            } => {}
                        }
                    }
                    if let Some(Toward { field: _, span: _ }) = toward {}
                }
                let arrow = self.tokens[self.cursor..]
                    .iter()
                    .find(|t| matches!(t.text, "->" | "<-"))
                    .unwrap()
                    .end;
                let mut p = self.parser();
                p.i = arrow;
                p.eat_word("link");
                let header = self.until(p.i, false);
                let target = self.selection(target, columns)?;
                Ok(Node {
                    doc: Doc::seq([header, Doc::text(" "), target.doc]),
                    block: true,
                })
            }
            Item::Similarity(_, similarity) => {
                similarity_form(similarity);
                Ok(Node::leaf(self.until(end, false)))
            }
            Item::Distance(_, distance) => {
                distance_form(distance);
                Ok(Node::leaf(self.until(end, false)))
            }
            Item::Score(_, _)
            | Item::Prop(_, _)
            | Item::Hops(_)
            | Item::Id(_)
            | Item::Detach(_)
            | Item::EdgeProp(_, _)
            | Item::EdgeSet(_, _, _) => Ok(Node::leaf(self.until(end, false))),
        }
    }
}
#[derive(Clone, Copy, PartialEq)]
enum Block {
    Top,
    Schema,
    Fields,
    Selection,
    Subblock,
}

struct Node {
    doc: Doc,
    block: bool,
}
impl Node {
    fn leaf(doc: Doc) -> Self {
        Self { doc, block: false }
    }
}

// Enumerate every nested AST form as well as every block/item above. Literal
// leaves retain source tokens because serializing their runtime values can lose
// spelling or precision. No catch-all arms: new syntax must choose a layout.
fn condition_forms(expr: &BoolExpr) {
    match expr {
        BoolExpr::And(terms) | BoolExpr::Or(terms) => {
            for term in terms {
                condition_forms(term);
            }
        }
        BoolExpr::Test(pred) => match pred {
            Pred::Similarity(similarity, cmp, _) => {
                similarity_form(similarity);
                cmp_text(*cmp);
            }
            Pred::Distance(distance, cmp, _) => {
                distance_form(distance);
                cmp_text(*cmp);
            }
            Pred::Cmp(_, cmp, _, _) => {
                cmp_text(*cmp);
            }
            Pred::Box(_, _, _)
            | Pred::Eq(_, _, _)
            | Pred::Ne(_, _, _)
            | Pred::FindExact(_, _, _)
            | Pred::StartsExact(_, _, _)
            | Pred::EndsExact(_, _, _)
            | Pred::FindLike(_, _, _)
            | Pred::StartsLike(_, _, _)
            | Pred::EndsLike(_, _, _) => {}
        },
    }
}
fn similarity_form(
    Similarity {
        field: _,
        query: _,
        span: _,
    }: &Similarity,
) {
}
fn distance_form(
    Distance {
        field: _,
        origin: _,
        span: _,
    }: &Distance,
) {
}
fn direction_text(direction: Direction) -> &'static str {
    match direction {
        Direction::Out => "->",
        Direction::In => "<-",
    }
}
fn unit_text(unit: DistanceUnit) -> &'static str {
    match unit {
        DistanceUnit::Metres => "m",
        DistanceUnit::Kilometres => "km",
        DistanceUnit::Miles => "mi",
    }
}
fn cmp_text(cmp: Cmp) -> &'static str {
    match cmp {
        Cmp::Gt => ">",
        Cmp::Lt => "<",
        Cmp::Gte => ">=",
        Cmp::Lte => "<=",
    }
}

#[cfg(test)]
mod tests;
