//! Diagnostics and the text that presents them.
//!
//! The checker in `zega-lang` decides what is wrong. This crate owns the
//! value it reports and the layout of that report, so a CLI and the wasm
//! build print the same characters. A browser displays the string.

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
}

/// Which text the diagnostic refers to. A CLI passes the same labels the
/// browser does (`schema`, `query`) so the rendered line is identical.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Pane {
    Schema,
    Query,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub severity: Severity,
    pub pane: Pane,
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub underline_length: u32,
    pub message: String,
    pub help: Option<String>,
}

impl Diagnostic {
    pub fn at(
        pane: Pane,
        line: u32,
        column: u32,
        end_line: u32,
        end_column: u32,
        message: impl Into<String>,
        help: Option<String>,
    ) -> Self {
        let underline_length = if line == 0 {
            0
        } else if end_line == line {
            end_column.saturating_sub(column).max(1)
        } else {
            1
        };
        Self {
            severity: Severity::Error,
            pane,
            line,
            column,
            end_line,
            end_column,
            underline_length,
            message: message.into(),
            help,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Report {
    pub diagnostics: Vec<Diagnostic>,
    pub text: String,
}

impl Report {
    pub fn new(schema_src: &str, query_src: &str, diagnostics: Vec<Diagnostic>) -> Self {
        let text = render_all(schema_src, query_src, &diagnostics);
        Self { diagnostics, text }
    }
}

/// The closest name within two single-character edits, ignoring case.
///
/// `nme` against `name` costs one insertion (the missing `a`), so it wins
/// over anything farther away. An exact spelling is skipped. More than two
/// edits returns nothing, and the caller lists the legal names instead.
pub fn closest<'a>(wanted: &str, options: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let mut best: Option<(usize, &'a str)> = None;
    for opt in options {
        if opt == wanted {
            continue;
        }
        let distance = edit_distance(&wanted.to_ascii_lowercase(), &opt.to_ascii_lowercase());
        if distance > 2 {
            continue;
        }
        if best.is_none_or(|(best_distance, _)| distance < best_distance) {
            best = Some((distance, opt));
        }
    }
    best.map(|(_, name)| name)
}

/// Plain-text report. No color, so a terminal and the browser show the same
/// characters.
pub fn render(source_name: &str, source: &str, diag: &Diagnostic) -> String {
    if diag.line == 0 {
        let mut text = format!("error: {}", diag.message);
        if let Some(help) = &diag.help {
            text.push_str("\n  help: ");
            text.push_str(help);
        }
        return text;
    }
    let line_text = source
        .lines()
        .nth(diag.line.saturating_sub(1) as usize)
        .unwrap_or("");
    let col = diag.column.max(1);
    let len = diag.underline_length.max(1) as usize;
    let caret = format!("{}{}", " ".repeat(col as usize - 1), "^".repeat(len));
    let mut text = format!(
        "error: {}\n  {source_name}:{}:{col}\n  {line_text}\n  {caret}",
        diag.message, diag.line
    );
    if let Some(help) = &diag.help {
        text.push_str("\n  help: ");
        text.push_str(help);
    }
    text
}

pub fn render_all(schema_src: &str, query_src: &str, diags: &[Diagnostic]) -> String {
    diags
        .iter()
        .map(|diag| {
            let (name, source) = match diag.pane {
                Pane::Schema => ("schema", schema_src),
                Pane::Query => ("query", query_src),
            };
            render(name, source, diag)
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn char_index(line: &str, needle: char) -> usize {
        line.chars()
            .position(|ch| ch == needle)
            .unwrap_or_else(|| panic!("no `{needle}` in {line:?}"))
    }

    #[test]
    fn one_missing_letter_suggests_the_name() {
        assert_eq!(closest("nme", ["salary", "name", "position"]), Some("name"));
        assert_eq!(closest("zzz", ["name", "salary"]), None);
        assert_eq!(closest("name", ["name", "salary"]), None);
    }

    #[test]
    fn caret_sits_under_the_column_it_names() {
        let source = "{\n  Player {\n    nme\n  }\n}\n";
        let diag = Diagnostic::at(
            Pane::Query,
            3,
            5,
            3,
            8,
            "Player has no field nme",
            Some("did you mean `name`?".into()),
        );
        let text = render("query", source, &diag);
        let lines: Vec<&str> = text.lines().collect();
        let source_row = lines
            .iter()
            .copied()
            .find(|line| line.trim() == "nme")
            .expect("source row");
        let caret_row = lines
            .iter()
            .copied()
            .find(|line| line.contains('^'))
            .expect("caret row");
        assert_eq!(
            char_index(source_row, 'n'),
            char_index(caret_row, '^'),
            "caret must sit under n:\n{source_row}\n{caret_row}"
        );
        assert!(text.contains("query:3:5"));
        assert!(text.contains("help: did you mean `name`?"));
        assert!(caret_row.contains("^^^"));
    }

    #[test]
    fn a_spanless_error_is_message_and_help_only() {
        let diag = Diagnostic::at(
            Pane::Query,
            0,
            0,
            0,
            0,
            "relationship traversal work budget exceeded",
            None,
        );
        assert_eq!(
            render("query", "", &diag),
            "error: relationship traversal work budget exceeded"
        );
    }
}
