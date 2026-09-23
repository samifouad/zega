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
    assert_eq!(view["results"], result);
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

#[test]
fn optional_vectors_are_skipped_by_near_and_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let schema = "type Ticket { n: Int embedding?: Vector<3> }";
    let all = "{ Ticket { n } }";
    let near = "{ Ticket near(embedding, vector[1,0,0], 10) { n } }";
    {
        let db = Zega::open(dir.path().to_str().unwrap())
            .wal_flush_every_write()
            .build()
            .unwrap();
        db.run_lang(schema, "mutation { Ticket(n: 0) { n } }")
            .unwrap();
        db.run_lang(
            schema,
            "mutation { Ticket(n: 1 && embedding: vector[1,0,0]) { n } }",
        )
        .unwrap();
        db.run_lang(
            schema,
            "mutation { Ticket(n: 2 && embedding: vector[0,1,0]) { n } }",
        )
        .unwrap();
        assert_eq!(ids(&db.run_lang(schema, all).unwrap()), vec![0, 1, 2]);
        assert_eq!(ids(&db.run_lang(schema, near).unwrap()), vec![1, 2]);
    }
    let reopened = Zega::open(dir.path().to_str().unwrap()).build().unwrap();
    assert_eq!(ids(&reopened.run_lang(schema, all).unwrap()), vec![0, 1, 2]);
    assert_eq!(ids(&reopened.run_lang(schema, near).unwrap()), vec![1, 2]);
}

#[test]
fn near_and_order_diagnostic_has_selection_span() {
    let schema = "type Ticket { n: Int at: Point embedding: Vector<3> }";
    let query =
        "{ Ticket near(embedding, vector[1,0,0], 2) order by distance(at, point(0,0)) { n } }";
    let report = zega::diagnose(schema, query);
    let diagnostic = report
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message == "near already orders by similarity")
        .expect("near/order conflict diagnostic");
    let ticket_column = query.find("Ticket").unwrap() as u32 + 1;
    assert_eq!((diagnostic.line, diagnostic.column), (1, ticket_column));
    assert_eq!(diagnostic.end_column, ticket_column + "Ticket".len() as u32);
    assert!(report.text.contains("near already orders by similarity"));
}

#[test]
fn near_limit_is_the_brute_force_top_limit() {
    let db = Zega::in_memory().build().unwrap();
    let rows = [
        "vector[1,0,0]",
        "vector[0.9,0.1,0]",
        "vector[0.8,0.2,0]",
        "vector[0.7,0.3,0]",
        "vector[0.6,0.4,0]",
        "vector[0,1,0]",
        "vector[-1,0,0]",
        "vector[0,0,1]",
    ];
    for (n, vector) in rows.iter().enumerate() {
        db.run_lang(
            SCHEMA,
            &format!("mutation {{ Ticket(n: {n} && embedding: {vector}) {{ n }} }}"),
        )
        .unwrap();
    }
    let query = "{ Ticket near(embedding, vector[1,0,0], 8) limit 3 { n score } }";
    let exact_query = query.replace(", 8)", ", 8, exact)");
    let actual = db.run_lang(SCHEMA, query).unwrap();
    let exact = db.run_lang(SCHEMA, &exact_query).unwrap();
    assert_eq!(actual.as_array().unwrap(), &exact.as_array().unwrap()[..3]);
    assert_eq!(ids(&actual), vec![0, 1, 2]);
}

#[test]
fn vector_view_projects_in_independent_dimension_and_metric_groups() {
    let schema = "type A { v: Vector<2> } type B { v: Vector<3,dot> } type C { v: Vector<2,dot> } display { vector2d { A, B, C }: Default }";
    let db = Zega::in_memory().build().unwrap();
    for (ty, vector) in [
        ("A", "vector[1,0]"),
        ("A", "vector[0,1]"),
        ("B", "vector[1,0,0]"),
        ("C", "vector[0,1]"),
    ] {
        db.run_lang(
            schema,
            &format!("mutation {{ {ty}(v: {vector}) {{ id }} }}"),
        )
        .unwrap();
    }
    let a = db.run_lang(schema, "{ A { id } }").unwrap();
    let b = db.run_lang(schema, "{ B { id } }").unwrap();
    let c = db.run_lang(schema, "{ C { id } }").unwrap();
    let result = json!([
        a.as_array().unwrap()[0],
        a.as_array().unwrap()[1],
        b.as_array().unwrap()[0],
        c.as_array().unwrap()[0],
    ]);
    let view = db
        .vector_view(schema, &result, ViewKind::Vector2d, None, 10, 0.8)
        .unwrap();
    let points = view["points"].as_array().unwrap();
    assert_eq!(points.len(), 4);
    let cosine = points
        .iter()
        .filter(|point| point["dimensions"] == 2 && point["metric"] == "cosine")
        .collect::<Vec<_>>();
    let dot3 = points
        .iter()
        .find(|point| point["dimensions"] == 3 && point["metric"] == "dot")
        .unwrap();
    let dot2 = points
        .iter()
        .find(|point| point["dimensions"] == 2 && point["metric"] == "dot")
        .unwrap();
    let distinct: std::collections::HashSet<_> = points
        .iter()
        .map(|point| point["group"].as_u64().unwrap())
        .collect();
    assert_eq!(distinct.len(), 3);
    assert_eq!(cosine.len(), 2);
    assert_eq!(cosine[0]["group"], cosine[1]["group"]);
    assert_ne!(cosine[0]["group"], dot3["group"]);
    assert_ne!(cosine[0]["group"], dot2["group"]);
    assert_ne!(dot2["group"], dot3["group"]);
    assert!(points
        .iter()
        .all(|point| point["position"].as_array().unwrap().len() == 3));
}

#[test]
fn vector_dimensions_one_and_4096_round_trip() {
    let db = Zega::in_memory().build().unwrap();
    let schema = "type Edge { n: Int v: Vector<1> } type Wide { n: Int v: Vector<4096> }";
    let wide = format!("vector[{}]", vec!["0.25"; 4096].join(","));
    db.run_lang(schema, "mutation { Edge(n: 1 && v: vector[0.5]) { id } }")
        .unwrap();
    db.run_lang(
        schema,
        &format!("mutation {{ Wide(n: 4096 && v: {wide}) {{ id }} }}"),
    )
    .unwrap();
    let bytes = db.snapshot_bytes().unwrap();
    let restored = Zega::in_memory().build().unwrap();
    restored.restore_bytes(&bytes).unwrap();
    let edge = restored.run_lang(schema, "{ Edge { v } }").unwrap();
    let wide_result = restored.run_lang(schema, "{ Wide { v } }").unwrap();
    assert_eq!(edge[0]["v"], json!([0.5]));
    assert_eq!(wide_result[0]["v"].as_array().unwrap().len(), 4096);
    assert!(wide_result[0]["v"]
        .as_array()
        .unwrap()
        .iter()
        .all(|value| value == 0.25));
}
