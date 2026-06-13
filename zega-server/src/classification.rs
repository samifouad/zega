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
                | Statement::KvSet { .. }
                | Statement::KvDel { .. }
                | Statement::KvIncr { .. }
        )
    }))
}

pub fn kv_is_write(op: &str) -> bool {
    matches!(op, "set" | "del" | "incr" | "lpush" | "rpush" | "ltrim" | "expire" | "incr_with_ttl")
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

    #[test]
    fn kv_classification_matches_exclusion_contract() {
        for read in ["get", "lrange", "ttl", "exists", "scan"] {
            assert!(!kv_is_write(read), "{read}");
        }
        for write in ["set", "del", "incr", "lpush", "rpush", "ltrim", "expire", "incr_with_ttl"] {
            assert!(kv_is_write(write), "{write}");
        }
    }
}
