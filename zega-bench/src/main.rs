// zega-bench: side by side, zega (embedded, ZQL) and Neo4j (client/server,
// Cypher over Bolt). Identical workloads: load USERS single-node writes, then
// LOOKUPS indexed lookups by id. Reports throughput (ops/sec) and latency
// (p50/p99). Memory (RSS) is captured by the surrounding driver script.
//
// Until zega#55 the zega side ran the same Cypher text as Neo4j through
// zega's legacy query path. That path is gone; the zega side now runs ZQL
// through `Zega::run_lang`, the call `zega-server` makes for `/zql`. ZQL has
// no parameters, so each statement's values are written into its text, and
// every call parses its schema; both costs are inside the measured time.

use std::time::Instant;
use zega::Zega;

const USERS: usize = 10_000;
const LOOKUPS: usize = 10_000;

/// `unique { User { id } }` gives `User(id: …)` an index, as the Neo4j side's
/// `CREATE INDEX … ON (u.id)` does.
const SCHEMA: &str = "schema { type User { id: String email: String } } unique { User { id } }";

fn pct(mut v: Vec<u128>, p: f64) -> u128 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    let idx = ((v.len() as f64 - 1.0) * p).round() as usize;
    v[idx]
}

fn report(name: &str, n: usize, total_ns: u128, lats: &[u128]) {
    let secs = total_ns as f64 / 1e9;
    let ops = n as f64 / secs;
    let p50 = pct(lats.to_vec(), 0.50);
    let p99 = pct(lats.to_vec(), 0.99);
    println!(
        "{:<34} {:>12.0} ops/s   p50={:>8.2}us  p99={:>8.2}us",
        name,
        ops,
        p50 as f64 / 1000.0,
        p99 as f64 / 1000.0
    );
}

fn bench_zega() {
    println!("\n=== ZEGA (embedded, in-process, ZQL) ===");
    let zega = Zega::in_memory().build().expect("zega build");

    // Graph load: USERS User nodes, one write statement each.
    let t = Instant::now();
    for i in 0..USERS {
        let write = format!(r#"mutation {{ User(id: "u{i}" && email: "u{i}@t.gg") }}"#);
        zega.run_lang(SCHEMA, &write).unwrap();
    }
    println!(
        "{:<34} loaded {} users in {:.2}s",
        "zega graph load",
        USERS,
        t.elapsed().as_secs_f64()
    );

    // Graph node lookup (indexed by id).
    let mut lats = Vec::with_capacity(LOOKUPS);
    let t = Instant::now();
    for i in 0..LOOKUPS {
        let uid = i % USERS;
        let read = format!(r#"query {{ User(id: "u{uid}") {{ id email }} }}"#);
        let s = Instant::now();
        let found = zega.run_lang(SCHEMA, &read).unwrap();
        lats.push(s.elapsed().as_nanos());
        assert!(found.is_object(), "u{uid} not found: {found}");
    }
    report("zega node lookup", LOOKUPS, t.elapsed().as_nanos(), &lats);
}

async fn bench_neo4j() {
    use neo4rs::{query, Graph};
    println!("\n=== NEO4J (localhost:7687, Bolt over TCP) ===");
    let graph = match Graph::new("127.0.0.1:7687", "neo4j", "benchpass123").await {
        Ok(g) => g,
        Err(e) => {
            println!("neo4j connect failed: {e}");
            return;
        }
    };
    let _ = graph.run(query("MATCH (n) DETACH DELETE n")).await;
    let _ = graph
        .run(query("CREATE INDEX user_id IF NOT EXISTS FOR (u:User) ON (u.id)"))
        .await;

    let t = Instant::now();
    for i in 0..USERS {
        graph
            .run(
                query("CREATE (u:User {id: $id, email: $email})")
                    .param("id", format!("u{i}"))
                    .param("email", format!("u{i}@t.gg")),
            )
            .await
            .unwrap();
    }
    println!(
        "{:<34} loaded {} users in {:.2}s",
        "neo4j graph load",
        USERS,
        t.elapsed().as_secs_f64()
    );

    let mut lats = Vec::with_capacity(LOOKUPS);
    let t = Instant::now();
    for i in 0..LOOKUPS {
        let uid = i % USERS;
        let s = Instant::now();
        let mut r = graph
            .execute(query("MATCH (u:User {id:$id}) RETURN u").param("id", format!("u{uid}")))
            .await
            .unwrap();
        while let Ok(Some(_)) = r.next().await {}
        lats.push(s.elapsed().as_nanos());
    }
    report("neo4j node lookup", LOOKUPS, t.elapsed().as_nanos(), &lats);
}

#[tokio::main]
async fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();
    println!("zega-bench USERS={USERS} LOOKUPS={LOOKUPS}");
    match arg.as_str() {
        "zega" => bench_zega(),
        "neo4j" => bench_neo4j().await,
        _ => {
            bench_zega();
            bench_neo4j().await;
        }
    }
}
