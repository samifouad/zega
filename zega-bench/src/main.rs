// zega-bench — side-by-side: zega-core (embedded) vs Redis+Neo4j (client/server).
// Identical workloads. Reports throughput (ops/sec) and latency (p50/p99).
// Memory (RSS) is captured by the surrounding driver script via /proc.

use std::collections::HashMap;
use std::time::Instant;
use zega_core::Zega;
use zega_parser::Value;

const KV_N: usize = 100_000;
const USERS: usize = 10_000;
const ORDERS: usize = 50_000;
const LOOKUPS: usize = 10_000;

fn pct(mut v: Vec<u128>, p: f64) -> u128 {
    if v.is_empty() { return 0; }
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
        name, ops, p50 as f64 / 1000.0, p99 as f64 / 1000.0
    );
}

fn bench_zega() {
    println!("\n=== ZEGA (embedded, in-process) ===");
    let zega = Zega::in_memory().build().expect("zega build");

    // KV SET
    let mut lats = Vec::with_capacity(KV_N);
    let t = Instant::now();
    for i in 0..KV_N {
        let s = Instant::now();
        zega.kv_set(format!("k:{i}"), Value::String(format!("v:{i}")), None).unwrap();
        lats.push(s.elapsed().as_nanos());
    }
    report("zega kv_set", KV_N, t.elapsed().as_nanos(), &lats);

    // KV GET
    let mut lats = Vec::with_capacity(KV_N);
    let t = Instant::now();
    for i in 0..KV_N {
        let s = Instant::now();
        let _ = zega.kv_get(&format!("k:{i}"));
        lats.push(s.elapsed().as_nanos());
    }
    report("zega kv_get", KV_N, t.elapsed().as_nanos(), &lats);

    // Graph load: USERS User nodes (single-node CREATE; MVP parser does not yet
    // support CREATE with relationships, so 1-hop traversal is deferred to round 2)
    let t = Instant::now();
    for i in 0..USERS {
        let mut p = HashMap::new();
        p.insert("id".into(), Value::String(format!("u{i}")));
        p.insert("email".into(), Value::String(format!("u{i}@t.gg")));
        zega.query("CREATE (u:User {id: $id, email: $email})", p).unwrap();
    }
    println!("{:<34} loaded {} users in {:.2}s",
        "zega graph load", USERS, t.elapsed().as_secs_f64());

    // Graph node lookup (indexed by id)
    let mut lats = Vec::with_capacity(LOOKUPS);
    let t = Instant::now();
    for i in 0..LOOKUPS {
        let uid = i % USERS;
        let mut p = HashMap::new();
        p.insert("id".into(), Value::String(format!("u{uid}")));
        let s = Instant::now();
        let _ = zega.query("MATCH (u:User {id: $id}) RETURN u", p).unwrap();
        lats.push(s.elapsed().as_nanos());
    }
    report("zega node lookup", LOOKUPS, t.elapsed().as_nanos(), &lats);
}

fn bench_redis() {
    println!("\n=== REDIS (localhost:6379, RESP over TCP) ===");
    let client = redis::Client::open("redis://127.0.0.1:6379").expect("redis client");
    let mut con = client.get_connection().expect("redis connect");

    let mut lats = Vec::with_capacity(KV_N);
    let t = Instant::now();
    for i in 0..KV_N {
        let s = Instant::now();
        let _: () = redis::cmd("SET").arg(format!("k:{i}")).arg(format!("v:{i}"))
            .query(&mut con).unwrap();
        lats.push(s.elapsed().as_nanos());
    }
    report("redis SET", KV_N, t.elapsed().as_nanos(), &lats);

    let mut lats = Vec::with_capacity(KV_N);
    let t = Instant::now();
    for i in 0..KV_N {
        let s = Instant::now();
        let _: Option<String> = redis::cmd("GET").arg(format!("k:{i}"))
            .query(&mut con).unwrap();
        lats.push(s.elapsed().as_nanos());
    }
    report("redis GET", KV_N, t.elapsed().as_nanos(), &lats);
}

async fn bench_neo4j() {
    use neo4rs::{query, Graph};
    println!("\n=== NEO4J (localhost:7687, Bolt over TCP) ===");
    let graph = match Graph::new("127.0.0.1:7687", "neo4j", "benchpass123").await {
        Ok(g) => g,
        Err(e) => { println!("neo4j connect failed: {e}"); return; }
    };
    let _ = graph.run(query("MATCH (n) DETACH DELETE n")).await;
    let _ = graph.run(query("CREATE INDEX user_id IF NOT EXISTS FOR (u:User) ON (u.id)")).await;

    let t = Instant::now();
    for i in 0..USERS {
        graph.run(query("CREATE (u:User {id: $id, email: $email})")
            .param("id", format!("u{i}")).param("email", format!("u{i}@t.gg"))).await.unwrap();
    }
    println!("{:<34} loaded {} users in {:.2}s",
        "neo4j graph load", USERS, t.elapsed().as_secs_f64());

    let mut lats = Vec::with_capacity(LOOKUPS);
    let t = Instant::now();
    for i in 0..LOOKUPS {
        let uid = i % USERS;
        let s = Instant::now();
        let mut r = graph.execute(query("MATCH (u:User {id:$id}) RETURN u")
            .param("id", format!("u{uid}"))).await.unwrap();
        while let Ok(Some(_)) = r.next().await {}
        lats.push(s.elapsed().as_nanos());
    }
    report("neo4j node lookup", LOOKUPS, t.elapsed().as_nanos(), &lats);
}

#[tokio::main]
async fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();
    println!("zega-bench  KV_N={KV_N} USERS={USERS} ORDERS={ORDERS} LOOKUPS={LOOKUPS}");
    match arg.as_str() {
        "zega" => bench_zega(),
        "redis" => bench_redis(),
        "neo4j" => bench_neo4j().await,
        _ => { bench_zega(); bench_redis(); bench_neo4j().await; }
    }
}
