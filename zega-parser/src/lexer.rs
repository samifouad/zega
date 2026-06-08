use std::iter::Peekable;
use std::str::Chars;
use thiserror::Error;

#[derive(Error, Debug, PartialEq)]
pub enum LexError {
    #[error("integer literal is out of range for i64: {0}")]
    IntegerOutOfRange(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    // Keywords
    Match,
    Return,
    Create,
    Merge,
    Set,
    Delete,
    Where,
    Limit,
    Order,
    By,
    Asc,
    Desc,
    On,
    Key,
    Get,
    Del,
    Incr,
    Ttl,
    // Identifiers / literals
    Identifier(String),
    StringLiteral(String),
    Integer(i64),
    Float(f64),
    Bool(bool),
    Null,
    // Symbols
    Colon,
    Comma,
    Semicolon,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Arrow,     // ->
    LeftArrow, // <-
    Dash,      // -
    Dot,
    Dollar, // $
    Star,
    Eq,
    Ne, // !=
    Gt,
    Lt,
    Gte, // >=
    Lte, // <=
    Plus,
    Eof,
}

pub struct Lexer<'a> {
    input: &'a str,
    chars: Peekable<Chars<'a>>,
    pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Self {
        Lexer {
            input,
            chars: input.chars().peekable(),
            pos: 0,
        }
    }

    fn advance(&mut self) -> Option<char> {
        self.pos += 1;
        self.chars.next()
    }

    fn peek(&mut self) -> Option<&char> {
        self.chars.peek()
    }

    fn skip_whitespace(&mut self) {
        while let Some(&c) = self.peek() {
            if c.is_whitespace() {
                self.advance();
            } else if c == '/' {
                // Check for comment
                let pos = self.pos;
                let next_two: String = self.input[pos..].chars().take(2).collect();
                if next_two.starts_with("//") {
                    while let Some(&c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.advance();
                    }
                } else {
                    break;
                }
            } else {
                break;
            }
        }
    }

    fn read_string(&mut self, quote: char) -> Token {
        self.advance(); // consume opening quote
        let mut s = String::new();
        while let Some(c) = self.advance() {
            if c == quote {
                return Token::StringLiteral(s);
            }
            if c == '\\' {
                if let Some(next) = self.advance() {
                    match next {
                        'n' => s.push('\n'),
                        'r' => s.push('\r'),
                        't' => s.push('\t'),
                        '\\' => s.push('\\'),
                        '"' => s.push('"'),
                        '\'' => s.push('\''),
                        _ => s.push(next),
                    }
                }
            } else {
                s.push(c);
            }
        }
        Token::StringLiteral(s)
    }

    fn read_number(&mut self, first: char) -> Result<Token, LexError> {
        let mut s = String::new();
        s.push(first);
        while let Some(&c) = self.peek() {
            if c.is_ascii_digit() || c == '_' {
                s.push(self.advance().unwrap());
            } else if c == '.'
                && self
                    .chars
                    .clone()
                    .nth(1)
                    .is_some_and(|next| next.is_ascii_digit())
            {
                s.push(self.advance().unwrap());
                while let Some(&c) = self.peek() {
                    if c.is_ascii_digit() || c == '_' {
                        s.push(self.advance().unwrap());
                    } else {
                        break;
                    }
                }
                return Ok(Token::Float(s.replace('_', "").parse().unwrap_or(0.0)));
            } else {
                break;
            }
        }
        let normalized = s.replace('_', "");
        normalized
            .parse()
            .map(Token::Integer)
            .map_err(|_| LexError::IntegerOutOfRange(s))
    }

    fn read_identifier(&mut self, first: char) -> Token {
        let mut s = String::new();
        s.push(first);
        while let Some(&c) = self.peek() {
            if c.is_alphanumeric() || c == '_' {
                s.push(self.advance().unwrap());
            } else {
                break;
            }
        }
        let upper = s.to_ascii_uppercase();
        match upper.as_str() {
            "MATCH" => Token::Match,
            "RETURN" => Token::Return,
            "CREATE" => Token::Create,
            "MERGE" => Token::Merge,
            "SET" => Token::Set,
            "DELETE" => Token::Delete,
            "WHERE" => Token::Where,
            "LIMIT" => Token::Limit,
            "ORDER" => Token::Order,
            "BY" => Token::By,
            "ASC" => Token::Asc,
            "DESC" => Token::Desc,
            "ON" => Token::On,
            "KEY" => Token::Key,
            "GET" => Token::Get,
            "DEL" => Token::Del,
            "INCR" => Token::Incr,
            "TTL" => Token::Ttl,
            "TRUE" => Token::Bool(true),
            "FALSE" => Token::Bool(false),
            "NULL" => Token::Null,
            _ => Token::Identifier(s),
        }
    }

    pub fn next_token(&mut self) -> Result<Token, LexError> {
        self.skip_whitespace();
        match self.peek() {
            None => Ok(Token::Eof),
            Some(&c) => {
                let token = match c {
                    '(' => {
                        self.advance();
                        Token::LParen
                    }
                    ')' => {
                        self.advance();
                        Token::RParen
                    }
                    '{' => {
                        self.advance();
                        Token::LBrace
                    }
                    '}' => {
                        self.advance();
                        Token::RBrace
                    }
                    '[' => {
                        self.advance();
                        Token::LBracket
                    }
                    ']' => {
                        self.advance();
                        Token::RBracket
                    }
                    ',' => {
                        self.advance();
                        Token::Comma
                    }
                    ';' => {
                        self.advance();
                        Token::Semicolon
                    }
                    ':' => {
                        self.advance();
                        Token::Colon
                    }
                    '.' => {
                        self.advance();
                        Token::Dot
                    }
                    '$' => {
                        self.advance();
                        Token::Dollar
                    }
                    '*' => {
                        self.advance();
                        Token::Star
                    }
                    '+' => {
                        self.advance();
                        Token::Plus
                    }
                    '-' => {
                        self.advance();
                        if let Some(&'>') = self.peek() {
                            self.advance();
                            Token::Arrow
                        } else {
                            Token::Dash
                        }
                    }
                    '=' => {
                        self.advance();
                        if let Some(&'=') = self.peek() {
                            self.advance();
                        }
                        Token::Eq
                    }
                    '!' => {
                        self.advance();
                        if let Some(&'=') = self.peek() {
                            self.advance();
                            Token::Ne
                        } else {
                            Token::Dash // fallback
                        }
                    }
                    '>' => {
                        self.advance();
                        if let Some(&'=') = self.peek() {
                            self.advance();
                            Token::Gte
                        } else {
                            Token::Gt
                        }
                    }
                    '<' => {
                        self.advance();
                        if let Some(&'=') = self.peek() {
                            self.advance();
                            Token::Lte
                        } else if let Some(&'-') = self.peek() {
                            self.advance();
                            Token::LeftArrow
                        } else {
                            Token::Lt
                        }
                    }
                    '"' | '\'' => self.read_string(c),
                    c if c.is_ascii_digit() => {
                        let ch = self.advance().unwrap();
                        return self.read_number(ch);
                    }
                    c if c.is_alphabetic() || c == '_' => {
                        let ch = self.advance().unwrap();
                        self.read_identifier(ch)
                    }
                    _ => {
                        self.advance();
                        return self.next_token();
                    }
                };
                Ok(token)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_tokens() {
        let mut lex = Lexer::new("MATCH (n:Label) RETURN n");
        assert_eq!(lex.next_token().unwrap(), Token::Match);
        assert_eq!(lex.next_token().unwrap(), Token::LParen);
        assert_eq!(
            lex.next_token().unwrap(),
            Token::Identifier("n".to_string())
        );
        assert_eq!(lex.next_token().unwrap(), Token::Colon);
        assert_eq!(
            lex.next_token().unwrap(),
            Token::Identifier("Label".to_string())
        );
        assert_eq!(lex.next_token().unwrap(), Token::RParen);
        assert_eq!(lex.next_token().unwrap(), Token::Return);
        assert_eq!(
            lex.next_token().unwrap(),
            Token::Identifier("n".to_string())
        );
        assert_eq!(lex.next_token().unwrap(), Token::Eof);
    }

    #[test]
    fn test_string_literal() {
        let mut lex = Lexer::new("'hello world'");
        assert_eq!(
            lex.next_token().unwrap(),
            Token::StringLiteral("hello world".to_string())
        );
    }

    #[test]
    fn test_number() {
        let mut lex = Lexer::new("42 3.125");
        assert_eq!(lex.next_token().unwrap(), Token::Integer(42));
        assert_eq!(lex.next_token().unwrap(), Token::Float(3.125));
    }
}
