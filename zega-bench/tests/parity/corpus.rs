#[derive(Clone, Copy)]
pub enum ColumnKind {
    String,
    Integer,
    Float,
}

pub struct GraphOp {
    pub name: &'static str,
    pub query: String,
    pub columns: &'static [(&'static str, ColumnKind)],
    pub ordered: bool,
}

const STRING: ColumnKind = ColumnKind::String;
const INTEGER: ColumnKind = ColumnKind::Integer;
const FLOAT: ColumnKind = ColumnKind::Float;

pub fn fixture(run: &str) -> Vec<String> {
    let nodes = [
        ("ParityUser", "u1", "alpha", 0),
        ("ParityUser", "u2", "beta", 0),
        ("ParityOrder", "o1", "a", 20),
        ("ParityOrder", "o2", "a", 30),
        ("ParityOrder", "o3", "b", 10),
        ("ParityProduct", "p1", "one", 0),
        ("ParityProduct", "p2", "two", 0),
        ("ParityProduct", "p3", "three", 0),
        ("ParityCategory", "c0", "root", 0),
        ("ParityCategory", "c1", "child", 0),
        ("ParityCategory", "c2", "grandchild", 0),
        ("ParityCategory", "c3", "great-grandchild", 0),
    ];
    let mut statements: Vec<String> = nodes
        .into_iter()
        .map(|(label, id, group, amount)| {
            format!(
                "CREATE (n:{label} {{id:'{run}-{id}', parity_run:'{run}', group:'{group}', amount:{amount}}})"
            )
        })
        .collect();
    let relationships = [
        ("ParityUser", "u1", "PLACED", "ParityOrder", "o1"),
        ("ParityUser", "u1", "PLACED", "ParityOrder", "o2"),
        ("ParityUser", "u2", "PLACED", "ParityOrder", "o3"),
        ("ParityOrder", "o1", "CONTAINS", "ParityProduct", "p1"),
        ("ParityOrder", "o1", "CONTAINS", "ParityProduct", "p2"),
        ("ParityOrder", "o2", "CONTAINS", "ParityProduct", "p1"),
        ("ParityOrder", "o2", "CONTAINS", "ParityProduct", "p3"),
        ("ParityCategory", "c0", "REL", "ParityCategory", "c1"),
        ("ParityCategory", "c1", "REL", "ParityCategory", "c2"),
        ("ParityCategory", "c2", "REL", "ParityCategory", "c3"),
    ];
    statements.extend(relationships.into_iter().map(|(a_label, a, rel, b_label, b)| {
        format!(
            "MATCH (a:{a_label} {{id:'{run}-{a}'}}) MATCH (b:{b_label} {{id:'{run}-{b}'}}) CREATE (a)-[:{rel}]->(b)"
        )
    }));
    statements
}

pub fn graph_ops(run: &str) -> Vec<GraphOp> {
    vec![
        GraphOp {
            name: "graph.node_create",
            query: format!("CREATE (n:ParityProbe {{id:'{run}-probe', parity_run:'{run}'}})"),
            columns: &[],
            ordered: false,
        },
        GraphOp {
            name: "graph.create_then_return",
            query: format!(
                "CREATE (n:ParityProbe {{id:'{run}-returned', parity_run:'{run}'}}) RETURN n.id AS id"
            ),
            columns: &[("id", STRING)],
            ordered: false,
        },
        GraphOp {
            name: "graph.match_by_property",
            query: format!(
                "MATCH (n:ParityProbe {{id:'{run}-probe'}}) RETURN n.id AS id"
            ),
            columns: &[("id", STRING)],
            ordered: false,
        },
        GraphOp {
            name: "graph.where_filter",
            query: format!(
                "MATCH (o:ParityOrder) WHERE o.parity_run = '{run}' AND o.amount > 15 RETURN o.id AS id, o.amount AS amount"
            ),
            columns: &[("id", STRING), ("amount", INTEGER)],
            ordered: false,
        },
        GraphOp {
            name: "graph.projection_alias",
            query: format!(
                "MATCH (u:ParityUser {{id:'{run}-u1'}}) RETURN u.id AS customer"
            ),
            columns: &[("customer", STRING)],
            ordered: false,
        },
        GraphOp {
            name: "graph.order_by_limit",
            query: format!(
                "MATCH (o:ParityOrder) WHERE o.parity_run = '{run}' RETURN o.id AS id, o.amount AS amount ORDER BY amount DESC LIMIT 2"
            ),
            columns: &[("id", STRING), ("amount", INTEGER)],
            ordered: true,
        },
        GraphOp {
            name: "graph.aggregate_group",
            query: format!(
                "MATCH (o:ParityOrder) WHERE o.parity_run = '{run}' RETURN o.group AS group, count(o) AS count, sum(o.amount) AS total, avg(o.amount) AS average"
            ),
            columns: &[("group", STRING), ("count", INTEGER), ("total", INTEGER), ("average", FLOAT)],
            ordered: false,
        },
        GraphOp {
            name: "graph.variable_length_1_3",
            query: format!(
                "MATCH (c:ParityCategory {{id:'{run}-c0'}})-[:REL*1..3]->(sub:ParityCategory) RETURN sub.id AS id"
            ),
            columns: &[("id", STRING)],
            ordered: false,
        },
        GraphOp {
            name: "graph.two_hop_copurchase",
            query: format!(
                "MATCH (p:ParityProduct {{id:'{run}-p2'}})<-[:CONTAINS]-(o:ParityOrder)-[:CONTAINS]->(rec:ParityProduct) RETURN rec.id AS id, count(*) AS frequency"
            ),
            columns: &[("id", STRING), ("frequency", INTEGER)],
            ordered: false,
        },
    ]
}
