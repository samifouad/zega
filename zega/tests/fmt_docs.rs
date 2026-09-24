//! docs/fmt.md shows `zega fmt` on real input (zegadb/zega#65). Each example is a
//! fence tagged `before` followed by one tagged `after`; this test formats every
//! `before` and requires its `after` byte for byte, so the page cannot drift from
//! the formatter. To change an example, edit its `before` and paste in what
//! `zega fmt --stdin` (add `--lang json` for JSON) prints for it.
use zega::fmt::{format_json, format_zql};

const DOC: &str = include_str!("../../docs/fmt.md");

struct Fence {
    lang: String,
    tag: String,
    code: String,
    line: usize,
}

fn fences(doc: &str) -> Vec<Fence> {
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
                .unwrap_or_else(|| panic!("docs/fmt.md:{}: unclosed fence", n + 1));
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

fn format(lang: &str, source: &str) -> String {
    match lang {
        "zql" => format_zql(source).unwrap(),
        "json" => format_json(source),
        other => panic!("no formatter for {other}"),
    }
}

#[test]
fn every_after_block_is_what_zega_fmt_prints_for_its_before_block() {
    // Every ZQL and JSON block on the page is an example, so none goes unchecked.
    let examples: Vec<Fence> = fences(DOC)
        .into_iter()
        .filter(|f| f.lang == "zql" || f.lang == "json")
        .collect();
    let pairs = examples.chunks(2);
    assert!(
        examples.len() >= 12,
        "expected at least 6 before/after pairs"
    );
    let mut langs = Vec::new();
    for pair in pairs {
        let [before, after] = pair else {
            panic!(
                "docs/fmt.md:{}: a `before` block without its `after`",
                pair[0].line
            )
        };
        assert_eq!(
            (before.tag.as_str(), after.tag.as_str(), &after.lang),
            ("before", "after", &before.lang),
            "docs/fmt.md:{}: expected ```{0} before then ```{0} after",
            before.line,
        );
        assert_ne!(
            before.code, after.code,
            "docs/fmt.md:{}: the example does not change anything",
            before.line
        );
        assert_eq!(
            format(&before.lang, &before.code),
            after.code,
            "docs/fmt.md:{}: the after block is not what zega fmt prints",
            after.line
        );
        assert_eq!(
            format(&after.lang, &after.code),
            after.code,
            "docs/fmt.md:{}: the after block is not stable under zega fmt",
            after.line
        );
        langs.push(before.lang.clone());
    }
    assert!(langs.iter().any(|l| l == "zql") && langs.iter().any(|l| l == "json"));
}

#[test]
fn fence_reader_pairs_tags_and_bodies() {
    let doc = "text\n\n```zql before\nquery{A{b}}\n```\n\n```sh\nzega fmt .\n```\n";
    let found = fences(doc);
    assert_eq!(found.len(), 2);
    assert_eq!(
        (
            found[0].lang.as_str(),
            found[0].tag.as_str(),
            found[0].code.as_str(),
            found[0].line
        ),
        ("zql", "before", "query{A{b}}\n", 3)
    );
    assert_eq!((found[1].lang.as_str(), found[1].tag.as_str()), ("sh", ""));
}
