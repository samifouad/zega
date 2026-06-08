#[path = "parity/canonical.rs"]
mod canonical;
#[path = "parity_fuzz/generator.rs"]
mod generator;
#[path = "parity_fuzz/shrink.rs"]
mod shrink;

use canonical::CanonicalRows;
use generator::{
    ColumnKind, GeneratedGraph, GeneratedQuery, QueryShape, Rng, SortValue, TraversalLength,
};
use neo4rs::{query, BoltType, Graph};
use std::collections::HashMap;
use std::env;
use zega_core::Zega;
use zega_parser::Parser;

const DEFAULT_SEED: u64 = 0x5eed_cafe_d15c_a11e;

#[test]
fn generator_is_deterministic_for_fixed_seed() {
    let mut left = Rng::new(42);
    let mut right = Rng::new(42);
    for _ in 0..20 {
        assert_eq!(
            generator::generate_case(&mut left),
            generator::generate_case(&mut right)
        );
    }
}

#[test]
fn shrinker_reduces_a_seeded_synthetic_mismatch() {
    let (graph, query) = generator::generate_case(&mut Rng::new(7));
    let before = graph.nodes.len()
        + graph.relationships.len()
        + query.predicates.len()
        + query.columns.len();
    let (graph, query) = shrink::shrink_with(graph, query, |graph, query| {
        !graph.nodes.is_empty() && !query.columns.is_empty()
    });
    let after = graph.nodes.len()
        + graph.relationships.len()
        + query.predicates.len()
        + query.columns.len();
    assert!(after < before, "{before} should shrink below {after}");
    assert_eq!(graph.nodes.len(), 1);
    assert_eq!(query.columns.len(), 1);
}

#[test]
fn generated_cypher_parses() {
    let mut rng = Rng::new(99);
    for case in 0..500 {
        let (graph, generated_query) = generator::generate_case(&mut rng);
        for statement in graph.creates("parse-self-test") {
            Parser::new(&statement)
                .unwrap()
                .parse()
                .unwrap_or_else(|error| panic!("case {case}: {statement}: {error}"));
        }
        let statement = generated_query.cypher("parse-self-test");
        Parser::new(&statement)
            .unwrap()
            .parse()
            .unwrap_or_else(|error| panic!("case {case}: {statement}: {error}"));
    }
}

#[test]
fn generated_grammar_covers_v2_shapes() {
    let mut rng = Rng::new(DEFAULT_SEED);
    let mut null_sort = false;
    let mut sort_kinds = std::collections::HashSet::new();
    let mut raw_order = false;
    let mut alias_order = false;
    let mut order_lengths = [false; 4];
    let mut mixed_directions = false;
    let mut aggregates = std::collections::HashSet::new();
    let mut grouped = [false; 2];
    let mut aggregate_order = false;
    let mut traversal = [false; 4];
    for _ in 0..1000 {
        let (graph, query) = generator::generate_case(&mut rng);
        for node in graph.nodes {
            match node.sort {
                SortValue::Null => null_sort = true,
                SortValue::String(_) => {
                    sort_kinds.insert("string");
                }
                SortValue::Integer(_) => {
                    sort_kinds.insert("integer");
                }
                SortValue::Float(_) => {
                    sort_kinds.insert("float");
                }
                SortValue::Boolean(_) => {
                    sort_kinds.insert("boolean");
                }
            }
        }
        for key in &query.order {
            raw_order |= key.expression.starts_with("n.");
            alias_order |= !key.expression.contains('.');
        }
        order_lengths[query.order.len().min(3)] = true;
        mixed_directions |= query
            .order
            .windows(2)
            .any(|keys| keys[0].desc != keys[1].desc);
        if matches!(&query.shape, QueryShape::Aggregate) {
            grouped[(query.columns.len() > 1) as usize] = true;
            aggregates.insert(query.columns.last().unwrap().expression);
            aggregate_order |= query.is_ordered();
        }
        if let QueryShape::Traversal { length, .. } = &query.shape {
            traversal[match length {
                TraversalLength::Unbounded => 0,
                TraversalLength::Bounded(_, _) => 1,
                TraversalLength::UpTo(_) => 2,
                TraversalLength::AtLeast(_) => 3,
            }] = true;
        }
    }
    assert!(null_sort && sort_kinds.len() == 4);
    assert!(raw_order && alias_order && order_lengths[2] && order_lengths[3]);
    assert!(mixed_directions);
    assert_eq!(aggregates.len(), 7);
    assert!(grouped.into_iter().all(|covered| covered) && aggregate_order);
    assert!(traversal.into_iter().all(|covered| covered));
}

#[test]
fn v2_shrink_candidates_keep_queries_parseable() {
    let mut rng = Rng::new(DEFAULT_SEED);
    for case in 0..200 {
        let (_, query) = generator::generate_case(&mut rng);
        for candidate in shrink::query_candidates(&query) {
            let statement = candidate.cypher("shrink-parse-self-test");
            Parser::new(&statement)
                .unwrap()
                .parse()
                .unwrap_or_else(|error| panic!("case {case}: {statement}: {error}"));
        }
    }
}

#[test]
fn rejected_queries_are_not_counted_as_parity() {
    let diff = compare_results(Err("zega error".into()), Err("neo4j error".into()))
        .expect("two rejected executions are not a successful differential comparison");
    assert!(diff.contains("Zega error") && diff.contains("Neo4j error"));
}

#[test]
fn live_comparison_tolerates_float_accumulation_noise_recursively() {
    let left = vec![canonical::row([
        ("sum".into(), canonical::float(63.900000000000006)),
        (
            "nested".into(),
            serde_json::json!({
                "type": "map",
                "value": {
                    "values": {
                        "type": "list",
                        "value": [canonical::float(18.333333333333332)]
                    }
                }
            }),
        ),
    ])];
    let right = vec![canonical::row([
        ("sum".into(), canonical::float(63.9)),
        (
            "nested".into(),
            serde_json::json!({
                "type": "map",
                "value": {
                    "values": {
                        "type": "list",
                        "value": [canonical::float(18.333333333333336)]
                    }
                }
            }),
        ),
    ])];

    assert!(canonical::compare_tolerant(&left, &right).is_ok());
}

#[test]
fn live_comparison_keeps_real_values_and_type_tags_exact() {
    assert!(canonical::compare_tolerant(
        &vec![canonical::row([("value".into(), canonical::float(1.0))])],
        &vec![canonical::row([("value".into(), canonical::float(1.001))])],
    )
    .is_err());
    assert!(canonical::compare_tolerant(
        &vec![canonical::row([("value".into(), canonical::integer(1))])],
        &vec![canonical::row([("value".into(), canonical::float(1.0))])],
    )
    .is_err());
}

#[tokio::test]
async fn generative_differential_parity() {
    let seed = env_u64("ZEGA_PARITY_SEED", DEFAULT_SEED);
    let cases = env_u64("ZEGA_PARITY_CASES", 1000) as usize;
    println!("GENERATIVE PARITY seed={seed}");
    if env::var("ZEGA_PARITY_LIVE").as_deref() != Ok("1") {
        println!("GENERATIVE PARITY: 0/0 cases, 0 mismatches (seed={seed}); live compare gated by ZEGA_PARITY_LIVE=1");
        return;
    }
    let graph = connect_neo4j().await;
    let mut rng = Rng::new(seed);
    let mut mismatches = Vec::new();
    for case_index in 0..cases {
        let run = format!("parity-fuzz-{}-{case_index}", std::process::id());
        let (generated_graph, generated_query) = generator::generate_case(&mut rng);
        if let Some(diff) = run_case(&graph, &run, &generated_graph, &generated_query).await {
            let (minimal_graph, minimal_query, minimal_diff) =
                shrink_live(&graph, &run, generated_graph, generated_query, diff).await;
            print_repro(
                seed,
                case_index,
                &run,
                &minimal_graph,
                &minimal_query,
                &minimal_diff,
            );
            mismatches.push(case_index);
        }
        cleanup(&graph, &run).await;
    }
    let passed = cases - mismatches.len();
    println!(
        "GENERATIVE PARITY: {passed}/{cases} cases, {} mismatches (seed={seed})",
        mismatches.len()
    );
    assert!(
        mismatches.is_empty(),
        "generative parity mismatches in cases {mismatches:?}"
    );
}

async fn connect_neo4j() -> Graph {
    let uri = env::var("NEO4J_URI").unwrap_or_else(|_| "127.0.0.1:7687".into());
    let address = uri
        .strip_prefix("bolt://")
        .or_else(|| uri.strip_prefix("neo4j://"))
        .unwrap_or(&uri);
    let user = env::var("NEO4J_USER").unwrap_or_else(|_| "neo4j".into());
    let pass = env::var("NEO4J_PASS").unwrap_or_else(|_| "neo4j".into());
    Graph::new(address, &user, &pass)
        .await
        .unwrap_or_else(|error| panic!("Neo4j reference connection failed ({uri}): {error}"))
}

async fn run_case(
    graph: &Graph,
    run: &str,
    generated_graph: &GeneratedGraph,
    generated_query: &GeneratedQuery,
) -> Option<String> {
    cleanup(graph, run).await;
    let zega = Zega::in_memory().build().expect("embedded Zega");
    for statement in generated_graph.creates(run) {
        if let Err(error) = zega.query(&statement, HashMap::new()) {
            return Some(format!("Zega graph load failed: {statement}: {error}"));
        }
        if let Err(error) = graph.run(query(&statement)).await {
            return Some(format!("Neo4j graph load failed: {statement}: {error}"));
        }
    }
    let cypher = generated_query.cypher(run);
    let zega_result = zega
        .query(&cypher, HashMap::new())
        .map(|rows| canonical::zega_rows(rows, generated_query.is_ordered()))
        .map_err(|error| error.to_string());
    let neo4j_result = neo4j_rows(graph, &cypher, generated_query).await;
    compare_results(zega_result, neo4j_result)
}

async fn neo4j_rows(
    graph: &Graph,
    cypher: &str,
    generated_query: &GeneratedQuery,
) -> Result<CanonicalRows, String> {
    let mut stream = graph
        .execute(query(cypher))
        .await
        .map_err(|error| error.to_string())?;
    let mut rows = Vec::new();
    while let Some(row) = stream.next().await.map_err(|error| error.to_string())? {
        let mut fields = Vec::new();
        for column in &generated_query.columns {
            let value = neo4j_value(
                row.get::<BoltType>(column.alias)
                    .map_err(|e| format!("{}: {e}", column.alias))?,
                column.kind,
            )?;
            fields.push((column.alias.to_string(), value));
        }
        rows.push(canonical::row(fields));
    }
    Ok(canonical::normalize(rows, generated_query.is_ordered()))
}

fn neo4j_value(value: BoltType, expected: ColumnKind) -> Result<serde_json::Value, String> {
    Ok(match value {
        BoltType::Null(_) => canonical::null(),
        BoltType::String(value) => canonical::string(value.value),
        BoltType::Integer(value) => canonical::integer(value.value),
        BoltType::Float(value) => canonical::float(value.value),
        BoltType::Boolean(value) => canonical::boolean(value.value),
        other => return Err(format!("unexpected {expected:?} Neo4j value: {other:?}")),
    })
}

fn compare_results(
    zega: Result<CanonicalRows, String>,
    neo4j: Result<CanonicalRows, String>,
) -> Option<String> {
    match (zega, neo4j) {
        (Ok(left), Ok(right)) => canonical::compare_tolerant(&left, &right).err(),
        (Err(left), Err(right)) => Some(format!("Zega error: {left}\nNeo4j error: {right}")),
        (Err(error), Ok(rows)) => Some(format!("Zega error: {error}\nNeo4j rows: {rows:?}")),
        (Ok(rows), Err(error)) => Some(format!("Zega rows: {rows:?}\nNeo4j error: {error}")),
    }
}

async fn shrink_live(
    graph: &Graph,
    run: &str,
    mut generated_graph: GeneratedGraph,
    mut generated_query: GeneratedQuery,
    mut diff: String,
) -> (GeneratedGraph, GeneratedQuery, String) {
    loop {
        let mut changed = false;
        for candidate in shrink::graph_candidates(&generated_graph) {
            if let Some(candidate_diff) = run_case(graph, run, &candidate, &generated_query).await {
                generated_graph = candidate;
                diff = candidate_diff;
                changed = true;
                break;
            }
        }
        for candidate in shrink::query_candidates(&generated_query) {
            if let Some(candidate_diff) = run_case(graph, run, &generated_graph, &candidate).await {
                generated_query = candidate;
                diff = candidate_diff;
                changed = true;
                break;
            }
        }
        if !changed {
            return (generated_graph, generated_query, diff);
        }
    }
}

async fn cleanup(graph: &Graph, run: &str) {
    let _ = graph
        .run(query("MATCH (n {parity_run: $run}) DETACH DELETE n").param("run", run.to_string()))
        .await;
}

fn print_repro(
    seed: u64,
    case: usize,
    run: &str,
    graph: &GeneratedGraph,
    generated_query: &GeneratedQuery,
    diff: &str,
) {
    println!("\nMINIMAL REPRO seed={seed} case={case}");
    for statement in graph.creates(run) {
        println!("{statement};");
    }
    println!("{};", generated_query.cypher(run));
    println!("{diff}\n");
}

fn env_u64(name: &str, default: u64) -> u64 {
    env::var(name)
        .ok()
        .map(|value| {
            value
                .parse()
                .unwrap_or_else(|_| panic!("{name} must be an unsigned integer"))
        })
        .unwrap_or(default)
}
