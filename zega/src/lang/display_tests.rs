use super::*;

fn error(source: &str, message: &str, line: u32, column: u32, end: u32, help: &str) {
    assert_eq!(
        parse_schema(source).unwrap_err(),
        Error {
            message: message.into(),
            help: Some(help.into()),
            line,
            column,
            end_line: line,
            end_column: end,
        }
    );
}

#[test]
fn display_missing_coordinates() {
    for fields in ["name: String", "lat: Float", "lon: Float"] {
        error(
            &format!("type Place {{ {fields} }}\ndisplay {{ map }}"),
            "display `map` needs coordinates, and no type has them",
            2,
            11,
            14,
            "add `lat: Float` and `lon: Float` to a type, e.g. Place",
        );
    }
}

#[test]
fn display_wrong_coordinate_type() {
    for fields in ["lat: String lon: Float", "lat: Float lon: Int"] {
        error(
            &format!("type Place {{ {fields} }}\ndisplay {{ map {{ Place }} }}"),
            "display `map` needs coordinates on type Place",
            2,
            17,
            22,
            "add `lat: Float` and `lon: Float` to Place, or remove Place from this view",
        );
    }
}

#[test]
fn display_every_listed_type_is_checked() {
    error("type Place { lat: Float lon: Float }\ntype Review { name: String }\ndisplay { map { Place, Review } }",
        "display `map` needs coordinates on type Review", 3, 24, 30,
        "add `lat: Float` and `lon: Float` to Review, or remove Review from this view");
}

#[test]
fn display_unknown_singular_type() {
    let source = "type Place { name: String }\ndisplay { table { Places } }";
    let help = type_help(
        &parse_schema("type Place { name: String }").unwrap(),
        "Places",
    );
    error(
        source,
        "display refers to unknown type Places",
        2,
        19,
        25,
        &help,
    );
}

#[test]
fn display_two_defaults() {
    error(
        "type Place {}\ndisplay { table: Default graph: Default }",
        "display has more than one Default",
        2,
        33,
        40,
        "mark only one view `: Default`, or omit it to start on the first view",
    );
}

#[test]
fn display_empty() {
    error(
        "type Place {}\ndisplay {}",
        "display block is empty",
        2,
        1,
        8,
        "list at least one view, e.g. `display { graph }`",
    );
    error(
        "type Place {}\ndisplay { table {} }",
        "display type list is empty",
        2,
        11,
        16,
        "name at least one type, or omit braces to show all types",
    );
}

#[test]
fn display_timeline_requirements() {
    error(
        "type Event { year: String }\ndisplay { timeline }",
        "display `timeline` needs a year/date field, and no type has them",
        2,
        11,
        19,
        "add `year: Int` or `date: String` to a type, e.g. Event",
    );
    error(
        "type Event { date: Float }\ndisplay { timeline { Event } }",
        "display `timeline` needs a year/date field on type Event",
        2,
        22,
        27,
        "add `year: Int` or `date: String` to Event, or remove Event from this view",
    );
    for field in ["year: Int", "date: String"] {
        assert!(parse_schema(&format!(
            "type Event {{ {field} }} display {{ timeline {{ Event }} }}"
        ))
        .is_ok());
    }
}

#[test]
fn display_syntax_errors() {
    error(
        "type Place {}\ndisplay { globe }",
        "unknown display view globe",
        2,
        11,
        16,
        "use `graph`, `table`, `map`, `timeline`, `vector2d`, or `vector3d`",
    );
    error(
        "type Place {}\ndisplay { graph: default }",
        "expected Default",
        2,
        18,
        25,
        "write `: Default` after the view and its type list",
    );
    error(
        "type Place {}\ndisplay { graph graph }",
        "duplicate display view",
        2,
        17,
        22,
        "list each view once",
    );
    error(
        "type Place {}\ndisplay { graph } display { table }",
        "duplicate display block",
        2,
        19,
        26,
        "a schema has one display block",
    );
}

#[test]
fn display_round_trip_and_defaults() {
    let source = "schema { display { map { Place }: Default table { Review } graph } type Place { lat: Float lon: Float } type Review {} }";
    let schema = parse_schema(source).unwrap();
    let json = serde_json::to_value(&schema.display).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"views": [
        {"kind":"map", "types":["Place"]}, {"kind":"table", "types":["Review"]},
        {"kind":"graph", "types":null}], "default":"map"})
    );
    assert_eq!(
        serde_json::from_value::<DisplayConfig>(json).unwrap(),
        schema.display
    );
    assert_eq!(
        parse_schema("type Place {} display { table graph }")
            .unwrap()
            .display
            .default,
        ViewKind::Table
    );
    assert_eq!(
        parse_schema("type Place {} display { table graph: Default }")
            .unwrap()
            .display
            .default,
        ViewKind::Graph
    );
    assert_eq!(
        parse_schema("type Place { lat: Float lon: Float }")
            .unwrap()
            .display,
        DisplayConfig::default()
    );
    assert!(
        parse_schema("type Place { lat: Float lon: Float } type Review {} display { map }").is_ok()
    );
    assert_eq!(
        parse_zql(&format!(
            "{source} mutation {{ Place(lat: 51.0 && lon: -114.0) {{ lat lon }} }}"
        ))
        .unwrap()
        .schema
        .display,
        schema.display
    );
}

#[test]
fn display_diagnostic_caret_and_public_api() {
    let db = crate::Zega::in_memory().build().unwrap();
    let source = "type Place {}\ndisplay { map }";
    let report = diagnose(source, "");
    assert!(report.text.contains("display `map` needs coordinates"));
    assert!(report.text.contains("^^^"));
    assert!(report.text.contains("help:"));
    assert!(db
        .schema(source)
        .unwrap_err()
        .to_string()
        .contains(&report.diagnostics[0].message));
    assert_eq!(
        db.schema("type Place {} display { table }")
            .unwrap()
            .display
            .default,
        ViewKind::Table
    );
}

#[test]
fn node_display_serialization_and_defaults() {
    let schema = parse_schema("type Log {} type Contract { scan: String<url> } type Player { photo?: String<url> } display { graph { Log(@shape: document) Contract(@shape: document, @image: &scan), Player(@shape: circle, @image: &photo, @size: 2) } table { Log(@size: 3) } }").unwrap();
    assert_eq!(
        serde_json::to_value(&schema.display).unwrap(),
        serde_json::json!({
            "views": [
                {"kind":"graph","types":["Log","Contract","Player"],"nodes":{
                    "Log":{"shape":"document","size":1,"image":null},
                    "Contract":{"shape":"document","size":1,"image":"scan"},
                    "Player":{"shape":"circle","size":2,"image":"photo"}}},
                {"kind":"table","types":["Log"],"nodes":{"Log":{"shape":"circle","size":3,"image":null}}}
            ],"default":"graph"
        })
    );
    assert_eq!(
        serde_json::from_value::<DisplayConfig>(serde_json::to_value(&schema.display).unwrap())
            .unwrap(),
        schema.display
    );
    assert_eq!(
        parse_schema("type Log {} display { graph { Log } }")
            .unwrap()
            .display
            .views[0]
            .nodes
            .len(),
        0
    );
}

fn attribute_error(attribute: &str, token: &str, message: &str, help: &str) {
    let source = format!("type Log {{ scan: String photo: Int related -> Log }}\ndisplay {{ graph {{ Log({attribute}) }} }}");
    let error = parse_schema(&source).unwrap_err();
    assert_eq!(error.message, message, "{source}");
    assert_eq!(error.help.as_deref(), Some(help));
    assert_eq!(error.line, 2);
    assert_eq!(error.end_line, 2);
    let line = source.lines().nth(1).unwrap();
    assert_eq!(
        &line[(error.column - 1) as usize..(error.end_column - 1) as usize],
        token
    );
}

#[test]
fn node_display_unknown_attribute_span() {
    attribute_error(
        "@colour: blue",
        "@colour",
        "unknown display attribute @colour",
        "use `@shape`, `@image` or `@size`",
    );
}

#[test]
fn node_display_shape_errors() {
    for value in ["square", "1", "\"document\"", "&scan", "true"] {
        attribute_error(
            &format!("@shape: {value}"),
            value,
            "@shape must be circle or document",
            "write `@shape: circle` or `@shape: document`",
        );
    }
}

#[test]
fn node_display_size_errors() {
    for value in [
        "0", "4", "-1", "1.5", "2.0", "large", "\"2\"", "&scan", "false",
    ] {
        attribute_error(
            &format!("@size: {value}"),
            value,
            "@size must be 1, 2 or 3",
            "write `@size: 1`, `@size: 2` or `@size: 3`",
        );
    }
}

#[test]
fn node_display_image_requires_url_field() {
    for field in ["scan", "photo", "missing", "related"] {
        attribute_error(
            &format!("@image: &{field}"),
            &format!("&{field}"),
            &format!("@image needs Log.{field} to be String<url>"),
            &format!("declare `{field}: String<url>` on Log"),
        );
    }
}

#[test]
fn node_display_image_reference_errors() {
    for value in ["scan", "\"https://example.com/a.png\"", "2", "null"] {
        attribute_error(
            &format!("@image: {value}"),
            value,
            "@image needs a field reference",
            "write `@image: &scan` and declare `scan: String<url>`",
        );
    }
}

#[test]
fn node_display_duplicate_and_at_rule() {
    for (name, value) in [("shape", "circle"), ("size", "2"), ("image", "&scan")] {
        // Use valid URL fields so a duplicate image is the first error.
        let source = format!("type Log {{ scan: String<url> }} display {{ graph {{ Log(@{name}: {value}, @{name}: {value}) }} }}");
        let error = parse_schema(&source).unwrap_err();
        assert_eq!(
            error.message,
            format!("duplicate display attribute @{name}")
        );
        assert_eq!(
            &source[(error.column - 1) as usize..(error.end_column - 1) as usize],
            format!("@{name}")
        );
        attribute_error(
            &format!("{name}: {value}"),
            name,
            &format!("display attribute {name} needs @{name}"),
            &format!("write `@{name}: …`; `@` names language attributes"),
        );
    }
    let source = "type Log {} display { graph { Log Log(@size: 2) } }";
    assert_eq!(
        parse_schema(source).unwrap_err().message,
        "duplicate display type Log"
    );
}
