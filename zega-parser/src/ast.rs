use crate::value::Value;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Parameter(String),                       // $name
    Literal(Value),                          // literal value
    Identifier(String),                      // ident
    PropertyAccess(Box<Expr>, String),       // expr.prop
    BinaryOp(Box<Expr>, BinaryOperator, Box<Expr>),
    Aggregate {
        function: AggregateFunction,
        argument: Option<Box<Expr>>, // None represents count(*)
        distinct: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum AggregateFunction {
    Count,
    Sum,
    Avg,
    Min,
    Max,
    Collect,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BinaryOperator {
    Eq,
    Ne,
    Gt,
    Lt,
    Gte,
    Lte,
    And,
    Or,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Statement {
    Match {
        pattern: Vec<PatternElement>,
        where_clause: Option<Expr>,
        return_clause: ReturnClause,
        order_by: Option<Vec<(Expr, OrderDirection)>>,
        limit: Option<Expr>,
    },
    Create {
        pattern: Vec<PatternElement>,
    },
    MatchCreate {
        match_pattern: Vec<PatternElement>,
        where_clause: Option<Expr>,
        create_pattern: Vec<PatternElement>,
    },
    Merge {
        pattern: Vec<PatternElement>,
        on_create: Vec<SetClause>,
    },
    Set {
        assignments: Vec<SetClause>,
    },
    Delete {
        identifiers: Vec<String>,
    },
    KvGet {
        key: Expr,
    },
    KvSet {
        key: Expr,
        value: Expr,
        ttl: Option<Expr>,
    },
    KvDel {
        key: Expr,
    },
    KvIncr {
        key: Expr,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct PatternElement {
    pub variable: String,
    pub labels: Vec<String>,
    pub properties: HashMap<String, Expr>,
    pub relationship: Option<RelationshipPattern>,
    pub direction: Option<Direction>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RelationshipPattern {
    pub variable: String,
    pub kinds: Vec<String>,
    pub properties: HashMap<String, Expr>,
    pub length: Option<RelationshipLength>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RelationshipLength {
    pub min: usize,
    pub max: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Direction {
    Outgoing,
    Incoming,
    Both,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SetClause {
    pub target: Expr,    // PropertyAccess or Identifier
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReturnClause {
    pub items: Vec<ReturnItem>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReturnItem {
    pub expr: Expr,
    pub alias: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum OrderDirection {
    Asc,
    Desc,
}
