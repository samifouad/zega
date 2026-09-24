//! Where a scan's time goes, per row: SQLite step, blob decode, node build.
use std::time::Instant;
use rusqlite::Connection;
use zega::Value;

fn main() {
    let db = std::env::args().nth(1).unwrap();
    let conn = Connection::open(db).unwrap();
    conn.execute_batch("PRAGMA cache_size=-8192;").unwrap();
    for variant in ["join-step", "join-decode", "node-step", "node-decode"] {
        let sql = if variant.starts_with("join") {
            "SELECT n.id, n.labels, n.props FROM lbl l JOIN node n ON n.id = l.id WHERE l.label = 1 ORDER BY l.id"
        } else {
            "SELECT id, labels, props FROM node ORDER BY id"
        };
        for _ in 0..3 {
            let t = Instant::now();
            let mut stmt = conn.prepare(sql).unwrap();
            let mut rows = stmt.query([]).unwrap();
            let mut n = 0u64;
            let mut acc = 0usize;
            while let Some(row) = rows.next().unwrap() {
                n += 1;
                let props = row.get_ref(2).unwrap().as_blob().unwrap();
                if variant.ends_with("decode") {
                    let v: Vec<(u32, Value)> = bincode::deserialize(props).unwrap();
                    acc += v.len();
                } else {
                    acc += props.len();
                }
            }
            println!("{variant:12} {:.3} us/row ({n} rows, {acc})", t.elapsed().as_nanos() as f64 / n as f64 / 1000.0);
        }
    }
}
