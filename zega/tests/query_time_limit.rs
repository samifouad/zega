//! A ZQL statement that runs past `query_time_limit` stops where it is, with
//! its own error, and a mutation that does leaves nothing behind (#63).
use std::collections::HashMap;
use std::time::{Duration, Instant};
use zega::{Zega, ZegaError};

const SCHEMA: &str = "schema { type Stop { name: String at: Point seen?: Int } }";

/// `n` stops at one place. `near { &at < 0 m }` then tests every pair and
/// keeps none: quadratic work, and no result to hold in memory.
fn stops(n: usize) -> HashMap<String, String> {
    let rows: Vec<String> = (0..n)
        .map(|i| format!(r#"{{"name":"s{i}","at":{{"lat":51.05,"lon":-114.07}}}}"#))
        .collect();
    HashMap::from([("stops.json".to_string(), format!("[{}]", rows.join(",")))])
}

fn load_stops(db: &Zega, n: usize) {
    db.run_lang_with_sources(SCHEMA, r#"mutation json ["stops.json"] { Stop(name: $name && at: $at) }"#, &stops(n))
        .unwrap();
}

const PAIRS: &str = "query { Stop } display { skip } then { near { &at < 0 m } }";

/// Each row finds its stop by name and marks it seen. No index answers the
/// name, so every row scans every stop: quadratic, and none of it is
/// traversal budget.
fn visits(n: usize) -> HashMap<String, String> {
    let rows: Vec<String> = (0..n).map(|i| format!(r#"{{"n":{i},"stop":"s{i}"}}"#)).collect();
    HashMap::from([("visits.json".to_string(), format!("[{}]", rows.join(",")))])
}
const VISITS: &str = r#"mutation json ["visits.json"] { Stop(name = $stop) set seen: $n }"#;

fn count(db: &Zega, query: &str) -> usize {
    db.run_lang(SCHEMA, query).unwrap().as_array().unwrap().len()
}
const ALL: &str = "query { Stop { name } }";
const SEEN: &str = "query { Stop(seen >= 0) { name } }";

#[test]
fn a_slow_read_stops_at_the_limit_with_its_own_error() {
    let limit = Duration::from_millis(200);
    let db = Zega::in_memory()
        .traversal_work_budget(usize::MAX)
        .query_time_limit(limit)
        .build()
        .unwrap();
    load_stops(&db, 2000); // about 4 s of pairs in a debug build
    let started = Instant::now();
    let error = db.run_lang(SCHEMA, PAIRS).unwrap_err();
    let took = started.elapsed();
    assert!(matches!(error, ZegaError::QueryTimeLimit { limit: l } if l == limit), "{error}");
    assert_eq!(error.to_string(), "query exceeded the 0.2 s limit");
    assert!(took >= limit && took < limit * 5, "stopped after {took:?}");
    // The database is fine afterwards.
    assert_eq!(count(&db, ALL), 2000);
}

/// One scan: every stop tested against a thousand names, none of them
/// indexed, none matching. Seconds of work inside a single filter.
fn one_long_scan() -> String {
    let names: Vec<String> = (0..1000).map(|i| format!("name = \"x{i}\"")).collect();
    format!("query {{ Stop({}) {{ name }} }}", names.join(" || "))
}

#[test]
fn a_single_long_scan_stops_at_the_limit() {
    let limit = Duration::from_millis(200);
    let db = Zega::in_memory().query_time_limit(limit).build().unwrap();
    load_stops(&db, 4000);
    let started = Instant::now();
    let error = db.run_lang(SCHEMA, &one_long_scan()).unwrap_err();
    let took = started.elapsed();
    assert!(matches!(error, ZegaError::QueryTimeLimit { .. }), "{error}");
    assert!(took >= limit && took < limit * 5, "stopped after {took:?}");
}

#[test]
fn a_mutation_that_hits_the_limit_writes_nothing_in_memory_or_on_disk() {
    let data = tempfile::tempdir().unwrap();
    let path = data.path().to_str().unwrap();
    {
        let db = Zega::open(path).build().unwrap();
        load_stops(&db, 3000);
    }
    {
        let db = Zega::open(path)
            .query_time_limit(Duration::from_millis(300))
            .build()
            .unwrap();
        let started = Instant::now();
        let error = db.run_lang_with_sources(SCHEMA, VISITS, &visits(3000)).unwrap_err();
        assert!(matches!(error, ZegaError::QueryTimeLimit { .. }), "{error}");
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
        assert_eq!(count(&db, SEEN), 0, "rolled back in memory");
    }
    let reopened = Zega::open(path).build().unwrap();
    assert_eq!(count(&reopened, SEEN), 0, "nothing reached the WAL");
    assert_eq!(count(&reopened, ALL), 3000);
}

#[test]
fn without_a_limit_nothing_is_stopped() {
    // The default: an embedded or in-browser database has no clock to answer to.
    let db = Zega::in_memory().build().unwrap();
    load_stops(&db, 300);
    db.run_lang_with_sources(SCHEMA, VISITS, &visits(300)).unwrap();
    assert_eq!(count(&db, SEEN), 300);
    assert!(db.run_lang(SCHEMA, PAIRS).is_ok());
}

#[test]
fn a_limit_that_is_not_reached_changes_nothing() {
    let db = Zega::in_memory().query_time_limit(Duration::from_secs(30)).build().unwrap();
    load_stops(&db, 300);
    db.run_lang_with_sources(SCHEMA, VISITS, &visits(300)).unwrap();
    assert_eq!(count(&db, SEEN), 300);
}

