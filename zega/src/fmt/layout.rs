//! A small document algebra: the AST chooses blocks; token leaves keep spelling
//! (notably float32 vectors and ZQL string escapes) and positional comments.
#[derive(Clone)]
pub(super) enum Doc {
    Text(String),
    Comment(String, bool),
    Line(&'static str),
    Hard,
    Blank,
    Seq(Vec<Doc>),
    Nest(Box<Doc>),
    Group(Box<Doc>),
}
impl Doc {
    pub fn text(s: impl Into<String>) -> Self {
        Self::Text(s.into())
    }
    pub fn seq(items: impl IntoIterator<Item = Self>) -> Self {
        Self::Seq(items.into_iter().collect())
    }
    pub fn nest(self) -> Self {
        Self::Nest(Box::new(self))
    }
    pub fn group(self) -> Self {
        Self::Group(Box::new(self))
    }
    pub(super) fn width(&self) -> usize {
        match self {
            Self::Text(s) => {
                if s.contains('\n') {
                    usize::MAX
                } else {
                    s.chars().count()
                }
            }
            Self::Line(s) => s.len(),
            Self::Hard | Self::Blank | Self::Comment(..) => usize::MAX,
            Self::Seq(items) => items
                .iter()
                .fold(0usize, |n, d| n.saturating_add(d.width())),
            Self::Nest(d) | Self::Group(d) => d.width(),
        }
    }
    pub fn render(&self) -> String {
        fn write(d: &Doc, out: &mut String, indent: usize, flat: bool) {
            match d {
                Doc::Text(s) => {
                    if s.is_empty() {
                        return;
                    }
                    if out.ends_with('\n') || out.is_empty() {
                        out.push_str(&"  ".repeat(indent));
                    }
                    out.push_str(s);
                }
                Doc::Comment(s, inline) => {
                    while out.ends_with([' ', '\t']) {
                        out.pop();
                    }
                    if *inline && !out.is_empty() && !out.ends_with('\n') {
                        out.push(' ');
                    } else {
                        if !out.is_empty() && !out.ends_with('\n') {
                            out.push('\n');
                        }
                        out.push_str(&"  ".repeat(indent));
                    }
                    out.push_str(s);
                    out.push('\n');
                }
                Doc::Line(s) if flat => out.push_str(s),
                Doc::Line(_) | Doc::Hard => {
                    while out.ends_with([' ', '\t']) {
                        out.pop();
                    }
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                }
                Doc::Blank => {
                    while out.ends_with(char::is_whitespace) {
                        out.pop();
                    }
                    out.push_str("\n\n");
                }
                Doc::Seq(items) => {
                    for item in items {
                        write(item, out, indent, flat);
                    }
                }
                Doc::Nest(d) => write(d, out, indent + 1, flat),
                Doc::Group(d) => {
                    let col = if out.ends_with('\n') || out.is_empty() {
                        indent * 2
                    } else {
                        out.rsplit('\n').next().unwrap_or("").chars().count()
                    };
                    write(d, out, indent, flat || d.width().saturating_add(col) <= 80);
                }
            }
        }
        let mut out = String::new();
        write(self, &mut out, 0, false);
        while out.ends_with(char::is_whitespace) {
            out.pop();
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out
    }
}

#[derive(Clone, Debug)]
pub(super) struct Token<'a> {
    pub text: &'a str,
    pub start: usize,
    pub end: usize,
    pub inline_comment: bool,
}

// ZQL only has double-quoted strings and // comments. Keep both opaque: a URL
// or escaped quote must never turn into comment trivia or get re-escaped.
pub(super) fn tokens(source: &str) -> Vec<Token<'_>> {
    let mut out: Vec<Token<'_>> = Vec::new();
    let mut i = 0;
    while i < source.len() {
        let start = i;
        let rest = &source[i..];
        let c = rest.chars().next().unwrap();
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if rest.starts_with("//") {
            i += rest.find('\n').unwrap_or(rest.len());
        } else if c == '"' {
            i += 1;
            while i < source.len() {
                let c = source[i..].chars().next().unwrap();
                i += c.len_utf8();
                if c == '\\' {
                    if let Some(c) = source[i..].chars().next() {
                        i += c.len_utf8();
                    }
                } else if c == '"' {
                    break;
                }
            }
        } else if c.is_ascii_alphanumeric()
            || c == '_'
            || (c == '-' && rest.as_bytes().get(1).is_some_and(u8::is_ascii_digit))
        {
            i += c.len_utf8();
            while i < source.len() {
                let c = source.as_bytes()[i];
                if c.is_ascii_alphanumeric()
                    || c == b'_'
                    || (c == b'.' && source.as_bytes().get(i + 1).is_some_and(u8::is_ascii_digit))
                    || (source.as_bytes()[start].is_ascii_digit()
                        || source.as_bytes()[start] == b'-')
                        && (matches!(c, b'+' | b'-')
                            && matches!(source.as_bytes()[i - 1], b'e' | b'E'))
                {
                    i += 1;
                } else {
                    break;
                }
            }
        } else if ["->", "<-", "..", "&&", "||", "!=", "<=", ">=", "<>"]
            .iter()
            .any(|s| rest.starts_with(s))
        {
            i += 2;
        } else {
            i += c.len_utf8();
        }
        let inline_comment = source[start..i].starts_with("//")
            && out
                .last()
                .is_some_and(|last| !source[last.end..start].contains('\n'));
        out.push(Token {
            text: &source[start..i],
            start,
            end: i,
            inline_comment,
        });
    }
    out
}

pub(super) fn join(items: Vec<Doc>, separator: Doc) -> Doc {
    let mut out = Vec::new();
    for item in items {
        if !out.is_empty() {
            out.push(separator.clone());
        }
        out.push(item);
    }
    Doc::seq(out)
}

/// Source leaves, including parentheses/arguments. Comments remain attached to
/// their written line, including end-of-line comments.
pub(super) fn fragment(ts: &[Token<'_>], types: bool) -> Doc {
    fn spaced(prev: &str, next: &str, types: bool) -> bool {
        if prev.is_empty()
            || matches!(prev, "(" | "[" | "@" | "&" | "$" | ".." | "*")
            || matches!(next, ")" | "]" | "," | ":" | "?" | "..")
        {
            return false;
        }
        if types && (matches!(prev, "<") || matches!(next, "<" | ">")) {
            return false;
        }
        if next == "(" {
            return matches!(prev, "from" | "->" | "<-" | "|" | "&&" | "||");
        }
        if next == "[" {
            return !matches!(prev, "vector") && !types;
        }
        true
    }
    fn sequence(ts: &[Token<'_>], types: bool) -> Doc {
        let mut docs = Vec::new();
        let mut i = 0;
        let mut prev = "";
        while i < ts.len() {
            let t = ts[i].text;
            if t.starts_with("//") {
                docs.push(Doc::Comment(t.trim_end().to_owned(), ts[i].inline_comment));
                prev = "";
                i += 1;
                continue;
            }
            if spaced(prev, t, types) {
                docs.push(Doc::text(" "));
            }
            if matches!(t, "(" | "[") {
                let close = if t == "(" { ")" } else { "]" };
                let mut depth = 1;
                let mut end = i + 1;
                while end < ts.len() {
                    if ts[end].text == t {
                        depth += 1;
                    }
                    if ts[end].text == close {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    end += 1;
                }
                if end < ts.len() {
                    let inner = sequence(&ts[i + 1..end], types);
                    docs.push(
                        Doc::seq([
                            Doc::text(t),
                            Doc::seq([Doc::Line(""), inner]).nest(),
                            Doc::Line(""),
                            Doc::text(close),
                        ])
                        .group(),
                    );
                    prev = close;
                    i = end + 1;
                    continue;
                }
            }
            docs.push(Doc::text(t));
            if matches!(t, "," | "&&" | "||")
                && ts.get(i + 1).is_some_and(|next| !next.inline_comment)
            {
                docs.push(Doc::Line(" "));
                prev = "";
            } else {
                prev = t;
            }
            i += 1;
        }
        Doc::seq(docs)
    }
    sequence(ts, types).group()
}
