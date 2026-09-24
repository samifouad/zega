use serde_json::json;
use std::collections::HashMap;
use zega::Zega;

const SCHEMA: &str = "type Country { name: String iso: String<iso2> former?: String<iso2> } type City { name: String at: Point } display { globe(@zoom: 1.4, @tilt: 20, @center: @point(51.05, -114.07)) { Country } : Default map { City } }";

#[test]
fn iso2_writes_updates_queries_and_indexes() {
    let db = Zega::in_memory().build().unwrap();
    db.run_lang(SCHEMA, r#"mutation { Country(name: "Canada" && iso: "CA" && former: null) }"#).unwrap();
    db.run_lang(SCHEMA, r#"mutation { Country(name: "Japan" && iso: "JP") }"#).unwrap();
    assert_eq!(
        db.run_lang(SCHEMA, r#"query { Country(iso startsWith "C") { name iso former } }"#).unwrap(),
        json!([{"name":"Canada","iso":"CA","former":null}])
    );
    db.run_lang(SCHEMA, r#"mutation { Country(name: "Japan") set former: "JP" { former } }"#).unwrap();
    let source = r#"schema { type Country { iso: String<iso2> } }
        index { text Country { iso } range Country { iso } }
        mutation { Country(iso: "GB") }
        query { Country(iso findWith "B") { iso } }"#;
    assert_eq!(Zega::in_memory().build().unwrap().apply_zql(source).unwrap(), json!([{"iso":"GB"}]));
}

#[test]
fn invalid_iso2_writes_and_updates_are_atomic() {
    let db = Zega::in_memory().build().unwrap();
    db.run_lang(SCHEMA, r#"mutation { Country(name: "Canada" && iso: "CA") }"#).unwrap();
    let before = db.graph_json().unwrap();
    for value in [r#""ca""#, r#""CAN""#, r#""XK""#, r#""UK""#, r#""""#, r#"" CA""#, "42", "null"] {
        for source in [
            format!("mutation {{ Country(name: \"B\" && iso: {value}) }}"),
            format!("mutation {{ Country(name: \"Canada\") set iso: {value} {{ iso }} }}"),
        ] {
            let error = db.run_lang(SCHEMA, &source).unwrap_err().to_string();
            assert!(error.contains("Country.iso must be String<iso2>"), "{source}: {error}");
            assert!(error.contains("ISO 3166-1 alpha-2"), "{error}");
            assert_eq!(db.graph_json().unwrap(), before);
        }
    }
}

#[test]
fn iso2_import_validation_rejects_entire_batch() {
    let db = Zega::in_memory().build().unwrap();
    for (format, rows) in [
        ("json", r#"[{"name":"Canada","iso":"CA"},{"name":"Nowhere","iso":"QQ"}]"#),
        ("csv", "name,iso\nCanada,CA\nNowhere,QQ\n"),
    ] {
        let source = format!("mutation {format} [\"rows.{format}\"] {{ Country(name: $name && iso: $iso) }}");
        let sources = HashMap::from([(format!("rows.{format}"), rows.into())]);
        let error = db.run_lang_with_sources(SCHEMA, &source, &sources).unwrap_err().to_string();
        assert!(error.contains("String<iso2>"), "{error}");
        assert_eq!(db.graph_json().unwrap()["nodes"], json!([]));
    }
}

#[test]
fn iso2_edge_properties_are_checked() {
    let db = Zega::in_memory().build().unwrap();
    let schema = "type Person { name: String visited -> Person[] { via: String<iso2> } }";
    db.run_lang(schema, r#"mutation { Person(name: "A") { visited -> Person(name: "B") { &via: "FR" } } }"#).unwrap();
    let error = db
        .run_lang(schema, r#"mutation { Person(name: "C") { visited -> Person(name: "D") { &via: "France" } } }"#)
        .unwrap_err()
        .to_string();
    assert!(error.contains("String<iso2>"), "{error}");
}

#[test]
fn schema_api_returns_the_globe_camera() {
    let db = Zega::in_memory().build().unwrap();
    let schema = db.schema(SCHEMA).unwrap();
    let display = serde_json::to_value(&schema.display).unwrap();
    assert_eq!(display["default"], json!("globe"));
    assert_eq!(display["views"][0]["globe"], json!({"zoom":1.4,"tilt":20.0,"center":{"lat":51.05,"lon":-114.07}}));
    assert_eq!(display["views"][1].get("globe"), None);
}
