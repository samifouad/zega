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
    FunctionCall {
        name: String, // lowercased scalar function name (toString, coalesce, datetime, ...)
        args: Vec<Expr>,
    },
    /// `expr IS NULL` / `expr IS NOT NULL`.
    IsNull {
        operand: Box<Expr>,
        negated: bool,
    },
    /// `CASE [subject] WHEN cond THEN result ... [ELSE result] END`.
    /// With `subject`, each `cond` is compared for equality to it; without,
    /// each `cond` is evaluated as a boolean predicate.
    Case {
        subject: Option<Box<Expr>>,
        branches: Vec<(Expr, Expr)>,
        default: Option<Box<Expr>>,
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
    Add,
    Sub,
    Mul,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Statement {
    Match {
        pattern: Vec<PatternElement>,
        /// Each `OPTIONAL MATCH` segment, in order. Left-outer-joined onto the
        /// required `pattern`: a segment that does not match leaves its
        /// variables unbound (→ null) rather than dropping the row.
        optional_patterns: Vec<Vec<PatternElement>>,
        where_clause: Option<Expr>,
        /// An optional `WITH ...` projection/aggregation boundary between the
        /// match and the RETURN. Its items become the variable scope the
        /// RETURN sees.
        with_clause: Option<WithClause>,
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
    /// `MATCH ... [WHERE ...] SET ...` — update matched nodes' properties.
    MatchSet {
        match_pattern: Vec<PatternElement>,
        where_clause: Option<Expr>,
        assignments: Vec<SetClause>,
    },
    Delete {
        identifiers: Vec<String>,
    },
    /// `MATCH ... [WHERE ...] [DETACH] DELETE var[, ...]` — delete matched
    /// nodes (and, with DETACH, their relationships first).
    MatchDelete {
        match_pattern: Vec<PatternElement>,
        where_clause: Option<Expr>,
        detach: bool,
        identifiers: Vec<String>,
    },
    WriteThenReturn {
        write: Box<Statement>,
        return_clause: ReturnClause,
        order_by: Option<Vec<(Expr, OrderDirection)>>,
        limit: Option<Expr>,
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
    pub distinct: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WithClause {
    pub items: Vec<ReturnItem>,
    pub where_clause: Option<Expr>,
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
