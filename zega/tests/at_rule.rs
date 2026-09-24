use serde_json::json;
use zega::Zega;

#[test]
fn user_field_named_hops_is_stored_and_read() {
    let db = Zega::in_memory().build().unwrap();
    let schema = "type Stop { name: String hops: Int cost: Int shape: String road -> Stop[] { hops: Int cost: Int shape: String } }";
    db.run_lang(schema, r#"mutation { Stop(name: "A" && hops: 11 && cost: 12 && shape: "circle") { road -> Stop(name: "B" && hops: 21 && cost: 22 && shape: "document") { &hops: 7 &cost: 8 &shape: "map" } } }"#).unwrap();
    assert_eq!(
        db.run_lang(schema, r#"query { Stop(hops: 11 && cost: 12 && shape: "circle") { hops cost shape road -> Stop { &hops &cost &shape } } }"#).unwrap(),
        json!({"hops":11,"cost":12,"shape":"circle","road":[{"hops":7,"cost":8,"shape":"map"}]})
    );
    assert_eq!(
        db.run_lang(schema, r#"query { Stop(name: "A") { road *path(@cost <= 8) by &cost -> Stop(name: "B") { &hops &cost &shape } } }"#).unwrap()["road"]["nodes"][1],
        json!({"hops":7,"cost":8,"shape":"map"})
    );
    assert_eq!(
        db.run_lang(
            schema,
            r#"query { Stop(name: "A") { road -> Stop { @hops } } }"#
        )
        .unwrap(),
        json!({"road":[{"hops":1}]})
    );
}

#[test]
fn old_spellings_name_the_replacement_and_mark_the_source() {
    let schema = "type Stop { name: String road -> Stop[] }";
    for (query, old, new) in [
        ("query { Stop { &hops } }", "&hops", "@hops"),
        (
            "query { Stop { road *path(hops <= 2) -> Stop } }",
            "hops",
            "@hops",
        ),
        (
            "query { Stop { road *path(cost <= 2) -> Stop } }",
            "cost",
            "@cost",
        ),
        (
            r#"query { Stop(name STARTS WITH "A") }"#,
            "STARTS WITH",
            "startsWith",
        ),
        (
            r#"query { Stop(name ENDS WITH "A") }"#,
            "ENDS WITH",
            "endsWith",
        ),
        (
            r#"query { Stop(name CONTAINS "A") }"#,
            "CONTAINS",
            "findWith",
        ),
    ] {
        let report = zega::diagnose(schema, query);
        let diagnostic = &report.diagnostics[0];
        assert!(diagnostic.message.contains(new), "{}", report.text);
        assert_eq!(
            &query[(diagnostic.column - 1) as usize..(diagnostic.end_column - 1) as usize],
            old,
            "{}",
            report.text
        );
        assert!(Zega::in_memory()
            .build()
            .unwrap()
            .run_lang(schema, query)
            .is_err());
    }
}

#[test]
fn user_names_and_builtin_values_can_be_projected_together() {
    let db = Zega::in_memory().build().unwrap();
    let schema = "type Sample { id: Int score: Int hops: Int cost: Int shape: String point: Int vector: Int distance: Int similarity: Int within_box: Int near: Int findWith: Int startsWith: Int endsWith: Int query: Int graph: Int embedding: Vector<2> }";
    db.run_lang(schema, "mutation { Sample(id: 99 && score: 42 && hops: 7 && cost: 8 && shape: \"circle\" && point: 1 && vector: 2 && distance: 3 && similarity: 4 && within_box: 5 && near: 6 && findWith: 7 && startsWith: 8 && endsWith: 9 && query: 10 && graph: 11 && embedding: @vector[1,0]) }").unwrap();
    let fields = "id score hops cost shape point vector distance similarity within_box near findWith startsWith endsWith query graph node: @id depth: @hops";
    let expected = json!({"id":99,"score":42,"hops":7,"cost":8,"shape":"circle","point":1,"vector":2,"distance":3,"similarity":4,"within_box":5,"near":6,"findWith":7,"startsWith":8,"endsWith":9,"query":10,"graph":11,"node":1,"depth":0});
    assert_eq!(
        db.run_lang(
            schema,
            &format!("query {{ Sample(id: 99 && similarity: 4 && within_box: 5) {{ {fields} }} }}")
        )
        .unwrap(),
        expected
    );
    assert_eq!(
        db.run_lang(
            schema,
            &format!("query {{ Sample(@id: 1) {{ {fields} }} }}")
        )
        .unwrap(),
        expected
    );
    let rows = db
        .run_lang(
            schema,
            "query { Sample @near(embedding, @vector[1,0], 1) { id score measured: @score } }",
        )
        .unwrap();
    assert_eq!(rows, json!([{"id":99,"score":42,"measured":1.0}]));
}

#[test]
fn old_builtin_names_are_errors_with_exact_spans() {
    let schema = "type Sample { name: String at: Point embedding: Vector<2> }";
    for (query, old) in [
        ("query { Sample { id } }", "id"),
        ("query { Sample(id: 1) }", "id"),
        ("query { Sample { score } }", "score"),
        ("query { Sample { value: score } }", "score"),
        ("query { Sample near(embedding, @vector[1,0], 1) }", "near"),
        ("query { Sample(at: point(0,0)) }", "point"),
        ("query { Sample(embedding: vector[1,0]) }", "vector"),
        (
            "query { Sample(distance(at, @point(0,0)) < 1) }",
            "distance",
        ),
        ("query { Sample { distance(at, @point(0,0)) } }", "distance"),
        (
            "query { Sample(similarity(embedding, @vector[1,0]) > 0) }",
            "similarity",
        ),
        (
            "query { Sample(within_box(at, @point(0,0), @point(1,1))) }",
            "within_box",
        ),
    ] {
        let report = zega::diagnose(schema, query);
        let diagnostic = &report.diagnostics[0];
        assert!(
            diagnostic.message.contains(&format!("@{old}")),
            "{}",
            report.text
        );
        assert_eq!(
            &query[(diagnostic.column - 1) as usize..(diagnostic.end_column - 1) as usize],
            old,
            "{}",
            report.text
        );
        assert!(Zega::in_memory()
            .build()
            .unwrap()
            .run_lang(schema, query)
            .is_err());
    }
}

#[test]
fn from_can_name_a_user_field_beside_a_mapping() {
    let db = Zega::in_memory().build().unwrap();
    let schema = "type Sample { name: String from: Int at?: Point from (lat, lon) }";
    db.run_lang(schema, "mutation { Sample(name: \"A\" && from: 7) }")
        .unwrap();
    assert_eq!(
        db.run_lang(schema, "query { Sample(from: 7) { from } }")
            .unwrap(),
        json!({"from":7})
    );
}

#[test]
fn comments_can_separate_builtin_names_from_arguments() {
    let db = Zega::in_memory().build().unwrap();
    let schema = "type Sample { at: Point v: Vector<2> }";
    db.run_lang(
        schema,
        "mutation { Sample(at: @point // origin\n (0,0) && v: @vector // unit\n [1,0]) }",
    )
    .unwrap();
    assert_eq!(
        db.run_lang(
            schema,
            "query { Sample @near // exact\n (v, @vector[1,0], 1, exact) { @score } }"
        )
        .unwrap(),
        json!([{"score":1.0}])
    );
}
