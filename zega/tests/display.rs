use zega::{DisplayConfig, ViewKind, Zega};

#[test]
fn calgary_point_sample_matches_independent_haversine() {
    let db = Zega::in_memory().build().unwrap();
    let source = include_str!("../../browser/samples/calgary.zql");
    let schema = db.schema(source).unwrap();
    assert_eq!(schema.display.default, ViewKind::Map);
    let sources = std::collections::HashMap::from([(
        "./samples/calgary.csv".into(),
        include_str!("../../browser/samples/calgary.csv").into(),
    )]);
    let result = db.apply_zql_with_sources(source, &sources).unwrap();
    let stored = db.graph_json().unwrap();
    let all = stored["nodes"].as_array().unwrap();
    assert_eq!(all.len(), 30);
    let tower = &all
        .iter()
        .find(|node| node["name"] == "Calgary Tower")
        .unwrap()["at"];
    let lat = tower["lat"].as_f64().unwrap().to_radians();
    let lon = tower["lon"].as_f64().unwrap().to_radians();
    let distance = |node: &serde_json::Value| {
        let a = node["at"]["lat"].as_f64().unwrap().to_radians();
        let b = node["at"]["lon"].as_f64().unwrap().to_radians();
        let h =
            ((a - lat) / 2.0).sin().powi(2) + lat.cos() * a.cos() * ((b - lon) / 2.0).sin().powi(2);
        2.0 * 6_371_008.8 * h.sqrt().atan2((1.0 - h).sqrt())
    };
    let mut expected: Vec<_> = all.iter().filter(|node| distance(node) <= 1500.0).collect();
    expected.sort_by(|a, b| distance(a).total_cmp(&distance(b)));
    let places = result.as_array().unwrap();
    assert_eq!(
        places.iter().map(|node| &node["name"]).collect::<Vec<_>>(),
        expected
            .iter()
            .map(|node| &node["name"])
            .collect::<Vec<_>>()
    );
    assert!(places
        .iter()
        .all(|place| place["at"]["lat"].is_number() && place["at"]["lon"].is_number()));
    assert!(places.iter().any(|place| place["name"] == "Calgary Tower"));
    let json = serde_json::to_value(&schema).unwrap();
    assert_eq!(
        serde_json::from_value::<DisplayConfig>(json["display"].clone()).unwrap(),
        schema.display
    );
}
