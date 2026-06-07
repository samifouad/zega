#[path = "parity/canonical.rs"]
mod canonical;
#[path = "parity_fuzz/generator.rs"]
mod generator;
#[path = "parity_fuzz/shrink.rs"]
mod shrink;

use canonical::CanonicalRows;
use generator::{ColumnKind, GeneratedGraph, GeneratedQuery, Rng};
use neo4rs::{query, Graph};
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
        .map(|rows| canonical::zega_rows(rows, generated_query.order.is_some()))
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
            let value = match column.kind {
                ColumnKind::String => {
                    canonical::string(row.get::<String>(column.alias).map_err(|e| e.to_string())?)
                }
                ColumnKind::Integer => {
                    canonical::integer(row.get::<i64>(column.alias).map_err(|e| e.to_string())?)
                }
                ColumnKind::Float => {
                    canonical::float(row.get::<f64>(column.alias).map_err(|e| e.to_string())?)
                }
                ColumnKind::Boolean => {
                    canonical::boolean(row.get::<bool>(column.alias).map_err(|e| e.to_string())?)
                }
            };
            fields.push((column.alias.to_string(), value));
        }
        rows.push(canonical::row(fields));
    }
    Ok(canonical::normalize(rows, generated_query.order.is_some()))
}

fn compare_results(
    zega: Result<CanonicalRows, String>,
    neo4j: Result<CanonicalRows, String>,
) -> Option<String> {
    match (zega, neo4j) {
        (Ok(left), Ok(right)) => canonical::compare(&left, &right).err(),
        (Err(_), Err(_)) => None,
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
