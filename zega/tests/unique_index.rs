//! Unique constraints through ZQL with per-(type, field) unique indexes
//! (zegadb/zega#100), from the review of #114: a rolled-back statement, a
//! declaration dropped and declared again, floats, and a graph replaced by
//! a restore, an import, `clear()` or a reopen from disk.
use zega::Zega;

const S: &str = r#"schema { type P { name: String n?: Float friend: KNOWS -> P[] } } unique { P { name } P { n } }"#;
const S_NO: &str = r#"schema { type P { name: String n?: Float friend: KNOWS -> P[] } }"#;

/// Run `q`, which must succeed.
fn ok(z: &Zega, s: &str, q: &str) { z.run_lang(s, q).unwrap_or_else(|e| panic!("{q}: {e}")); }
/// Whether `q` is refused as a duplicate of a unique value.
fn dup(z: &Zega, s: &str, q: &str) -> bool {
    match z.run_lang(s, q) { Ok(_) => false, Err(e) => { let t = e.to_string(); assert!(t.contains("already used"), "{q}: {t}"); true } }
}

#[test]
fn rollback_of_a_failed_statement_keeps_uniques_exact() {
    let z = Zega::in_memory().build().unwrap();
    ok(&z, S, r#"mutation { P(name: "a") }"#);
    // Creates "b", then fails on the link to a missing node: rolled back.
    assert!(z.run_lang(S, r#"mutation { P(name: "b") { friend -> link P(name: "zzz") { name } } }"#).is_err());
    assert_eq!(z.graph_json().unwrap()["nodes"].as_array().unwrap().len(), 1);
    // "b" is free again, "a" is still taken.
    ok(&z, S, r#"mutation { P(name: "b") }"#);
    assert!(dup(&z, S, r#"mutation { P(name: "a") }"#));
    assert!(dup(&z, S, r#"mutation { P(name: "b") }"#));
    // A set that would duplicate is refused and rolled back; the old value stays indexed.
    assert!(dup(&z, S, r#"mutation { P(name: "b") set name: "a" { name } }"#));
    assert!(dup(&z, S, r#"mutation { P(name: "b") }"#));
    // Rename then the old name is free.
    ok(&z, S, r#"mutation { P(name: "b") set name: "c" { name } }"#);
    ok(&z, S, r#"mutation { P(name: "b") }"#);
    assert!(dup(&z, S, r#"mutation { P(name: "c") }"#));
    // Lookup via unique after rollback.
    assert_eq!(z.run_lang(S, r#"{ P(name: "c") { name } }"#).unwrap(), serde_json::json!({"name": "c"}));
}

#[test]
fn a_declaration_dropped_and_readded_mid_run() {
    let z = Zega::in_memory().build().unwrap();
    ok(&z, S, r#"mutation { P(name: "a") }"#);
    // Without the declaration a duplicate is allowed (as on main).
    ok(&z, S_NO, r#"mutation { P(name: "x") }"#);
    ok(&z, S_NO, r#"mutation { P(name: "a") }"#);
    // Re-declared: the index is built from what is stored, both "a"s included.
    assert!(dup(&z, S, r#"mutation { P(name: "a") }"#));
    assert!(dup(&z, S, r#"mutation { P(name: "x") }"#));
    ok(&z, S, r#"mutation { P(name: "y") }"#);
}

#[test]
fn floats_in_a_unique_field() {
    let z = Zega::in_memory().build().unwrap();
    ok(&z, S, r#"mutation { P(name: "z" && n: 0.0) }"#);
    // 1 (Int) vs 1.0: ZQL literals
    ok(&z, S, r#"mutation { P(name: "one" && n: 1.5) }"#);
    assert!(dup(&z, S, r#"mutation { P(name: "z2" && n: 0.0) }"#));
    assert!(dup(&z, S, r#"mutation { P(name: "o2" && n: 1.5) }"#));
}

fn fill(z: &Zega) { for i in 0..50 { ok(z, S, &format!(r#"mutation {{ P(name: "p{i}" && n: {i}.5) }}"#)); } }

#[test]
fn lazy_build_after_restore_and_import_and_clear() {
    let z = Zega::in_memory().build().unwrap();
    fill(&z);
    let snap = z.snapshot_bytes().unwrap();
    let mut file = Vec::new();
    z.export(&mut file).unwrap();

    // restore: the first statement is a duplicate write
    let r = Zega::in_memory().build().unwrap();
    r.restore_bytes(&snap).unwrap();
    assert!(dup(&r, S, r#"mutation { P(name: "p7") }"#));
    assert!(dup(&r, S, r#"mutation { P(name: "new" && n: 3.5) }"#));
    // restore over a graph that already had indexes built
    let r2 = Zega::in_memory().build().unwrap();
    ok(&r2, S, r#"mutation { P(name: "only-here") }"#);
    r2.restore_bytes(&snap).unwrap();
    ok(&r2, S, r#"mutation { P(name: "only-here") }"#);
    assert!(dup(&r2, S, r#"mutation { P(name: "p8") }"#));

    // import
    let i = Zega::in_memory().build().unwrap();
    ok(&i, S, r#"mutation { P(name: "gone") }"#);
    i.import(&file[..]).unwrap();
    assert!(dup(&i, S, r#"mutation { P(name: "p9") }"#));
    ok(&i, S, r#"mutation { P(name: "gone") }"#);

    // clear
    z.clear().unwrap();
    ok(&z, S, r#"mutation { P(name: "p7") }"#);
    assert!(dup(&z, S, r#"mutation { P(name: "p7") }"#));
}

#[test]
fn reopen_from_disk_then_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("d");
    {
        let z = Zega::open(path.to_str().unwrap()).build().unwrap();
        fill(&z);
        z.snapshot().unwrap();
        ok(&z, S, r#"mutation { P(name: "after-snap") }"#);
    }
    let z = Zega::open(path.to_str().unwrap()).build().unwrap();
    assert!(dup(&z, S, r#"mutation { P(name: "p1") }"#));
    assert!(dup(&z, S, r#"mutation { P(name: "after-snap") }"#));
}
