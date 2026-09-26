//! zegadb/zega#48: deep nesting must be an ordinary parse error, and a long
//! flat `&&`/`||` chain must not recurse per term, in the parser or in any pass
//! after it (check, bind, execute, format).
//!
//! Every test runs on a thread with a deliberately small stack. Without the
//! depth limit the deep inputs overflow it and abort the test process, so a
//! regression reproduces the crash instead of passing on a large stack.
use super::*;
use crate::Zega;

/// Below the 2 MiB of a tokio blocking thread or a test thread. Input at
/// [`MAX_NESTING`] needs about 1.25 MiB through every pass in an unoptimized
/// build (about 384 KiB in release); input past it, without the limit, needs
/// hundreds of MiB and aborts here.
const LIMIT_STACK: usize = 1536 * 1024;

/// A flat chain never nests, so it gets a much smaller stack still.
const CHAIN_STACK: usize = 256 * 1024;

/// The shapes from the issue's stress run killed a server at 1,000 to 5,000;
/// this is far past any of them.
const HOSTILE: usize = 50_000;

/// Terms in the flat chain from the issue.
const CHAIN: usize = 5_000;

const SCHEMA: &str = "type Item { key: String c?: Int links -> Item[] }";

fn small_stack<T: Send + 'static>(run: impl FnOnce() -> T + Send + 'static) -> T {
    on_stack(LIMIT_STACK, run)
}

fn on_stack<T: Send + 'static>(bytes: usize, run: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(bytes)
        .spawn(run)
        .unwrap()
        .join()
        .unwrap()
}

/// `Item(((…(key: "s1")…)))` with `depth` parentheses inside the condition's own.
fn brackets(depth: usize) -> String {
    format!(
        "{{ Item({}key: \"s1\"{}) {{ key }} }}",
        "(".repeat(depth),
        ")".repeat(depth)
    )
}

/// `Item(key: "s1") { links -> Item { links -> … { key } } }`, `depth` arrows.
fn selections(depth: usize) -> String {
    format!(
        "{{ Item(key: \"s1\") {{ {}key{} }} }}",
        "links -> Item { ".repeat(depth),
        " }".repeat(depth)
    )
}

/// `Item(has links(((…(key: "s1")…))))`: a chain's test, `depth` levels
/// with its own parentheses (zegadb/zega#86). Chains never nest; their tests
/// count toward the one limit.
fn walks(depth: usize) -> String {
    format!(
        "{{ Item(has links{}key: \"s1\"{}) {{ key }} }}",
        "(".repeat(depth),
        ")".repeat(depth)
    )
}

/// A create mutation `depth` walks deep, every level a new node.
fn creates(depth: usize) -> String {
    let mut out = String::from("mutation { Item(key: \"s0\") { ");
    for level in 1..=depth {
        out.push_str(&format!("links -> Item(key: \"s{level}\") {{ "));
    }
    out.push_str("key");
    out.push_str(&" }".repeat(depth));
    out.push_str(" } }");
    out
}

/// A `then` stage with `depth` parentheses around one discovery sub-block.
fn discovery(depth: usize) -> String {
    format!(
        "query {{ Item {{ key }} }} then {{ {}findExact {{ \"s\" }}{} }}",
        "(".repeat(depth),
        ")".repeat(depth)
    )
}

fn chain(op: &str, terms: usize) -> String {
    let tests: Vec<String> = (0..terms).map(|n| format!("c = {n}")).collect();
    format!("{{ Item({}) {{ key c }} }}", tests.join(&format!(" {op} ")))
}

fn discovery_chain(terms: usize) -> String {
    let tests = vec!["findExact { \"s\" }"; terms];
    format!("query {{ Item {{ key }} }} then {{ {} }}", tests.join(" || "))
}

type Shape = fn(usize) -> String;
const SHAPES: [(&str, Shape); 5] = [
    ("brackets", brackets),
    ("walks", walks),
    ("selections", selections),
    ("creates", creates),
    ("discovery", discovery),
];

fn too_deep(source: &str) -> Error {
    let error = parse_statement(source).expect_err("deeper than the limit must not parse");
    assert_eq!(error.message, format!("nested too deeply (limit {MAX_NESTING})"));
    assert!(error.line >= 1 && error.column >= 1, "the error has a location: {error:?}");
    error
}

#[test]
fn each_construct_parses_at_the_limit_and_fails_one_past_it() {
    small_stack(|| {
        for (name, shape) in SHAPES {
            parse_statement(&shape(MAX_NESTING))
                .unwrap_or_else(|error| panic!("{name} at the limit: {error:?}"));
            too_deep(&shape(MAX_NESTING + 1));
        }
    });
}

#[test]
fn the_error_points_at_the_level_that_passes_the_limit() {
    small_stack(|| {
        let source = brackets(MAX_NESTING + 1);
        let error = too_deep(&source);
        // `{ Item(` then 128 accepted parentheses: the 129th is the one.
        let column = "{ Item(".len() + MAX_NESTING + 1;
        assert_eq!((error.line, error.column), (1, column as u32));

        let source = selections(MAX_NESTING + 1);
        let error = too_deep(&source);
        let prefix = format!("{{ Item(key: \"s1\") {{ {}links -> ", "links -> Item { ".repeat(MAX_NESTING));
        assert_eq!((error.line, error.column), (1, prefix.len() as u32 + 1));
        assert_eq!(error.end_column, error.column + "Item".len() as u32);
    });
}

#[test]
fn parentheses_and_selections_count_toward_one_limit() {
    small_stack(|| {
        let half = MAX_NESTING / 2;
        let inner = format!("{}key: \"s1\"{}", "(".repeat(half), ")".repeat(half));
        let at = format!(
            "{{ Item {{ {}Item({inner}) {{ key }}{} }} }}",
            "links -> Item { ".repeat(half - 1) + "links -> ",
            " }".repeat(half - 1)
        );
        parse_statement(&at).unwrap();
        let past = at.replacen("key: \"s1\"", "(key: \"s1\")", 1);
        too_deep(&past);
    });
}

#[test]
fn hostile_nesting_is_a_parse_error_on_every_entry_point() {
    small_stack(|| {
        let db = Zega::in_memory().build().unwrap();
        for (name, shape) in SHAPES {
            let source = shape(HOSTILE);
            too_deep(&source);
            let run = db.run_lang(SCHEMA, &source).expect_err(name).to_string();
            assert!(run.contains("nested too deeply (limit 128)"), "{name}: {run}");
            let file = format!("schema {{ {SCHEMA} }}\n{source}");
            let apply = db.apply_zql(&file).expect_err(name).to_string();
            assert!(apply.contains("nested too deeply (limit 128)"), "{name}: {apply}");
            let report = diagnose(SCHEMA, &source);
            assert!(
                report.diagnostics.iter().any(|d| d.message.contains("nested too deeply")),
                "{name}: diagnose reports the limit"
            );
            assert_eq!(fmt::format_zql(&source).unwrap(), source, "{name}: left unformatted");
            assert!(crate::check_zql(crate::ZqlEntryPoint::Statement, &source).is_err());
        }
    });
}

#[test]
fn input_at_the_limit_runs_checks_and_formats() {
    small_stack(|| {
        let db = Zega::in_memory().build().unwrap();
        let created = db.run_lang(SCHEMA, &creates(MAX_NESTING)).unwrap();
        let mut level = &created;
        for _ in 0..MAX_NESTING {
            level = match &level["links"] {
                Json::Array(many) => &many[0],
                one => one,
            };
        }
        assert_eq!(level["key"], Json::from(format!("s{MAX_NESTING}")), "{created}");

        let read = db.run_lang(SCHEMA, &brackets(MAX_NESTING)).unwrap();
        assert_eq!(read, serde_json::json!({ "key": "s1" }));
        db.run_lang(SCHEMA, &selections(MAX_NESTING)).unwrap();
        // s0 links to s1, and every other item links onward from s1.
        assert_eq!(db.run_lang(SCHEMA, &walks(MAX_NESTING)).unwrap(), serde_json::json!([{ "key": "s0" }]));
        db.run_lang(SCHEMA, &discovery(MAX_NESTING)).unwrap();

        for (name, shape) in SHAPES {
            let source = shape(MAX_NESTING);
            let report = diagnose(SCHEMA, &source);
            assert!(report.diagnostics.is_empty(), "{name}: {:?}", report.diagnostics);
            let formatted = fmt::format_zql(&source).unwrap();
            assert_ne!(formatted, source, "{name}: the formatter laid it out");
            assert_eq!(fmt::format_zql(&formatted).unwrap(), formatted, "{name}: stable");
        }
    });
}

#[test]
fn a_flat_chain_is_one_node_and_runs_without_recursing_per_term() {
    on_stack(CHAIN_STACK, || {
        for op in ["||", "&&"] {
            let Statement::Run(query) = parse_statement(&chain(op, CHAIN)).unwrap() else {
                panic!("a read");
            };
            let condition = query.root.unwrap().condition.unwrap();
            let (BoolExpr::Or(terms) | BoolExpr::And(terms)) = &condition else {
                panic!("{op} chain is n-ary");
            };
            assert_eq!(terms.len(), CHAIN);
            assert!(terms.iter().all(|term| matches!(term, BoolExpr::Test(_))));
        }

        let db = Zega::in_memory().build().unwrap();
        db.run_lang(SCHEMA, "mutation { Item(key: \"a\" && c: 4321) { key } }").unwrap();
        db.run_lang(SCHEMA, "mutation { Item(key: \"b\" && c: 99999) { key } }").unwrap();
        let found = db.run_lang(SCHEMA, &chain("||", CHAIN)).unwrap();
        assert_eq!(found, serde_json::json!([{ "key": "a", "c": 4321 }]));
        // Only equalities joined by `&&`: a root read of one object, and no
        // node has 5,000 values of `c`.
        let none = db.run_lang(SCHEMA, &chain("&&", CHAIN)).unwrap();
        assert_eq!(none, Json::Null);

        let source = chain("||", CHAIN);
        assert!(diagnose(SCHEMA, &source).diagnostics.is_empty());
        let formatted = fmt::format_zql(&source).unwrap();
        assert_eq!(fmt::format_zql(&formatted).unwrap(), formatted);
        assert_eq!(formatted.lines().filter(|line| line.contains("||")).count(), CHAIN - 1);

        let then = discovery_chain(CHAIN);
        parse_statement(&then).unwrap();
        db.run_lang(SCHEMA, &then).unwrap();
        let formatted = fmt::format_zql(&then).unwrap();
        assert_eq!(fmt::format_zql(&formatted).unwrap(), formatted);
    });
}

#[test]
fn a_chain_as_long_as_the_hostile_input_still_parses() {
    on_stack(CHAIN_STACK, || {
        let source = chain("||", HOSTILE);
        parse_statement(&source).unwrap();
        let db = Zega::in_memory().build().unwrap();
        db.run_lang(SCHEMA, &source).unwrap();
    });
}

