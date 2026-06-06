use zega_parser::{
    ast::{BinaryOperator, Expr as ZqlExpr, PatternElement, ReturnClause, Statement},
    value::Value,
    OrderDirection,
};

use crate::{
    policy::{Expr, ExprValue, Policy, PolicyCondition},
    ResolvedContext, Result, ZegaError,
};

#[derive(Clone, Debug, PartialEq)]
pub enum Plan {
    Execute(Statement),
    FilteredKvGet,
}

pub struct Planner<'a> {
    policies: &'a [Policy],
}

impl<'a> Planner<'a> {
    pub fn new(policies: &'a [Policy]) -> Self {
        Self { policies }
    }

    pub fn plan_statement(
        &self,
        stmt: &Statement,
        params: &std::collections::HashMap<String, Value>,
        ctx: &ResolvedContext,
    ) -> Result<Plan> {
        if ctx.is_system {
            return Ok(Plan::Execute(stmt.clone()));
        }

        match stmt {
            Statement::Match {
                pattern,
                where_clause,
                return_clause,
                order_by,
                limit,
            } => self.plan_match(pattern, where_clause, return_clause, order_by, limit, ctx),
            Statement::MatchCreate {
                match_pattern,
                where_clause,
                create_pattern,
            } => {
                let planned = self.plan_match(
                    match_pattern,
                    where_clause,
                    &ReturnClause { items: vec![] },
                    &None,
                    &None,
                    ctx,
                )?;
                match planned {
                    Plan::Execute(Statement::Match { where_clause, .. }) => {
                        Ok(Plan::Execute(Statement::MatchCreate {
                            match_pattern: match_pattern.clone(),
                            where_clause,
                            create_pattern: create_pattern.clone(),
                        }))
                    }
                    _ => unreachable!("MATCH planning always returns an executable MATCH"),
                }
            }
            Statement::KvGet { key } => self.plan_kv_get(key, params, ctx),
            _ => Ok(Plan::Execute(stmt.clone())),
        }
    }

    fn plan_match(
        &self,
        pattern: &[PatternElement],
        where_clause: &Option<ZqlExpr>,
        return_clause: &ReturnClause,
        order_by: &Option<Vec<(ZqlExpr, OrderDirection)>>,
        limit: &Option<ZqlExpr>,
        ctx: &ResolvedContext,
    ) -> Result<Plan> {
        let mut policy_expr: Option<ZqlExpr> = None;

        for element in pattern {
            let matching_policies: Vec<&Policy> = if element.labels.is_empty() {
                self.policies
                    .iter()
                    .filter(|policy| policy.applies_to_unlabeled_node())
                    .collect()
            } else {
                self.policies
                    .iter()
                    .filter(|policy| {
                        element
                            .labels
                            .iter()
                            .any(|label| policy.applies_to_label(label))
                    })
                    .collect()
            };

            for policy in matching_policies {
                match &policy.condition {
                    PolicyCondition::AlwaysAllow => {}
                    PolicyCondition::SystemOnly => {
                        return Err(permission_denied(policy));
                    }
                    PolicyCondition::AllowWhen(expr) => {
                        let lowered = lower_expr(expr, ctx, &element.variable)?;
                        policy_expr = Some(and_opt(policy_expr, lowered));
                    }
                }
            }
        }

        let merged_where = match (where_clause.clone(), policy_expr) {
            (Some(existing), Some(policy)) => Some(ZqlExpr::BinaryOp(
                Box::new(existing),
                BinaryOperator::And,
                Box::new(policy),
            )),
            (Some(existing), None) => Some(existing),
            (None, Some(policy)) => Some(policy),
            (None, None) => None,
        };

        Ok(Plan::Execute(Statement::Match {
            pattern: pattern.to_vec(),
            where_clause: merged_where,
            return_clause: return_clause.clone(),
            order_by: order_by.clone(),
            limit: limit.clone(),
        }))
    }

    fn plan_kv_get(
        &self,
        key: &ZqlExpr,
        params: &std::collections::HashMap<String, Value>,
        ctx: &ResolvedContext,
    ) -> Result<Plan> {
        let mut has_kv_policy = false;
        let mut allowed = true;

        for policy in self.policies.iter().filter(|policy| policy.applies_to_kv()) {
            has_kv_policy = true;
            match &policy.condition {
                PolicyCondition::AlwaysAllow => {}
                PolicyCondition::SystemOnly => return Err(permission_denied(policy)),
                PolicyCondition::AllowWhen(expr) => {
                    let key_value = static_expr_value(key, params).unwrap_or(Value::Null);
                    allowed &= eval_policy_expr(expr, ctx, Some(&key_value));
                }
            }
        }

        if has_kv_policy && !allowed {
            Ok(Plan::FilteredKvGet)
        } else {
            Ok(Plan::Execute(Statement::KvGet { key: key.clone() }))
        }
    }
}

fn and_opt(left: Option<ZqlExpr>, right: ZqlExpr) -> ZqlExpr {
    match left {
        Some(left) => ZqlExpr::BinaryOp(Box::new(left), BinaryOperator::And, Box::new(right)),
        None => right,
    }
}

fn lower_expr(expr: &Expr, ctx: &ResolvedContext, node_variable: &str) -> Result<ZqlExpr> {
    match expr {
        Expr::Eq(left, right) => Ok(ZqlExpr::BinaryOp(
            Box::new(lower_value(left, ctx, node_variable)),
            BinaryOperator::Eq,
            Box::new(lower_value(right, ctx, node_variable)),
        )),
        Expr::Neq(left, right) => Ok(ZqlExpr::BinaryOp(
            Box::new(lower_value(left, ctx, node_variable)),
            BinaryOperator::Ne,
            Box::new(lower_value(right, ctx, node_variable)),
        )),
        Expr::In(left, right) => lower_in(left, right, ctx, node_variable),
        Expr::And(left, right) => Ok(ZqlExpr::BinaryOp(
            Box::new(lower_expr(left, ctx, node_variable)?),
            BinaryOperator::And,
            Box::new(lower_expr(right, ctx, node_variable)?),
        )),
        Expr::Or(left, right) => Ok(ZqlExpr::BinaryOp(
            Box::new(lower_expr(left, ctx, node_variable)?),
            BinaryOperator::Or,
            Box::new(lower_expr(right, ctx, node_variable)?),
        )),
        Expr::Not(inner) => Ok(ZqlExpr::BinaryOp(
            Box::new(lower_expr(inner, ctx, node_variable)?),
            BinaryOperator::Eq,
            Box::new(ZqlExpr::Literal(Value::Bool(false))),
        )),
    }
}

fn lower_in(
    left: &ExprValue,
    right: &ExprValue,
    ctx: &ResolvedContext,
    node_variable: &str,
) -> Result<ZqlExpr> {
    let left = lower_value(left, ctx, node_variable);
    let right = resolve_static_value(right, ctx, None);
    if let Value::List(values) = right {
        let mut values = values.into_iter();
        let Some(first) = values.next() else {
            return Ok(ZqlExpr::Literal(Value::Bool(false)));
        };
        let mut expr = ZqlExpr::BinaryOp(
            Box::new(left.clone()),
            BinaryOperator::Eq,
            Box::new(ZqlExpr::Literal(first)),
        );
        for value in values {
            expr = ZqlExpr::BinaryOp(
                Box::new(expr),
                BinaryOperator::Or,
                Box::new(ZqlExpr::BinaryOp(
                    Box::new(left.clone()),
                    BinaryOperator::Eq,
                    Box::new(ZqlExpr::Literal(value)),
                )),
            );
        }
        Ok(expr)
    } else {
        Ok(ZqlExpr::BinaryOp(
            Box::new(left),
            BinaryOperator::Eq,
            Box::new(ZqlExpr::Literal(right)),
        ))
    }
}

fn lower_value(value: &ExprValue, ctx: &ResolvedContext, node_variable: &str) -> ZqlExpr {
    match value {
        ExprValue::ContextField(field) => ZqlExpr::Literal(context_value(ctx, field)),
        ExprValue::NodeField(field) => {
            let property = field.rsplit('.').next().unwrap_or(field).to_string();
            ZqlExpr::PropertyAccess(
                Box::new(ZqlExpr::Identifier(node_variable.to_string())),
                property,
            )
        }
        ExprValue::Literal(value) => ZqlExpr::Literal(value.clone()),
    }
}

fn eval_policy_expr(expr: &Expr, ctx: &ResolvedContext, key: Option<&Value>) -> bool {
    match expr {
        Expr::Eq(left, right) => {
            resolve_static_value(left, ctx, key) == resolve_static_value(right, ctx, key)
        }
        Expr::Neq(left, right) => {
            resolve_static_value(left, ctx, key) != resolve_static_value(right, ctx, key)
        }
        Expr::In(left, right) => {
            let needle = resolve_static_value(left, ctx, key);
            match resolve_static_value(right, ctx, key) {
                Value::List(values) => values.contains(&needle),
                Value::String(prefix) => needle
                    .as_string()
                    .is_some_and(|value| value.starts_with(&prefix)),
                _ => false,
            }
        }
        Expr::And(left, right) => {
            eval_policy_expr(left, ctx, key) && eval_policy_expr(right, ctx, key)
        }
        Expr::Or(left, right) => {
            eval_policy_expr(left, ctx, key) || eval_policy_expr(right, ctx, key)
        }
        Expr::Not(inner) => !eval_policy_expr(inner, ctx, key),
    }
}

fn resolve_static_value(value: &ExprValue, ctx: &ResolvedContext, key: Option<&Value>) -> Value {
    match value {
        ExprValue::ContextField(field) => context_value(ctx, field),
        ExprValue::NodeField(field) if field == "key" => key.cloned().unwrap_or(Value::Null),
        ExprValue::NodeField(field) if field == "key_prefix" => key
            .and_then(Value::as_string)
            .and_then(|key| key.split_once(':').map(|(prefix, _)| prefix.to_string()))
            .map(Value::String)
            .unwrap_or(Value::Null),
        ExprValue::NodeField(_) => Value::Null,
        ExprValue::Literal(value) => value.clone(),
    }
}

fn context_value(ctx: &ResolvedContext, field: &str) -> Value {
    let name = field.trim_start_matches('.');
    ctx.claims.get(name).cloned().unwrap_or(Value::Null)
}

fn static_expr_value(
    expr: &ZqlExpr,
    params: &std::collections::HashMap<String, Value>,
) -> Option<Value> {
    match expr {
        ZqlExpr::Literal(value) => Some(value.clone()),
        ZqlExpr::Parameter(name) => params.get(name).cloned(),
        _ => None,
    }
}

fn permission_denied(policy: &Policy) -> ZegaError {
    ZegaError::PermissionDenied(format!("policy {} requires system context", policy.name))
}
