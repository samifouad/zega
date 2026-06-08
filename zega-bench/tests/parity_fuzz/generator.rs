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
    pub group: Option<&'static str>,
    pub sort: SortValue,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SortValue {
    Null,
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
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
    Dynamic,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub expression: &'static str,
    pub alias: &'static str,
    pub kind: ColumnKind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OrderKey {
    pub expression: &'static str,
    pub desc: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum QueryShape {
    Scan,
    Aggregate,
    Traversal {
        kind: &'static str,
        length: TraversalLength,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TraversalLength {
    Unbounded,
    Bounded(usize, usize),
    UpTo(usize),
    AtLeast(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct GeneratedQuery {
    pub shape: QueryShape,
    pub label: Option<&'static str>,
    pub predicates: Vec<String>,
    pub connective: &'static str,
    pub columns: Vec<Column>,
    pub order: Vec<OrderKey>,
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
    const GROUPS: [Option<&str>; 4] = [None, Some("a"), Some("b"), Some("c")];
    let node_count = 5 + rng.usize(46);
    let nodes = (0..node_count)
        .map(|id| Node {
            id,
            label: LABELS[rng.usize(LABELS.len())],
            score: rng.usize(41) as i64,
            ratio: rng.usize(401) as f64 / 10.0,
            name: format!("value_{}", rng.usize(12)),
            active: rng.bool(),
            group: GROUPS[id % GROUPS.len()],
            sort: match id % 5 {
                0 => SortValue::Null,
                1 => SortValue::String(format!("sort_{}", rng.usize(8))),
                2 => SortValue::Integer(rng.usize(15) as i64),
                3 => SortValue::Float(rng.usize(151) as f64 / 10.0),
                _ => SortValue::Boolean(rng.bool()),
            },
        })
        .collect();
    let relationships = (0..rng.usize(81))
        .map(|_| {
            let from = rng.usize(node_count - 1);
            Relationship {
                from,
                to: from + 1 + rng.usize(node_count - from - 1),
                kind: RELS[rng.usize(RELS.len())],
            }
        })
        .collect();
    let graph = GeneratedGraph {
        nodes,
        relationships,
    };
    let query = match rng.usize(5) {
        0 | 1 | 2 => generate_ordered_scan(rng),
        3 => generate_aggregate(rng),
        _ => generate_traversal(rng),
    };
    (graph, query)
}

fn generate_ordered_scan(rng: &mut Rng) -> GeneratedQuery {
    let mut columns = vec![
        column("n.uid", "id", ColumnKind::String),
        column("n.sort", "sort_key", ColumnKind::Dynamic),
    ];
    if rng.bool() {
        columns.push(column("n.group", "group_key", ColumnKind::String));
    }
    if rng.bool() {
        columns.push(column("n.score", "score", ColumnKind::Integer));
    }

    let alias_order = rng.bool();
    let primary = if alias_order { "sort_key" } else { "n.sort" };
    let mut order = vec![OrderKey {
        expression: primary,
        desc: rng.bool(),
    }];
    if rng.bool() {
        order.push(OrderKey {
            expression: if alias_order { "group_key" } else { "n.group" },
            desc: !order[0].desc,
        });
        if !columns.iter().any(|column| column.alias == "group_key") {
            columns.push(column("n.group", "group_key", ColumnKind::String));
        }
    }
    order.push(OrderKey {
        expression: if alias_order { "id" } else { "n.uid" },
        desc: !order.last().unwrap().desc,
    });

    GeneratedQuery {
        shape: QueryShape::Scan,
        label: random_label(rng),
        predicates: generate_predicates(rng),
        connective: if rng.bool() { "AND" } else { "OR" },
        columns,
        order,
        limit: rng.bool().then(|| 1 + rng.usize(20)),
    }
}

fn generate_aggregate(rng: &mut Rng) -> GeneratedQuery {
    let grouped = rng.bool();
    let mut columns = Vec::new();
    if grouped {
        columns.push(column("n.group", "group_key", ColumnKind::String));
    }
    let aggregate = match rng.usize(7) {
        0 => column("count(*)", "aggregate", ColumnKind::Integer),
        1 => column("count(n.sort)", "aggregate", ColumnKind::Integer),
        2 => column("sum(n.score)", "aggregate", ColumnKind::Integer),
        3 => column("sum(n.ratio)", "aggregate", ColumnKind::Float),
        4 => column("avg(n.score)", "aggregate", ColumnKind::Float),
        5 => column("min(n.score)", "aggregate", ColumnKind::Integer),
        _ => column("max(n.ratio)", "aggregate", ColumnKind::Float),
    };
    columns.push(aggregate);
    let order = rng
        .bool()
        .then(|| {
            let desc = rng.bool();
            let mut keys = vec![OrderKey {
                expression: if rng.bool() {
                    "aggregate"
                } else {
                    columns.last().unwrap().expression
                },
                desc,
            }];
            if grouped {
                keys.push(OrderKey {
                    expression: "group_key",
                    desc: !desc,
                });
            }
            keys
        })
        .unwrap_or_default();
    GeneratedQuery {
        shape: QueryShape::Aggregate,
        label: random_label(rng),
        predicates: generate_predicates(rng),
        connective: if rng.bool() { "AND" } else { "OR" },
        columns,
        order,
        limit: None,
    }
}

fn generate_traversal(rng: &mut Rng) -> GeneratedQuery {
    let length = match rng.usize(4) {
        0 => TraversalLength::Unbounded,
        1 => {
            let min = 1 + rng.usize(2);
            TraversalLength::Bounded(min, min + rng.usize(3))
        }
        2 => TraversalLength::UpTo(1 + rng.usize(4)),
        _ => TraversalLength::AtLeast(1 + rng.usize(3)),
    };
    GeneratedQuery {
        shape: QueryShape::Traversal {
            kind: ["KNOWS", "PLACED", "CONTAINS"][rng.usize(3)],
            length,
        },
        label: None,
        predicates: Vec::new(),
        connective: "AND",
        columns: vec![
            column("a.uid", "start_id", ColumnKind::String),
            column("b.uid", "endpoint_id", ColumnKind::String),
        ],
        order: Vec::new(),
        limit: None,
    }
}

fn column(expression: &'static str, alias: &'static str, kind: ColumnKind) -> Column {
    Column {
        expression,
        alias,
        kind,
    }
}

fn random_label(rng: &mut Rng) -> Option<&'static str> {
    rng.bool()
        .then(|| ["FuzzPerson", "FuzzOrder", "FuzzProduct"][rng.usize(3)])
}

fn generate_predicates(rng: &mut Rng) -> Vec<String> {
    const OPS: [&str; 5] = ["=", "<", ">", "<=", ">="];
    (0..rng.usize(4))
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
        .collect()
}

impl GeneratedGraph {
    pub fn creates(&self, run: &str) -> Vec<String> {
        let mut creates: Vec<_> = self
            .nodes
            .iter()
            .map(|node| {
                let mut properties = format!(
                    "uid:'{}-{}', parity_run:'{}', score:{}, ratio:{:.1}, name:'{}', active:{}",
                    run, node.id, run, node.score, node.ratio, node.name, node.active
                );
                if let Some(group) = node.group {
                    write!(properties, ", group:'{group}'").unwrap();
                }
                match &node.sort {
                    SortValue::Null => {}
                    SortValue::String(value) => write!(properties, ", sort:'{value}'").unwrap(),
                    SortValue::Integer(value) => write!(properties, ", sort:{value}").unwrap(),
                    SortValue::Float(value) => write!(properties, ", sort:{value:.1}").unwrap(),
                    SortValue::Boolean(value) => write!(properties, ", sort:{value}").unwrap(),
                }
                format!("CREATE (n:{} {{{properties}}})", node.label)
            })
            .collect();
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
        let mut result = match &self.shape {
            QueryShape::Traversal { kind, length } => format!(
                "MATCH (a {{uid:'{run}-0'}})-[:{kind}{}]->(b) WHERE b.parity_run = '{run}'",
                length.cypher()
            ),
            QueryShape::Scan | QueryShape::Aggregate => format!(
                "MATCH (n{}) WHERE n.parity_run = '{}'",
                self.label
                    .map(|label| format!(":{label}"))
                    .unwrap_or_default(),
                run
            ),
        };
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
                .map(|column| format!("{} AS {}", column.expression, column.alias))
                .collect::<Vec<_>>()
                .join(", "),
        );
        if !self.order.is_empty() {
            result.push_str(" ORDER BY ");
            result.push_str(
                &self
                    .order
                    .iter()
                    .map(|key| {
                        format!(
                            "{} {}",
                            key.expression,
                            if key.desc { "DESC" } else { "ASC" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        if let Some(limit) = self.limit {
            write!(result, " LIMIT {limit}").unwrap();
        }
        result
    }

    pub fn is_ordered(&self) -> bool {
        !self.order.is_empty()
    }
}

impl TraversalLength {
    fn cypher(self) -> String {
        match self {
            Self::Unbounded => "*".into(),
            Self::Bounded(min, max) => format!("*{min}..{max}"),
            Self::UpTo(max) => format!("*..{max}"),
            Self::AtLeast(min) => format!("*{min}.."),
        }
    }
}

// TODO(V3): add Cypher's <> spelling, NOT, and SKIP after Zega's parser supports them.
