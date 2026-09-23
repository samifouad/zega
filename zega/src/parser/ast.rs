use super::value::Value;
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
    /// A map literal `{ key: expr, ... }` in expression position (e.g. the
    /// argument to `duration({hours: 1})`).
    MapLiteral(HashMap<String, Expr>),
    /// A list literal `[expr, ...]` in expression position (e.g. `[1]` / `[]`
    /// in `FOREACH (_ IN CASE WHEN ... THEN [1] ELSE [] END | ...)`).
    ListLiteral(Vec<Expr>),
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
    StartsWith,
    EndsWith,
    Contains,
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
        skip: Option<Expr>,
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
        on_match: Vec<SetClause>,
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
    /// A clause pipeline: `MATCH ... [OPTIONAL MATCH ...] [WHERE ...] [WITH ...]`
    /// followed by a sequence of write clauses (SET / CREATE / REMOVE / FOREACH /
    /// UNWIND) applied in order, then an optional RETURN. Handles multi-clause
    /// writes the single-clause variants cannot express.
    MatchWrite {
        match_pattern: Vec<PatternElement>,
        optional_patterns: Vec<Vec<PatternElement>>,
        where_clause: Option<Expr>,
        with_clause: Option<WithClause>,
        writes: Vec<WriteClause>,
        return_clause: ReturnClause,
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

/// One write clause inside a `MatchWrite` pipeline.
#[derive(Clone, Debug, PartialEq)]
pub enum WriteClause {
    Set(Vec<SetClause>),
    Create(Vec<PatternElement>),
    /// `FOREACH (var IN list | body)` — run the body once per list element
    /// (write-loop; does not change the row set). The workhorse of conditional
    /// writes: `FOREACH (_ IN CASE WHEN cond THEN [1] ELSE [] END | SET ...)`.
    Foreach {
        variable: String,
        list: Expr,
        body: Vec<WriteClause>,
    },
    /// `UNWIND list AS var` — expand each row into one row per list element
    /// (binds `var`), feeding subsequent clauses + RETURN.
    Unwind {
        variable: String,
        list: Expr,
    },
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
