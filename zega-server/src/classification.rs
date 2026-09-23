use zega_parser::{Parser, Statement};

pub fn cql_is_write(query: &str) -> Result<bool, String> {
    let mut parser = Parser::new(query).map_err(|error| error.to_string())?;
    let statements = parser.parse().map_err(|error| error.to_string())?;
    Ok(statements.iter().any(|statement| {
        matches!(
            statement,
            Statement::Create { .. }
                | Statement::MatchCreate { .. }
                | Statement::Merge { .. }
                | Statement::Set { .. }
                | Statement::Delete { .. }
        )
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_classifies_cql_reads_and_writes() {
        assert!(!cql_is_write("MATCH (n) RETURN n").unwrap());
        assert!(cql_is_write("MATCH (n) CREATE (m)").unwrap());
        assert!(cql_is_write("MERGE (n:Person {id: 1})").unwrap());
    }
}
