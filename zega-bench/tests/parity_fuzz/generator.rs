use std::fmt::Write;

#[derive(Clone, Debug, PartialEq)]
pub struct GeneratedGraph {
    pub nodes: Vec<Node>,
    pub relationships: Vec<Relationship>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: usize,
    pub label: &'static str,
    pub score: i64,
    pub ratio: f64,
    pub name: String,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Relationship {
    pub from: usize,
    pub to: usize,
    pub kind: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColumnKind {
    String,
    Integer,
    Float,
    Boolean,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub property: &'static str,
    pub alias: &'static str,
    pub kind: ColumnKind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GeneratedQuery {
    pub label: Option<&'static str>,
    pub predicates: Vec<String>,
    pub connective: &'static str,
    pub columns: Vec<Column>,
    pub order: Option<(usize, bool)>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn usize(&mut self, upper: usize) -> usize {
        (self.next() as usize) % upper
    }

    fn bool(&mut self) -> bool {
        self.next() & 1 == 1
    }
}

pub fn generate_case(rng: &mut Rng) -> (GeneratedGraph, GeneratedQuery) {
    const LABELS: [&str; 3] = ["FuzzPerson", "FuzzOrder", "FuzzProduct"];
    const RELS: [&str; 3] = ["KNOWS", "PLACED", "CONTAINS"];
    let node_count = 1 + rng.usize(50);
    let nodes = (0..node_count)
        .map(|id| Node {
            id,
            label: LABELS[rng.usize(LABELS.len())],
            score: rng.usize(41) as i64,
            ratio: rng.usize(401) as f64 / 10.0,
            name: format!("value_{}", rng.usize(12)),
            active: rng.bool(),
        })
        .collect();
    let relationships = (0..rng.usize(81))
        .map(|_| Relationship {
            from: rng.usize(node_count),
            to: rng.usize(node_count),
            kind: RELS[rng.usize(RELS.len())],
        })
        .collect();
    let graph = GeneratedGraph {
        nodes,
        relationships,
    };
    (graph, generate_query(rng))
}

fn generate_query(rng: &mut Rng) -> GeneratedQuery {
    const OPS: [&str; 5] = ["=", "<", ">", "<=", ">="];
    const ALL_COLUMNS: [Column; 5] = [
        Column {
            property: "uid",
            alias: "id",
            kind: ColumnKind::String,
        },
        Column {
            property: "score",
            alias: "score",
            kind: ColumnKind::Integer,
        },
        Column {
            property: "ratio",
            alias: "ratio",
            kind: ColumnKind::Float,
        },
        Column {
            property: "name",
            alias: "name",
            kind: ColumnKind::String,
        },
        Column {
            property: "active",
            alias: "active",
            kind: ColumnKind::Boolean,
        },
    ];
    let predicate_count = rng.usize(4);
    let predicates = (0..predicate_count)
        .map(|_| match rng.usize(4) {
            0 => format!("n.score {} {}", OPS[rng.usize(OPS.len())], rng.usize(41)),
            1 => format!(
                "n.ratio {} {:.1}",
                OPS[rng.usize(OPS.len())],
                rng.usize(401) as f64 / 10.0
            ),
            2 => format!("n.name = 'value_{}'", rng.usize(12)),
            _ => format!("n.active = {}", rng.bool()),
        })
        .collect();
    let column_count = 1 + rng.usize(ALL_COLUMNS.len());
    let columns = ALL_COLUMNS[..column_count].to_vec();
    let order = rng.bool().then(|| (0, rng.bool()));
    let limit = order.is_some().then(|| 1 + rng.usize(20));
    GeneratedQuery {
        label: rng
            .bool()
            .then(|| ["FuzzPerson", "FuzzOrder", "FuzzProduct"][rng.usize(3)]),
        predicates,
        connective: if rng.bool() { "AND" } else { "OR" },
        columns,
        order,
        limit,
    }
}

impl GeneratedGraph {
    pub fn creates(&self, run: &str) -> Vec<String> {
        let mut creates: Vec<_> = self.nodes.iter().map(|node| {
            format!(
                "CREATE (n:{} {{uid:'{}-{}', parity_run:'{}', score:{}, ratio:{:.1}, name:'{}', active:{}}})",
                node.label, run, node.id, run, node.score, node.ratio, node.name, node.active
            )
        }).collect();
        creates.extend(self.relationships.iter().map(|rel| {
            format!(
                "MATCH (a {{uid:'{}-{}'}}) MATCH (b {{uid:'{}-{}'}}) CREATE (a)-[:{}]->(b)",
                run, rel.from, run, rel.to, rel.kind
            )
        }));
        creates
    }
}

impl GeneratedQuery {
    pub fn cypher(&self, run: &str) -> String {
        let mut result = format!(
            "MATCH (n{}) WHERE n.parity_run = '{}'",
            self.label
                .map(|label| format!(":{label}"))
                .unwrap_or_default(),
            run
        );
        if !self.predicates.is_empty() {
            write!(
                result,
                " AND ({})",
                self.predicates.join(&format!(" {} ", self.connective))
            )
            .unwrap();
        }
        result.push_str(" RETURN ");
        result.push_str(
            &self
                .columns
                .iter()
                .map(|column| format!("n.{} AS {}", column.property, column.alias))
                .collect::<Vec<_>>()
                .join(", "),
        );
        if let Some((index, desc)) = self.order {
            write!(
                result,
                " ORDER BY {} {}",
                self.columns[index].alias,
                if desc { "DESC" } else { "ASC" }
            )
            .unwrap();
        }
        if let Some(limit) = self.limit {
            write!(result, " LIMIT {limit}").unwrap();
        }
        result
    }
}

// TODO(V2): add Cypher's <> spelling, NOT, and SKIP after Zega's parser
// supports them, plus aggregation and variable-length traversal generation.
