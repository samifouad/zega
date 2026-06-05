use zega_parser::value::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    pub name: String,
    pub targets: PolicyTargets,
    pub condition: PolicyCondition,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolicyTargets {
    Labels(Vec<String>),
    All,
    Kv,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolicyCondition {
    AllowWhen(Expr),
    AlwaysAllow,
    SystemOnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    Eq(ExprValue, ExprValue),
    Neq(ExprValue, ExprValue),
    In(ExprValue, ExprValue),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExprValue {
    ContextField(String),
    NodeField(String),
    Literal(Value),
}

impl Policy {
    pub fn new(
        name: impl Into<String>,
        targets: PolicyTargets,
        condition: PolicyCondition,
    ) -> Self {
        Self {
            name: name.into(),
            targets,
            condition,
        }
    }

    pub fn applies_to_label(&self, label: &str) -> bool {
        match &self.targets {
            PolicyTargets::Labels(labels) => labels.iter().any(|candidate| candidate == label),
            PolicyTargets::All => true,
            PolicyTargets::Kv => false,
        }
    }

    pub fn applies_to_unlabeled_node(&self) -> bool {
        matches!(self.targets, PolicyTargets::All)
    }

    pub fn applies_to_kv(&self) -> bool {
        matches!(self.targets, PolicyTargets::Kv)
    }
}
