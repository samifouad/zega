//! Every ZQL block in docs/*.md goes through the engine (zegadb/zega#68), so the
//! docs cannot break without a test failing.
//!
//! - Every block must parse (as a file, a statement or a query; a bare type or a
//!   selection line is checked inside a schema or a selection) and be exactly
//!   what `zega fmt` prints. A `before` block in docs/fmt.md is unformatted on
//!   purpose; tests/fmt_docs.rs checks it against its `after` block.
//! - The concept pages in RUNNABLE are also run, top to bottom, on a fresh
//!   in-memory database, the way a reader would type them:
//!   - a block that starts with `schema` is the page's schema (with any
//!     `unique` and `index` blocks) and starts a new, empty database;
//!   - every other block runs against the latest schema;
//!   - a ```json block right after a ZQL block is its result, as `zega fmt`
//!     prints the JSON the engine returns;
//!   - a ```zql error block must fail, and the ```text block after it is the
//!     error, byte for byte.
use std::{fs, path::Path};
use zega::{check_zql, fmt::format_json, fmt::format_zql, Zega, ZqlEntryPoint};

const RUNNABLE: &[&str] = &[
    "schema.md",
    "mutation.md",
    "query.md",
    "conditions.md",
    "relationships.md",
    "unique.md",
    "errors.md",
];

#[derive(Debug)]
struct Fence {
    lang: String,
    tag: String,
    code: String,
    line: usize,
}

fn fences(doc: &str, file: &str) -> Vec<Fence> {
    let mut out = Vec::new();
    let mut lines = doc.lines().enumerate();
    while let Some((n, line)) = lines.next() {
        let Some(info) = line.strip_prefix("```") else {
            continue;
        };
        let mut words = info.split_whitespace();
        let lang = words.next().unwrap_or("").to_owned();
        let tag = words.next().unwrap_or("").to_owned();
        let mut code = String::new();
        loop {
            let (_, body) = lines
                .next()
                .unwrap_or_else(|| panic!("{file}:{}: unclosed fence", n + 1));
            if body.starts_with("```") {
                break;
            }
            code.push_str(body);
            code.push('\n');
        }
        out.push(Fence {
            lang,
            tag,
            code,
            line: n + 1,
        });
    }
    out
}

fn docs() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs");
    let mut files: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            (name, fs::read_to_string(&path).unwrap())
        })
        .collect()
}

fn parses(code: &str) -> bool {
    [
        ZqlEntryPoint::File,
        ZqlEntryPoint::Statement,
        ZqlEntryPoint::Query,
    ]
    .into_iter()
    .any(|entry| check_zql(entry, code).is_ok())
        // Several statements: the body of a file, after a placeholder schema.
        || check_zql(ZqlEntryPoint::File, &format!("schema {{\n  type T {{ x: String }}\n}}\n\n{code}")).is_ok()
        // A type on its own, as a schema excerpt.
        || check_zql(ZqlEntryPoint::File, &format!("schema {{\n{code}}}\n")).is_ok()
        // A line that is only valid inside a selection, with a placeholder type.
        || check_zql(ZqlEntryPoint::Query, &format!("query {{\n  T {{\n{code}  }}\n}}\n")).is_ok()
}

#[test]
fn every_zql_block_in_the_docs_parses_and_is_in_zega_fmt_layout() {
    let mut checked = 0;
    for (file, doc) in docs() {
        for fence in fences(&doc, &file) {
            if fence.lang != "zql" || fence.tag == "before" || fence.tag == "error" {
                continue;
            }
            let at = format!("docs/{file}:{}", fence.line);
            assert!(
                parses(&fence.code),
                "{at}: ZQL does not parse:\n{}",
                fence.code
            );
            // A fragment formats as whatever it parses as alone; only check
            // blocks that parse on their own.
            if format_zql(&fence.code).unwrap() != fence.code {
                assert!(
                    format_zql(&format!("{}\n\n", fence.code)).unwrap()
                        == format!("{}\n\n", fence.code),
                    "{at}: ZQL is not in zega fmt layout:\n{}",
                    fence.code
                );
            }
            checked += 1;
        }
    }
    assert!(
        checked >= 40,
        "expected the docs' ZQL blocks, found {checked}"
    );
}

fn run_page(file: &str, doc: &str) -> usize {
    let blocks = fences(doc, file);
    let mut db = Zega::in_memory().build().unwrap();
    let mut schema: Option<String> = None;
    let mut ran = 0;
    let mut i = 0;
    while i < blocks.len() {
        let block = &blocks[i];
        let at = format!("docs/{file}:{}", block.line);
        let next = blocks.get(i + 1);
        match (block.lang.as_str(), block.tag.as_str()) {
            ("zql", "error") => {
                let error = if block.code.starts_with("schema") {
                    Zega::in_memory().build().unwrap().apply_zql(&block.code)
                } else {
                    let schema = schema
                        .as_deref()
                        .unwrap_or_else(|| panic!("{at}: no schema yet"));
                    db.run_lang(schema, &block.code)
                }
                .expect_err(&format!("{at}: expected this example to fail"));
                let text = next.filter(|n| n.lang == "text").unwrap_or_else(|| {
                    panic!("{at}: an error example needs a ```text block with the error after it")
                });
                assert_eq!(
                    format!("{error}\n"),
                    text.code,
                    "{at}: the error is not what the engine reports"
                );
                ran += 1;
                i += 2;
                continue;
            }
            ("zql", "") => {
                let result = if block.code.starts_with("schema") {
                    db = Zega::in_memory().build().unwrap();
                    let result = db.apply_zql(&block.code);
                    schema = Some(block.code.clone());
                    result
                } else {
                    let schema = schema
                        .as_deref()
                        .unwrap_or_else(|| panic!("{at}: no schema yet"));
                    db.run_lang(schema, &block.code)
                }
                .unwrap_or_else(|error| panic!("{at}: example fails:\n{error}"));
                ran += 1;
                if let Some(json) = next.filter(|n| n.lang == "json") {
                    assert_eq!(
                        format_json(&result.to_string()),
                        json.code,
                        "docs/{file}:{}: the result is not what the engine returns",
                        json.line
                    );
                    i += 2;
                    continue;
                }
            }
            ("zql", tag) => panic!("{at}: unknown tag `{tag}`"),
            ("json", _) => {
                panic!("{at}: a JSON block must follow the ZQL block it is the result of")
            }
            ("text", _) => panic!("{at}: a text block must follow a ```zql error block"),
            _ => {}
        }
        i += 1;
    }
    ran
}

#[test]
fn the_concept_pages_run_top_to_bottom_with_the_results_they_show() {
    let docs = docs();
    for page in RUNNABLE {
        let (file, doc) = docs
            .iter()
            .find(|(file, _)| file == page)
            .unwrap_or_else(|| panic!("docs/{page} is missing"));
        let ran = run_page(file, doc);
        assert!(
            ran >= 3,
            "docs/{file}: expected runnable examples, ran {ran}"
        );
    }
}

#[test]
fn a_wrong_result_or_a_missing_error_fails_the_page() {
    let page = "```zql\nschema {\n  type P { name: String }\n}\n```\n\n```zql\nmutation {\n  P(name: \"Ada\")\n}\n```\n\n```zql\nquery {\n  P { name }\n}\n```\n\n```json\n[\n  { \"name\": \"Ada\" }\n]\n```\n";
    assert_eq!(run_page("ok.md", page), 3);
    let wrong = page.replace("{ \"name\": \"Ada\" }\n]", "{ \"name\": \"Bob\" }\n]");
    assert!(std::panic::catch_unwind(|| run_page("wrong.md", &wrong)).is_err());
    let no_error = page
        .replace("```zql\nquery", "```zql error\nquery")
        .replace("```json\n[\n  { \"name\": \"Ada\" }\n]", "```text\nnothing");
    assert!(std::panic::catch_unwind(|| run_page("no-error.md", &no_error)).is_err());
}
