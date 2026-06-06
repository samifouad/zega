// commerce-bench — strenuous Zega vs Neo4j on real commerce query shapes.
// Dataset: Users -[:PLACED]-> Orders -[:CONTAINS]-> Products -[:IN_CATEGORY]-> Categories,
//          Categories -[:SUBCATEGORY]-> Categories (a small tree).
// Queries: 2-hop co-purchase recs, 3-hop customers-like-you, revenue aggregation,
//          filtered traversal, variable-length category nav.

use std::collections::HashMap;
use std::time::Instant;
use std::io::Write;
use zega_core::Zega;
use zega_parser::Value;

const USERS: usize = 10_000;
const PRODUCTS: usize = 5_000;
const ORDERS: usize = 50_000;
const ITEMS_PER_ORDER: usize = 3;     // ~150k CONTAINS edges
const CATEGORIES: usize = 200;
const REPS: usize = 2_000;            // query repetitions for timing

fn pct(mut v: Vec<u128>, p: f64) -> u128 {
    if v.is_empty() { return 0; }
    v.sort_unstable();
    v[(((v.len()-1) as f64)*p).round() as usize]
}
fn report(name: &str, n: usize, total_ns: u128, lats: &[u128]) {
    let ops = n as f64 / (total_ns as f64 / 1e9);
    println!("{:<40} {:>10.0} ops/s  p50={:>9.1}us p99={:>9.1}us",
        name, ops, pct(lats.to_vec(),0.5) as f64/1e3, pct(lats.to_vec(),0.99) as f64/1e3);
    let _=std::io::stdout().flush();
}
fn sv(s: &str) -> Value { Value::String(s.to_string()) }

fn load_zega(z: &Zega) {
    let t = Instant::now();
    // categories + subcategory tree (each cat i has parent i/3)
    for i in 0..CATEGORIES {
        let mut p = HashMap::new(); p.insert("id".into(), sv(&format!("c{i}")));
        z.query("CREATE (c:Category {id: $id})", p).unwrap();
    }
    for i in 1..CATEGORIES {
        let parent = i/3;
        let mut p = HashMap::new();
        p.insert("pid".into(), sv(&format!("c{parent}"))); p.insert("cid".into(), sv(&format!("c{i}")));
        z.query("MATCH (a:Category {id:$pid}) MATCH (b:Category {id:$cid}) CREATE (a)-[:SUBCATEGORY]->(b)", p).unwrap();
    }
    // products in categories
    for i in 0..PRODUCTS {
        let cat = i % CATEGORIES;
        let mut p = HashMap::new();
        p.insert("id".into(), sv(&format!("p{i}")));
        p.insert("stock".into(), Value::Int((i % 2) as i64));   // half in stock
        p.insert("cid".into(), sv(&format!("c{cat}")));
        z.query("MATCH (c:Category {id:$cid}) CREATE (pr:Product {id:$id, in_stock:$stock})-[:IN_CATEGORY]->(c)", p).unwrap();
    }
    // users
    for i in 0..USERS {
        let mut p = HashMap::new(); p.insert("id".into(), sv(&format!("u{i}")));
        z.query("CREATE (u:User {id:$id})", p).unwrap();
    }
    // orders + items
    for i in 0..ORDERS {
        let uid = i % USERS;
        let total = ((i*7) % 500 + 10) as i64;
        let mut p = HashMap::new();
        p.insert("uid".into(), sv(&format!("u{uid}")));
        p.insert("oid".into(), sv(&format!("o{i}")));
        p.insert("total".into(), Value::Int(total));
        z.query("MATCH (u:User {id:$uid}) CREATE (u)-[:PLACED]->(o:Order {id:$oid, total:$total})", p).unwrap();
        for k in 0..ITEMS_PER_ORDER {
            let pid = (i*ITEMS_PER_ORDER + k) % PRODUCTS;
            let mut q = HashMap::new();
            q.insert("oid".into(), sv(&format!("o{i}"))); q.insert("pid".into(), sv(&format!("p{pid}")));
            z.query("MATCH (o:Order {id:$oid}) MATCH (pr:Product {id:$pid}) CREATE (o)-[:CONTAINS]->(pr)", q).unwrap();
        }
    }
    println!("zega load: {} users {} products {} orders ~{} items in {:.1}s",
        USERS, PRODUCTS, ORDERS, ORDERS*ITEMS_PER_ORDER, t.elapsed().as_secs_f64()); let _=std::io::stdout().flush();
}

fn bench_zega() {
    println!("\n=== ZEGA commerce ===");
    let z = Zega::in_memory().build().unwrap();
    load_zega(&z);

    // Q1: 2-hop co-purchase recommendation
    let mut l=Vec::new(); let t=Instant::now();
    for i in 0..REPS {
        let mut p=HashMap::new(); p.insert("pid".into(), sv(&format!("p{}", i%PRODUCTS)));
        let s=Instant::now();
        z.query("MATCH (pr:Product {id:$pid})<-[:CONTAINS]-(o:Order)-[:CONTAINS]->(rec:Product) RETURN rec.id, count(*) AS freq ORDER BY freq DESC LIMIT 10", p).unwrap();
        l.push(s.elapsed().as_nanos());
    }
    report("Q1 2-hop co-purchase rec", REPS, t.elapsed().as_nanos(), &l);

    // Q2: 3-hop customers-like-you
    let mut l=Vec::new(); let t=Instant::now();
    for i in 0..REPS {
        let mut p=HashMap::new(); p.insert("uid".into(), sv(&format!("u{}", i%USERS)));
        let s=Instant::now();
        z.query("MATCH (u:User {id:$uid})-[:PLACED]->(:Order)-[:CONTAINS]->(:Product)<-[:CONTAINS]-(:Order)<-[:PLACED]-(other:User) RETURN other.id, count(*) AS shared ORDER BY shared DESC LIMIT 10", p).unwrap();
        l.push(s.elapsed().as_nanos());
    }
    report("Q2 3-hop customers-like-you", REPS, t.elapsed().as_nanos(), &l);

    // Q3: revenue aggregation (whole-graph, run fewer reps)
    let mut l=Vec::new(); let t=Instant::now();
    for _ in 0..20 {
        let s=Instant::now();
        z.query("MATCH (u:User)-[:PLACED]->(o:Order) RETURN u.id, count(o) AS orders, sum(o.total) AS revenue ORDER BY revenue DESC LIMIT 20", HashMap::new()).unwrap();
        l.push(s.elapsed().as_nanos());
    }
    report("Q3 revenue agg GROUP BY user", 20, t.elapsed().as_nanos(), &l);

    // Q4: filtered traversal
    let mut l=Vec::new(); let t=Instant::now();
    for i in 0..REPS {
        let mut p=HashMap::new(); p.insert("uid".into(), sv(&format!("u{}", i%USERS))); p.insert("min".into(), Value::Int(200));
        let s=Instant::now();
        z.query("MATCH (u:User {id:$uid})-[:PLACED]->(o:Order) WHERE o.total > $min RETURN o.id, o.total", p).unwrap();
        l.push(s.elapsed().as_nanos());
    }
    report("Q4 filtered traversal", REPS, t.elapsed().as_nanos(), &l);

    // Q5: variable-length category nav
    let mut l=Vec::new(); let t=Instant::now();
    for i in 0..REPS {
        let mut p=HashMap::new(); p.insert("cid".into(), sv(&format!("c{}", i%CATEGORIES)));
        let s=Instant::now();
        z.query("MATCH (c:Category {id:$cid})-[:SUBCATEGORY*1..4]->(sub:Category) RETURN sub.id", p).unwrap();
        l.push(s.elapsed().as_nanos());
    }
    report("Q5 varlen category [*1..4]", REPS, t.elapsed().as_nanos(), &l);
}

async fn bench_neo4j() {
    use neo4rs::{query, Graph};
    println!("\n=== NEO4J commerce ===");
    let g = match Graph::new("127.0.0.1:7687","neo4j","benchpass123").await {
        Ok(g)=>g, Err(e)=>{println!("neo4j connect failed: {e}");return;} };
    let _=g.run(query("MATCH (n) DETACH DELETE n")).await;
    for idx in ["CREATE INDEX u_id IF NOT EXISTS FOR (u:User) ON (u.id)",
                "CREATE INDEX p_id IF NOT EXISTS FOR (p:Product) ON (p.id)",
                "CREATE INDEX o_id IF NOT EXISTS FOR (o:Order) ON (o.id)",
                "CREATE INDEX c_id IF NOT EXISTS FOR (c:Category) ON (c.id)"] {
        let _=g.run(query(idx)).await;
    }
    let t=Instant::now();
    for i in 0..CATEGORIES { g.run(query("CREATE (c:Category {id:$id})").param("id",format!("c{i}"))).await.unwrap(); }
    for i in 1..CATEGORIES { g.run(query("MATCH (a:Category {id:$p}),(b:Category {id:$c}) CREATE (a)-[:SUBCATEGORY]->(b)").param("p",format!("c{}",i/3)).param("c",format!("c{i}"))).await.unwrap(); }
    for i in 0..PRODUCTS { g.run(query("MATCH (c:Category {id:$cid}) CREATE (pr:Product {id:$id,in_stock:$s})-[:IN_CATEGORY]->(c)").param("id",format!("p{i}")).param("s",(i%2) as i64).param("cid",format!("c{}",i%CATEGORIES))).await.unwrap(); }
    for i in 0..USERS { g.run(query("CREATE (u:User {id:$id})").param("id",format!("u{i}"))).await.unwrap(); }
    for i in 0..ORDERS {
        g.run(query("MATCH (u:User {id:$uid}) CREATE (u)-[:PLACED]->(o:Order {id:$oid,total:$t})")
            .param("uid",format!("u{}",i%USERS)).param("oid",format!("o{i}")).param("t",((i*7)%500+10) as i64)).await.unwrap();
        for k in 0..ITEMS_PER_ORDER {
            g.run(query("MATCH (o:Order {id:$oid}),(pr:Product {id:$pid}) CREATE (o)-[:CONTAINS]->(pr)")
                .param("oid",format!("o{i}")).param("pid",format!("p{}",(i*ITEMS_PER_ORDER+k)%PRODUCTS))).await.unwrap();
        }
    }
    println!("neo4j load: done in {:.1}s", t.elapsed().as_secs_f64());

    macro_rules! run_q { ($name:expr,$n:expr,$build:expr) => {{
        let mut l=Vec::new(); let t=Instant::now();
        for i in 0..$n { let q=$build(i); let s=Instant::now();
            let mut r=g.execute(q).await.unwrap(); while let Ok(Some(_))=r.next().await {} l.push(s.elapsed().as_nanos()); }
        report($name,$n,t.elapsed().as_nanos(),&l);
    }};}

    run_q!("Q1 2-hop co-purchase rec", REPS, |i:usize| query("MATCH (pr:Product {id:$pid})<-[:CONTAINS]-(o:Order)-[:CONTAINS]->(rec:Product) RETURN rec.id, count(*) AS freq ORDER BY freq DESC LIMIT 10").param("pid",format!("p{}",i%PRODUCTS)));
    run_q!("Q2 3-hop customers-like-you", REPS, |i:usize| query("MATCH (u:User {id:$uid})-[:PLACED]->(:Order)-[:CONTAINS]->(:Product)<-[:CONTAINS]-(:Order)<-[:PLACED]-(other:User) RETURN other.id, count(*) AS shared ORDER BY shared DESC LIMIT 10").param("uid",format!("u{}",i%USERS)));
    run_q!("Q3 revenue agg GROUP BY user", 20, |_:usize| query("MATCH (u:User)-[:PLACED]->(o:Order) RETURN u.id, count(o) AS orders, sum(o.total) AS revenue ORDER BY revenue DESC LIMIT 20"));
    run_q!("Q4 filtered traversal", REPS, |i:usize| query("MATCH (u:User {id:$uid})-[:PLACED]->(o:Order) WHERE o.total > $min RETURN o.id, o.total").param("uid",format!("u{}",i%USERS)).param("min",200i64));
    run_q!("Q5 varlen category [*1..4]", REPS, |i:usize| query("MATCH (c:Category {id:$cid})-[:SUBCATEGORY*1..4]->(sub:Category) RETURN sub.id").param("cid",format!("c{}",i%CATEGORIES)));
}

#[tokio::main]
async fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();
    println!("commerce-bench USERS={USERS} PRODUCTS={PRODUCTS} ORDERS={ORDERS} REPS={REPS}");
    match arg.as_str() {
        "zega" => bench_zega(),
        "neo4j" => bench_neo4j().await,
        _ => { bench_zega(); bench_neo4j().await; }
    }
}
