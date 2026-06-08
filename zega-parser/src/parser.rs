use crate::ast::*;
use crate::lexer::{LexError, Lexer, Token};
use crate::value::Value;
use std::collections::HashMap;
use thiserror::Error;

const MAX_EXPRESSION_DEPTH: usize = 64;
const NESTING_TOO_DEEP: &str = "query nesting too deep";

#[derive(Error, Debug)]
pub enum ParseError {
    #[error(transparent)]
    Lex(#[from] LexError),
    #[error("unexpected token: expected {expected:?}, got {got:?}")]
    UnexpectedToken { expected: String, got: Token },
    #[error("unexpected end of input")]
    UnexpectedEof,
    #[error("{0}")]
    Message(String),
}

pub struct Parser<'a> {
    lexer: Lexer<'a>,
    current: Token,
    expression_depth: usize,
}

impl<'a> Parser<'a> {
    pub fn new(input: &'a str) -> Result<Self, ParseError> {
        let mut lexer = Lexer::new(input);
        let current = lexer.next_token()?;
        Ok(Parser {
            lexer,
            current,
            expression_depth: 0,
        })
    }

    fn advance(&mut self) -> Result<(), ParseError> {
        self.current = self.lexer.next_token()?;
        Ok(())
    }

    fn expect(&mut self, expected: Token) -> Result<(), ParseError> {
        if std::mem::discriminant(&self.current) == std::mem::discriminant(&expected) {
            self.advance()?;
            Ok(())
        } else {
            Err(ParseError::UnexpectedToken {
                expected: format!("{:?}", expected),
                got: self.current.clone(),
            })
        }
    }

    pub fn parse(&mut self) -> Result<Vec<Statement>, ParseError> {
        let mut stmts = Vec::new();
        while self.current != Token::Eof {
            stmts.push(self.parse_statement()?);
            if self.current == Token::Semicolon {
                self.advance()?;
            }
        }
        Ok(stmts)
    }

    fn parse_statement(&mut self) -> Result<Statement, ParseError> {
        match &self.current {
            Token::Match => self.parse_match(),
            Token::Create => self.parse_create(),
            Token::Merge => self.parse_merge(),
            Token::Set => self.parse_set_or_kv(),
            Token::Delete => self.parse_delete(),
            Token::Get => self.parse_kv_get(),
            Token::Del => self.parse_kv_del(),
            Token::Incr => self.parse_kv_incr(),
            _ => Err(ParseError::UnexpectedToken {
                expected: "statement keyword".to_string(),
                got: self.current.clone(),
            }),
        }
    }

    fn parse_match(&mut self) -> Result<Statement, ParseError> {
        self.advance()?; // MATCH
        let mut pattern = self.parse_pattern()?;
        while self.current == Token::Match {
            self.advance()?;
            pattern.extend(self.parse_pattern()?);
        }
        let where_clause = if self.current == Token::Where {
            self.advance()?;
            Some(self.parse_expression()?)
        } else {
            None
        };
        if self.current == Token::Create {
            self.advance()?;
            let create_pattern = self.parse_pattern()?;
            return Ok(Statement::MatchCreate {
                match_pattern: pattern,
                where_clause,
                create_pattern,
            });
        }
        let return_clause = if self.current == Token::Return {
            self.advance()?;
            self.parse_return_clause()?
        } else {
            ReturnClause { items: vec![] }
        };
        let mut order_by = None;
        if self.current == Token::Order {
            self.advance()?;
            self.expect(Token::By)?;
            order_by = Some(self.parse_order_by()?);
        }
        let mut limit = None;
        if self.current == Token::Limit {
            self.advance()?;
            limit = Some(self.parse_expression()?);
        }
        Ok(Statement::Match {
            pattern,
            where_clause,
            return_clause,
            order_by,
            limit,
        })
    }

    fn parse_create(&mut self) -> Result<Statement, ParseError> {
        self.advance()?; // CREATE
        let pattern = self.parse_pattern()?;
        Ok(Statement::Create { pattern })
    }

    fn parse_merge(&mut self) -> Result<Statement, ParseError> {
        self.advance()?; // MERGE
        let pattern = self.parse_pattern()?;
        let mut on_create = Vec::new();
        if self.current == Token::On {
            self.advance()?;
            if self.current == Token::Create
                || matches!(&self.current, Token::Identifier(s) if s.eq_ignore_ascii_case("CREATE"))
            {
                self.advance()?;
                self.expect(Token::Set)?;
                on_create = self.parse_set_clauses()?;
            } else {
                return Err(ParseError::Message("expected CREATE after ON".to_string()));
            }
        }
        Ok(Statement::Merge { pattern, on_create })
    }

    fn parse_set_or_kv(&mut self) -> Result<Statement, ParseError> {
        self.advance()?; // SET
        if self.current == Token::Key {
            self.parse_kv_set_after_key()
        } else {
            let assignments = self.parse_set_clauses()?;
            Ok(Statement::Set { assignments })
        }
    }

    fn parse_kv_set_after_key(&mut self) -> Result<Statement, ParseError> {
        self.expect(Token::Key)?;
        let key = self.parse_primary()?;
        self.expect(Token::Eq)?;
        let value = self.parse_primary()?;
        let mut ttl = None;
        if self.current == Token::Ttl {
            self.advance()?;
            ttl = Some(self.parse_primary()?);
        }
        Ok(Statement::KvSet { key, value, ttl })
    }

    fn parse_delete(&mut self) -> Result<Statement, ParseError> {
        self.advance()?; // DELETE
        let mut ids = Vec::new();
        while let Token::Identifier(id) = &self.current {
            ids.push(id.clone());
            self.advance()?;
            if self.current == Token::Comma {
                self.advance()?;
            } else {
                break;
            }
        }
        Ok(Statement::Delete { identifiers: ids })
    }

    fn parse_kv_get(&mut self) -> Result<Statement, ParseError> {
        self.advance()?; // GET
        self.expect(Token::Key)?;
        let key = self.parse_primary()?;
        Ok(Statement::KvGet { key })
    }

    fn parse_kv_del(&mut self) -> Result<Statement, ParseError> {
        self.advance()?; // DEL
        self.expect(Token::Key)?;
        let key = self.parse_primary()?;
        Ok(Statement::KvDel { key })
    }

    fn parse_kv_incr(&mut self) -> Result<Statement, ParseError> {
        self.advance()?; // INCR
        self.expect(Token::Key)?;
        let key = self.parse_primary()?;
        Ok(Statement::KvIncr { key })
    }

    fn parse_pattern(&mut self) -> Result<Vec<PatternElement>, ParseError> {
        let mut result = vec![self.parse_pattern_element()?];
        while self.current == Token::Dash || self.current == Token::LeftArrow {
            let incoming = self.current == Token::LeftArrow;
            self.advance()?;
            let rel = if self.current == Token::LBracket {
                Some(self.parse_relationship()?)
            } else {
                None
            };
            let dir = if incoming {
                self.expect(Token::Dash)?;
                Some(Direction::Incoming)
            } else if self.current == Token::Arrow {
                self.advance()?;
                Some(Direction::Outgoing)
            } else if self.current == Token::Dash {
                self.advance()?;
                Some(Direction::Both)
            } else {
                return Err(ParseError::UnexpectedToken {
                    expected: "relationship direction".to_string(),
                    got: self.current.clone(),
                });
            };
            let mut next_el = self.parse_pattern_element()?;
            next_el.relationship = rel;
            next_el.direction = dir;
            result.push(next_el);
        }
        Ok(result)
    }

    fn parse_pattern_element(&mut self) -> Result<PatternElement, ParseError> {
        self.expect(Token::LParen)?;
        let variable = if let Token::Identifier(id) = &self.current {
            let v = id.clone();
            self.advance()?;
            v
        } else {
            String::new()
        };
        let mut labels = Vec::new();
        if self.current == Token::Colon {
            self.advance()?;
            if let Some(label) = self.take_symbolic_name()? {
                labels.push(label);
            }
        }
        let properties = if self.current == Token::LBrace {
            self.parse_properties()?
        } else {
            HashMap::new()
        };
        self.expect(Token::RParen)?;
        Ok(PatternElement {
            variable,
            labels,
            properties,
            relationship: None,
            direction: None,
        })
    }

    fn parse_relationship(&mut self) -> Result<RelationshipPattern, ParseError> {
        self.expect(Token::LBracket)?;
        let variable = if let Token::Identifier(id) = &self.current {
            let v = id.clone();
            self.advance()?;
            v
        } else {
            String::new()
        };
        let mut kinds = Vec::new();
        if self.current == Token::Colon {
            self.advance()?;
            if let Some(kind) = self.take_symbolic_name()? {
                kinds.push(kind);
            }
        }
        let length = if self.current == Token::Star {
            self.advance()?;
            Some(self.parse_relationship_length()?)
        } else {
            None
        };
        let properties = if self.current == Token::LBrace {
            self.parse_properties()?
        } else {
            HashMap::new()
        };
        self.expect(Token::RBracket)?;
        Ok(RelationshipPattern {
            variable,
            kinds,
            properties,
            length,
        })
    }

    fn parse_relationship_length(&mut self) -> Result<RelationshipLength, ParseError> {
        let first = self.take_non_negative_integer()?;
        if self.current != Token::Dot {
            return Ok(RelationshipLength {
                min: first.unwrap_or(1),
                max: first,
            });
        }

        self.advance()?;
        self.expect(Token::Dot)?;
        let max = self.take_non_negative_integer()?;
        let min = first.unwrap_or(1);
        if max.is_some_and(|max| min > max) {
            return Err(ParseError::Message(
                "relationship length minimum exceeds maximum".to_string(),
            ));
        }
        Ok(RelationshipLength { min, max })
    }

    fn take_non_negative_integer(&mut self) -> Result<Option<usize>, ParseError> {
        match self.current.clone() {
            Token::Integer(value) if value >= 0 => {
                self.advance()?;
                Ok(Some(value as usize))
            }
            Token::Integer(_) => Err(ParseError::Message(
                "relationship length must be non-negative".to_string(),
            )),
            _ => Ok(None),
        }
    }

    fn take_symbolic_name(&mut self) -> Result<Option<String>, ParseError> {
        let name = match &self.current {
            Token::Identifier(name) => name.clone(),
            Token::Order => "Order".to_string(),
            _ => return Ok(None),
        };
        self.advance()?;
        Ok(Some(name))
    }

    fn parse_properties(&mut self) -> Result<HashMap<String, Expr>, ParseError> {
        self.expect(Token::LBrace)?;
        let mut props = HashMap::new();
        while self.current != Token::RBrace {
            let key = match &self.current {
                Token::Identifier(id) => {
                    let k = id.clone();
                    self.advance()?;
                    k
                }
                Token::Key => {
                    self.advance()?;
                    "key".to_string()
                }
                Token::Get => {
                    self.advance()?;
                    "get".to_string()
                }
                Token::Set => {
                    self.advance()?;
                    "set".to_string()
                }
                Token::Del => {
                    self.advance()?;
                    "del".to_string()
                }
                Token::Incr => {
                    self.advance()?;
                    "incr".to_string()
                }
                Token::Ttl => {
                    self.advance()?;
                    "ttl".to_string()
                }
                Token::Match => {
                    self.advance()?;
                    "match".to_string()
                }
                Token::Return => {
                    self.advance()?;
                    "return".to_string()
                }
                Token::Create => {
                    self.advance()?;
                    "create".to_string()
                }
                Token::Merge => {
                    self.advance()?;
                    "merge".to_string()
                }
                Token::Delete => {
                    self.advance()?;
                    "delete".to_string()
                }
                Token::Where => {
                    self.advance()?;
                    "where".to_string()
                }
                Token::Limit => {
                    self.advance()?;
                    "limit".to_string()
                }
                Token::Order => {
                    self.advance()?;
                    "order".to_string()
                }
                Token::By => {
                    self.advance()?;
                    "by".to_string()
                }
                Token::Asc => {
                    self.advance()?;
                    "asc".to_string()
                }
                Token::Desc => {
                    self.advance()?;
                    "desc".to_string()
                }
                Token::On => {
                    self.advance()?;
                    "on".to_string()
                }
                _ => break,
            };
            self.expect(Token::Colon)?;
            let val = self.parse_expression()?;
            props.insert(key, val);
            if self.current == Token::Comma {
                self.advance()?;
            }
        }
        self.expect(Token::RBrace)?;
        Ok(props)
    }

    fn parse_return_clause(&mut self) -> Result<ReturnClause, ParseError> {
        let mut items = Vec::new();
        loop {
            let expr = self.parse_expression()?;
            let alias = if let Token::Identifier(ref id) = self.current {
                if id.eq_ignore_ascii_case("AS") {
                    self.advance()?;
                    if let Token::Identifier(a) = &self.current {
                        let a = a.clone();
                        self.advance()?;
                        Some(a)
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };
            items.push(ReturnItem { expr, alias });
            if self.current == Token::Comma {
                self.advance()?;
            } else {
                break;
            }
        }
        Ok(ReturnClause { items })
    }

    fn parse_order_by(&mut self) -> Result<Vec<(Expr, OrderDirection)>, ParseError> {
        let mut items = Vec::new();
        loop {
            let expr = self.parse_expression()?;
            let dir = if self.current == Token::Asc {
                self.advance()?;
                OrderDirection::Asc
            } else if self.current == Token::Desc {
                self.advance()?;
                OrderDirection::Desc
            } else {
                OrderDirection::Asc
            };
            items.push((expr, dir));
            if self.current == Token::Comma {
                self.advance()?;
            } else {
                break;
            }
        }
        Ok(items)
    }

    fn parse_set_clauses(&mut self) -> Result<Vec<SetClause>, ParseError> {
        let mut clauses = Vec::new();
        loop {
            let target = self.parse_primary()?;
            self.expect(Token::Eq)?;
            let value = self.parse_expression()?;
            clauses.push(SetClause { target, value });
            if self.current == Token::Comma {
                self.advance()?;
            } else {
                break;
            }
        }
        Ok(clauses)
    }

    fn parse_expression(&mut self) -> Result<Expr, ParseError> {
        if self.expression_depth >= MAX_EXPRESSION_DEPTH {
            return Err(ParseError::Message(NESTING_TOO_DEEP.to_string()));
        }

        self.expression_depth += 1;
        let result = self.parse_or();
        self.expression_depth -= 1;
        result
    }

    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_and()?;
        while let Token::Identifier(ref id) = self.current {
            if id.eq_ignore_ascii_case("OR") {
                self.advance()?;
                let right = self.parse_and()?;
                left = Expr::BinaryOp(Box::new(left), BinaryOperator::Or, Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_equality()?;
        while let Token::Identifier(ref id) = self.current {
            if id.eq_ignore_ascii_case("AND") {
                self.advance()?;
                let right = self.parse_equality()?;
                left = Expr::BinaryOp(Box::new(left), BinaryOperator::And, Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_comparison()?;
        loop {
            match &self.current {
                Token::Eq => {
                    self.advance()?;
                    let right = self.parse_comparison()?;
                    left = Expr::BinaryOp(Box::new(left), BinaryOperator::Eq, Box::new(right));
                }
                Token::Ne => {
                    self.advance()?;
                    let right = self.parse_comparison()?;
                    left = Expr::BinaryOp(Box::new(left), BinaryOperator::Ne, Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_add()?;
        loop {
            match &self.current {
                Token::Gt => {
                    self.advance()?;
                    let right = self.parse_add()?;
                    left = Expr::BinaryOp(Box::new(left), BinaryOperator::Gt, Box::new(right));
                }
                Token::Lt => {
                    self.advance()?;
                    let right = self.parse_add()?;
                    left = Expr::BinaryOp(Box::new(left), BinaryOperator::Lt, Box::new(right));
                }
                Token::Gte => {
                    self.advance()?;
                    let right = self.parse_add()?;
                    left = Expr::BinaryOp(Box::new(left), BinaryOperator::Gte, Box::new(right));
                }
                Token::Lte => {
                    self.advance()?;
                    let right = self.parse_add()?;
                    left = Expr::BinaryOp(Box::new(left), BinaryOperator::Lte, Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_add(&mut self) -> Result<Expr, ParseError> {
        let left = self.parse_primary()?;
        // MVP: no arithmetic needed beyond what's in primary
        Ok(left)
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        match &self.current {
            Token::Dollar => {
                self.advance()?;
                let name = match &self.current {
                    Token::Identifier(id) => {
                        let n = id.clone();
                        self.advance()?;
                        n
                    }
                    Token::Key => {
                        self.advance()?;
                        "key".to_string()
                    }
                    Token::Get => {
                        self.advance()?;
                        "get".to_string()
                    }
                    Token::Set => {
                        self.advance()?;
                        "set".to_string()
                    }
                    Token::Del => {
                        self.advance()?;
                        "del".to_string()
                    }
                    Token::Incr => {
                        self.advance()?;
                        "incr".to_string()
                    }
                    Token::Ttl => {
                        self.advance()?;
                        "ttl".to_string()
                    }
                    Token::Match => {
                        self.advance()?;
                        "match".to_string()
                    }
                    Token::Return => {
                        self.advance()?;
                        "return".to_string()
                    }
                    Token::Create => {
                        self.advance()?;
                        "create".to_string()
                    }
                    Token::Merge => {
                        self.advance()?;
                        "merge".to_string()
                    }
                    Token::Delete => {
                        self.advance()?;
                        "delete".to_string()
                    }
                    Token::Where => {
                        self.advance()?;
                        "where".to_string()
                    }
                    Token::Limit => {
                        self.advance()?;
                        "limit".to_string()
                    }
                    Token::Order => {
                        self.advance()?;
                        "order".to_string()
                    }
                    Token::By => {
                        self.advance()?;
                        "by".to_string()
                    }
                    Token::Asc => {
                        self.advance()?;
                        "asc".to_string()
                    }
                    Token::Desc => {
                        self.advance()?;
                        "desc".to_string()
                    }
                    Token::On => {
                        self.advance()?;
                        "on".to_string()
                    }
                    _ => {
                        return Err(ParseError::Message(
                            "expected parameter name after $".to_string(),
                        ))
                    }
                };
                Ok(Expr::Parameter(name))
            }
            Token::StringLiteral(s) => {
                let val = Value::String(s.clone());
                self.advance()?;
                Ok(Expr::Literal(val))
            }
            Token::Integer(i) => {
                let val = Value::Int(*i);
                self.advance()?;
                Ok(Expr::Literal(val))
            }
            Token::Float(f) => {
                let val = Value::from_f64(*f);
                self.advance()?;
                Ok(Expr::Literal(val))
            }
            Token::Bool(b) => {
                let val = Value::Bool(*b);
                self.advance()?;
                Ok(Expr::Literal(val))
            }
            Token::Null => {
                self.advance()?;
                Ok(Expr::Literal(Value::Null))
            }
            Token::Identifier(id) => {
                let name = id.clone();
                self.advance()?;
                if self.current == Token::LParen {
                    self.parse_function_call(name)
                } else if self.current == Token::Dot {
                    self.advance()?;
                    if let Token::Identifier(prop) = &self.current {
                        let p = prop.clone();
                        self.advance()?;
                        Ok(Expr::PropertyAccess(Box::new(Expr::Identifier(name)), p))
                    } else {
                        Err(ParseError::Message(
                            "expected property name after .".to_string(),
                        ))
                    }
                } else {
                    Ok(Expr::Identifier(name))
                }
            }
            Token::LParen => {
                self.advance()?;
                let expr = self.parse_expression()?;
                self.expect(Token::RParen)?;
                Ok(expr)
            }
            _ => Err(ParseError::UnexpectedToken {
                expected: "expression".to_string(),
                got: self.current.clone(),
            }),
        }
    }

    fn parse_function_call(&mut self, name: String) -> Result<Expr, ParseError> {
        let function = match name.to_ascii_lowercase().as_str() {
            "count" => AggregateFunction::Count,
            "sum" => AggregateFunction::Sum,
            "avg" => AggregateFunction::Avg,
            "min" => AggregateFunction::Min,
            "max" => AggregateFunction::Max,
            "collect" => AggregateFunction::Collect,
            _ => return Err(ParseError::Message(format!("unsupported function: {name}"))),
        };

        self.expect(Token::LParen)?;
        let distinct =
            matches!(&self.current, Token::Identifier(id) if id.eq_ignore_ascii_case("DISTINCT"));
        if distinct {
            self.advance()?;
        }
        let argument = if self.current == Token::Star {
            self.advance()?;
            if function != AggregateFunction::Count || distinct {
                return Err(ParseError::Message(
                    "only count(*) supports a star argument".to_string(),
                ));
            }
            None
        } else {
            Some(Box::new(self.parse_expression()?))
        };
        self.expect(Token::RParen)?;
        Ok(Expr::Aggregate {
            function,
            argument,
            distinct,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_match_return() {
        let mut p = Parser::new("MATCH (n:Label {prop: $param}) RETURN n").unwrap();
        let stmts = p.parse().unwrap();
        assert_eq!(stmts.len(), 1);
        match &stmts[0] {
            Statement::Match {
                pattern,
                return_clause,
                ..
            } => {
                assert_eq!(pattern.len(), 1);
                assert_eq!(pattern[0].variable, "n");
                assert_eq!(pattern[0].labels, vec!["Label"]);
                assert_eq!(return_clause.items.len(), 1);
            }
            _ => panic!("expected MATCH"),
        }
    }

    #[test]
    fn test_parse_return_aggregates() {
        let mut p = Parser::new(
            "MATCH (n:Label) RETURN count(*), count(DISTINCT n), sum(n.total), collect(n.id)",
        )
        .unwrap();
        let stmts = p.parse().unwrap();
        let Statement::Match { return_clause, .. } = &stmts[0] else {
            panic!("expected MATCH");
        };
        assert_eq!(return_clause.items.len(), 4);
        assert_eq!(
            return_clause.items[0].expr,
            Expr::Aggregate {
                function: AggregateFunction::Count,
                argument: None,
                distinct: false,
            }
        );
        assert!(matches!(
            return_clause.items[1].expr,
            Expr::Aggregate {
                function: AggregateFunction::Count,
                distinct: true,
                ..
            }
        ));
    }

    #[test]
    fn test_parse_relationship() {
        let mut p = Parser::new("MATCH (a:Label)-[:REL]->(b:Label) RETURN a, b").unwrap();
        let stmts = p.parse().unwrap();
        assert_eq!(stmts.len(), 1);
        match &stmts[0] {
            Statement::Match { pattern, .. } => {
                assert_eq!(pattern.len(), 2);
                assert_eq!(pattern[1].relationship.as_ref().unwrap().kinds, vec!["REL"]);
                assert_eq!(pattern[1].direction, Some(Direction::Outgoing));
            }
            _ => panic!("expected MATCH"),
        }
    }

    #[test]
    fn test_parse_variable_length_relationship_ranges_and_directions() {
        let cases = [
            (
                "MATCH (a)-[:REL*]->(b) RETURN b",
                1,
                None,
                Direction::Outgoing,
            ),
            (
                "MATCH (a)-[:REL*1..5]->(b) RETURN b",
                1,
                Some(5),
                Direction::Outgoing,
            ),
            (
                "MATCH (a)-[:REL*..3]->(b) RETURN b",
                1,
                Some(3),
                Direction::Outgoing,
            ),
            (
                "MATCH (a)<-[:REL*2..]-(b) RETURN b",
                2,
                None,
                Direction::Incoming,
            ),
            (
                "MATCH (a)-[*1..3]-(b) RETURN b",
                1,
                Some(3),
                Direction::Both,
            ),
        ];

        for (query, min, max, direction) in cases {
            let mut parser = Parser::new(query).unwrap();
            let statements = parser.parse().unwrap();
            let Statement::Match { pattern, .. } = &statements[0] else {
                panic!("expected MATCH");
            };
            let relationship = pattern[1].relationship.as_ref().unwrap();
            assert!(relationship.kinds.is_empty() || relationship.kinds == vec!["REL"]);
            assert_eq!(relationship.length, Some(RelationshipLength { min, max }));
            assert_eq!(pattern[1].direction, Some(direction));
        }
    }

    #[test]
    fn test_parse_create() {
        let mut p = Parser::new("CREATE (n:Label {prop: $param})").unwrap();
        let stmts = p.parse().unwrap();
        assert_eq!(stmts.len(), 1);
        match &stmts[0] {
            Statement::Create { pattern } => {
                assert_eq!(pattern[0].labels, vec!["Label"]);
            }
            _ => panic!("expected CREATE"),
        }
    }

    #[test]
    fn test_parse_create_relationship() {
        let mut p =
            Parser::new("CREATE (u:User {id: $uid})-[:PLACED]->(o:Order {id: $oid})").unwrap();
        let stmts = p.parse().unwrap();
        assert_eq!(stmts.len(), 1);
        match &stmts[0] {
            Statement::Create { pattern } => {
                assert_eq!(pattern.len(), 2);
                assert_eq!(
                    pattern[1].relationship.as_ref().unwrap().kinds,
                    vec!["PLACED"]
                );
            }
            _ => panic!("expected CREATE"),
        }
    }

    #[test]
    fn test_parse_match_create() {
        let mut p =
            Parser::new("MATCH (u:User {id: $uid}) CREATE (u)-[:PLACED]->(o:Order {id: $oid})")
                .unwrap();
        let stmts = p.parse().unwrap();
        assert_eq!(stmts.len(), 1);
        match &stmts[0] {
            Statement::MatchCreate {
                match_pattern,
                create_pattern,
                ..
            } => {
                assert_eq!(match_pattern.len(), 1);
                assert_eq!(create_pattern.len(), 2);
                assert_eq!(
                    create_pattern[1].relationship.as_ref().unwrap().kinds,
                    vec!["PLACED"]
                );
            }
            _ => panic!("expected MATCH...CREATE"),
        }
    }

    #[test]
    fn test_parse_multi_match_create_as_one_statement() {
        let mut p = Parser::new(
            "MATCH (a:Category {id: $pid}) MATCH (b:Category {id: $cid}) CREATE (a)-[:SUBCATEGORY]->(b)",
        ).unwrap();
        let stmts = p.parse().unwrap();
        assert_eq!(stmts.len(), 1);
        let Statement::MatchCreate {
            match_pattern,
            create_pattern,
            ..
        } = &stmts[0]
        else {
            panic!("expected MATCH...MATCH...CREATE");
        };
        assert_eq!(match_pattern.len(), 2);
        assert_eq!(match_pattern[0].variable, "a");
        assert_eq!(match_pattern[1].variable, "b");
        assert_eq!(create_pattern.len(), 2);
    }

    #[test]
    fn test_parse_merge() {
        let mut p =
            Parser::new("MERGE (n:Label {key: $param}) ON CREATE SET n.prop = $val").unwrap();
        let stmts = p.parse().unwrap();
        assert_eq!(stmts.len(), 1);
        match &stmts[0] {
            Statement::Merge { on_create, .. } => {
                assert_eq!(on_create.len(), 1);
            }
            _ => panic!("expected MERGE"),
        }
    }

    #[test]
    fn test_parse_where() {
        let mut p = Parser::new("MATCH (n:Label) WHERE n.prop = $val RETURN n").unwrap();
        let stmts = p.parse().unwrap();
        assert_eq!(stmts.len(), 1);
        match &stmts[0] {
            Statement::Match { where_clause, .. } => {
                assert!(where_clause.is_some());
            }
            _ => panic!("expected MATCH"),
        }
    }

    #[test]
    fn test_parse_kv() {
        let mut p = Parser::new("GET KEY $key").unwrap();
        let stmts = p.parse().unwrap();
        assert_eq!(stmts.len(), 1);
        match &stmts[0] {
            Statement::KvGet { .. } => {}
            _ => panic!("expected KvGet"),
        }
    }
}
