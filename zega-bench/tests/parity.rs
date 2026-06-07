mod parity {
    pub mod canonical;
    pub mod corpus;
}

use neo4rs::{query, Graph};
use parity::canonical::{self, CanonicalRows};
use parity::corpus::{ColumnKind, GraphOp};
use redis::Connection;
use serde_json::Value as Json;
use std::collections::HashMap;
use std::env;
use std::time::{SystemTime, UNIX_EPOCH};
use zega_core::Zega;
use zega_parser::Value;

#[test]
fn unordered_rows_compare_equal_after_normalization() {
    let a = vec![
        canonical::row([("id".into(), canonical::string("b"))]),
        canonical::row([("id".into(), canonical::string("a"))]),
    ];
    let b = vec![
        canonical::row([("id".into(), canonical::string("a"))]),
        canonical::row([("id".into(), canonical::string("b"))]),
    ];
    assert!(canonical::compare(
        &canonical::normalize(a, false),
        &canonical::normalize(b, false)
    )
    .is_ok());
}

#[test]
fn genuine_value_difference_is_reported() {
    let zega = vec![canonical::row([("count".into(), canonical::integer(2))])];
    let oracle = vec![canonical::row([("count".into(), canonical::integer(3))])];
    let diff = canonical::compare(&zega, &oracle).expect_err("values differ");
    assert!(diff.contains("first difference at row 0"));
    assert!(diff.contains("\"value\":2"));
    assert!(diff.contains("\"value\":3"));
}

#[test]
fn type_only_difference_is_reported() {
    let zega = vec![canonical::row([("count".into(), canonical::integer(2))])];
    let oracle = vec![canonical::row([("count".into(), canonical::float(2.0))])];
    let diff = canonical::compare(&zega, &oracle).expect_err("types differ");
    assert!(diff.contains("\"type\":\"integer\""));
    assert!(diff.contains("\"type\":\"float\""));
}

#[test]
fn ordered_rows_are_not_reordered_before_comparison() {
    let zega = canonical::normalize(
        vec![
            canonical::row([("id".into(), canonical::string("a"))]),
            canonical::row([("id".into(), canonical::string("b"))]),
        ],
        true,
    );
    let oracle = canonical::normalize(
        vec![
            canonical::row([("id".into(), canonical::string("b"))]),
            canonical::row([("id".into(), canonical::string("a"))]),
        ],
        true,
    );
    let diff = canonical::compare(&zega, &oracle).expect_err("ordered rows differ");
    assert!(diff.contains("first difference at row 0"));
}

#[test]
fn float_last_digit_difference_is_reported() {
    let zega = vec![canonical::row([("average".into(), canonical::float(1.0))])];
    let oracle = vec![canonical::row([(
        "average".into(),
        canonical::float(f64::from_bits(1.0f64.to_bits() + 1)),
    )])];
    assert!(canonical::compare(&zega, &oracle).is_err());
}

#[test]
fn non_finite_float_difference_is_reported() {
    let zega = vec![canonical::row([(
        "average".into(),
        canonical::float(f64::INFINITY),
    )])];
    let oracle = vec![canonical::row([(
        "average".into(),
        canonical::float(f64::NEG_INFINITY),
    )])];
    assert!(canonical::compare(&zega, &oracle).is_err());
}

#[test]
fn missing_null_and_empty_string_are_not_equal() {
    let missing = vec![canonical::row([])];
    let null = vec![canonical::row([("value".into(), canonical::null())])];
    let empty = vec![canonical::row([("value".into(), canonical::string(""))])];
    assert!(canonical::compare(&missing, &null).is_err());
    assert!(canonical::compare(&null, &empty).is_err());
}

#[tokio::test]
async fn live_zega_neo4j_redis_parity() {
    if env::var("ZEGA_PARITY_LIVE").as_deref() != Ok("1") {
        println!("reference not configured, ran 2 self-tests");
        println!("Set ZEGA_PARITY_LIVE=1 to use localhost defaults or override NEO4J_URI/USER/PASS and REDIS_URL.");
        return;
    }

    let run = format!(
        "parity-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_millis()
    );
    let neo4j_uri = env::var("NEO4J_URI").unwrap_or_else(|_| "127.0.0.1:7687".into());
    let neo4j_address = neo4j_uri
        .strip_prefix("bolt://")
        .or_else(|| neo4j_uri.strip_prefix("neo4j://"))
        .unwrap_or(&neo4j_uri);
    let neo4j_user = env::var("NEO4J_USER").unwrap_or_else(|_| "neo4j".into());
    let neo4j_pass = env::var("NEO4J_PASS").unwrap_or_else(|_| "neo4j".into());
    let redis_url = env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".into());

    let graph = Graph::new(neo4j_address, &neo4j_user, &neo4j_pass)
        .await
        .unwrap_or_else(|error| panic!("Neo4j reference connection failed ({neo4j_uri}): {error}"));
    let client = redis::Client::open(redis_url.as_str()).expect("valid REDIS_URL");
    let mut redis = client
        .get_connection()
        .unwrap_or_else(|error| panic!("Redis reference connection failed ({redis_url}): {error}"));
    let zega = Zega::in_memory().build().expect("embedded Zega");

    load_fixture(&zega, &graph, &run).await;
    let mut reports = Vec::new();
    for op in parity::corpus::graph_ops(&run) {
        reports.push(run_graph_op(&zega, &graph, &op).await);
    }
    reports.extend(run_kv_ops(&zega, &mut redis, &run));
    let _ = graph
        .run(query("MATCH (n {parity_run: $run}) DETACH DELETE n").param("run", run.clone()))
        .await;
    let _: redis::RedisResult<()> = redis::cmd("DEL")
        .arg(format!("{run}:value"))
        .arg(format!("{run}:counter"))
        .query(&mut redis);

    println!("\n{:<34} RESULT", "OPERATION");
    for report in &reports {
        println!(
            "{:<34} {}",
            report.name,
            if report.diff.is_none() {
                "MATCH"
            } else {
                "MISMATCH"
            }
        );
        if let Some(diff) = &report.diff {
            println!("{diff}");
        }
    }
    let passed = reports
        .iter()
        .filter(|report| report.diff.is_none())
        .count();
    println!("PARITY: {passed}/{} ops match", reports.len());
    assert_eq!(passed, reports.len(), "parity mismatch; see report above");
}

struct Report {
    name: &'static str,
    diff: Option<String>,
}

async fn load_fixture(zega: &Zega, graph: &Graph, run: &str) {
    for statement in parity::corpus::fixture(run) {
        zega.query(&statement, HashMap::new())
            .unwrap_or_else(|error| panic!("Zega fixture failed for {statement}: {error}"));
        graph
            .run(query(&statement))
            .await
            .unwrap_or_else(|error| panic!("Neo4j fixture failed for {statement}: {error}"));
    }
}

async fn run_graph_op(zega: &Zega, graph: &Graph, op: &GraphOp) -> Report {
    let zega_rows = zega
        .query(&op.query, HashMap::new())
        .map(|rows| canonical::zega_rows(rows, op.ordered));
    let reference_rows = neo4j_rows(graph, op).await;
    Report {
        name: op.name,
        diff: compare_results(zega_rows, reference_rows),
    }
}

async fn neo4j_rows(graph: &Graph, op: &GraphOp) -> Result<CanonicalRows, String> {
    let mut stream = graph
        .execute(query(&op.query))
        .await
        .map_err(|e| e.to_string())?;
    let mut rows = Vec::new();
    while let Some(row) = stream.next().await.map_err(|e| e.to_string())? {
        let mut fields = Vec::new();
        for (name, kind) in op.columns {
            let value = match kind {
                ColumnKind::String => canonical::string(
                    row.get::<String>(name)
                        .map_err(|e| format!("{name}: {e}"))?,
                ),
                ColumnKind::Integer => {
                    canonical::integer(row.get::<i64>(name).map_err(|e| format!("{name}: {e}"))?)
                }
                ColumnKind::Float => {
                    canonical::float(row.get::<f64>(name).map_err(|e| format!("{name}: {e}"))?)
                }
            };
            fields.push(((*name).to_string(), value));
        }
        rows.push(canonical::row(fields));
    }
    Ok(canonical::normalize(rows, op.ordered))
}

fn compare_results(
    zega: Result<CanonicalRows, zega_core::ZegaError>,
    reference: Result<CanonicalRows, String>,
) -> Option<String> {
    match (zega, reference) {
        (Ok(zega), Ok(reference)) => canonical::compare(&zega, &reference).err(),
        (Err(zega), Err(reference)) => Some(format!(
            "both backends errored (still a mismatch):\n  zega: {zega}\n  reference: {reference}"
        )),
        (Err(zega), Ok(reference)) => Some(format!(
            "zega errored: {zega}\n  reference: {}",
            Json::Array(reference)
        )),
        (Ok(zega), Err(reference)) => Some(format!(
            "reference errored: {reference}\n  zega: {}",
            Json::Array(zega)
        )),
    }
}

fn run_kv_ops(zega: &Zega, redis: &mut Connection, run: &str) -> Vec<Report> {
    let value_key = format!("{run}:value");
    let counter_key = format!("{run}:counter");
    let missing_key = format!("{run}:missing");
    let mut reports = Vec::new();

    let zega_set = zega.kv_set(value_key.clone(), Value::String("hello".into()), None);
    let redis_set: redis::RedisResult<String> =
        redis::cmd("SET").arg(&value_key).arg("hello").query(redis);
    reports.push(kv_report(
        "kv.set",
        zega_set.map(|_| canonical::string("OK")),
        redis_set.map(canonical::string),
    ));
    reports.push(kv_report(
        "kv.get",
        Ok(zega
            .kv_get(&value_key)
            .map_or_else(canonical::null, |v| canonical::zega_value(&v))),
        redis::cmd("GET")
            .arg(&value_key)
            .query::<Option<String>>(redis)
            .map(|v| v.map_or_else(canonical::null, canonical::string)),
    ));
    reports.push(kv_report(
        "kv.del",
        zega.kv_del(&value_key).map(canonical::boolean),
        redis::cmd("DEL")
            .arg(&value_key)
            .query::<i64>(redis)
            .map(|v| canonical::boolean(v == 1)),
    ));
    reports.push(kv_report(
        "kv.incr",
        zega.query(&format!("INCR KEY '{counter_key}'"), HashMap::new())
            .map(|rows| canonical::zega_rows(rows, true)[0]["value"].clone()),
        redis::cmd("INCR")
            .arg(&counter_key)
            .query::<i64>(redis)
            .map(canonical::integer),
    ));
    reports.push(kv_report(
        "kv.get_missing",
        Ok(zega
            .kv_get(&missing_key)
            .map_or_else(canonical::null, |v| canonical::zega_value(&v))),
        redis::cmd("GET")
            .arg(&missing_key)
            .query::<Option<String>>(redis)
            .map(|v| v.map_or_else(canonical::null, canonical::string)),
    ));
    reports
}

fn kv_report(
    name: &'static str,
    zega: Result<Json, zega_core::ZegaError>,
    reference: redis::RedisResult<Json>,
) -> Report {
    let diff = match (zega, reference) {
        (Ok(zega), Ok(reference)) if zega == reference => None,
        (Ok(zega), Ok(reference)) => Some(format!("zega: {zega}\n  reference: {reference}")),
        (Err(zega), Ok(reference)) => {
            Some(format!("zega errored: {zega}\n  reference: {reference}"))
        }
        (Ok(zega), Err(reference)) => {
            Some(format!("reference errored: {reference}\n  zega: {zega}"))
        }
        (Err(zega), Err(reference)) => Some(format!(
            "zega errored: {zega}\n  reference errored: {reference}"
        )),
    };
    Report { name, diff }
}
