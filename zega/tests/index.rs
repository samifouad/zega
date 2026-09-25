//! `index { range … text … }`: the index is used, it is kept current by every
//! write and by replay, and a query returns the same thing with or without it.

use serde_json::{json, Value as Json};
use zega::Zega;

const TYPES: &str = "
  type Player {
    n: Int
    name: String
    salary?: Int
    rating?: Float
    active?: Bool
    playsFor -> Team
  }
  type Team { name: String }
";

fn schema(blocks: &str) -> String {
    format!("schema {{{TYPES}}}\n{blocks}")
}

fn indexed() -> String {
    schema("index {\n  range Player { salary rating name }\n  text Player { name }\n}\n")
}

fn plain() -> String {
    schema("")
}

fn run(db: &Zega, schema: &str, query: &str) -> Result<Json, String> {
    db.run_lang(schema, query).map_err(|error| error.to_string())
}

fn examined(db: &Zega) -> u64 {
    db.rows_examined().unwrap()
}

/// Rows a query tested, and its result.
fn measure(db: &Zega, schema: &str, query: &str) -> (u64, Json) {
    let before = examined(db);
    let result = run(db, schema, query).unwrap();
    (examined(db) - before, result)
}

fn ns(value: &Json) -> Vec<i64> {
    let mut out: Vec<i64> = value
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["n"].as_i64().unwrap())
        .collect();
    out.sort_unstable();
    out
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len() as u64) as usize]
    }
}

const SYLLABLES: &[&str] = &["ab", "con", "nor", "mc", "da", "vid", "le", "on", "dra", "i", "sa", "itl", "é", "ü", ""];

fn name(rng: &mut Rng) -> String {
    let parts = 1 + rng.below(4);
    (0..parts).map(|_| rng.pick(SYLLABLES)).collect()
}

fn player(rng: &mut Rng, n: u64) -> String {
    let mut fields = vec![format!("n: {n}"), format!("name: {}", json!(name(rng)))];
    match rng.below(6) {
        0 => {}
        1 => fields.push(format!("salary: {}", -(rng.below(50) as i64))),
        _ => fields.push(format!("salary: {}", rng.below(200))),
    }
    match rng.below(5) {
        0 => {}
        1 => fields.push("rating: 0.0".into()),
        2 => fields.push("rating: -0.0".into()),
        _ => fields.push(format!("rating: {}.{}", rng.below(10), rng.below(100))),
    }
    format!("mutation {{ Player({}) {{ n }} }}", fields.join(" && "))
}

fn load(db: &Zega, schema: &str, count: u64, seed: u64) {
    let mut rng = Rng(seed);
    for n in 0..count {
        run(db, schema, &player(&mut rng, n)).unwrap();
    }
}

fn number(rng: &mut Rng) -> String {
    match rng.below(4) {
        0 => format!("{}.5", rng.below(200)),
        1 => format!("-{}", rng.below(60)),
        2 => "0.0".into(),
        _ => rng.below(210).to_string(),
    }
}

fn condition(rng: &mut Rng, depth: u32) -> String {
    if depth < 2 && rng.below(3) == 0 {
        let op = if rng.below(2) == 0 { "&&" } else { "||" };
        return format!("{} {op} {}", condition(rng, depth + 1), condition(rng, depth + 1));
    }
    let cmp = rng.pick(&["=", ">", "<", ">=", "<=", ":", "!="]);
    match rng.below(7) {
        0 => format!("salary {cmp} {}", number(rng)),
        1 => format!("rating {cmp} {}", number(rng)),
        2 => format!("name {cmp} {}", json!(name(rng))),
        3 => format!("salary {cmp} {}", json!(name(rng))),
        4 => format!("name findExact {}", json!(name(rng))),
        5 => format!("name startsExact {}", json!(name(rng))),
        _ => format!("name endsExact {}", json!(name(rng))),
    }
}

/// Generated data, generated writes, generated queries: the indexed database
/// answers every query exactly like the one without an index, errors included.
#[test]
fn indexed_and_unindexed_answers_are_identical() {
    let (with, without) = (indexed(), plain());
    let a = Zega::in_memory().build().unwrap();
    let b = Zega::in_memory().build().unwrap();
    load(&a, &with, 600, 0x9e3779b97f4a7c15);
    load(&b, &without, 600, 0x9e3779b97f4a7c15);
    let mut rng = Rng(0xd1b54a32d192ed03);
    let (mut rows_a, mut rows_b, mut nonempty) = (0, 0, 0);
    for round in 0..1500 {
        // Every tenth round writes, so the indexes follow inserts, updates and deletes.
        if round % 10 == 0 {
            let n = rng.below(600);
            let write = match rng.below(4) {
                0 => format!("mutation {{ Player(n: {n}) set salary: {} {{ n }} }}", rng.below(200)),
                1 => format!("mutation {{ Player(n: {n}) set name: {} {{ n }} }}", json!(name(&mut rng))),
                2 => format!("mutation {{ Player(n: {n}) set rating: {} {{ n }} }}", number(&mut rng)),
                _ => {
                    let id = 1 + rng.below(600);
                    a.delete_node(id).unwrap();
                    b.delete_node(id).unwrap();
                    continue;
                }
            };
            assert_eq!(run(&a, &with, &write), run(&b, &without, &write), "{write}");
        }
        let query = if rng.below(8) == 0 {
            // An equality root read returns one object or reports many rows.
            format!("{{ Player(salary = {}) {{ n }} }}", rng.below(200))
        } else {
            format!("{{ Player({}) {{ n name salary rating }} }}", condition(&mut rng, 0))
        };
        let (before_a, before_b) = (examined(&a), examined(&b));
        let left = run(&a, &with, &query);
        let right = run(&b, &without, &query);
        assert_eq!(left, right, "{query}");
        if left.as_ref().is_ok_and(|rows| rows.as_array().is_some_and(|rows| !rows.is_empty())) {
            nonempty += 1;
        }
        rows_a += examined(&a) - before_a;
        rows_b += examined(&b) - before_b;
    }
    eprintln!("1500 queries, {nonempty} with rows; rows tested: indexed {rows_a}, unindexed {rows_b}");
    assert!(nonempty > 300, "only {nonempty} queries returned rows");
    // Mostly indexable conditions: the index tests far fewer rows.
    assert!(rows_a * 2 < rows_b, "indexed {rows_a} rows, unindexed {rows_b}");
}

#[test]
fn range_index_answers_comparisons_without_a_scan() {
    let (with, without) = (indexed(), plain());
    let a = Zega::in_memory().build().unwrap();
    let b = Zega::in_memory().build().unwrap();
    for db in [&a, &b] {
        for n in 0..1000 {
            let schema = if std::ptr::eq(db, &a) { &with } else { &without };
            run(db, schema, &format!("mutation {{ Player(n: {n} && name: \"p{n}\" && salary: {n} && rating: {n}.5) {{ n }} }}")).unwrap();
        }
    }
    let cases: &[(&str, u64)] = &[
        ("salary >= 990", 10),
        ("salary > 989", 11), // `>` is widened to `>=`; the filter drops 989
        ("salary < 5", 6),
        ("salary <= 4", 5),
        ("salary > 100 && salary < 111", 12),
        ("salary >= 100 && salary <= 110 && n != 105", 11),
        ("salary < 3 || salary > 996", 8),
        ("rating > 997.0", 3),
        ("rating >= 997.5 && rating <= 998.5", 2),
        ("name >= \"p998\"", 2),
        ("salary > 10 && salary < \"z\"", 0),
    ];
    for (condition, bound) in cases {
        let query = format!("{{ Player({condition}) {{ n }} }}");
        let (rows_a, result_a) = measure(&a, &with, &query);
        let (rows_b, result_b) = measure(&b, &without, &query);
        assert_eq!(result_a, result_b, "{query}");
        assert!(rows_a <= *bound, "{query}: indexed tested {rows_a} rows, expected at most {bound}");
        assert_eq!(rows_b, 1000, "{query}: the unindexed read scans every Player");
    }
    // An equality read and a `set` lookup use the range index too.
    let (rows, row) = measure(&a, &with, "{ Player(salary = 500) { n } }");
    assert_eq!((rows, row), (1, json!({ "n": 500 })));
    let (rows, _) = measure(&a, &with, "mutation { Player(salary: 7) set name: \"seven\" { n } }");
    assert_eq!(rows, 1);
    assert_eq!(
        run(&a, &with, "{ Player(name = \"seven\") { n salary } }").unwrap(),
        json!({ "n": 7, "salary": 7 })
    );
}

#[test]
fn text_index_answers_contains_starts_and_ends_with() {
    let (with, without) = (indexed(), plain());
    let a = Zega::in_memory().build().unwrap();
    let b = Zega::in_memory().build().unwrap();
    let names = ["Connor McDavid", "Leon Draisaitl", "Zach Hyman", "Evan Bouchard", "Mattias Ekholm", "Connor Brown"];
    for i in 0..600 {
        let name = json!(format!("{} {i:03}", names[i % names.len()]));
        let insert = format!("mutation {{ Player(n: {i} && name: {name}) {{ n }} }}");
        run(&a, &with, &insert).unwrap();
        run(&b, &without, &insert).unwrap();
    }
    let cases: &[(&str, u64, usize)] = &[
        ("name findExact \"McDavid\"", 100, 100),
        ("name startsExact \"Connor\"", 200, 200),
        ("name endsExact \"599\"", 1, 1),
        ("name findExact \"Hyman 01\"", 1, 1),
        // `endsExact "1"` is too short for a trigram; the Leon trigrams narrow it.
        ("name startsExact \"Leon\" && name endsExact \"1\"", 100, 20),
        ("name findExact \"nobody\"", 0, 0),
        ("name findExact \"Brown\" || name endsExact \"000\"", 101, 101),
        // Too short for a trigram: every Player with a name is a candidate.
        ("name findExact \"99\"", 600, 6),
    ];
    for (condition, rows, count) in cases {
        let query = format!("{{ Player({condition}) {{ n }} }}");
        let (rows_a, result_a) = measure(&a, &with, &query);
        let (rows_b, result_b) = measure(&b, &without, &query);
        assert_eq!(result_a, result_b, "{query}");
        assert_eq!(ns(&result_a).len(), *count, "{query}");
        assert_eq!(rows_a, *rows, "{query}: rows tested with the index");
        assert_eq!(rows_b, 600, "{query}: the unindexed read scans every Player");
    }
}

#[test]
fn unique_fields_get_a_range_index() {
    let with = schema("unique { Player { n } }\n");
    let db = Zega::in_memory().build().unwrap();
    for n in 0..500 {
        run(&db, &with, &format!("mutation {{ Player(n: {n} && name: \"p\") {{ n }} }}")).unwrap();
    }
    let (rows, result) = measure(&db, &with, "{ Player(n > 494) { n } }");
    assert_eq!(ns(&result), vec![495, 496, 497, 498, 499]);
    assert!(rows <= 6, "tested {rows} rows");
}

#[test]
fn only_the_types_named_are_indexed() {
    // A range index on Player does not answer for Team, and a union read uses
    // the index only when every type in it has one.
    let with = schema("index { range Player { name } }\n");
    let db = Zega::in_memory().build().unwrap();
    for n in 0..100 {
        run(&db, &with, &format!("mutation {{ Player(n: {n} && name: \"p{n:02}\") {{ n }} }}")).unwrap();
        run(&db, &with, &format!("mutation {{ Team(name: \"p{n:02}\") {{ name }} }}")).unwrap();
    }
    let (rows, result) = measure(&db, &with, "{ Player(name >= \"p98\") { n } }");
    assert_eq!((rows, ns(&result)), (2, vec![98, 99]));
    let (rows, result) = measure(&db, &with, "{ Team(name >= \"p98\") { name } }");
    assert_eq!(result.as_array().unwrap().len(), 2);
    assert_eq!(rows, 100);
}

#[test]
fn indexes_survive_restart_writes_and_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_str().unwrap();
    let with = indexed();
    let open = || Zega::open(path).wal_flush_every_write().build().unwrap();
    {
        let db = open();
        let mut document = with.clone();
        for n in 0..300 {
            document.push_str(&format!("mutation {{ Player(n: {n} && name: \"player {n}\" && salary: {n}) {{ n }} }}\n"));
        }
        db.apply_zql(&document).unwrap();
    }
    let top = "{ Player(salary >= 295) { n } }";
    let named = "{ Player(name endsExact \" 42\") { n } }";
    {
        let db = open();
        let (rows, result) = measure(&db, &with, top);
        assert_eq!((rows, ns(&result)), (5, vec![295, 296, 297, 298, 299]));
        let (rows, result) = measure(&db, &with, named);
        assert_eq!((rows, ns(&result)), (1, vec![42]));
        run(&db, &with, "mutation { Player(n: 10) set salary: 1000 { n } }").unwrap();
        run(&db, &with, "mutation { Player(n: 10) set name: \"renamed\" { n } }").unwrap();
        db.delete_node(297).unwrap(); // n = 296
        run(&db, &with, "mutation { Player(n: 1000 && name: \"late 42\" && salary: 299) { n } }").unwrap();
    }
    let expected_top = vec![10, 295, 297, 298, 299, 1000];
    {
        let db = open();
        let (rows, result) = measure(&db, &with, top);
        assert_eq!(ns(&result), expected_top);
        assert_eq!(rows, 6);
        let (rows, result) = measure(&db, &with, named);
        assert_eq!((rows, ns(&result)), (2, vec![42, 1000]));
        db.snapshot().unwrap();
    }
    let db = open();
    let (rows, result) = measure(&db, &with, top);
    assert_eq!((rows, ns(&result)), (6, expected_top.clone()));
    let restored = Zega::in_memory().build().unwrap();
    restored.restore_bytes(&db.snapshot_bytes().unwrap()).unwrap();
    let (rows, result) = measure(&restored, &with, top);
    assert_eq!((rows, ns(&result)), (6, expected_top));
}

#[test]
fn a_schema_without_the_block_drops_the_index() {
    let (with, without) = (indexed(), plain());
    let db = Zega::in_memory().build().unwrap();
    for n in 0..200 {
        run(&db, &with, &format!("mutation {{ Player(n: {n} && name: \"p\" && salary: {n}) {{ n }} }}")).unwrap();
    }
    let query = "{ Player(salary >= 198) { n } }";
    assert_eq!(measure(&db, &with, query).0, 2);
    assert_eq!(measure(&db, &without, query).0, 200);
    // Writes made while the index was dropped are there when it is rebuilt.
    run(&db, &without, "mutation { Player(n: 999 && name: \"late\" && salary: 199) { n } }").unwrap();
    let (rows, result) = measure(&db, &with, query);
    assert_eq!((rows, ns(&result)), (3, vec![198, 199, 999]));
}

#[test]
fn checker_rejects_bad_index_blocks_with_spans() {
    let cases: &[(&str, &str, &str)] = &[
        ("index { range Playr { salary } }", "unknown type Playr", "Playr"),
        ("index { range Player { salry } }", "Player has no field salry", "salry"),
        ("index { range Player { playsFor } }", "Player.playsFor is a relationship", "playsFor"),
        ("index { text Player { salary } }", "text index needs a String field; Player.salary is Int", "salary"),
        ("index { range Player { active } }", "range index needs an Int, Float or String field; Player.active is Bool", "active"),
        ("index { range Player { salary } range Player { salary } }", "Player.salary already has a range index", "salary"),
        ("unique { Player { name } }\nindex { range Player { name } }", "Player.name is already indexed by unique", "name"),
        ("index { range Player { name } }\nunique { Player { name } }", "Player.name is already indexed by unique", "name"),
        ("index { btree Player { salary } }", "unknown index kind btree", "btree"),
        ("index { range Player { salary } }\nindex { text Player { name } }", "duplicate index block", "index"),
    ];
    for (blocks, message, marked) in cases {
        let source = schema(blocks);
        let error = Zega::in_memory()
            .build()
            .unwrap()
            .apply_zql(&source)
            .unwrap_err()
            .to_string();
        assert!(error.contains(message), "{blocks}: {error}");
        let report = zega::diagnose(&source, "");
        let diagnostic = report
            .diagnostics
            .first()
            .unwrap_or_else(|| panic!("{blocks}: no diagnostic"));
        assert_eq!(diagnostic.message, *message, "{blocks}");
        assert_eq!(diagnostic.pane, zega::Pane::Schema);
        let line = source.lines().nth(diagnostic.line as usize - 1).unwrap();
        let text: String = line
            .chars()
            .skip(diagnostic.column as usize - 1)
            .take((diagnostic.end_column - diagnostic.column) as usize)
            .collect();
        assert_eq!(text, *marked, "{blocks}");
    }
    // The same field may carry both kinds.
    let both = schema("index { range Player { name } text Player { name } }");
    Zega::in_memory().build().unwrap().apply_zql(&both).unwrap();
    assert!(zega::diagnose(&both, "").diagnostics.is_empty());
    // `run_lang` checks the block in its schema text too.
    let bad = schema("index { text Player { salary } }");
    let error = Zega::in_memory().build().unwrap().run_lang(&bad, "{ Player { n } }").unwrap_err();
    assert!(error.to_string().contains("text index needs a String field"), "{error}");
}
