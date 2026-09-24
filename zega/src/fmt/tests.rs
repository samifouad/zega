use super::*;

#[test]
fn golden_query() {
    let source =
        "query{Player(salary>10000000&&position=\"C\"){name salary country->Country{name}}}";
    let expected = "query {\n  Player(salary > 10000000 && position = \"C\") {\n    name\n    salary\n    country -> Country { name }\n  }\n}\n";
    assert_eq!(format_zql(source).unwrap(), expected);
    assert_eq!(format_zql(expected).unwrap(), expected);
}
#[test]
fn parse_errors_unchanged() {
    for source in [
        "query {",
        "schema { type A { name: } }",
        "type A { name: String } trailing",
        "query { A(name: \"unterminated) }",
    ] {
        assert_eq!(format_zql(source).unwrap(), source);
    }
}

// Compare actual AST values, clearing only source locations.
fn ast(source: &str) -> Result<Parsed> {
    let mut parsed = Parsed::parse(source)?;
    fn span(s: &mut Span) {
        *s = Span {
            line: 0,
            column: 0,
            end_line: 0,
            end_column: 0,
        };
    }
    fn schema(s: &mut Schema) {
        for ty in &mut s.types {
            span(&mut ty.span);
            for field in &mut ty.fields {
                match field {
                    Field::Prop { .. } => {}
                    Field::Edge {
                        target_spans,
                        props,
                        props_span,
                        ..
                    } => {
                        for s in target_spans {
                            span(s);
                        }
                        for field in props {
                            span(&mut field.span);
                        }
                        if let Some(s) = props_span {
                            span(s);
                        }
                    }
                }
            }
        }
    }
    fn expr(e: &mut BoolExpr) {
        match e {
            BoolExpr::And(a, b) | BoolExpr::Or(a, b) => {
                expr(a);
                expr(b);
            }
            BoolExpr::Test(p) => match p {
                Pred::Similarity(sim, _, _) => span(&mut sim.span),
                Pred::Distance(distance, _, _) => span(&mut distance.span),
                Pred::Box(_, _, s)
                | Pred::Eq(_, _, s)
                | Pred::Ne(_, _, s)
                | Pred::Cmp(_, _, _, s)
                | Pred::Contains(_, _, s)
                | Pred::StartsWith(_, _, s)
                | Pred::EndsWith(_, _, s) => span(s),
            },
        }
    }
    fn selection(sel: &mut Selection) {
        span(&mut sel.type_span);
        for s in &mut sel.also_spans {
            span(s);
        }
        if let Some(e) = &mut sel.condition {
            expr(e);
        }
        for (_, _, s) in &mut sel.sets {
            span(s);
        }
        if let Some(near) = &mut sel.near {
            span(&mut near.similarity.span);
        }
        if let Some(order) = &mut sel.order {
            span(&mut order.span);
        }
        for item in &mut sel.items {
            match item {
                Item::Score(_, s)
                | Item::Prop(_, s)
                | Item::EdgeProp(_, s)
                | Item::EdgeSet(_, _, s) => span(s),
                Item::Similarity(_, sim) => span(&mut sim.span),
                Item::Distance(_, distance) => span(&mut distance.span),
                Item::Hops(_) | Item::Id(_) => {}
                Item::Walk {
                    span: s,
                    path,
                    target,
                    ..
                } => {
                    span(s);
                    if let Some(path) = path {
                        span(&mut path.span);
                        if let Some((_, s)) = &mut path.bound {
                            span(s);
                        }
                        if let Some((_, s)) = &mut path.weight {
                            span(s);
                        }
                        if let Some(toward) = &mut path.toward {
                            span(&mut toward.span);
                        }
                    }
                    selection(target);
                }
            }
        }
    }
    fn statements(statements: &mut [Statement]) {
        for stmt in statements {
            let query = match stmt {
                Statement::Run(q) | Statement::Load { template: q, .. } => q,
            };
            if let Some(root) = &mut query.root {
                selection(root);
            }
        }
    }
    match &mut parsed {
        Parsed::File(file) => {
            schema(&mut file.schema);
            statements(&mut file.statements);
        }
        Parsed::Schema(s, _) => schema(s),
        Parsed::Statements(s) => statements(s),
        Parsed::Empty => {}
    }
    Ok(parsed)
}
fn invariant(source: &str, label: &str) {
    let output = format_zql(source).unwrap_or_else(|e| panic!("{label}: {e}\n{source}"));
    assert_eq!(
        output,
        format_zql(&output).unwrap(),
        "idempotence: {label}\n{source}"
    );
    match ast(source) {
        Ok(before) => assert_eq!(
            before,
            ast(&output).unwrap_or_else(|e| panic!("{label}: {e}\n{output}")),
            "AST: {label}\n{output}"
        ),
        Err(_) => assert_eq!(source, output, "parse-error: {label}"),
    }
    let comments = |s: &str| {
        tokens(s)
            .into_iter()
            .filter(|t| t.text.starts_with("//"))
            .map(|t| t.text.trim_end().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(comments(source), comments(&output), "comments: {label}");
}
#[test]
fn every_repository_sample() {
    for (name, source) in [
        (
            "calgary",
            include_str!("../../../browser/samples/calgary.zql"),
        ),
        (
            "tickets",
            include_str!("../../../browser/samples/tickets.zql"),
        ),
    ] {
        invariant(source, name);
    }
}
#[test]
fn comments_everywhere() {
    let source = "// before\nquery // keyword\n{ // root\nPlayer // type\n( // condition\nname // field\n: // value\n\"https://a//b\" // string\n&& // and\nsalary > 1 // end filter\n) // body\n{ // selection\nname // between\nsalary // close selection\n} // close query\n} // eof\n";
    assert!(ast(source).is_ok());
    invariant(source, "comments everywhere");
}
#[test]
#[ignore = "requires the testsuite corpus at .tmp/corpus; CI runs this explicitly"]
fn external_corpus() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.tmp/corpus");
    assert!(
        root.exists(),
        "check out zegadb/testsuite tests at .tmp/corpus"
    );
    fn visit(path: &std::path::Path, n: &mut usize) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(&path, n);
            } else if path.extension().is_some_and(|e| e == "code") {
                let source = std::fs::read_to_string(&path).unwrap();
                assert!(!source.trim().is_empty(), "corpus source must not be a marker: {}", path.display());
                invariant(&source, &path.display().to_string());
                *n += 1;
            }
        }
    }
    let mut count = 0;
    visit(&root, &mut count);
    assert!(count > 200);
    eprintln!("verified {count} corpus sources");
}

#[test]
fn syntax_goldens() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/fmt/goldens");
    let mut count = 0;
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "input") {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap();
        let expected = std::fs::read_to_string(path.with_extension("expected")).unwrap();
        assert!(
            ast(&source).is_ok(),
            "golden must parse: {}",
            path.display()
        );
        assert_eq!(format_zql(&source).unwrap(), expected, "{}", path.display());
        invariant(&source, &path.display().to_string());
        count += 1;
    }
    assert_eq!(count, 18);
}

#[test]
#[ignore = "requires scripts/fmt-samples.py --extract .tmp/samples; CI runs this explicitly"]
fn every_embedded_sample() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.tmp/samples");
    let mut count = 0;
    for entry in std::fs::read_dir(root).expect("extract documentation samples first") {
        let path = entry.unwrap().path();
        invariant(
            &std::fs::read_to_string(&path).unwrap(),
            &path.display().to_string(),
        );
        count += 1;
    }
    assert!(count > 20);
    eprintln!("verified {count} embedded samples");
}

#[test]
fn positional_comments_cover_every_syntax_form() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/fmt/goldens");
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "expected") { continue; }
        let source = std::fs::read_to_string(&path).unwrap();
        let ts = tokens(&source);
        if ts.iter().any(|t| t.text.starts_with("//")) { continue; }
        let mut marked = String::from("// leading\n");
        let mut previous = 0;
        for (i, token) in ts.iter().enumerate() {
            let gap = &source[previous..token.start];
            if !gap.is_empty() { marked.push_str(&format!(" // comment {i}\n")); }
            marked.push_str(token.text);
            previous = token.end;
        }
        marked.push_str(" // trailing\n");
        assert!(ast(&marked).is_ok(), "commented fixture must still parse: {}", path.display());
        invariant(&marked, &path.display().to_string());
    }
}

#[test]
fn future_syntax_is_conservative_until_the_ast_supports_it() {
    for source in [
        "schema { type Contract { scan: String } display { graph { Contract(@shape: document, @image: &scan) } } }",
        "query { Person { name } } then { take 10 }",
    ] {
        assert!(Parsed::parse(source).is_err());
        assert_eq!(format_zql(source).unwrap(), source);
    }
}
