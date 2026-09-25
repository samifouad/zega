//! `.graph` reader and writer against the engine's own graph: round trips,
//! one encoding per graph, and the frozen version 1 fixture.

use super::*;
use crate::graph::Graph;
use proptest::prelude::*;
use std::collections::HashMap;

const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/golden-v1.graph");

fn export(graph: &Graph) -> Vec<u8> {
    export_with(graph, &ExportOptions::default(), "zega test")
}

fn export_with(graph: &Graph, options: &ExportOptions, created_by: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    let summary = write(graph, options, created_by, &mut bytes).unwrap();
    assert_eq!(summary.bytes, bytes.len() as u64);
    bytes
}

/// Everything a graph is: its nodes and relationships with their ids, the
/// id counters, the declared indexes, and what its import carried. The label, property, spatial and
/// vector indexes are rebuilt from the nodes by the same `restore_*` calls
/// every other load path uses.
fn assert_same(a: &Graph, b: &Graph) {
    assert_eq!(a.all_nodes(), b.all_nodes());
    assert_eq!(a.all_relationships(), b.all_relationships());
    assert_eq!(a.next_ids(), b.next_ids());
    assert_eq!(a.declared_indexes(), b.declared_indexes());
    assert_eq!(a.carried(), b.carried());
}

fn props(entries: Vec<(&str, Value)>) -> HashMap<String, Value> {
    entries.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

fn vector(values: &[f32], metric: Metric) -> Value {
    Value::Vector(Box::new(Vector::new(values, metric).unwrap()))
}

/// Every `Value` variant, the float edge cases, unicode names, an empty
/// label list, gaps in both id sequences, a self-loop, id counters ahead of
/// the highest id, and declared indexes.
fn golden_graph() -> Graph {
    let mut graph = Graph::new();
    graph.restore_node(
        1,
        vec!["City".into()],
        props(vec![
            ("name", "Calgary".into()),
            ("population", Value::Int(1_306_784)),
            ("area", Value::from_f64(825.56)),
            ("capital", Value::Bool(false)),
            ("founded", Value::Null),
            ("location", Value::Point(Point::new(51.0447, -114.0719).unwrap())),
            ("embedding", vector(&[0.25, -1.5, 3.0], Metric::Cosine)),
            (
                "tags",
                Value::List(vec![
                    "yyc".into(),
                    Value::Int(403),
                    Value::Bool(true),
                    Value::Null,
                    Value::List(Box::default()),
                ].into()),
            ),
            (
                "extra",
                Value::Map(Box::new(props(vec![
                    ("k", Value::Int(1)),
                    ("nested", Value::List(vec![Value::Map(Box::default())].into())),
                    ("ünï", "Zürich ✈".into()),
                ]))),
            ),
            ("näme", "東京".into()),
        ]),
    );
    graph.restore_node(
        2,
        vec!["Airport".into(), "Place".into()],
        props(vec![
            ("iata", "YYC".into()),
            ("int_min", Value::Int(i64::MIN)),
            ("int_max", Value::Int(i64::MAX)),
            ("nan", Value::Float(0x7ff8_0000_dead_beef)),
            ("negative_zero", Value::Float((-0.0f64).to_bits())),
            ("infinity", Value::from_f64(f64::INFINITY)),
            ("subnormal", Value::Float(1)),
            ("dot", vector(&[1.0], Metric::Dot)),
            ("l2", vector(&[0.0, -2.5e-3], Metric::L2)),
            ("pole", Value::Point(Point::new(-90.0, 180.0).unwrap())),
            ("empty", "".into()),
        ]),
    );
    graph.restore_node(4, vec![], HashMap::new());
    graph.restore_relationship(
        1,
        "SERVES".into(),
        2,
        1,
        props(vec![("distance_km", Value::from_f64(16.5)), ("since", Value::Int(1914))]),
    );
    graph.restore_relationship(3, "NEAR".into(), 1, 1, HashMap::new());
    graph.reset_next_ids((7, 9));
    // As imported from a file with the golden schema and metadata.
    graph.sync_indexes(&golden_indexes());
    graph.set_carried(golden_carried());
    graph
}

const GOLDEN_SCHEMA: &str = "type City { name: String population: Int }\n\
                             unique { City { name } }\n\
                             index { range City { population } text City { name } }\n";

fn golden_meta() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("licence".into(), "CC0-1.0".into()),
        ("source".into(), "zega golden fixture".into()),
        ("title".into(), "golden v1".into()),
    ])
}

fn golden_carried() -> Carried {
    Carried {
        schema: Some(GOLDEN_SCHEMA.into()),
        uniques: vec![("City".into(), "name".into())],
        indexes: golden_indexes(),
        meta: golden_meta(),
    }
}

fn golden_indexes() -> Vec<IndexSpec> {
    let spec = |kind, field: &str| IndexSpec { kind, type_name: "City".into(), field: field.into() };
    vec![
        spec(IndexKind::Range, "name"),
        spec(IndexKind::Text, "name"),
        spec(IndexKind::Range, "population"),
    ]
}

/// Written once, for format version 1, and committed. Never regenerate it
/// for version 1: `golden_v1_*` exist to prove old files keep reading.
#[test]
#[ignore = "writes tests/fixtures/golden-v1.graph; run once per format version"]
fn write_golden_v1() {
    let bytes = export_with(&golden_graph(), &ExportOptions::default(), crate::CREATED_BY);
    std::fs::write(GOLDEN, bytes).unwrap();
}

#[test]
fn golden_v1_reads_back_as_the_graph_it_was_written_from() {
    let bytes = std::fs::read(GOLDEN).unwrap();
    let (graph, summary) = read(&bytes[..]).unwrap();
    assert_same(&graph, &golden_graph());
    assert_eq!(summary.format_version, 1);
    assert_eq!(summary.created_by, "zega 0.2.0");
    assert_eq!((summary.nodes, summary.relationships), (3, 2));
    assert_eq!(summary.schema.as_deref(), Some(GOLDEN_SCHEMA));
    assert_eq!(summary.meta, golden_meta());
    assert_eq!(summary.uniques, vec![("City".to_string(), "name".to_string())]);
    let declared: Vec<_> = golden_indexes()
        .into_iter()
        .map(|spec| IndexDeclaration { kind: spec.kind.as_str(), type_name: spec.type_name, field: spec.field })
        .collect();
    assert_eq!(summary.indexes, declared);
}

/// The version 1 writer still produces the fixture byte for byte. When the
/// format moves to version 2 this test goes (the writer writes 2), and the
/// read test above stays: old files must keep importing.
#[test]
fn golden_v1_is_what_the_writer_writes() {
    let bytes = std::fs::read(GOLDEN).unwrap();
    let (_, summary) = read(&bytes[..]).unwrap();
    let written = export_with(&golden_graph(), &ExportOptions::default(), &summary.created_by);
    assert_eq!(written, bytes);
    // And what the carried schema declares is what the engine derives from it.
    assert_eq!(
        declarations(GOLDEN_SCHEMA).unwrap(),
        (golden_carried().uniques, golden_carried().indexes)
    );
}

#[test]
fn the_content_digest_ignores_the_writer_and_metadata_but_not_the_id_counters() {
    let mut graph = golden_graph();
    let mut a = Vec::new();
    let mut b = Vec::new();
    let first = write(&graph, &ExportOptions::default(), "zega 0.2.0", &mut a).unwrap();
    let meta = ExportOptions { meta: BTreeMap::from([("title".into(), "x".into())]), ..Default::default() };
    let second = write(&graph, &meta, "zega 9.9.9", &mut b).unwrap();
    assert_ne!(a, b);
    assert_eq!(first.content_sha256, second.content_sha256);
    // The id counters are graph state: a graph that will give its next node
    // another id is another graph.
    graph.reset_next_ids((8, 9));
    let third = write(&graph, &ExportOptions::default(), "zega 0.2.0", &mut Vec::new()).unwrap();
    assert_ne!(first.content_sha256, third.content_sha256);
    graph.reset_next_ids((7, 10));
    let fourth = write(&graph, &ExportOptions::default(), "zega 0.2.0", &mut Vec::new()).unwrap();
    assert_ne!(first.content_sha256, fourth.content_sha256);
}

/// Declared indexes follow the schema a query last ran with, and a restart
/// forgets them: they are session state, so they do not change the bytes.
/// A file declares what its schema declares, and an import carries it on.
#[test]
fn declarations_come_from_the_carried_schema_not_the_session() {
    let mut graph = golden_graph();
    let bytes = export(&graph);
    graph.sync_indexes(&[]);
    assert_eq!(export(&graph), bytes);
    let (imported, summary) = read(&bytes[..]).unwrap();
    assert_eq!(summary.schema.as_deref(), Some(GOLDEN_SCHEMA));
    assert_eq!(imported.declared_indexes(), golden_indexes());
    assert_eq!(export(&imported), bytes, "import then export gives the same file");

    // A schema given at export replaces the carried one; metadata merges.
    let options = ExportOptions {
        schema: Some("type City { name: String }\nindex { text City { name } }\n".into()),
        meta: BTreeMap::from([("title".into(), "renamed".into())]),
    };
    let (imported, summary) = read(&export_with(&graph, &options, "t")[..]).unwrap();
    assert_eq!(summary.uniques, Vec::<(String, String)>::new());
    assert_eq!(imported.declared_indexes(), vec![IndexSpec { kind: IndexKind::Text, type_name: "City".into(), field: "name".into() }]);
    assert_eq!(summary.meta["title"], "renamed");
    assert_eq!(summary.meta["licence"], "CC0-1.0");

    let bad = ExportOptions { schema: Some("type {".into()), ..Default::default() };
    let error = write(&graph, &bad, "t", &mut Vec::new()).unwrap_err();
    assert!(error.to_string().starts_with("cannot export this graph as .graph: the schema does not parse"), "{error}");
}

#[test]
fn declarations_without_schema_text_are_refused() {
    let bytes = export(&golden_graph());
    let tampered = rewrite_section(&bytes, Section::Schema, |payload| {
        // Drop the source (flag 1, u32 length, text), keep the declarations.
        let len = u32::from_le_bytes(payload[1..5].try_into().unwrap()) as usize;
        [&[0u8][..], &payload[5 + len..]].concat()
    });
    let error = read(&tampered[..]).map(|_| ()).unwrap_err();
    assert!(matches!(error, Error::Invalid { ref reason, .. } if reason.contains("need the schema text")), "{error}");
}

#[test]
fn a_label_twice_on_one_node_is_refused_on_both_sides() {
    let mut graph = Graph::new();
    graph.restore_node(1, vec!["A".into(), "B".into(), "A".into()], HashMap::new());
    let error = write(&graph, &ExportOptions::default(), "t", &mut Vec::new()).unwrap_err();
    assert!(error.to_string().contains("label \"A\" twice"), "{error}");
    let mut graph = Graph::new();
    graph.restore_node(1, vec!["A".into(), "B".into()], HashMap::new());
    // B stays in use, so only the repeat can be what is refused.
    graph.restore_node(2, vec!["B".into()], HashMap::new());
    let tampered = rewrite_section(&export(&graph), Section::Nodes, |payload| {
        // next id, node id, then 2 labels: A (0), B (1) becomes A, A.
        let mut out = payload.to_vec();
        out[24..28].copy_from_slice(&0u32.to_le_bytes());
        out
    });
    let error = read(&tampered[..]).map(|_| ()).unwrap_err();
    assert!(error.to_string().contains("label \"A\" twice"), "{error}");
}

#[test]
fn a_relationship_to_a_missing_node_is_refused_by_the_writer() {
    let mut graph = Graph::new();
    graph.restore_node(1, vec!["A".into()], HashMap::new());
    graph.restore_relationship(1, "R".into(), 1, 2, HashMap::new());
    let error = write(&graph, &ExportOptions::default(), "t", &mut Vec::new()).unwrap_err();
    assert!(matches!(error, Error::Unexportable(ref m) if m.contains("node 2")), "{error}");
}

#[test]
fn a_name_nothing_uses_is_refused() {
    // Hand-build a file whose dictionary has an extra name: the only way to
    // get one, since the writer never emits it.
    let mut graph = Graph::new();
    graph.restore_node(1, vec!["A".into()], HashMap::new());
    let bytes = export(&graph);
    let tampered = rewrite_section(&bytes, Section::Names, |_| {
        let mut payload = 2u32.to_le_bytes().to_vec();
        for name in ["A", "B"] {
            payload.extend((name.len() as u32).to_le_bytes());
            payload.extend(name.as_bytes());
        }
        payload
    });
    let error = read(&tampered[..]).map(|_| ()).unwrap_err();
    assert!(matches!(error, Error::Invalid { ref reason, .. } if reason.contains("\"B\" is never used")), "{error}");
}

#[test]
fn unsorted_property_keys_are_refused() {
    let mut graph = Graph::new();
    graph.restore_node(1, vec![], props(vec![("a", Value::Null), ("b", Value::Null)]));
    let bytes = export(&graph);
    let tampered = rewrite_section(&bytes, Section::Nodes, |payload| {
        // next id, id, 0 labels, 2 props: (key 1, null) then (key 0, null).
        let mut out = payload[..24].to_vec();
        out.extend(1u32.to_le_bytes());
        out.push(tag::NULL);
        out.extend(0u32.to_le_bytes());
        out.push(tag::NULL);
        out
    });
    let error = read(&tampered[..]).map(|_| ()).unwrap_err();
    assert!(matches!(error, Error::Invalid { ref reason, .. } if reason.contains("ascending")), "{error}");
}

/// Replace one section's payload, fixing its length, checksum and the
/// content digest, so only the payload's own rules can reject it.
fn rewrite_section(bytes: &[u8], target: Section, edit: impl Fn(&[u8]) -> Vec<u8>) -> Vec<u8> {
    let mut out = bytes[..12].to_vec();
    let mut at = 12;
    let mut content: Option<Sha256> = None;
    for section in [
        Section::Manifest,
        Section::Names,
        Section::Schema,
        Section::Nodes,
        Section::Relationships,
        Section::Done,
    ] {
        let len = u64::from_le_bytes(bytes[at + 4..at + 12].try_into().unwrap()) as usize;
        let payload = &bytes[at + 12..at + 12 + len];
        let payload = if section == target {
            edit(payload)
        } else if section == Section::Done {
            content.take().unwrap().finalize().to_vec()
        } else {
            payload.to_vec()
        };
        let mut framed = section.tag().to_vec();
        framed.extend((payload.len() as u64).to_le_bytes());
        framed.extend(&payload);
        framed.extend(crc32fast::hash(&payload).to_le_bytes());
        if let Some(content) = &mut content {
            content.update(&framed);
        }
        if section == Section::Manifest {
            content = Some(Sha256::new());
        }
        out.extend(framed);
        at += 12 + len + 4;
    }
    out
}

// ---------------------------------------------------------------------------
// Random graphs

fn name() -> impl Strategy<Value = String> {
    prop::sample::select(vec!["City", "Airport", "näme", "a", "b", "ROUTE", "ÿ", "", " x", "東京"])
        .prop_map(String::from)
}

fn value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::Int),
        // Any bits: NaN payloads, infinities and subnormals included.
        any::<u64>().prop_map(Value::Float),
        prop_oneof![
            Just(Value::Float((-0.0f64).to_bits())),
            Just(Value::Float(f64::NAN.to_bits())),
            Just(Value::from_f64(f64::NEG_INFINITY)),
        ],
        ".{0,12}".prop_map(Value::from),
        (-90.0f64..=90.0, -180.0f64..=180.0)
            .prop_map(|(lat, lon)| Value::Point(Point::new(lat, lon).unwrap())),
        (
            prop::collection::vec(-1e6f32..1e6, 1..6),
            prop::sample::select(vec![Metric::Cosine, Metric::Dot, Metric::L2]),
        )
            .prop_map(|(values, metric)| vector(&values, metric)),
    ];
    leaf.prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(|items| Value::List(items.into())),
            prop::collection::hash_map(".{0,6}", inner, 0..4).prop_map(|map| Value::Map(Box::new(map))),
        ]
    })
}

fn properties() -> impl Strategy<Value = HashMap<String, Value>> {
    prop::collection::hash_map(name(), value(), 0..5)
}

/// Mostly dense ids, sometimes a jump far enough to take the sorted path.
fn id_gap() -> impl Strategy<Value = u64> {
    prop_oneof![8 => 1u64..4, 1 => Just(1u64 << 40)]
}

/// `(id, labels, props)`.
type NodeSpec = (u64, Vec<String>, HashMap<String, Value>);
/// `(id, kind, index of from node, index of to node, props)`.
type RelSpec = (u64, String, usize, usize, HashMap<String, Value>);

#[derive(Clone, Debug)]
struct GraphSpec {
    nodes: Vec<NodeSpec>,
    rels: Vec<RelSpec>,
    next_ahead: (u64, u64),
}

fn graph_spec() -> impl Strategy<Value = GraphSpec> {
    // Labels in any order, each at most once per node.
    let labels = prop::collection::vec(name(), 0..3).prop_map(|labels| {
        let mut seen = std::collections::HashSet::new();
        labels.into_iter().filter(|label| seen.insert(label.clone())).collect::<Vec<_>>()
    });
    let node = (id_gap(), labels, properties());
    prop::collection::vec(node, 0..16).prop_flat_map(|nodes| {
        let n = nodes.len().max(1);
        let rel = (id_gap(), name(), 0..n, 0..n, properties());
        let rels = prop::collection::vec(rel, 0..if nodes.is_empty() { 1 } else { 24 });
        (Just(nodes), rels, (0u64..5, 0u64..5))
            .prop_map(|(nodes, rels, next_ahead)| {
                let mut id = 0;
                let nodes = nodes
                    .into_iter()
                    .map(|(gap, labels, props)| {
                        id += gap;
                        (id, labels, props)
                    })
                    .collect();
                let mut id = 0;
                let rels = rels
                    .into_iter()
                    .map(|(gap, kind, from, to, props)| {
                        id += gap;
                        (id, kind, from, to, props)
                    })
                    .collect();
                GraphSpec { nodes, rels, next_ahead }
            })
    })
}

/// Build the graph, inserting in forward or reverse order: the file must
/// not depend on insertion order (or on any `HashMap`'s iteration order).
fn build(spec: &GraphSpec, reverse: bool) -> Graph {
    let mut graph = Graph::new();
    let mut nodes: Vec<_> = spec.nodes.iter().collect();
    let mut rels: Vec<_> = spec.rels.iter().collect();
    if reverse {
        nodes.reverse();
        rels.reverse();
    }
    for (id, labels, props) in nodes {
        graph.restore_node(*id, labels.clone(), props.clone());
    }
    for (id, kind, from, to, props) in rels {
        let (from, to) = (spec.nodes[*from].0, spec.nodes[*to].0);
        graph.restore_relationship(*id, kind.clone(), from, to, props.clone());
    }
    let (node, rel) = graph.next_ids();
    graph.reset_next_ids((node + spec.next_ahead.0, rel + spec.next_ahead.1));
    graph
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// import(export(g)) == g, and one graph has one encoding: the same
    /// bytes whatever order it was built in, and an imported file exports
    /// back to itself.
    #[test]
    fn round_trip_is_exact_and_deterministic(spec in graph_spec()) {
        let graph = build(&spec, false);
        let bytes = export(&graph);
        let (imported, summary) = read(&bytes[..]).unwrap();
        assert_same(&graph, &imported);
        prop_assert_eq!(summary.nodes, spec.nodes.len() as u64);
        prop_assert_eq!(summary.relationships, spec.rels.len() as u64);
        prop_assert_eq!(&export(&imported), &bytes);
        prop_assert_eq!(&export(&build(&spec, true)), &bytes);
    }

    /// Every strict prefix of a file is refused as truncated.
    #[test]
    fn every_prefix_is_truncated(spec in graph_spec(), cut in any::<prop::sample::Index>()) {
        let bytes = export(&build(&spec, false));
        let cut = cut.index(bytes.len());
        let error = read(&bytes[..cut]).map(|_| ()).unwrap_err();
        prop_assert!(matches!(error, Error::Truncated { .. }), "cut at {}: {}", cut, error);
    }

    /// Changing any one byte is refused.
    #[test]
    fn every_single_byte_change_is_refused(
        spec in graph_spec(),
        at in any::<prop::sample::Index>(),
        mask in 1u8..=255,
    ) {
        let mut bytes = export(&build(&spec, false));
        let at = at.index(bytes.len());
        bytes[at] ^= mask;
        prop_assert!(read(&bytes[..]).is_err(), "byte {} ^ {:#04x} was accepted", at, mask);
    }
}
