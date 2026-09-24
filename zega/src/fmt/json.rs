//! JSON syntax is scanned without converting literal values. In particular,
//! neither floating point conversion nor a map may alter numbers or key order.
use super::layout::{join, Doc};

/// Format JSON with the APS 12 layout. Invalid input is returned unchanged.
pub fn format_json(source: &str) -> String {
    let mut parser = Parser { source, at: 0 };
    let Some(value) = parser.value(0) else {
        return source.to_owned();
    };
    parser.space();
    if parser.at != source.len() {
        return source.to_owned();
    }
    value.doc().render()
}

enum Value<'a> {
    Scalar(&'a str),
    Array(Vec<Self>),
    Object(Vec<(&'a str, Self)>),
}
impl Value<'_> {
    fn doc(&self) -> Doc {
        let (open, close, items, inline) = match self {
            Self::Scalar(s) => return Doc::text(*s),
            Self::Array(values) => (
                "[",
                "]",
                values.iter().map(Self::doc).collect::<Vec<_>>(),
                values.iter().all(|v| matches!(v, Self::Scalar(_))),
            ),
            Self::Object(values) => (
                "{",
                "}",
                values
                    .iter()
                    .map(|(key, value)| Doc::seq([Doc::text(*key), Doc::text(": "), value.doc()]))
                    .collect(),
                values.len() <= 2,
            ),
        };
        if items.is_empty() {
            return Doc::text(format!("{open}{close}"));
        }
        let line = if inline {
            Doc::Line(if open == "{" { " " } else { "" })
        } else {
            Doc::Hard
        };
        let separator = Doc::seq([
            Doc::text(","),
            if inline { Doc::Line(" ") } else { Doc::Hard },
        ]);
        let doc = Doc::seq([
            Doc::text(open),
            Doc::seq([line.clone(), join(items, separator)]).nest(),
            line,
            Doc::text(close),
        ]);
        if inline {
            doc.group()
        } else {
            doc
        }
    }
}
struct Parser<'a> {
    source: &'a str,
    at: usize,
}
impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.source.as_bytes().get(self.at).copied()
    }
    fn space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.at += 1;
        }
    }
    fn eat(&mut self, byte: u8) -> bool {
        self.space();
        if self.peek() == Some(byte) {
            self.at += 1;
            true
        } else {
            false
        }
    }
    fn string(&mut self) -> Option<&'a str> {
        self.space();
        let start = self.at;
        if !self.eat(b'"') {
            return None;
        }
        loop {
            match self.peek()? {
                b'"' => {
                    self.at += 1;
                    return Some(&self.source[start..self.at]);
                }
                b'\\' => {
                    self.at += 1;
                    match self.peek()? {
                        b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => self.at += 1,
                        b'u' => {
                            self.at += 1;
                            for _ in 0..4 {
                                if !self.peek()?.is_ascii_hexdigit() {
                                    return None;
                                }
                                self.at += 1;
                            }
                        }
                        _ => return None,
                    }
                }
                0..=31 => return None,
                _ => self.at += 1,
            }
        }
    }
    fn digits(&mut self) -> Option<()> {
        let start = self.at;
        while self.peek().is_some_and(|b| b.is_ascii_digit()) {
            self.at += 1;
        }
        (self.at > start).then_some(())
    }
    fn value(&mut self, depth: usize) -> Option<Value<'a>> {
        // Bound recursive layout as well as parsing on untrusted editor input.
        if depth > 128 {
            return None;
        }
        self.space();
        let start = self.at;
        match self.peek()? {
            b'"' => self.string().map(Value::Scalar),
            b'{' => {
                self.at += 1;
                let mut items = Vec::new();
                if self.eat(b'}') {
                    return Some(Value::Object(items));
                }
                loop {
                    let key = self.string()?;
                    if !self.eat(b':') {
                        return None;
                    }
                    items.push((key, self.value(depth + 1)?));
                    if self.eat(b'}') {
                        return Some(Value::Object(items));
                    }
                    if !self.eat(b',') {
                        return None;
                    }
                }
            }
            b'[' => {
                self.at += 1;
                let mut items = Vec::new();
                if self.eat(b']') {
                    return Some(Value::Array(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    if self.eat(b']') {
                        return Some(Value::Array(items));
                    }
                    if !self.eat(b',') {
                        return None;
                    }
                }
            }
            b't' | b'f' | b'n' => {
                let literal = match self.peek()? {
                    b't' => "true",
                    b'f' => "false",
                    _ => "null",
                };
                if !self.source[self.at..].starts_with(literal) {
                    return None;
                }
                self.at += literal.len();
                Some(Value::Scalar(&self.source[start..self.at]))
            }
            b'-' | b'0'..=b'9' => {
                if self.peek() == Some(b'-') {
                    self.at += 1;
                }
                if self.peek() == Some(b'0') {
                    self.at += 1;
                } else {
                    self.digits()?;
                }
                if self.peek() == Some(b'.') {
                    self.at += 1;
                    self.digits()?;
                }
                if matches!(self.peek(), Some(b'e' | b'E')) {
                    self.at += 1;
                    if matches!(self.peek(), Some(b'+' | b'-')) {
                        self.at += 1;
                    }
                    self.digits()?;
                }
                Some(Value::Scalar(&self.source[start..self.at]))
            }
            _ => None,
        }
    }
}
