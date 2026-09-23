use zega::{DisplayConfig, ViewKind, Zega};

#[test]
fn calgary_sample_executes_and_returns_all_thirty_places() {
    let db = Zega::in_memory().build().unwrap();
    let source = include_str!("../../browser/samples/calgary.zql");
    let schema = db.schema(source).unwrap();
    assert_eq!(schema.display.default, ViewKind::Map);
    let result = db.apply_zql(source).unwrap();
    let places = result.as_array().unwrap();
    assert_eq!(places.len(), 30);
    assert!(places
        .iter()
        .all(|place| place["lat"].is_number() && place["lon"].is_number()));
    assert!(places.iter().any(|place| place["name"] == "Calgary Tower"));
    let json = serde_json::to_value(&schema).unwrap();
    assert_eq!(
        serde_json::from_value::<DisplayConfig>(json["display"].clone()).unwrap(),
        schema.display
    );
}
