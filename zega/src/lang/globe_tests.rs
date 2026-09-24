use super::*;
use serde_json::json;

const TYPES: &str = "type Country { name: String iso: String<iso2> }\ntype City { name: String at: Point }\n";

fn display(source: &str) -> Json {
    serde_json::to_value(parse_schema(&format!("{TYPES}{source}")).unwrap().display).unwrap()
}

/// Message, help and the underlined source text.
fn fails(source: &str) -> (String, String, String) {
    let full = format!("{TYPES}{source}");
    let error = parse_schema(&full).unwrap_err();
    let line = full.lines().nth(error.line as usize - 1).unwrap();
    let text: String = line
        .chars()
        .skip(error.column as usize - 1)
        .take((error.end_column - error.column) as usize)
        .collect();
    (error.message, error.help.unwrap_or_default(), text)
}

#[test]
fn decided_example_checks_into_a_complete_camera() {
    assert_eq!(
        display("display { globe(@zoom: 1.4, @tilt: 20, @center: @point(51.05, -114.07)) { Country } : Default  map { City } }"),
        json!({
            "views": [
                { "kind": "globe", "types": ["Country"], "globe": { "zoom": 1.4, "tilt": 20.0, "center": { "lat": 51.05, "lon": -114.07 } } },
                { "kind": "map", "types": ["City"] }
            ],
            "default": "globe"
        })
    );
}

#[test]
fn globe_settings_are_optional_and_default() {
    let camera = json!({ "zoom": 1.5, "tilt": 0.0, "center": { "lat": 20.0, "lon": 0.0 } });
    for source in ["display { globe }", "display { globe() }", "display { globe { Country, City } }"] {
        assert_eq!(display(source)["views"][0]["globe"], camera, "{source}");
    }
    assert_eq!(display("display { globe(@tilt: 85) }")["views"][0]["globe"]["tilt"], json!(85.0));
    assert_eq!(display("display { globe(@zoom: 0) }")["views"][0]["globe"]["zoom"], json!(0.0));
    assert_eq!(display("display { globe(@zoom: 22) }")["views"][0]["globe"]["zoom"], json!(22.0));
}

#[test]
fn globe_accepts_coordinates_or_a_country_code() {
    for types in [
        "type T { iso: String<iso2> }",
        "type T { at: Point }",
        "type T { lat: Float lon: Float }",
        "type T { iso?: String<iso2> }",
    ] {
        parse_schema(&format!("{types} display {{ globe {{ T }} }}")).unwrap();
    }
    let error = parse_schema("type T { name: String iso: String }\ndisplay { globe { T } }").unwrap_err();
    assert_eq!(error.message, "display `globe` needs a country code or coordinates on type T");
    assert_eq!(error.help.unwrap(), "add `iso: String<iso2>` or `at: Point` to T, or remove T from this view");
    let error = parse_schema("type T { name: String }\ndisplay { globe }").unwrap_err();
    assert_eq!(error.message, "display `globe` needs a country code or coordinates, and no type has them");
}

#[test]
fn globe_setting_diagnostics_name_the_rule_at_the_value() {
    for (source, message, help, text) in [
        ("display { globe(@zoom: 23) }", "@zoom must be a number from 0 to 22", "1.5 shows the whole globe; the globe becomes the flat map near 12", "23"),
        ("display { globe(@zoom: -1) }", "@zoom must be a number from 0 to 22", "1.5 shows the whole globe; the globe becomes the flat map near 12", "-1"),
        ("display { globe(@zoom: \"far\") }", "@zoom must be a number from 0 to 22", "1.5 shows the whole globe; the globe becomes the flat map near 12", "\"far\""),
        ("display { globe(@zoom: &iso) }", "@zoom must be a number from 0 to 22", "1.5 shows the whole globe; the globe becomes the flat map near 12", "&iso"),
        ("display { globe(@tilt: 90) }", "@tilt must be a number of degrees from 0 to 85", "0 looks straight down; `@tilt: 20` leans toward the horizon", "90"),
        ("display { globe(@center: 51) }", "@center must be a @point(latitude, longitude)", "write `@center: @point(51.05, -114.07)`", "51"),
        ("display { globe(@center: north) }", "@center must be a @point(latitude, longitude)", "write `@center: @point(51.05, -114.07)`", "north"),
        ("display { globe(@spin: 2) }", "unknown globe setting @spin", "use `@zoom`, `@tilt` or `@center`", "@spin"),
        ("display { globe(@zoom: 2, @zoom: 3) }", "duplicate globe setting @zoom", "set each globe setting once", "@zoom"),
        ("display { globe(zoom: 2) }", "display attribute zoom needs @zoom", "write `@zoom: …`; `@` names language attributes", "zoom"),
        ("display { map(@zoom: 2) { City } }", "display view map takes no settings", "only `globe(@zoom: …, @tilt: …, @center: @point(…))` has settings", "(@zoom: 2)"),
    ] {
        assert_eq!(fails(source), (message.into(), help.into(), text.into()), "{source}");
    }
}

#[test]
fn globe_center_is_a_checked_builtin_point() {
    let error = parse_schema(&format!("{TYPES}display {{ globe(@center: point(51.05, -114.07)) }}")).unwrap_err();
    assert_eq!(error.message, "`point` is built in: write `@point`");
    let error = parse_schema(&format!("{TYPES}display {{ globe(@center: @point(95, 0)) }}")).unwrap_err();
    assert_eq!(error.message, "Point latitude must be a number in [-90, 90]");
}

#[test]
fn iso2_is_a_string_unit() {
    let schema = parse_schema("type C { iso: String<iso2> }").unwrap();
    assert!(matches!(&schema.types[0].fields[0], Field::Prop { ty, unit: None, .. } if ty == "String<iso2>"));
    let error = parse_schema("type C { iso: String<iso3> }").unwrap_err();
    assert_eq!((error.message.as_str(), error.help.as_deref()), ("unknown string unit iso3", Some("a String unit is `url` or `iso2`, as in `String<iso2>`")));
    assert!(parse_schema("type C { iso: Int<iso2> }").is_err());
    assert!(parse_schema("type C { iso: Bool<iso2> }").is_err());
}

#[test]
fn iso2_accepts_exactly_the_assigned_codes() {
    for code in ["CA", "GB", "JP", "US", "AX", "SS", "BQ", "ZW"] {
        assert!(globe::valid_iso2(code), "{code}");
    }
    for code in ["ca", "Ca", "CAN", "C", "", "UK", "EU", "XK", "ZZ", "AA", "C A", "ÇA"] {
        assert!(!globe::valid_iso2(code), "{code}");
    }
}

#[test]
fn iso2_list_is_the_249_assigned_codes_and_covers_the_bundled_outlines() {
    let codes: Vec<&str> = globe::ISO2_CODES.split_ascii_whitespace().collect();
    assert_eq!(codes.len(), 249);
    assert!(codes.windows(2).all(|pair| pair[0] < pair[1]), "sorted and distinct");
    assert!(codes.iter().all(|code| code.len() == 2 && code.bytes().all(|b| b.is_ascii_uppercase())));
    // The explorer joins String<iso2> values to these outlines; every code
    // they carry must be writable, or that country could never highlight.
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../browser/data/countries-110m.geojson");
    let outlines: Json = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let features = outlines["features"].as_array().unwrap();
    assert_eq!(features.len(), 177);
    let mut matched = 0;
    for feature in features {
        if let Some(iso) = feature["properties"]["iso"].as_str() {
            assert!(globe::valid_iso2(iso), "{iso}");
            matched += 1;
        }
    }
    assert_eq!(matched, 174);
}
