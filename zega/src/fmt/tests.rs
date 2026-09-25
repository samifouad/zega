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
            BoolExpr::And(terms) | BoolExpr::Or(terms) => {
                terms.iter_mut().for_each(expr);
            }
            BoolExpr::Test(p) => match p {
                Pred::Similarity(sim, _, _) => span(&mut sim.span),
                Pred::Distance(distance, _, _) => span(&mut distance.span),
                Pred::Box(_, _, s)
                | Pred::Eq(_, _, s)
                | Pred::Ne(_, _, s)
                | Pred::Cmp(_, _, _, s)
                | Pred::FindExact(_, _, s)
                | Pred::StartsExact(_, _, s)
                | Pred::EndsExact(_, _, s)
                | Pred::FindLike(_, _, s)
                | Pred::StartsLike(_, _, s)
                | Pred::EndsLike(_, _, s) => span(s),
                Pred::Related(related) => {
                    span(&mut related.span);
                    selection(&mut related.target);
                }
                Pred::Count(count, _, _) => count_spans(count),
            },
        }
    }
    fn count_spans(count: &mut Count) {
        span(&mut count.span);
        if let Some((_, target)) = &mut count.to {
            selection(target);
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
        for key in &mut sel.order {
            span(&mut key.span);
            match &mut key.by {
                OrderBy::Distance(distance) => span(&mut distance.span),
                OrderBy::Count(count) => count_spans(count),
                OrderBy::Field(_) => {}
            }
        }
        if let Some(s) = &mut sel.delete {
            span(s);
        }
        for item in &mut sel.items {
            match item {
                Item::Score(_, s)
                | Item::Prop(_, s)
                | Item::EdgeProp(_, s)
                | Item::Detach(s)
                | Item::EdgeSet(_, _, s) => span(s),
                Item::Similarity(_, sim) => span(&mut sim.span),
                Item::Distance(_, distance) => span(&mut distance.span),
                Item::Count(_, count) => count_spans(count),
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
    fn discovery(expr: &mut DiscoveryExpr) {
        match expr {
            DiscoveryExpr::And(terms) | DiscoveryExpr::Or(terms) => {
                terms.iter_mut().for_each(discovery);
            }
            DiscoveryExpr::Test(p) => match p {
                Primitive::Text {
                    fields, span: s, ..
                } => {
                    span(s);
                    for (_, s) in fields {
                        span(s);
                    }
                }
                Primitive::Common { types, span: s } => {
                    span(s);
                    for (_, fields, s) in types {
                        span(s);
                        for (_, s) in fields {
                            span(s);
                        }
                    }
                }
                Primitive::Similar { span: s, .. } | Primitive::Near { span: s, .. } => span(s),
            },
        }
    }
    fn statements(statements: &mut [Statement]) {
        for stmt in statements {
            let query = match stmt {
                Statement::Run(q) | Statement::Load { template: q, .. } => q,
            };
            for stage in &mut query.then {
                discovery(&mut stage.condition);
            }
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
                assert!(
                    !source.trim().is_empty(),
                    "corpus source must not be a marker: {}",
                    path.display()
                );
                invariant(&source, &path.display().to_string());
                *n += 1;
            } else if path.extension().is_some_and(|e| e == "json") {
                json_invariant(
                    &std::fs::read_to_string(&path).unwrap(),
                    &path.display().to_string(),
                );
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
    assert_eq!(count, 25);
}

/// CRLF input (a Windows editor, or a checkout with core.autocrlf) formats to
/// the same canonical LF output; a raw CRLF inside a string literal is part of
/// the value and is kept.
#[test]
fn crlf_input_emits_lf() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/fmt/goldens");
    let mut count = 0;
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "input") {
            continue;
        }
        let source = std::fs::read_to_string(&path)
            .unwrap()
            .replace('\n', "\r\n");
        let expected = std::fs::read_to_string(path.with_extension("expected")).unwrap();
        assert_eq!(format_zql(&source).unwrap(), expected, "{}", path.display());
        invariant(&source, &path.display().to_string());
        count += 1;
    }
    assert_eq!(count, 25);
    let literal = "query { A(name = \"one\r\ntwo\") { name } }";
    let output = format_zql(literal).unwrap();
    assert!(output.contains("\"one\r\ntwo\""), "{output:?}");
    assert_eq!(output.matches('\r').count(), 1, "{output:?}");
    assert_eq!(
        format_json("{\r\n  \"a\": [1, 2],\r\n  \"b\": \"c\"\r\n}\r\n"),
        "{ \"a\": [1, 2], \"b\": \"c\" }\n"
    );
}

#[test]
#[ignore = "requires scripts/fmt-samples.py --extract .tmp/samples; CI runs this explicitly"]
fn every_embedded_sample() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.tmp/samples");
    let mut count = 0;
    for entry in std::fs::read_dir(root).expect("extract documentation samples first") {
        let path = entry.unwrap().path();
        let source = std::fs::read_to_string(&path).unwrap();
        let label = path.display().to_string();
        if path.extension().is_some_and(|ext| ext == "json") {
            json_invariant(&source, &label);
        } else {
            invariant(&source, &label);
        }
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
        if path.extension().is_none_or(|e| e != "expected") {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap();
        let ts = tokens(&source);
        if ts.iter().any(|t| t.text.starts_with("//")) {
            continue;
        }
        let mut marked = source.clone();
        let mut comments = 0;
        for (i, pair) in ts.windows(2).enumerate().rev() {
            if pair[0].end == pair[1].start {
                continue;
            }
            let mut candidate = marked.clone();
            candidate.insert_str(pair[1].start, &format!("// comment {i}\n"));
            // Embedded JSON locations have JSON's grammar: unlike ZQL they
            // reject comments. Only inject at parser-accepted trivia boundaries.
            if ast(&candidate).is_ok() {
                marked = candidate;
                comments += 1;
            }
        }
        assert!(comments > 0);
        marked.insert_str(0, "// leading\n");
        marked.push_str("// trailing\n");
        invariant(&marked, &path.display().to_string());
    }
}

#[test]
fn invalid_display_and_discovery_are_unchanged() {
    for source in [
        "schema { type Contract { scan: String } display { graph { Contract(@shape: document, @image: &scan) } } }",
        "query { Person { name } } then { take 10 }",
    ] {
        assert!(Parsed::parse(source).is_err());
        assert_eq!(format_zql(source).unwrap(), source);
    }
}

#[test]
fn r1_selections() {
    let source = "query{Team{name country}}query{Player{name salary position}}query{Team{name players->Player{name}}}";
    let expected = "query {\n  Team { name country }\n}\n\nquery {\n  Player {\n    name\n    salary\n    position\n  }\n}\n\nquery {\n  Team {\n    name\n    players -> Player { name }\n  }\n}\n";
    assert_eq!(format_zql(source).unwrap(), expected);
    for width in [80, 81] {
        let name = "a".repeat(width - "  Team {  b }".len());
        let input = format!("query{{Team{{{name} b}}}}");
        let output = format_zql(&input).unwrap();
        let selection = if width == 80 {
            format!("  Team {{ {name} b }}")
        } else {
            format!("  Team {{\n    {name}\n    b\n  }}")
        };
        assert_eq!(output, format!("query {{\n{selection}\n}}\n"));
        invariant(&input, "R1 width");
    }
    invariant(source, "R1");
}
#[test]
fn r2_top_level_blocks() {
    let source = "schema{type A{x:Int}}unique{A{x}}index{}mutation{A(x:1)}query{A{x}}display{skip}then{findExact{\"a\"}}display{skip}";
    let expected = "schema {\n  type A { x: Int }\n}\n\nunique {\n  A { x }\n}\n\nindex {\n}\n\nmutation {\n  A(x: 1)\n}\n\nquery {\n  A { x }\n}\n\ndisplay {\n  skip\n}\n\nthen {\n  findExact { \"a\" }\n}\n\ndisplay {\n  skip\n}\n";
    assert_eq!(format_zql(source).unwrap(), expected);
    invariant(source, "R2");
}
#[test]
fn r3_schema_fields() {
    let source = "schema{type Country{name:String}type Team{name:String country:String}}";
    let expected = "schema {\n  type Country { name: String }\n\n  type Team {\n    name: String\n    country: String\n  }\n}\n";
    assert_eq!(format_zql(source).unwrap(), expected);
    assert_eq!(
        format_zql("type Country{name:String}").unwrap(),
        "type Country { name: String }\n"
    );
    assert_eq!(
        format_zql("schema{type A{b:REL->B{since:Int}}type B{x:Int}}").unwrap(),
        "schema {\n  type A { b: REL -> B { since: Int } }\n\n  type B { x: Int }\n}\n"
    );
    invariant(source, "R3");
}
#[test]
fn r4_spacing() {
    let source = "schema{type A{x:Int longer:String b:REL->B}type B{a:REL<-A}}";
    let expected = "schema {\n  type A {\n    x: Int\n    longer: String\n    b: REL -> B\n  }\n\n  type B { a: REL <- A }\n}\n";
    assert_eq!(format_zql(source).unwrap(), expected);
    invariant(source, "R4");
}
#[test]
fn r5_parentheses() {
    for width in [80, 81] {
        let value = "x".repeat(width - "  A(name: \"\" && age: 1)".len());
        let source = format!("query{{A(name:\"{value}\"&&age:1)}}");
        let expected = if width == 80 {
            format!("query {{\n  A(name: \"{value}\" && age: 1)\n}}\n")
        } else {
            format!("query {{\n  A(\n    name: \"{value}\" &&\n    age: 1\n  )\n}}\n")
        };
        assert_eq!(format_zql(&source).unwrap(), expected);
        invariant(&source, "R5 width");
    }
}
#[test]
fn r6_blank_lines() {
    let source = "schema{type A{x:Int}type B{y:Int}}query{A{x\n\nx\n\nx}}";
    let expected = "schema {\n  type A { x: Int }\n\n  type B { y: Int }\n}\n\nquery {\n  A {\n    x\n    x\n    x\n  }\n}\n";
    assert_eq!(format_zql(source).unwrap(), expected);
    invariant(source, "R6");
}
#[test]
fn r7_display() {
    let source =
        "schema{type A{x:Int}type B{x:Int}type C{x:Int}display{graph{A B}:Default table{A}}}";
    let expected = "schema {\n  type A { x: Int }\n\n  type B { x: Int }\n\n  type C { x: Int }\n\n  display {\n    graph { A B } : Default\n    table { A }\n  }\n}\n";
    assert_eq!(format_zql(source).unwrap(), expected);
    invariant(source, "R7 one/two types");
    let source = source.replace("graph{A B}", "graph{A,B,C}");
    let expected = expected.replace(
        "graph { A B }",
        "graph {\n      A,\n      B,\n      C\n    }",
    );
    assert_eq!(format_zql(&source).unwrap(), expected);
    invariant(&source, "R7 three types");
}
#[test]
fn r7_attribute_width() {
    for width in [80, 81] {
        let name = "A".repeat(width - "    graph { (@shape: document, @size: 2) }".len());
        let source = format!(
            "schema{{type {name}{{x:Int}}display{{graph{{{name}(@shape:document,@size:2)}}}}}}"
        );
        let output = format_zql(&source).unwrap();
        let entry = if width == 80 {
            format!("    graph {{ {name}(@shape: document, @size: 2) }}")
        } else {
            // The view opens first; at the narrower entry indent, its two
            // attributes can still fit together on the same line.
            format!("    graph {{\n      {name}(@shape: document, @size: 2)\n    }}")
        };
        assert!(output.contains(&entry), "{output}");
        invariant(&source, "R7 width");
    }
}
#[test]
fn then_chain_width() {
    for op in ["&&", "||"] {
        for width in [80, 81] {
            let value = "x".repeat(
                width - "  findExact { \"\" } && startsExact { \"b\" } && endsExact { \"c\" }".len(),
            );
            let source = format!("query{{A{{name}}}}then{{findExact{{\"{value}\"}}{op}startsExact{{\"b\"}}{op}endsExact{{\"c\"}}}}");
            let condition = if width == 80 {
                format!("  findExact {{ \"{value}\" }} {op} startsExact {{ \"b\" }} {op} endsExact {{ \"c\" }}")
            } else {
                format!("  findExact {{ \"{value}\" }} {op}\n  startsExact {{ \"b\" }} {op}\n  endsExact {{ \"c\" }}")
            };
            assert_eq!(
                format_zql(&source).unwrap(),
                format!("query {{\n  A {{ name }}\n}}\n\nthen {{\n{condition}\n}}\n")
            );
            invariant(&source, "then chain width");
        }
    }
}
#[test]
fn r8_comments_and_rejections() {
    let source = "// heading\nquery{A{name // name\nx}} // end\n";
    assert_eq!(
        format_zql(source).unwrap(),
        "// heading\nquery {\n  A {\n    name // name\n    x\n  }\n} // end\n"
    );
    invariant(source, "R8");
    for marker in [
        "/* nope */",
        "# nope",
        "-- nope",
        "<!-- nope -->",
        "(* nope *)",
        "*/",
    ] {
        for prefix in [
            "",
            "query { ",
            "query { A { name ",
            "query { A(name: 1 ",
            "query { A { name } } ",
        ] {
            let source = format!("{prefix}{marker}\n");
            let error = Parsed::parse(&source).unwrap_err();
            assert!(error.message.contains("//"), "{source}: {error}");
            assert_eq!((error.line, error.column), (1, prefix.len() as u32 + 1));
            assert!(error.end_column > error.column);
            assert_eq!(format_zql(&source).unwrap(), source);
        }
    }
    invariant(
        "query{A(name:\"/* # -- */\"){name}}",
        "comment-looking string",
    );
}
#[test]
fn r9_json() {
    let source = r#"{"b":1e3,"a":0.10,"nested":{"a":1,"b":{"c":"\u0041"}},"array":[true,false,null,-0,1.00e+02],"objects":[{"a":1}]}"#;
    let expected = "{\n  \"b\": 1e3,\n  \"a\": 0.10,\n  \"nested\": { \"a\": 1, \"b\": { \"c\": \"\\u0041\" } },\n  \"array\": [true, false, null, -0, 1.00e+02],\n  \"objects\": [\n    { \"a\": 1 }\n  ]\n}\n";
    assert_eq!(format_json(source), expected);
    assert_eq!(format_json(expected), expected);
    assert_eq!(format_json(r#"{"a":1,"b":2}"#), "{ \"a\": 1, \"b\": 2 }\n");
    assert_eq!(
        format_json(r#"{"a":1,"b":2,"c":3}"#),
        "{\n  \"a\": 1,\n  \"b\": 2,\n  \"c\": 3\n}\n"
    );
    for width in [80, 81] {
        for object in [false, true] {
            let prefix = if object { "{ \"a\": \"" } else { "[\"" };
            let suffix = if object { "\" }" } else { "\"]" };
            let line = format!(
                "{prefix}{}{suffix}",
                "x".repeat(width - prefix.len() - suffix.len())
            );
            let output = format_json(&line);
            if width == 80 {
                assert_eq!(output, format!("{line}\n"));
            } else {
                assert_eq!(output.lines().count(), 3);
            }
            assert_eq!(format_json(&output), output);
        }
    }
}
#[test]
fn json_literal_bytes_and_invalid_input() {
    let input = r#"{"z":-0,"a":1e400,"z":123456789012345678901234567890,"q":"\"\\\/\b\f\n\r\t\uabcd\uD834\uDD1Eé","last":0.10e-009}"#;
    let output = format_json(input);
    let literals = |s: &str| {
        tokens(s)
            .into_iter()
            .map(|t| t.text.to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(literals(input), literals(&output));
    assert_eq!(format_json(&output), output);
    for invalid in [
        "",
        "{",
        "[1,]",
        "{\"a\":1,}",
        "01",
        "1.",
        "1e",
        "--1",
        "+1",
        "NaN",
        "true false",
        "[// hi\n1]",
        "\"\\x41\"",
        "\"\\u12\"",
        "\"new\nline\"",
        "\u{a0}1",
        "{a:1}",
    ] {
        assert_eq!(format_json(invalid), invalid, "{invalid}");
    }
}
#[test]
fn every_repository_json() {
    fn visit(path: &std::path::Path, count: &mut usize) {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_symlink() {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().unwrap().to_str().unwrap();
                if !name.starts_with('.')
                    && !matches!(
                        name,
                        "node_modules" | "target" | "dist" | "test-results" | "playwright-report"
                    )
                {
                    visit(&path, count);
                }
            } else if path.extension().is_some_and(|e| e == "json") {
                let input = std::fs::read_to_string(&path).unwrap();
                json_invariant(&input, &path.display().to_string());
                *count += 1;
            }
        }
    }
    let mut count = 0;
    visit(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."),
        &mut count,
    );
    eprintln!("verified {count} JSON files");
    assert!(count > 10);
}

fn json_invariant(input: &str, label: &str) {
    let output = format_json(input);
    assert_eq!(format_json(&output), output, "idempotence: {label}");
    match serde_json::from_str::<serde_json::Value>(input) {
        Ok(before) => assert_eq!(
            before,
            serde_json::from_str::<serde_json::Value>(&output).unwrap(),
            "parse: {label}"
        ),
        Err(_) => assert_eq!(input, output, "invalid: {label}"),
    }
    let literals = |s: &str| {
        tokens(s)
            .into_iter()
            .map(|t| t.text.to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(literals(input), literals(&output), "literal bytes: {label}");
}
