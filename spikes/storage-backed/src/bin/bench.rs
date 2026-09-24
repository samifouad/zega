//! SPIKE benchmark. One process per engine and size, so memory is its own.
//!
//! ```text
//! bench load   <n> <db>                   build the SQLite graph
//! bench sqlite <n> <db> <cache-mb> [full] queries over SQLite, bounded cache
//! bench memory <n> [wal-dir]              the same queries on Zega::run_lang
//! bench seam   <n>                        the same plans over MemStore
//! ```
//! Each prints one JSON object on stdout.

use std::time::Instant;

use serde_json::{json, Value as Json};
use zega::Zega;
use zega_storage_spike::exec;
use zega_storage_spike::gen::{self, Query};
use zega_storage_spike::mem::MemStore;
use zega_storage_spike::sqlite::{Options, SqliteStore};
use zega_storage_spike::store::{GraphStore, Instrumented, Stats};

fn rss_bytes() -> u64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps runs");
    String::from_utf8_lossy(&out.stdout).trim().parse::<u64>().unwrap_or(0) * 1024
}

fn peak_rss_bytes() -> u64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    // Bytes on macOS, KiB on Linux.
    if cfg!(target_os = "macos") {
        usage.ru_maxrss as u64
    } else {
        usage.ru_maxrss as u64 * 1024
    }
}

fn mb(bytes: u64) -> f64 {
    (bytes as f64 / 1_048_576.0 * 10.0).round() / 10.0
}

fn summary(mut ns: Vec<u64>) -> Json {
    ns.sort_unstable();
    let at = |p: f64| ns[((ns.len() as f64 * p) as usize).min(ns.len() - 1)] as f64 / 1000.0;
    let mean = ns.iter().sum::<u64>() as f64 / ns.len() as f64 / 1000.0;
    json!({ "n": ns.len(), "p50_us": at(0.50), "p99_us": at(0.99), "max_us": at(1.0), "mean_us": (mean * 10.0).round() / 10.0 })
}

/// Parameters for rep `k` of each shape. `hot` sends 90% of reads to the
/// first 1% of nodes (a skewed workload); otherwise uniform.
fn node_for(k: u64, n: u64, hot: bool) -> u64 {
    let r = gen::mix(k ^ 0x5EED);
    if hot && !r.is_multiple_of(10) {
        1 + (r >> 8) % (n / 100).max(1)
    } else {
        1 + (r >> 8) % n
    }
}

struct Shape {
    name: &'static str,
    reps: u64,
    make: Box<dyn Fn(u64) -> Query>,
}

fn shapes(n: u64) -> Vec<Shape> {
    let full_reps = if n >= 1_000_000 { 5 } else if n >= 100_000 { 20 } else { 100 };
    vec![
        Shape { name: "point", reps: 20_000, make: Box::new(move |k| gen::point(node_for(k, n, false))) },
        Shape { name: "point_hot", reps: 20_000, make: Box::new(move |k| gen::point(node_for(k, n, true))) },
        Shape { name: "filter", reps: 5_000, make: Box::new(move |k| gen::filter(gen::mix(k + 3) % (n / 10).max(1))) },
        Shape { name: "two_hop", reps: 5_000, make: Box::new(move |k| gen::two_hop(node_for(k, n, false))) },
        Shape { name: "two_hop_hot", reps: 5_000, make: Box::new(move |k| gen::two_hop(node_for(k, n, true))) },
        Shape { name: "scan_limit", reps: 2_000, make: Box::new(move |k| gen::scan_limit(gen::mix(k) % gen::CITIES)) },
        Shape { name: "scan_all", reps: full_reps, make: Box::new(|_| gen::scan_all()) },
    ]
}

const WRITES: u64 = 2_000;

fn links(k: u64, n: u64) -> [u64; 3] {
    [1 + gen::mix(k) % n, 1 + gen::mix(k + 1) % n, 1 + gen::mix(k + 2) % n]
}

fn run_sqlite(n: u64, db: &str, cache_mb: usize, sync_full: bool) -> Json {
    let open_start = Instant::now();
    let mut store = SqliteStore::open(db.as_ref(), Options { cache_bytes: cache_mb << 20, sync_full }).unwrap();
    let open_us = open_start.elapsed().as_micros();
    // First query after open: nothing cached, nothing replayed.
    let first = Instant::now();
    let q = gen::point(n / 2);
    assert!(exec::read(&store, &q.plan).unwrap().is_object());
    let first_query_us = first.elapsed().as_micros();
    let mut results = serde_json::Map::new();
    for shape in shapes(n) {
        let warm = (shape.reps / 10).max(1);
        for k in 0..warm {
            exec::read(&store, &(shape.make)(k + 1_000_000).plan).unwrap();
        }
        let before = store.stats();
        let mut ns = Vec::with_capacity(shape.reps as usize);
        for k in 0..shape.reps {
            let q = (shape.make)(k);
            let t = Instant::now();
            let out = exec::read(&store, &q.plan).unwrap();
            ns.push(t.elapsed().as_nanos() as u64);
            std::hint::black_box(out);
        }
        results.insert(shape.name.into(), with_stats(summary(ns), store.stats().since(before), shape.reps));
    }
    let (mut create_ns, mut link_ns) = (Vec::new(), Vec::new());
    let (mut create_stats, mut link_stats) = (Stats::default(), Stats::default());
    for k in 0..WRITES {
        for (q, ns, stats) in [
            (gen::create(k), &mut create_ns, &mut create_stats),
            (gen::link(&format!("new{k}"), links(k, n)), &mut link_ns, &mut link_stats),
        ] {
            let s0 = store.stats();
            let t = Instant::now();
            exec::mutate(&mut store, &q.plan, &gen::uniques()).unwrap();
            ns.push(t.elapsed().as_nanos() as u64);
            *stats = add(*stats, store.stats().since(s0));
        }
    }
    results.insert("create".into(), with_stats(summary(create_ns), create_stats, WRITES));
    results.insert("link".into(), with_stats(summary(link_ns), link_stats, WRITES));
    json!({
        "engine": "sqlite", "n": n, "cache_mb": cache_mb, "sync": if sync_full { "FULL" } else { "NORMAL" },
        "open_us": open_us, "first_query_us": first_query_us,
        "rss_mb": mb(rss_bytes()), "peak_rss_mb": mb(peak_rss_bytes()),
        "object_cache_mb": mb(store.cache_used() as u64),
        "queries": results,
    })
}

fn add(a: Stats, b: Stats) -> Stats {
    Stats {
        statements: a.statements + b.statements,
        rows_read: a.rows_read + b.rows_read,
        rows_written: a.rows_written + b.rows_written,
        cache_hits: a.cache_hits + b.cache_hits,
        cache_misses: a.cache_misses + b.cache_misses,
        billed_rows_read: a.billed_rows_read + b.billed_rows_read,
        billed_rows_written: a.billed_rows_written + b.billed_rows_written,
    }
}

fn with_stats(mut summary: Json, stats: Stats, reps: u64) -> Json {
    let per = |v: u64| (v as f64 / reps as f64 * 10.0).round() / 10.0;
    let lookups = stats.cache_hits + stats.cache_misses;
    summary["statements_per_query"] = json!(per(stats.statements));
    summary["rows_read_per_query"] = json!(per(stats.rows_read));
    summary["rows_written_per_query"] = json!(per(stats.rows_written));
    summary["cache_hit_rate"] = json!(if lookups == 0 { Json::Null } else { json!((stats.cache_hits as f64 / lookups as f64 * 1000.0).round() / 1000.0) });
    summary
}

fn run_memory(n: u64, wal_dir: Option<&str>) -> Json {
    let rss_empty = rss_bytes();
    let t = Instant::now();
    let zega = match wal_dir {
        None => Zega::in_memory().build().unwrap(),
        Some(dir) => {
            let _ = std::fs::remove_dir_all(dir);
            Zega::open(dir).wal_flush_every_write().build().unwrap()
        }
    };
    {
        let bytes = gen::snapshot_bytes(n);
        zega.restore_bytes(&bytes).unwrap();
    }
    // The first query builds the declared indexes.
    zega.run_lang(gen::SCHEMA, &gen::point(1).zql).unwrap();
    let load_ms = t.elapsed().as_millis();
    let rss_loaded = rss_bytes();
    let mut results = serde_json::Map::new();
    let mut parse = serde_json::Map::new();
    for shape in shapes(n) {
        let warm = (shape.reps / 10).max(1);
        for k in 0..warm {
            zega.run_lang(gen::SCHEMA, &(shape.make)(k + 1_000_000).zql).unwrap();
        }
        let mut ns = Vec::with_capacity(shape.reps as usize);
        let mut parse_ns = Vec::with_capacity(shape.reps as usize);
        for k in 0..shape.reps {
            let q = (shape.make)(k);
            let t = Instant::now();
            let out = zega.run_lang(gen::SCHEMA, &q.zql).unwrap();
            ns.push(t.elapsed().as_nanos() as u64);
            std::hint::black_box(out);
            // What run_lang spends before touching the graph: schema, unique
            // and index blocks, and the statement, parsed per call.
            let t = Instant::now();
            zega.schema(gen::SCHEMA).unwrap();
            zega::check_zql(zega::ZqlEntryPoint::File, gen::SCHEMA).unwrap();
            zega::check_zql(zega::ZqlEntryPoint::Statement, &q.zql).unwrap();
            parse_ns.push(t.elapsed().as_nanos() as u64);
        }
        results.insert(shape.name.into(), summary(ns));
        parse.insert(shape.name.into(), summary(parse_ns));
    }
    let (mut create_ns, mut link_ns) = (Vec::new(), Vec::new());
    for k in 0..WRITES {
        let q = gen::create(k);
        let t = Instant::now();
        zega.run_lang(gen::SCHEMA, &q.zql).unwrap();
        create_ns.push(t.elapsed().as_nanos() as u64);
        let q = gen::link(&format!("new{k}"), links(k, n));
        let t = Instant::now();
        zega.run_lang(gen::SCHEMA, &q.zql).unwrap();
        link_ns.push(t.elapsed().as_nanos() as u64);
    }
    results.insert("create".into(), summary(create_ns));
    results.insert("link".into(), summary(link_ns));
    json!({
        "engine": if wal_dir.is_some() { "memory+wal(flush every write)" } else { "memory" }, "n": n,
        "load_ms": load_ms,
        "rss_mb": mb(rss_loaded), "graph_mb": mb(rss_loaded.saturating_sub(rss_empty)), "peak_rss_mb": mb(peak_rss_bytes()),
        "queries": results,
        "parse_overhead": parse,
    })
}

fn run_seam(n: u64) -> Json {
    let mut store = MemStore::new();
    for (ty, field) in gen::RANGES {
        store.declare_range(ty, field);
    }
    for lo in (1..=n).step_by(10_000) {
        store.apply(gen::ops(lo, (lo + 9_999).min(n), n)).unwrap();
    }
    let mut results = serde_json::Map::new();
    for shape in shapes(n) {
        let mut ns = Vec::new();
        for k in 0..shape.reps {
            let q = (shape.make)(k);
            let t = Instant::now();
            std::hint::black_box(exec::read(&store, &q.plan).unwrap());
            ns.push(t.elapsed().as_nanos() as u64);
        }
        results.insert(shape.name.into(), summary(ns));
    }
    json!({ "engine": "seam over MemStore", "n": n, "rss_mb": mb(rss_bytes()), "queries": results })
}

fn load(n: u64, db: &str) -> Json {
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{db}{suffix}"));
    }
    let t = Instant::now();
    // A big cache while loading only; the query runs reopen with a small one.
    let mut store = SqliteStore::open(db.as_ref(), Options { cache_bytes: 1 << 30, sync_full: false }).unwrap();
    for (ty, field) in gen::RANGES {
        store.declare_range(ty, field).unwrap();
    }
    for lo in (1..=n).step_by(5_000) {
        store.apply(gen::ops(lo, (lo + 4_999).min(n), n)).unwrap();
    }
    store.connection().execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
    let load_ms = t.elapsed().as_millis();
    let tables: Vec<(String, i64)> = {
        let mut stmt = store
            .connection()
            .prepare("SELECT name, SUM(pgsize) FROM dbstat GROUP BY name ORDER BY 2 DESC")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
    };
    let size = std::fs::metadata(db).unwrap().len();
    json!({
        "engine": "sqlite load", "n": n, "load_ms": load_ms, "db_mb": mb(size),
        "bytes_per_node": size / n,
        "tables_mb": tables.into_iter().map(|(name, bytes)| (name, json!(mb(bytes as u64)))).collect::<serde_json::Map<_, _>>(),
    })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: u64 = args.get(2).and_then(|s| s.parse().ok()).expect("n");
    let out = match args.get(1).map(String::as_str) {
        Some("load") => load(n, &args[3]),
        Some("sqlite") => run_sqlite(n, &args[3], args[4].parse().unwrap(), args.get(5).is_some_and(|s| s == "full")),
        Some("memory") => run_memory(n, args.get(3).map(String::as_str)),
        Some("seam") => run_seam(n),
        _ => panic!("usage: bench load|sqlite|memory|seam <n> …"),
    };
    println!("{out}");
}
