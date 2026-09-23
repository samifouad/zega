use serde_json::{json, Value as Json};
use std::collections::HashMap;
use zega::Zega;

const SCHEMA: &str = "type Place { n: Int at: Point from (lat, lon) }";

// Independent reference, using atan2 rather than the engine's asin formula.
fn haversine(a: (f64, f64), b: (f64, f64)) -> f64 {
    let lat = ((b.0 - a.0).to_radians() / 2.0).sin().powi(2);
    let lon = ((b.1 - a.1).to_radians() / 2.0).sin().powi(2);
    let h = (lat + a.0.to_radians().cos() * b.0.to_radians().cos() * lon).clamp(0.0, 1.0);
    2.0 * 6_371_008.8 * h.sqrt().atan2((1.0 - h).sqrt())
}
fn run(db: &Zega, query: &str) -> Json {
    db.run_lang(SCHEMA, query).unwrap()
}
fn numbers(value: &Json) -> Vec<usize> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["n"].as_u64().unwrap() as usize)
        .collect()
}
fn make_points() -> Vec<(f64, f64)> {
    let mut seed = 0x6a09e667f3bcc909u64;
    let mut random = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / ((1u64 << 53) as f64)
    };
    (0..10_000)
        .map(|i| {
            let (a, b) = (random(), random());
            match i % 10 {
                0 => (89.0 + a, b * 360.0 - 180.0),
                1 => (-89.0 - a, b * 360.0 - 180.0),
                2 => (a * 180.0 - 90.0, 179.0 + b),
                3 => (a * 180.0 - 90.0, -179.0 - b),
                _ => (a * 180.0 - 90.0, b * 360.0 - 180.0),
            }
        })
        .collect()
}
fn load_points(db: &Zega, points: &[(f64, f64)]) {
    let rows: Vec<_> = points
        .iter()
        .enumerate()
        .map(|(n, (lat, lon))| json!({"n":n,"lat":lat,"lon":lon}))
        .collect();
    db.run_lang_with_sources(
        SCHEMA,
        "mutation json [\"points.json\"] { Place(n: $n) { id } }",
        &HashMap::from([("points.json".into(), json!(rows).to_string())]),
    )
    .unwrap();
}

#[test]
fn location_literals_and_diagnostics() {
    let db = Zega::in_memory().build().unwrap();
    assert_eq!(
        run(
            &db,
            "mutation { Place(n: 0 && at: point(51.0447, -114.0719)) { at } }"
        )["at"],
        json!({"lat":51.0447,"lon":-114.0719})
    );
    for (value, offending, message) in [
        ("point(91, 0)", "91", "latitude"),
        ("point(-90.01, 0)", "-90.01", "latitude"),
        ("point(0, 181)", "181", "longitude"),
        ("point(0, -180.01)", "-180.01", "longitude"),
        ("point(\"north\", 0)", "\"north\"", "latitude"),
        ("point(0, true)", "true", "longitude"),
    ] {
        let query = format!("mutation {{ Place(n: 1 && at: {value}) {{ at }} }}");
        let report = zega::diagnose(SCHEMA, &query);
        let error = &report.diagnostics[0];
        assert!(error.message.contains(message), "{}", report.text);
        let start = query.find(offending).unwrap() as u32 + 1;
        assert_eq!(
            (error.line, error.column, error.end_column),
            (1, start, start + offending.len() as u32)
        );
        assert!(db.run_lang(SCHEMA, &query).is_err());
    }
    for value in ["point()", "point(1)", "point(1, 2, 3)", "point(1,)"] {
        let error = db
            .run_lang(
                SCHEMA,
                &format!("mutation {{ Place(n: 1 && at: {value}) {{ at }} }}"),
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("exactly two"), "{error}");
    }
    for (lat, lon) in [(-90, -180), (90, 180), (0, 0)] {
        run(
            &db,
            &format!("mutation {{ Place(n: 2 && at: point({lat}, {lon})) {{ at }} }}"),
        );
    }
    assert!(db
        .run_lang(SCHEMA, "mutation { Place(n: 9 && at: 3) { at } }")
        .is_err());
    assert!(db
        .run_lang(SCHEMA, "mutation { Place(n: 9) { at } }")
        .is_err());
    assert!(db
        .run_lang(SCHEMA, "{ Place(distance(n, point(0, 0)) <= 10) { n } }")
        .is_err());
    assert!(db
        .run_lang(
            SCHEMA,
            "{ Place(within_box(at, point(10, 0), point(0, 1))) { n } }"
        )
        .is_err());
    assert!(db
        .run_lang(
            SCHEMA,
            "{ Place order by distance(at, point(0, 0)) limit -1 { n } }"
        )
        .is_err());
    assert!(db
        .run_lang(SCHEMA, "mutation { Place(n: 0) set at: false { n } }")
        .is_err());
    let optional = "type Place { at?: Point } display { map { Place }: Default }";
    assert!(db.schema(optional).is_ok());
    assert_eq!(
        db.run_lang(
            optional,
            "mutation { Place(at: null) { at distance(at, point(0, 0)) } }"
        )
        .unwrap(),
        json!({"at":null,"distance":null})
    );
}

#[test]
fn location_loads_explicit_columns() {
    let db = Zega::in_memory().build().unwrap();
    let sources = HashMap::from([
        (
            "points.json".into(),
            r#"[{"n":0,"at":{"lat":12,"lon":34}}]"#.into(),
        ),
        (
            "points.csv".into(),
            "n,latitude,longitude\n1,-10,179.9\n2,90,-180\n".into(),
        ),
    ]);
    db.run_lang_with_sources(
        SCHEMA,
        "mutation json [\"points.json\"] { Place(n: $n && at: $at) { at } }",
        &sources,
    )
    .unwrap();
    db.run_lang_with_sources(
        "type Place { n: Int at: Point from (latitude, longitude) }",
        "mutation csv [\"points.csv\"] { Place(n: $n) { at } }",
        &sources,
    )
    .unwrap();
    assert_eq!(run(&db, "{ Place { n at } }").as_array().unwrap().len(), 3);
    // Object input can also populate the named field without an explicit binding.
    let fresh = Zega::in_memory().build().unwrap();
    fresh
        .run_lang_with_sources(
            "type Place { n: Int at: Point }",
            "mutation json [\"points.json\"] { Place(n: $n) { at } }",
            &sources,
        )
        .unwrap();
    assert_eq!(
        run(&fresh, "{ Place { at } }")[0]["at"],
        json!({"lat":12.0,"lon":34.0})
    );
    for (schema, text, expected) in [
        (SCHEMA, "n,lat\n0,1\n", "no column lon"),
        (SCHEMA, "n,lat,lon\n0,north,1\n", "lat must be numeric"),
        (SCHEMA, "n,lat,lon\n0,1,west\n", "lon must be numeric"),
        (SCHEMA, "n,lat,lon\n0,1,2\n1,91,2\n", "latitude"),
        (
            "type Place { n: Int at: Point }",
            "n,lat,lon\n0,1,2\n",
            "explicit from",
        ),
    ] {
        let fresh = Zega::in_memory().build().unwrap();
        let error = fresh
            .run_lang_with_sources(
                schema,
                "mutation csv [\"rows.csv\"] { Place(n: $n) { at } }",
                &HashMap::from([("rows.csv".into(), text.into())]),
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "{error}");
        assert_eq!(
            fresh.graph_json().unwrap()["nodes"],
            json!([]),
            "Point loads validate every row before writing"
        );
    }
    for bad in [
        json!({"lat":91,"lon":0}),
        json!({"lat":0,"lon":181}),
        json!({"lat":"51","lon":0}),
        json!({"lat":0}),
    ] {
        assert!(db
            .run_lang_with_sources(
                SCHEMA,
                "mutation json [\"bad.json\"] { Place(n: $n && at: $at) { at } }",
                &HashMap::from([("bad.json".into(), json!({"n":8,"at":bad}).to_string())])
            )
            .is_err());
    }
    assert!(db
        .schema("type Place { at: Float from (lat, lon) }")
        .is_err());
}

#[test]
fn location_seeded_queries_equal_brute_force() {
    let db = Zega::in_memory().build().unwrap();
    let points = make_points();
    load_points(&db, &points);
    for origin in [
        (51.0447, -114.0719),
        (0.0, 179.99),
        (0.0, -179.99),
        (89.999, 30.0),
        (-89.999, -100.0),
        (90.0, 180.0),
        (-90.0, -180.0),
    ] {
        let p = format!("point({}, {})", origin.0, origin.1);
        let distances: Vec<_> = points
            .iter()
            .map(|point| haversine(origin, *point))
            .collect();
        let projection = run(
            &db,
            &format!("{{ Place {{ n metres: distance(at, {p}) }} }}"),
        );
        for (n, row) in projection.as_array().unwrap().iter().enumerate() {
            let actual = row["metres"].as_f64().unwrap();
            assert!(
                (actual - distances[n]).abs() < 0.002,
                "distance {actual} != {}",
                distances[n]
            );
        }
        for radius in [0.0, 1500.0, 100_000.0, 2_000_000.0, 21_000_000.0] {
            let actual = run(
                &db,
                &format!("{{ Place(distance(at, {p}) <= {radius}) {{ n }} }}"),
            );
            let expected: Vec<_> = distances
                .iter()
                .enumerate()
                .filter_map(|(n, d)| (*d <= radius).then_some(n))
                .collect();
            assert_eq!(numbers(&actual), expected, "radius {radius} at {origin:?}");
        }
        for k in [0, 1, 17, 10_001] {
            let mut expected: Vec<_> = (0..points.len()).collect();
            expected.sort_by(|a, b| distances[*a].total_cmp(&distances[*b]).then(a.cmp(b)));
            expected.truncate(k);
            assert_eq!(
                numbers(&run(
                    &db,
                    &format!("{{ Place order by distance(at, {p}) limit {k} {{ n }} }}")
                )),
                expected
            );
        }
        let mut expected: Vec<_> = (0..points.len())
            .filter(|n| *n > 5000 && distances[*n] <= 100_000.0)
            .collect();
        expected.sort_by(|a, b| distances[*a].total_cmp(&distances[*b]).then(a.cmp(b)));
        expected.truncate(8);
        assert_eq!(numbers(&run(&db,&format!("{{ Place(n > 5000 && distance(at, {p}) <= 100000) order by distance(at, {p}) limit 8 {{ n }} }}"))),expected);
    }
    for (south, west, north, east) in [
        (-90.0, -180.0, 90.0, 180.0),
        (-10.0, 179.0, 10.0, -179.0),
        (89.5, -180.0, 90.0, 180.0),
        (-90.0, 170.0, -89.5, -170.0),
        (40.0, -120.0, 55.0, -110.0),
    ] {
        let inside = |p: &(f64, f64)| {
            p.0 >= south
                && p.0 <= north
                && if west <= east {
                    p.1 >= west && p.1 <= east
                } else {
                    p.1 >= west || p.1 <= east
                }
        };
        let query = format!("within_box(at, point({south}, {west}), point({north}, {east}))");
        let expected: Vec<_> = points
            .iter()
            .enumerate()
            .filter_map(|(n, p)| inside(p).then_some(n))
            .collect();
        assert_eq!(
            numbers(&run(&db, &format!("{{ Place({query}) {{ n }} }}"))),
            expected
        );
        // OR with a non-spatial branch must not prune that branch's matches.
        let expected: Vec<_> = points
            .iter()
            .enumerate()
            .filter_map(|(n, p)| (inside(p) || n < 9).then_some(n))
            .collect();
        assert_eq!(
            numbers(&run(&db, &format!("{{ Place({query} || n < 9) {{ n }} }}"))),
            expected
        );
    }
}

#[test]
fn location_wal_snapshot_updates_and_deletes() {
    let dir = tempfile::tempdir().unwrap();
    {
        let db = Zega::open(dir.path().to_str().unwrap())
            .wal_flush_every_write()
            .build()
            .unwrap();
        run(&db, "mutation { Place(n: 0 && at: point(0, 0)) { n } }");
        run(&db, "mutation { Place(n: 1 && at: point(0, 1)) { n } }");
    }
    let near = "{ Place(distance(at, point(0, 0)) <= 100) { n } }";
    {
        let db = Zega::open(dir.path().to_str().unwrap())
            .wal_flush_every_write()
            .build()
            .unwrap();
        assert_eq!(numbers(&run(&db, near)), vec![0]);
        run(&db, "mutation { Place(n: 0) set at: point(60, 60) { n } }");
        run(&db, "mutation { Place(n: 1) set at: point(0, 0) { n } }");
        assert_eq!(numbers(&run(&db, near)), vec![1]);
    }
    {
        let db = Zega::open(dir.path().to_str().unwrap())
            .wal_flush_every_write()
            .build()
            .unwrap();
        assert_eq!(numbers(&run(&db, near)), vec![1]);
        db.snapshot().unwrap();
    }
    let db = Zega::open(dir.path().to_str().unwrap())
        .wal_flush_every_write()
        .build()
        .unwrap();
    assert_eq!(numbers(&run(&db, near)), vec![1]);
    let restored = Zega::in_memory().build().unwrap();
    run(
        &restored,
        "mutation { Place(n: 99 && at: point(0, 0)) { n } }",
    );
    restored
        .restore_bytes(&db.snapshot_bytes().unwrap())
        .unwrap();
    assert_eq!(numbers(&run(&restored, near)), vec![1]);
    db.delete_node(2).unwrap();
    assert!(numbers(&run(&db, near)).is_empty());
    drop(db);
    let db = Zega::open(dir.path().to_str().unwrap())
        .wal_flush_every_write()
        .build()
        .unwrap();
    assert!(numbers(&run(&db, near)).is_empty());
    db.snapshot().unwrap();
    restored
        .restore_bytes(&db.snapshot_bytes().unwrap())
        .unwrap();
    assert!(numbers(&run(&restored, near)).is_empty());
}

#[test]
fn location_boundaries_optional_points_ties_and_boolean_combinations() {
    let db = Zega::in_memory().build().unwrap();
    load_points(
        &db,
        &[
            (0.0, 180.0),
            (0.0, -180.0),
            (90.0, 90.0),
            (90.0, -90.0),
            (-90.0, 0.0),
            (0.0, 0.0),
            (0.0, 0.0),
        ],
    );
    assert_eq!(
        numbers(&run(
            &db,
            "{ Place(distance(at, point(0, 180)) <= 0) { n } }"
        )),
        vec![0, 1]
    );
    assert_eq!(
        numbers(&run(
            &db,
            "{ Place(distance(at, point(90, 0)) <= 0) { n } }"
        )),
        vec![2, 3]
    );
    assert_eq!(
        numbers(&run(&db, "{ Place(distance(at, point(0, 0)) < 0) { n } }")),
        Vec::<usize>::new()
    );
    assert_eq!(
        numbers(&run(
            &db,
            "{ Place order by distance(at, point(0, 0)) limit 2 { n } }"
        )),
        vec![5, 6]
    );
    assert_eq!(
        numbers(&run(
            &db,
            "{ Place(within_box(at, point(-90, 180), point(90, -180))) { n } }"
        )),
        vec![0, 1]
    );
    assert_eq!(
        numbers(&run(
            &db,
            "{ Place(distance(at, point(0, 0)) <= 0 || distance(at, point(90, 0)) <= 0) { n } }"
        )),
        vec![2, 3, 5, 6]
    );
    assert_eq!(numbers(&run(&db,"{ Place(within_box(at, point(-90, -180), point(90, 180)) && distance(at, point(0, 0)) <= 0) { n } }")),vec![5,6]);
}

#[test]
fn location_multiple_fields_labels_and_relationship_ordering() {
    let db = Zega::in_memory().build().unwrap();
    let schema = "type Tour { name: String visits -> Place[] } type Place { n: Int at: Point other?: Point } type Other { at: Point }";
    db.run_lang(schema,"mutation { Tour(name: \"walk\") { visits -> Place(n: 0 && at: point(0, 3) && other: point(0, 0)) { n } visits -> Place(n: 1 && at: point(0, 1)) { n } visits -> Place(n: 2 && at: point(0, 2)) { n } } }").unwrap();
    db.run_lang(schema, "mutation { Other(at: point(0, 0)) { at } }")
        .unwrap();
    let result = db.run_lang(schema,"{ Tour(name: \"walk\") { visits -> Place order by distance(at, point(0, 0)) limit 2 { n metres: distance(at, point(0, 0)) } } }").unwrap();
    assert_eq!(numbers(&result["visits"]), vec![1, 2]);
    assert_eq!(
        numbers(
            &db.run_lang(schema, "{ Place(distance(other, point(0, 0)) <= 1) { n } }")
                .unwrap()
        ),
        vec![0]
    );
    assert_eq!(
        numbers(
            &db.run_lang(
                schema,
                "{ Place order by distance(other, point(0, 0)) limit 4 { n } }"
            )
            .unwrap()
        ),
        vec![0]
    );
    assert!(numbers(
        &db.run_lang(schema, "{ Place(distance(at, point(0, 0)) <= 1) { n } }")
            .unwrap()
    )
    .is_empty());
    db.run_lang(schema, "mutation { Place(n: 0) set other: null { n } }")
        .unwrap();
    assert!(numbers(
        &db.run_lang(schema, "{ Place(distance(other, point(0, 0)) <= 1) { n } }")
            .unwrap()
    )
    .is_empty());
}

#[test]
fn location_load_links_do_not_require_new_coordinates() {
    let db = Zega::in_memory().build().unwrap();
    let schema = "type Tour { name: String visits -> Place[] } type Place { n: Int at: Point from (lat, lon) }";
    db.run_lang(schema, r#"mutation { Tour(name: "walk") { name } }"#)
        .unwrap();
    db.run_lang(schema, "mutation { Place(n: 0 && at: point(0, 0)) { n } }")
        .unwrap();
    let result = db.run_lang_with_sources(schema, "mutation csv [\"links.csv\"] { Tour(name: $tour) { visits -> link Place(n: $n) { n distance(at, point(0, 0)) } } }", &HashMap::from([("links.csv".into(), "tour,n\nwalk,0\n".into())])).unwrap();
    assert_eq!(result[0]["visits"][0]["distance"], json!(0.0));
}
