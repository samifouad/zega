use serde_json::{json, Value as Json};
use std::collections::HashMap;
use zega::{
    vector::{pca, Hnsw, Metric, Vector},
    ViewKind, Zega,
};
const SCHEMA: &str = "type Ticket { n: Int embedding: Vector<3> } display { vector2d { Ticket }: Default vector3d { Ticket } }";
fn run(db: &Zega, q: &str) -> Json {
    db.run_lang(SCHEMA, q).unwrap()
}
fn ids(value: &Json) -> Vec<u64> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["n"].as_u64().unwrap())
        .collect()
}
#[test]
fn vector_type_diagnostics_have_spans() {
    let db = Zega::in_memory().build().unwrap();
    for (ty, message) in [
        ("Vector<0>", "dimension"),
        ("Vector<4097>", "dimension"),
        ("Vector<3, angular>", "metric"),
    ] {
        let s = format!("type Ticket {{ embedding: {ty} }}");
        let report = zega::diagnose(&s, "");
        assert!(report.text.contains(message), "{}", report.text);
        assert!(report.diagnostics[0].column > 1);
        assert!(db.schema(&s).is_err());
    }
    for value in [
        "vector[1,2]",
        "vector[1,2,3,4]",
        "vector[1,\"bad\",3]",
        "vector[1,true,3]",
    ] {
        let q = format!("mutation {{ Ticket(n: 0 && embedding: {value}) {{ n }} }}");
        let report = zega::diagnose(SCHEMA, &q);
        assert!(!report.diagnostics.is_empty(), "{q}");
        assert!(report.diagnostics[0].column > 1);
        assert!(db.run_lang(SCHEMA, &q).is_err());
    }
    for kind in ["vector2d", "vector3d"] {
        let s = format!("type Ticket {{ title: String }} display {{ {kind} {{ Ticket }} }}");
        let report = zega::diagnose(&s, "");
        assert!(report.text.contains("needs a Vector field"));
        assert_eq!(
            report.diagnostics[0].column,
            s.rfind("Ticket").unwrap() as u32 + 1
        );
    }
    assert!(db
        .run_lang(SCHEMA, "mutation { Ticket(n: 0) { n } }")
        .is_err());
    assert!(db
        .run_lang(
            SCHEMA,
            "{ Ticket near(embedding, vector[1,2], 10) { score } }"
        )
        .is_err());
    assert!(db.run_lang(SCHEMA, "{ Ticket { score } }").is_err());
    assert!(db
        .schema("type Ticket { embedding: Vector<1> from (x) }")
        .is_ok());
    assert!(db
        .schema("type Ticket { embedding: Vector<3> from (x,y) }")
        .is_err());
}
#[test]
fn vector_json_csv_queries_filters_relationships_and_metrics() {
    let db = Zega::in_memory().build().unwrap();
    db.run_lang_with_sources(SCHEMA,"mutation json [\"rows.json\"] { Ticket(n: $n) { id } }", &HashMap::from([("rows.json".into(),json!([{"n":0,"embedding":[1,0,0]},{"n":1,"embedding":[0,1,0]},{"n":2,"embedding":[0.8,0.6,0]},{"n":3,"embedding":[-1,0,0]}]).to_string())])).unwrap();
    let q = "{ Ticket(n > 0) near(embedding, vector[1,0,0], 2) { n score } }";
    let result = run(&db, q);
    assert_eq!(ids(&result), vec![2, 1]);
    assert!((result[0]["score"].as_f64().unwrap() - 0.8).abs() < 1e-6);
    assert_eq!(result, run(&db, &q.replace(", 2)", ", 2, exact)")));
    assert_eq!(
        ids(&run(
            &db,
            "{ Ticket(similarity(embedding, vector[1,0,0]) >= 0.7) { n } }"
        )),
        vec![0, 2]
    );
    let schema = format!("{SCHEMA} type Queue {{ name: String tickets -> Ticket[] }}");
    db.run_lang(&schema, "mutation { Queue(name: \"inbox\") { name } }")
        .unwrap();
    db.run_lang(&schema,"mutation { Queue(name: \"inbox\") { tickets -> link Ticket(n: 1) { n } tickets -> link Ticket(n: 2) { n } } }").unwrap();
    let r=db.run_lang(&schema,"{ Queue(name: \"inbox\") { tickets -> Ticket(n > 0) near(embedding, vector[1,0,0], 1) { n score } } }").unwrap();
    assert_eq!(ids(&r["tickets"]), vec![2]);
    let fresh = Zega::in_memory().build().unwrap();
    let sources = HashMap::from([("rows.csv".into(), "n,x,y,z\n5,1,0,0\n6,0,1,0\n".into())]);
    assert!(fresh
        .run_lang_with_sources(
            SCHEMA,
            "mutation csv [\"rows.csv\"] { Ticket(n: $n) { n } }",
            &sources
        )
        .is_err());
    fresh
        .run_lang_with_sources(
            "type Ticket { n: Int embedding: Vector<3> from (x,y,z) }",
            "mutation csv [\"rows.csv\"] { Ticket(n: $n) { n } }",
            &sources,
        )
        .unwrap();
    assert_eq!(
        ids(&run(
            &fresh,
            "{ Ticket near(embedding, vector[1,0,0], 1) { n } }"
        )),
        vec![5]
    );
    for (metric, expected) in [("dot", vec![1, 0]), ("l2", vec![0, 1])] {
        let db = Zega::in_memory().build().unwrap();
        let s = format!("type Ticket {{ n: Int embedding: Vector<3,{metric}> }}");
        db.run_lang(
            &s,
            "mutation { Ticket(n: 0 && embedding: vector[1,0,0]) { n } }",
        )
        .unwrap();
        db.run_lang(
            &s,
            "mutation { Ticket(n: 1 && embedding: vector[2,0,0]) { n } }",
        )
        .unwrap();
        assert_eq!(
            ids(&db
                .run_lang(
                    &s,
                    "{ Ticket near(embedding, vector[1,0,0], 2) { n score } }"
                )
                .unwrap()),
            expected
        );
    }
}
#[test]
fn vector_wal_snapshot_updates_deletes() {
    let dir = tempfile::tempdir().unwrap();
    let nearest = "{ Ticket near(embedding, vector[1,0,0], 1) { n score } }";
    {
        let db = Zega::open(dir.path().to_str().unwrap())
            .wal_flush_every_write()
            .build()
            .unwrap();
        run(
            &db,
            "mutation { Ticket(n: 0 && embedding: vector[1,0,0]) { n } }",
        );
        run(
            &db,
            "mutation { Ticket(n: 1 && embedding: vector[0,1,0]) { n } }",
        );
    }
    {
        let db = Zega::open(dir.path().to_str().unwrap())
            .wal_flush_every_write()
            .build()
            .unwrap();
        assert_eq!(ids(&run(&db, nearest)), vec![0]);
        run(
            &db,
            "mutation { Ticket(n: 0) set embedding: vector[-1,0,0] { n } }",
        );
        assert_eq!(ids(&run(&db, nearest)), vec![1]);
        db.snapshot().unwrap();
    }
    let db = Zega::open(dir.path().to_str().unwrap())
        .wal_flush_every_write()
        .build()
        .unwrap();
    assert_eq!(ids(&run(&db, nearest)), vec![1]);
    let restored = Zega::in_memory().build().unwrap();
    restored
        .restore_bytes(&db.snapshot_bytes().unwrap())
        .unwrap();
    assert_eq!(run(&restored, nearest), run(&db, nearest));
    db.delete_node(2).unwrap();
    assert_eq!(ids(&run(&db, nearest)), vec![0]);
    drop(db);
    let db = Zega::open(dir.path().to_str().unwrap()).build().unwrap();
    assert_eq!(ids(&run(&db, nearest)), vec![0]);
    restored
        .restore_bytes(&db.snapshot_bytes().unwrap())
        .unwrap();
    assert_eq!(run(&restored, nearest), run(&db, nearest));
}
// Independent scalar cosine, evaluated on the actual stored float32 inputs.
fn cosine(a: &[f32], b: &[f32]) -> f64 {
    let dot: f64 = a
        .iter()
        .zip(b)
        .map(|(&a, &b)| f64::from(a) * f64::from(b))
        .sum();
    let aa: f64 = a.iter().map(|&a| f64::from(a).powi(2)).sum();
    let bb: f64 = b.iter().map(|&b| f64::from(b).powi(2)).sum();
    (dot / (aa * bb).sqrt()).clamp(-1.0, 1.0)
}
#[test]
fn vector_hnsw_recall_10000_by_128_and_exact() {
    let mut seed = 0x123456789abcdefu64;
    let mut random = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        ((seed >> 40) as f32 / 16777216.0) * 2.0 - 1.0
    };
    let data: Vec<Vec<f32>> = (0..10000)
        .map(|_| (0..128).map(|_| random()).collect())
        .collect();
    let queries: Vec<Vec<f32>> = (0..40)
        .map(|_| (0..128).map(|_| random()).collect())
        .collect();
    let mut index = Hnsw::new(42);
    for (i, v) in data.iter().enumerate() {
        index.insert(i as u64, Vector::new(v, Metric::Cosine).unwrap());
    }
    let mut hits = 0;
    for q in &queries {
        let mut expected: Vec<_> = data
            .iter()
            .enumerate()
            .map(|(i, v)| (i as u64, cosine(v, q)))
            .collect();
        expected.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        expected.truncate(10);
        let query = Vector::new(q, Metric::Cosine).unwrap();
        assert_eq!(index.nearest(&query, 10, true, &|_| true), expected);
        let approx = index.nearest(&query, 10, false, &|_| true);
        hits += approx
            .iter()
            .filter(|(id, _)| expected.iter().any(|(n, _)| n == id))
            .count();
    }
    let recall = hits as f64 / (queries.len() * 10) as f64;
    eprintln!(
        "seeded 10000 x 128, {} held-out queries: recall@10 = {recall:.4}",
        queries.len()
    );
    assert!(recall >= 0.95, "recall@10={recall}");
    let mut other = Hnsw::new(42);
    for (i, v) in data.iter().take(150).enumerate() {
        other.insert(i as u64, Vector::new(v, Metric::Cosine).unwrap());
    }
    let q = Vector::new(&queries[0], Metric::Cosine).unwrap();
    for i in 0..100 {
        other.remove(i);
    }
    assert!(other
        .nearest(&q, 150, false, &|_| true)
        .iter()
        .all(|(id, _)| *id >= 100));
}
#[test]
fn vector_projection_and_explanations_use_selected_full_vectors() {
    let db = Zega::in_memory().build().unwrap();
    let schema = format!("{SCHEMA} type Queue {{ tickets -> Ticket[] }}");
    db.run_lang(&schema,"mutation { Queue { tickets -> Ticket(n: 0 && embedding: vector[1,0,0]) { n } tickets -> Ticket(n: 1 && embedding: vector[0.9,0.1,0]) { n } tickets -> Ticket(n: 2 && embedding: vector[-1,0,0]) { n } } }").unwrap();
    let result = run(&db, "{ Ticket { id n } }");
    let view = db
        .vector_view(&schema, &result, ViewKind::Vector2d, Some(2), 10, 0.8)
        .unwrap();
    assert_eq!(view["points"].as_array().unwrap().len(), 3);
    assert_eq!(view["nearest"][0]["id"], json!(3));
    assert_eq!(view["flags"].as_array().unwrap().len(), 1);
    assert_eq!(view["flags"][0]["kind"], "near-but-unlinked");
    assert_eq!(
        view,
        db.vector_view(&schema, &result, ViewKind::Vector3d, Some(2), 10, 0.8)
            .unwrap()
    );
    let limited = run(&db, "{ Ticket(n < 2) { id n } }");
    assert_eq!(
        db.vector_view(&schema, &limited, ViewKind::Vector2d, None, 10, 0.8)
            .unwrap()["points"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let line = vec![
        Vector::new(&[-2.0, 0.0, 0.0], Metric::Cosine).unwrap(),
        Vector::new(&[2.0, 0.0, 0.0], Metric::Cosine).unwrap(),
    ];
    assert_eq!(pca(&line), vec![[-2.0, 0.0, 0.0], [2.0, 0.0, 0.0]]);
}
