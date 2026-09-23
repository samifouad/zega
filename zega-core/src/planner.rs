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
        ctx: &ResolvedContext,
    ) -> Result<Plan> {
        if ctx.is_system {
            return Ok(Plan::Execute(stmt.clone()));
        }

        match stmt {
            Statement::Match {
                pattern,
                optional_patterns,
                where_clause,
                with_clause,
                return_clause,
                order_by,
                skip,
                limit,
            } => {
                let planned =
                    self.plan_match(pattern, where_clause, return_clause, order_by, skip, limit, ctx)?;
                // Re-attach OPTIONAL MATCH segments + WITH (plan_match only plans
                // the required pattern's policy filtering).
                if optional_patterns.is_empty() && with_clause.is_none() {
                    return Ok(planned);
                }
                match planned {
                    Plan::Execute(Statement::Match {
                        pattern,
                        where_clause,
                        return_clause,
                        order_by,
                        skip,
                        limit,
                        ..
                    }) => Ok(Plan::Execute(Statement::Match {
                        pattern,
                        optional_patterns: optional_patterns.clone(),
                        where_clause,
                        with_clause: with_clause.clone(),
                        return_clause,
                        order_by,
                        skip,
                        limit,
                    })),
                    other => Ok(other),
                }
            }
            Statement::MatchCreate {
                match_pattern,
                where_clause,
                create_pattern,
            } => {
                let planned = self.plan_match(
                    match_pattern,
                    where_clause,
                    &ReturnClause {
                        items: vec![],
                        distinct: false,
                    },
                    &None,
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
            _ => Ok(Plan::Execute(stmt.clone())),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn plan_match(
        &self,
        pattern: &[PatternElement],
        where_clause: &Option<ZqlExpr>,
        return_clause: &ReturnClause,
        order_by: &Option<Vec<(ZqlExpr, OrderDirection)>>,
        skip: &Option<ZqlExpr>,
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
            optional_patterns: Vec::new(),
            where_clause: merged_where,
            with_clause: None,
            return_clause: return_clause.clone(),
            order_by: order_by.clone(),
            skip: skip.clone(),
            limit: limit.clone(),
        }))
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
    let right = resolve_static_value(right, ctx);
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

fn resolve_static_value(value: &ExprValue, ctx: &ResolvedContext) -> Value {
    match value {
        ExprValue::ContextField(field) => context_value(ctx, field),
        ExprValue::NodeField(_) => Value::Null,
        ExprValue::Literal(value) => value.clone(),
    }
}

fn context_value(ctx: &ResolvedContext, field: &str) -> Value {
    let name = field.trim_start_matches('.');
    ctx.claims.get(name).cloned().unwrap_or(Value::Null)
}

fn permission_denied(policy: &Policy) -> ZegaError {
    ZegaError::PermissionDenied(format!("policy {} requires system context", policy.name))
}
