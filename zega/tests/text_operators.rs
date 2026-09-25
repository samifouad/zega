//! zegadb/zega#98: `findExact`/`startsExact`/`endsExact` (byte-exact) replace
//! the old `findWith`/`startsWith`/`endsWith`, and `findLike`/`startsLike`/
//! `endsLike` add Unicode case- and accent-folding matching. `=` is unchanged.
use serde_json::json;
use zega::{fmt::format_zql, Zega};

const SCHEMA: &str = "type Player { name: String bio?: String }";

fn db() -> Zega {
    let db = Zega::in_memory().build().unwrap();
    for mutation in [
        r#"mutation { Player(name: "Webhook fired") }"#,
        r#"mutation { Player(name: "Café Society") }"#,
        r#"mutation { Player(name: "São Paulo") }"#,
        r#"mutation { Player(name: "STRASSE") }"#,
        r#"mutation { Player(name: "Οδυσσέας") }"#,
        r#"mutation { Player(name: "Привет") }"#,
        r#"mutation { Player(name: "50% off") }"#,
    ] {
        db.run_lang(SCHEMA, mutation).unwrap();
    }
    db
}

fn names(db: &Zega, query: &str) -> Vec<String> {
    db.run_lang(SCHEMA, query)
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["name"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn exact_is_byte_exact_on_mixed_case() {
    let db = db();
    assert_eq!(
        names(&db, r#"query { Player(name findExact "webhook") { name } }"#),
        Vec::<String>::new()
    );
    assert_eq!(
        names(&db, r#"query { Player(name findExact "Webhook") { name } }"#),
        vec!["Webhook fired"]
    );
    assert_eq!(
        names(&db, r#"query { Player(name startsExact "webhook") { name } }"#),
        Vec::<String>::new()
    );
    assert_eq!(
        names(&db, r#"query { Player(name startsExact "Webhook") { name } }"#),
        vec!["Webhook fired"]
    );
}

#[test]
fn like_folds_case_on_mixed_case() {
    let db = db();
    assert_eq!(
        names(&db, r#"query { Player(name findLike "webhook") { name } }"#),
        vec!["Webhook fired"]
    );
    assert_eq!(
        names(&db, r#"query { Player(name findLike "WEBHOOK") { name } }"#),
        vec!["Webhook fired"]
    );
    assert_eq!(
        names(&db, r#"query { Player(name startsLike "WEB") { name } }"#),
        vec!["Webhook fired"]
    );
    assert_eq!(
        names(&db, r#"query { Player(name endsLike "FIRED") { name } }"#),
        vec!["Webhook fired"]
    );
}

#[test]
fn like_folds_accents_both_directions() {
    let db = db();
    assert_eq!(
        names(&db, r#"query { Player(name findLike "cafe") { name } }"#),
        vec!["Café Society"]
    );
    assert_eq!(
        names(&db, r#"query { Player(name startsLike "sao paulo") { name } }"#),
        vec!["São Paulo"]
    );
    // Exact never folds accents, so the plain-ASCII spelling does not match.
    assert_eq!(
        names(&db, r#"query { Player(name findExact "cafe") { name } }"#),
        Vec::<String>::new()
    );
}

#[test]
fn like_folds_special_casing_greek_and_cyrillic() {
    let db = db();
    // ß case-folds to "ss" under full Unicode case folding.
    assert_eq!(
        names(&db, r#"query { Player(name findLike "straße") { name } }"#),
        vec!["STRASSE"]
    );
    assert_eq!(
        names(&db, r#"query { Player(name findLike "οδυσσεας") { name } }"#),
        vec!["Οδυσσέας"]
    );
    assert_eq!(
        names(&db, r#"query { Player(name findLike "ПРИВЕТ") { name } }"#),
        vec!["Привет"]
    );
}

#[test]
fn like_treats_percent_as_a_literal_character() {
    let db = db();
    assert_eq!(
        names(&db, r#"query { Player(name findLike "50%") { name } }"#),
        vec!["50% off"]
    );
    assert_eq!(
        names(&db, r#"query { Player(name findLike "50x") { name } }"#),
        Vec::<String>::new()
    );
}

#[test]
fn old_names_are_a_checker_error_with_a_fix_it() {
    let schema = "type Player { name: String }";
    for (old, exact, like) in [
        ("findWith", "findExact", "findLike"),
        ("startsWith", "startsExact", "startsLike"),
        ("endsWith", "endsExact", "endsLike"),
    ] {
        let query = format!(r#"query {{ Player(name {old} "x") {{ name }} }}"#);
        let report = zega::diagnose(schema, &query);
        let diagnostic = &report.diagnostics[0];
        assert!(
            diagnostic.message.contains(exact),
            "{old}: {}",
            report.text
        );
        let help = diagnostic.help.as_deref().unwrap_or_default();
        assert!(help.contains(like), "{old}: {}", report.text);
        assert!(Zega::in_memory()
            .build()
            .unwrap()
            .run_lang(schema, &query)
            .is_err());
    }
}

#[test]
fn equality_stays_byte_exact() {
    let db = db();
    // An equality-only filter names one node; no match is `null`, not `[]`.
    assert_eq!(
        db.run_lang(SCHEMA, r#"query { Player(name = "webhook fired") { name } }"#)
            .unwrap(),
        json!(null)
    );
    assert_eq!(
        db.run_lang(SCHEMA, r#"query { Player(name = "Webhook fired") { name } }"#)
            .unwrap(),
        json!({"name": "Webhook fired"})
    );
}

#[test]
fn zega_fmt_round_trips_exact_and_like() {
    for source in [
        "query{Player(name findExact \"a\"){name}}",
        "query{Player(name startsExact \"a\"){name}}",
        "query{Player(name endsExact \"a\"){name}}",
        "query{Player(name findLike \"a\"){name}}",
        "query{Player(name startsLike \"a\"){name}}",
        "query{Player(name endsLike \"a\"){name}}",
    ] {
        let formatted = format_zql(source).unwrap();
        assert_eq!(
            format_zql(&formatted).unwrap(),
            formatted,
            "not a fixed point: {source}"
        );
    }
}
