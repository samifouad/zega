//! The storage-backed executor returns exactly what `Zega::run_lang` returns,
//! on the same graph, for every measured query shape, including the writes.
//! The SQLite store runs with a 64 KiB cache so most reads miss and go to
//! storage.

use serde_json::{json, Value as Json};
use zega::Zega;
use zega_storage_spike::exec::{self, Item, Pred, Sel};
use zega_storage_spike::gen::{self, Query};
use zega_storage_spike::mem::MemStore;
use zega_storage_spike::model::{Dir, Op};
use zega_storage_spike::sqlite::{Options, SqliteStore};
use zega_storage_spike::store::{GraphStore, Instrumented};

const N: u64 = 3_000;

fn engines(tag: &str) -> (Zega, SqliteStore, MemStore, std::path::PathBuf) {
    let zega = Zega::in_memory().build().unwrap();
    zega.restore_bytes(&gen::snapshot_bytes(N)).unwrap();
    let dir = std::env::temp_dir().join(format!("zega-spike-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("graph.sqlite");
    let mut sql = SqliteStore::open(&path, Options { cache_bytes: 64 * 1024, sync_full: false }).unwrap();
    let mut mem = MemStore::new();
    for (ty, field) in gen::RANGES {
        sql.declare_range(ty, field).unwrap();
        mem.declare_range(ty, field);
    }
    for lo in (1..=N).step_by(1000) {
        let ops = gen::ops(lo, (lo + 999).min(N), N);
        sql.apply(ops.clone()).unwrap();
        mem.apply(ops).unwrap();
    }
    (zega, sql, mem, dir)
}

fn check(zega: &Zega, sql: &SqliteStore, mem: &MemStore, q: &Query) {
    let want = zega.run_lang(gen::SCHEMA, &q.zql).unwrap();
    let got_sql = exec::read(sql, &q.plan).unwrap();
    let got_mem = exec::read(mem, &q.plan).unwrap();
    assert_eq!(got_sql, want, "sqlite: {}", q.zql);
    assert_eq!(got_mem, want, "mem: {}", q.zql);
}

#[test]
fn reads_and_writes_match_the_in_memory_engine() {
    let (zega, mut sql, mut mem, dir) = engines("parity");

    let mut nonempty = 0;
    for k in 0..200u64 {
        let i = 1 + gen::mix(k) % N;
        check(&zega, &sql, &mem, &gen::point(i));
        check(&zega, &sql, &mem, &gen::one_hop(i));
        check(&zega, &sql, &mem, &gen::two_hop(i));
        let rank = gen::mix(k + 7) % (N / 10);
        let q = gen::filter(rank);
        check(&zega, &sql, &mem, &q);
        nonempty += usize::from(exec::read(&sql, &q.plan).unwrap().as_array().is_some_and(|a| !a.is_empty()));
    }
    assert!(nonempty > 150, "the filter must mostly return rows to prove anything");
    for city in 0..gen::CITIES {
        check(&zega, &sql, &mem, &gen::scan_limit(city));
    }
    check(&zega, &sql, &mem, &gen::scan_all());
    // A missing key is null, not an error.
    check(&zega, &sql, &mem, &gen::point(N + 50));
    // The two-hop answer really is 3 + 9 nodes deep.
    let two = exec::read(&sql, &gen::two_hop(5).plan).unwrap();
    assert_eq!(two["follows"].as_array().unwrap().len(), 3);
    assert!(two["follows"][0]["follows"].as_array().unwrap().len() == 3);

    for k in 0..25u64 {
        let links = [1 + gen::mix(k) % N, 1 + gen::mix(k + 100) % N, 1 + gen::mix(k + 200) % N];
        for q in [gen::create(k), gen::link(&format!("new{k}"), links)] {
            let want = zega.run_lang(gen::SCHEMA, &q.zql).unwrap();
            assert_eq!(exec::mutate(&mut sql, &q.plan, &gen::uniques()).unwrap(), want, "{}", q.zql);
            assert_eq!(exec::mutate(&mut mem, &q.plan, &gen::uniques()).unwrap(), want, "{}", q.zql);
        }
        // Read the new node back with its links.
        let back = format!("{{ Person(handle: \"new{k}\") {{ handle rank follows -> Person {{ handle }} }} }}");
        let plan = Sel {
            ty: "Person".into(),
            cond: vec![Pred::Eq("handle".into(), json!(format!("new{k}")))],
            limit: None,
            items: vec![
                Item::Prop("handle".into()),
                Item::Prop("rank".into()),
                Item::Walk {
                    field: "follows".into(),
                    rel: "follows".into(),
                    dir: Dir::Out,
                    many: true,
                    target: Sel { ty: "Person".into(), cond: vec![], limit: None, items: vec![Item::Prop("handle".into())] },
                },
            ],
        };
        let want = zega.run_lang(gen::SCHEMA, &back).unwrap();
        assert_eq!(want["follows"].as_array().unwrap().len(), 3);
        assert_eq!(exec::read(&sql, &plan).unwrap(), want);
        assert_eq!(exec::read(&mem, &plan).unwrap(), want);
    }
    // The new nodes are in the rank index too (rank 5 now has 25 more rows).
    check(&zega, &sql, &mem, &gen::filter(4));

    // A duplicate unique key is refused by both, and writes nothing.
    let dup = gen::create(0);
    assert!(zega.run_lang(gen::SCHEMA, &dup.zql).is_err());
    let before = sql.next_ids();
    assert!(exec::mutate(&mut sql, &dup.plan, &gen::uniques()).is_err());
    assert_eq!(sql.next_ids(), before);

    // The small cache really did send reads to storage.
    let stats = sql.stats();
    assert!(stats.cache_misses > 1000 && stats.statements > 1000, "{stats:?}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_statement_that_fails_part_way_writes_nothing() {
    let (_zega, mut sql, _mem, dir) = engines("rollback");
    // Storage refuses the statement's third write (as a full disk or a
    // refused DO transaction would), after a node and a relationship.
    sql.connection()
        .execute_batch(
            "CREATE TRIGGER refuse BEFORE INSERT ON rel WHEN NEW.id = 999999
             BEGIN SELECT RAISE(ABORT, 'injected'); END;",
        )
        .unwrap();
    let before = sql.next_ids();
    let adjacency_before = sql.adjacency(1, "follows", Dir::Out).unwrap();
    let mut props = std::collections::HashMap::new();
    props.insert("handle".to_string(), zega::Value::String("ghost".into()));
    let new_id = before.0;
    let result = sql.apply(vec![
        Op::InsertNode { id: new_id, labels: vec!["Person".into()], props },
        Op::InsertRel { id: before.1, kind: "follows".into(), from: 1, to: new_id, props: Default::default() },
        Op::InsertRel { id: 999999, kind: "follows".into(), from: 1, to: 2, props: Default::default() },
    ]);
    assert!(result.unwrap_err().0.contains("injected"));
    assert_eq!(sql.next_ids(), before);
    assert!(sql.node(new_id).unwrap().is_none(), "the node was rolled back");
    assert_eq!(sql.adjacency(1, "follows", Dir::Out).unwrap(), adjacency_before, "and the relationship");
    let lookup = Sel { ty: "Person".into(), cond: vec![Pred::Eq("handle".into(), json!("ghost"))], limit: None, items: vec![] };
    assert_eq!(exec::read(&sql, &lookup).unwrap(), Json::Null, "and its index entry");
    // Reopening reads the same state from disk.
    drop(sql);
    let sql = SqliteStore::open(&dir.join("graph.sqlite"), Options { cache_bytes: 64 * 1024, sync_full: false }).unwrap();
    assert_eq!(sql.next_ids(), before);
    assert_eq!(sql.adjacency(1, "follows", Dir::Out).unwrap(), adjacency_before);
    let _ = std::fs::remove_dir_all(dir);
}
