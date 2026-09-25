//! zega#52 measurements: what a long write history costs a disk database.
//!
//! ```text
//! cargo run --release --example wal_growth -- write <dir> [writes] [nodes] [snapshot-every-bytes]
//! cargo run --release --example wal_growth -- open <dir>
//! cargo run --release --example wal_growth -- checkpoint <dir>
//! cargo run --release --example wal_growth -- json <dir>
//! ```
//!
//! `write` creates `nodes` Player nodes (default 100,000), then updates them
//! round-robin until `writes` node writes are done (default 1,000,000), 1,000
//! writes per statement, and reports the WAL, the files, and statement
//! latency (writes wait while a checkpoint writes the graph out).
//! `snapshot-every-bytes` 0 never checkpoints, which is what `zega start`
//! did before zega#52.
//!
//! `open` times a restart (open, then the first answered query); `checkpoint`
//! and `json` open, then take a checkpoint or build `GET /graph`'s JSON. Run
//! each under `/usr/bin/time -l` (macOS) or `-v` (Linux) for its peak RSS.

use std::time::{Duration, Instant};

use zega::Zega;

const SCHEMA: &str = "schema { type Player { name: String salary: Int team: String } } unique { Player { name } }";
const BATCH: u64 = 1_000;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize, default: u64| args.get(i).map_or(default, |v| v.parse().unwrap());
    match args.first().map(String::as_str) {
        Some("write") => write(&args[1], arg(2, 1_000_000), arg(3, 100_000), arg(4, zega::DEFAULT_SNAPSHOT_EVERY_BYTES)),
        Some("open") => {
            let (zega, _) = open(&args[1]);
            println!("rss after open: {} MB", rss_mb());
            drop(zega);
        }
        Some("checkpoint") => {
            let (zega, _) = open(&args[1]);
            println!("rss before checkpoint: {} MB", rss_mb());
            let started = Instant::now();
            let checkpoint = zega.checkpoint().unwrap().unwrap();
            println!(
                "checkpoint: {:.0} ms, writes paused {:.0} ms, {} MB graph, WAL {} -> {} bytes",
                ms(started.elapsed()),
                ms(checkpoint.paused),
                checkpoint.graph_bytes >> 20,
                checkpoint.wal_bytes_before,
                checkpoint.wal_bytes_after
            );
            println!("rss after checkpoint: {} MB", rss_mb());
        }
        Some("json") => {
            let (zega, _) = open(&args[1]);
            println!("rss before GET /graph JSON: {} MB", rss_mb());
            let started = Instant::now();
            let body = serde_json::to_vec(&zega.graph_json().unwrap()).unwrap();
            println!("GET /graph JSON: {:.0} ms, {} MB body", ms(started.elapsed()), body.len() >> 20);
        }
        _ => eprintln!("usage: wal_growth write|open|checkpoint|json <dir> ..."),
    }
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// Open without a checkpoint thread, and answer one query: a restart.
fn open(dir: &str) -> (Zega, Duration) {
    let started = Instant::now();
    let zega = Zega::open(dir).snapshot_every(0).build().unwrap();
    let opened = started.elapsed();
    zega.run_lang(SCHEMA, r#"query { Player(name: "p0") { salary } }"#).unwrap();
    let answered = started.elapsed();
    println!("restart: open {:.0} ms, first answer {:.0} ms", ms(opened), ms(answered));
    (zega, answered)
}

fn write(dir: &str, writes: u64, nodes: u64, snapshot_every: u64) {
    let zega = Zega::open(dir).snapshot_every(snapshot_every).build().unwrap();
    let mut latencies = Vec::new();
    let started = Instant::now();
    let mut done = 0;
    while done < writes {
        let end = (done + BATCH).min(writes);
        let (statement, rows) = if done < nodes {
            let rows: Vec<String> = (done..end.min(nodes))
                .map(|i| format!(r#"{{"n":"p{i}","s":{i},"t":"t{}"}}"#, i % 97))
                .collect();
            let statement = r#"mutation json ["rows"] { Player(name: $n && salary: $s && team: $t) }"#;
            done = end.min(nodes);
            (statement, rows)
        } else {
            let rows: Vec<String> = (done..end)
                .map(|i| format!(r#"{{"n":"p{}","s":{i}}}"#, i % nodes))
                .collect();
            done = end;
            (r#"mutation json ["rows"] { Player(name: $n) set salary: $s }"#, rows)
        };
        let sources = std::collections::HashMap::from([("rows".to_string(), format!("[{}]", rows.join(",")))]);
        let one = Instant::now();
        zega.run_lang_with_sources(SCHEMA, statement, &sources).unwrap();
        latencies.push(one.elapsed());
    }
    let elapsed = started.elapsed();
    latencies.sort();
    let pct = |p: f64| ms(latencies[((latencies.len() - 1) as f64 * p) as usize]);
    println!(
        "{writes} node writes ({nodes} nodes, {BATCH} per statement) in {:.1} s; statement latency p50 {:.1} ms, p99 {:.1} ms, max {:.1} ms",
        elapsed.as_secs_f64(),
        pct(0.5),
        pct(0.99),
        pct(1.0)
    );
    drop(zega);
    let size = |name: &str| std::fs::metadata(std::path::Path::new(dir).join(name)).map_or(0, |m| m.len());
    let graphs: u64 = std::fs::read_dir(std::path::Path::new(dir).join("graphs"))
        .map(|dir| dir.map(|e| e.unwrap().metadata().unwrap().len()).sum())
        .unwrap_or(0);
    println!("wal.bin {} bytes; graphs/ {} bytes", size("wal.bin"), graphs);
}

/// This process's resident memory, in MB, as `ps` reports it.
fn rss_mb() -> u64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse::<u64>().unwrap_or(0) / 1024
}
