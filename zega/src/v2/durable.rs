use super::*;

/// A parsed HTTP request, executable one block at a time by a transactional
/// host. Uses the same parser, diagnostics and executor as the native server.
pub struct ZqlProgram {
    file: crate::lang::ZqlFile,
    source: String,
    source_name: &'static str,
}

impl ZqlProgram {
    pub fn parse(schema: &str, source: &str, document: bool) -> Result<Self, ZegaError> {
        let file = if document {
            crate::lang::parse_zql(source).map_err(|e| explain(e, "schema", source))?
        } else {
            crate::lang::ZqlFile {
                schema: crate::lang::parse_schema(schema).map_err(|e| explain(e, "schema", schema))?,
                uniques: crate::lang::parse_uniques(schema).map_err(|e| explain(e, "schema", schema))?,
                indexes: crate::lang::parse_indexes(schema).map_err(|e| explain(e, "schema", schema))?,
                statements: vec![crate::lang::parse_statement(source).map_err(|e| explain(e, "query", source))?],
            }
        };
        Ok(Self { file, source: source.into(), source_name: if document { "schema" } else { "query" } })
    }

    pub fn len(&self) -> usize { self.file.statements.len() }
    pub fn is_empty(&self) -> bool { self.file.statements.is_empty() }
    pub fn is_mutation(&self, block: usize) -> bool {
        match self.file.statements.get(block) {
            Some(Statement::Run(query)) => query.mutation,
            Some(Statement::Load { .. }) => true,
            None => false,
        }
    }
    pub fn execute(&self, db: &Zega, block: usize, sources: &HashMap<String, String>) -> Result<Json, ZegaError> {
        self.execute_with_loader(db, block, &|location| supplied_source(location, sources))
    }
    fn execute_with_loader(&self, db: &Zega, block: usize, loader: &dyn Fn(&str) -> Result<String, LangError>) -> Result<Json, ZegaError> {
        let statement = self.file.statements.get(block)
            .ok_or_else(|| ZegaError::Execution("block index out of range".into()))?;
        db.execute(&self.file.schema,
            Declared { uniques: &self.file.uniques, indexes: &self.file.indexes },
            statement, self.source_name, &self.source, loader)
    }
}

