use serde_json::json;
use std::collections::HashMap;
use zega::Zega;

const SCHEMA: &str = "type Document { name: String scan: String<url> backup?: String<url> } display { graph { Document(@shape: document, @image: &scan, @size: 3) } }";

#[test]
fn url_writes_updates_and_string_operations() {
    let db = Zega::in_memory().build().unwrap();
    db.run_lang(SCHEMA, r#"mutation { Document(name: "A" && scan: "https://example.com/a?x=1#page" && backup: null) }"#).unwrap();
    assert_eq!(
        db.run_lang(
            SCHEMA,
            r#"query { Document(scan startsExact "https://") { name scan backup } }"#
        )
        .unwrap(),
        json!([{"name":"A","scan":"https://example.com/a?x=1#page","backup":null}])
    );
    db.run_lang(
        SCHEMA,
        r#"mutation { Document(name: "A") set scan: "https://example.org/b" { scan } }"#,
    )
    .unwrap();
    assert_eq!(
        db.run_lang(SCHEMA, r#"query { Document(name: "A") { scan } }"#)
            .unwrap(),
        json!({"scan":"https://example.org/b"})
    );
}

#[test]
fn invalid_url_writes_and_updates_are_atomic() {
    let db = Zega::in_memory().build().unwrap();
    db.run_lang(
        SCHEMA,
        r#"mutation { Document(name: "A" && scan: "https://example.com/a") }"#,
    )
    .unwrap();
    let before = db.graph_json().unwrap();
    for value in [
        r#""javascript:alert(1)""#,
        r#""data:image/png;base64,AAAA""#,
        r#""file:///image.png""#,
        r#""ftp://example.org/image.png""#,
        r#""mailto:user@example.org""#,
        r#""https://user:pass@example.org/image.png""#,
        r#""https://user@example.org/image.png""#,
        r#""https://@example.org/image.png""#,
        r#""relative/path""#,
        r#""https://""#,
        r#""https://bad host/a""#,
        r#""https://example.com/white space""#,
        "42",
        "true",
        "null",
    ] {
        for source in [
            format!("mutation {{ Document(name: \"B\" && scan: {value}) }}"),
            format!("mutation {{ Document(name: \"A\") set scan: {value} {{ scan }} }}"),
        ] {
            let error = db.run_lang(SCHEMA, &source).unwrap_err().to_string();
            assert!(
                error.contains("Document.scan must be String<url>"),
                "{error}"
            );
            assert!(
                error.contains("http:// or https:// URL with a host and no userinfo"),
                "{error}"
            );
            assert_eq!(db.graph_json().unwrap(), before);
        }
    }
}

#[test]
fn url_import_validation_rejects_entire_batch() {
    let db = Zega::in_memory().build().unwrap();
    for (format, rows) in [
        (
            "json",
            r#"[{"name":"A","scan":"https://example.com/a"},{"name":"B","scan":"broken"}]"#,
        ),
        ("csv", "name,scan\nA,https://example.com/a\nB,broken\n"),
    ] {
        let source = format!(
            "mutation {format} [\"rows.{format}\"] {{ Document(name: $name && scan: $scan) }}"
        );
        let sources = HashMap::from([(format!("rows.{format}"), rows.into())]);
        let error = db
            .run_lang_with_sources(SCHEMA, &source, &sources)
            .unwrap_err()
            .to_string();
        assert!(error.contains("String<url>"), "{error}");
        assert_eq!(db.graph_json().unwrap()["nodes"], json!([]));
    }
}

#[test]
fn url_type_grammar_and_edge_properties() {
    let db = Zega::in_memory().build().unwrap();
    for ty in ["Float<url>", "Int<url>", "String<uri>", "Bool<url>"] {
        assert!(db.schema(&format!("type T {{ image: {ty} }}")).is_err());
    }
    let schema = "type Page { name: String link -> Page[] { source: String<url> } }";
    db.run_lang(schema, r#"mutation { Page(name: "A") { link -> Page(name: "B") { &source: "https://example.org/ref" } } }"#).unwrap();
    assert!(db
        .run_lang(
            schema,
            r#"mutation { Page(name: "C") { link -> Page(name: "D") { &source: "not-a-url" } } }"#
        )
        .is_err());
    assert_eq!(
        db.run_lang(
            schema,
            r#"query { Page(name: "A") { link -> Page { &source } } }"#
        )
        .unwrap(),
        json!({"link":[{"source":"https://example.org/ref"}]})
    );
}

#[test]
fn url_fields_keep_string_indexes_and_do_not_validate_search_fragments() {
    let db = Zega::in_memory().build().unwrap();
    let source = r#"schema { type Page { scan: String<url> } }
        index { text Page { scan } }
        mutation { Page(scan: "https://example.org/one") }
        query { Page(scan findExact "example") { scan } }"#;
    assert_eq!(
        db.apply_zql(source).unwrap(),
        json!([{"scan":"https://example.org/one"}])
    );
    assert_eq!(
        db.run_lang(
            "type Page { scan: String<url> }",
            r#"query { Page(scan: "missing") { scan } }"#
        )
        .unwrap(),
        json!(null)
    );
}

#[test]
fn url_accepts_http_and_https_case_insensitively() {
    for url in [
        "http://example.org/image.png",
        "https://example.org/image.png",
        "HTTP://example.org/image.png",
        "HtTpS://example.org/image.png",
        "https://example.org/@image.png?email=user@example.org",
        "http://[::1]:8080/image.png",
    ] {
        let db = Zega::in_memory().build().unwrap();
        let source = format!("mutation {{ Document(name: \"A\" && scan: {url:?}) }}");
        db.run_lang(SCHEMA, &source).unwrap();
        assert_eq!(
            db.run_lang(SCHEMA, r#"query { Document(name: "A") { scan } }"#)
                .unwrap(),
            json!({"scan": url})
        );
    }
}
