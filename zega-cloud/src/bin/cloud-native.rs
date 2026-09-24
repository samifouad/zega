//! Line-oriented native oracle. Corpus mode matches zegadb/testsuite/runner/lib.rs.
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{self, BufRead},
};
use zega::{check_zql, Zega, ZqlEntryPoint};
#[derive(Deserialize)]
struct Request {
    query: String,
    #[serde(default)]
    schema: String,
    #[serde(default)]
    document: bool,
    #[serde(default)]
    sources: HashMap<String, String>,
}
fn corpus(request: Value) -> Result<Value, Box<dyn std::error::Error>> {
    let source = request["source"].as_str().ok_or("missing source")?;
    let entry = match request["api"].as_str() {
        Some("file") => ZqlEntryPoint::File,
        Some("query") => ZqlEntryPoint::Query,
        Some("statement") => ZqlEntryPoint::Statement,
        _ => return Err("unknown parser API".into()),
    };
    if let Err(error) = check_zql(entry, source) {
        return Ok(
            json!({"ok": false, "stage": "parse", "stdout": "", "stderr": format!("{error}\n")}),
        );
    }
    if entry != ZqlEntryPoint::File {
        return Err("parser API case must reject input".into());
    }
    let sources = serde_json::from_value(request["sources"].clone())?;
    let db = Zega::in_memory().build()?;
    Ok(match db.apply_zql_with_sources(source, &sources) {
        Ok(value) => {
            json!({"ok": true, "stage": "run", "stdout": format!("{value}\n"), "stderr": ""})
        }
        Err(error) => {
            json!({"ok": false, "stage": "run", "stdout": "", "stderr": format!("{error}\n")})
        }
    })
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    for line in io::stdin().lock().lines() {
        let input: Value = serde_json::from_str(&line?)?;
        if input.get("api").is_some() {
            println!("{}", corpus(input)?);
            continue;
        }
        let request: Request = serde_json::from_value(input)?;
        let db = Zega::in_memory().build()?;
        let result = if request.document {
            db.apply_zql_with_sources(&request.query, &request.sources)
        } else {
            db.run_lang_with_sources(&request.schema, &request.query, &request.sources)
        };
        println!(
            "{}",
            match result {
                Ok(value) => json!({"ok": true, "result": value}),
                Err(e) => json!({"ok": false, "error": e.to_string()}),
            }
        );
    }
    Ok(())
}
