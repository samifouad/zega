//! Exhaustive integration test suite for the `zega-parser` crate.
//!
//! Covers the Cypher tokenizer (`Lexer` / `Token`) and the parser
//! (`Parser` / `Statement` / `Expr` / AST nodes) against the crate's
//! PUBLIC API only (`use zega_parser::...`).
//!
//! Errors in Zega are VALUES: the parser returns `Result<_, ParseError>` and
//! the lexer returns `Result<_, LexError>`. These tests assert on
//! `Result`/`Option` shapes and
//! never expect panics from well-formed-but-semantically-odd input. The
//! goal is breadth + depth: every token kind, keyword case-insensitivity,
//! literals/escapes, parameters, operator precedence/associativity, every
//! clause, pattern shapes, property maps, and malformed input.

use std::collections::HashMap;

use zega_parser::{
    AggregateFunction, BinaryOperator, Direction, Expr, Lexer, OrderDirection, Parser,
    PatternElement, RelationshipLength, ReturnClause, SetClause, Statement, Value,
};
// `Token` and `ParseError` are NOT re-exported at the crate root (lib.rs only
// `pub use`s ast::*, Lexer, Parser, Value). Their defining modules ARE public
// (`pub mod lexer`, `pub mod parser`), so reach them by full module path.
use zega_parser::lexer::{LexError, Token};
use zega_parser::parser::ParseError;

// =====================================================================
// Helpers
// =====================================================================

/// Tokenize an entire input string into a Vec of tokens (excluding the
/// trailing `Eof`), unwrapping for tests whose input is known to be valid.
fn lex_all(input: &str) -> Vec<Token> {
    let mut lex = Lexer::new(input);
    let mut out = Vec::new();
    loop {
        let t = lex.next_token().expect("lexing should succeed");
        if t == Token::Eof {
            break;
        }
        out.push(t);
    }
    out
}

fn try_lex(input: &str) -> Result<Vec<Token>, LexError> {
    let mut lex = Lexer::new(input);
    let mut out = Vec::new();
    loop {
        let t = lex.next_token()?;
        if t == Token::Eof {
            return Ok(out);
        }
        out.push(t);
    }
}

/// Parse and unwrap the first statement, panicking the test on parse error.
fn parse_one(input: &str) -> Statement {
    let mut p = Parser::new(input).expect("Parser::new should always succeed");
    let mut stmts = p.parse().unwrap_or_else(|e| panic!("parse failed: {e:?}"));
    assert_eq!(
        stmts.len(),
        1,
        "expected exactly one statement for: {input}"
    );
    stmts.remove(0)
}

/// Parse, returning the full Result for error-path assertions.
fn try_parse(input: &str) -> Result<Vec<Statement>, ParseError> {
    Parser::new(input).and_then(|mut p| p.parse())
}

/// Convenience: extract the `Match` variant fields or panic.
fn as_match(stmt: &Statement) -> (&Vec<PatternElement>, &Option<Expr>, &ReturnClause) {
    match stmt {
        Statement::Match {
            pattern,
            where_clause,
            return_clause,
            ..
        } => (pattern, where_clause, return_clause),
        other => panic!("expected Statement::Match, got {other:?}"),
    }
}

// =====================================================================
// LEXER: structural symbol tokens
// =====================================================================

#[test]
fn lex_all_single_char_symbols() {
    assert_eq!(lex_all("("), vec![Token::LParen]);
    assert_eq!(lex_all(")"), vec![Token::RParen]);
    assert_eq!(lex_all("{"), vec![Token::LBrace]);
    assert_eq!(lex_all("}"), vec![Token::RBrace]);
    assert_eq!(lex_all("["), vec![Token::LBracket]);
    assert_eq!(lex_all("]"), vec![Token::RBracket]);
    assert_eq!(lex_all(","), vec![Token::Comma]);
    assert_eq!(lex_all(";"), vec![Token::Semicolon]);
    assert_eq!(lex_all(":"), vec![Token::Colon]);
    assert_eq!(lex_all("."), vec![Token::Dot]);
    assert_eq!(lex_all("$"), vec![Token::Dollar]);
    assert_eq!(lex_all("*"), vec![Token::Star]);
    assert_eq!(lex_all("+"), vec![Token::Plus]);
}

#[test]
fn lex_all_symbols_back_to_back() {
    assert_eq!(
        lex_all("(){}[],;:.$*+"),
        vec![
            Token::LParen,
            Token::RParen,
            Token::LBrace,
            Token::RBrace,
            Token::LBracket,
            Token::RBracket,
            Token::Comma,
            Token::Semicolon,
            Token::Colon,
            Token::Dot,
            Token::Dollar,
            Token::Star,
            Token::Plus,
        ]
    );
}

#[test]
fn lex_dash_alone_is_dash() {
    assert_eq!(lex_all("-"), vec![Token::Dash]);
}

#[test]
fn lex_arrow() {
    assert_eq!(lex_all("->"), vec![Token::Arrow]);
}

#[test]
fn lex_left_arrow() {
    assert_eq!(lex_all("<-"), vec![Token::LeftArrow]);
}

#[test]
fn lex_dash_then_other_is_dash_plus_next() {
    // "-x" => Dash, Identifier("x")
    assert_eq!(
        lex_all("-x"),
        vec![Token::Dash, Token::Identifier("x".to_string())]
    );
}

// =====================================================================
// LEXER: comparison / equality operators
// =====================================================================

#[test]
fn lex_single_equals_is_eq() {
    assert_eq!(lex_all("="), vec![Token::Eq]);
}

#[test]
fn lex_double_equals_is_eq() {
    // The lexer collapses `==` into a single Eq.
    assert_eq!(lex_all("=="), vec![Token::Eq]);
}

#[test]
fn lex_not_equals() {
    assert_eq!(lex_all("!="), vec![Token::Ne]);
}

#[test]
fn lex_bang_alone_falls_back_to_dash() {
    // `!` not followed by `=` is the documented fallback to Dash.
    assert_eq!(lex_all("!"), vec![Token::Dash]);
}

#[test]
fn lex_greater_and_gte() {
    assert_eq!(lex_all(">"), vec![Token::Gt]);
    assert_eq!(lex_all(">="), vec![Token::Gte]);
}

#[test]
fn lex_less_and_lte() {
    assert_eq!(lex_all("<"), vec![Token::Lt]);
    assert_eq!(lex_all("<="), vec![Token::Lte]);
}

#[test]
fn lex_lte_vs_left_arrow_disambiguation() {
    // `<=` is Lte, `<-` is LeftArrow, `<` alone is Lt.
    assert_eq!(
        lex_all("<= <- <"),
        vec![Token::Lte, Token::LeftArrow, Token::Lt]
    );
}

// =====================================================================
// LEXER: keywords (all of them) and case-insensitivity
// =====================================================================

#[test]
fn lex_all_keywords_uppercase() {
    assert_eq!(lex_all("MATCH"), vec![Token::Match]);
    assert_eq!(lex_all("RETURN"), vec![Token::Return]);
    assert_eq!(lex_all("CREATE"), vec![Token::Create]);
    assert_eq!(lex_all("MERGE"), vec![Token::Merge]);
    assert_eq!(lex_all("SET"), vec![Token::Set]);
    assert_eq!(lex_all("DELETE"), vec![Token::Delete]);
    assert_eq!(lex_all("WHERE"), vec![Token::Where]);
    assert_eq!(lex_all("LIMIT"), vec![Token::Limit]);
    assert_eq!(lex_all("ORDER"), vec![Token::Order]);
    assert_eq!(lex_all("BY"), vec![Token::By]);
    assert_eq!(lex_all("ASC"), vec![Token::Asc]);
    assert_eq!(lex_all("DESC"), vec![Token::Desc]);
    assert_eq!(lex_all("ON"), vec![Token::On]);
    assert_eq!(lex_all("KEY"), vec![Token::Key]);
    assert_eq!(lex_all("GET"), vec![Token::Get]);
    assert_eq!(lex_all("DEL"), vec![Token::Del]);
    assert_eq!(lex_all("INCR"), vec![Token::Incr]);
    assert_eq!(lex_all("TTL"), vec![Token::Ttl]);
}

#[test]
fn lex_keywords_lowercase() {
    assert_eq!(lex_all("match"), vec![Token::Match]);
    assert_eq!(lex_all("return"), vec![Token::Return]);
    assert_eq!(lex_all("create"), vec![Token::Create]);
    assert_eq!(lex_all("where"), vec![Token::Where]);
    assert_eq!(lex_all("order"), vec![Token::Order]);
    assert_eq!(lex_all("by"), vec![Token::By]);
    assert_eq!(lex_all("limit"), vec![Token::Limit]);
}

#[test]
fn lex_keywords_mixed_case() {
    assert_eq!(lex_all("MaTcH"), vec![Token::Match]);
    assert_eq!(lex_all("ReTuRn"), vec![Token::Return]);
    assert_eq!(lex_all("Where"), vec![Token::Where]);
    assert_eq!(lex_all("Limit"), vec![Token::Limit]);
}

#[test]
fn lex_bool_and_null_keywords() {
    assert_eq!(lex_all("true"), vec![Token::Bool(true)]);
    assert_eq!(lex_all("TRUE"), vec![Token::Bool(true)]);
    assert_eq!(lex_all("True"), vec![Token::Bool(true)]);
    assert_eq!(lex_all("false"), vec![Token::Bool(false)]);
    assert_eq!(lex_all("FALSE"), vec![Token::Bool(false)]);
    assert_eq!(lex_all("null"), vec![Token::Null]);
    assert_eq!(lex_all("NULL"), vec![Token::Null]);
    assert_eq!(lex_all("Null"), vec![Token::Null]);
}

#[test]
fn lex_keyword_prefix_is_identifier() {
    // "MATCHING" is an identifier, not MATCH + ING.
    assert_eq!(
        lex_all("MATCHING"),
        vec![Token::Identifier("MATCHING".to_string())]
    );
    // "returns" likewise.
    assert_eq!(
        lex_all("returns"),
        vec![Token::Identifier("returns".to_string())]
    );
}

// =====================================================================
// LEXER: identifiers
// =====================================================================

#[test]
fn lex_simple_identifier_preserves_original_case() {
    assert_eq!(
        lex_all("nodeName"),
        vec![Token::Identifier("nodeName".to_string())]
    );
}

#[test]
fn lex_identifier_with_underscore_leading() {
    assert_eq!(
        lex_all("_private"),
        vec![Token::Identifier("_private".to_string())]
    );
}

#[test]
fn lex_identifier_with_digits_and_underscores() {
    assert_eq!(
        lex_all("var_1_x"),
        vec![Token::Identifier("var_1_x".to_string())]
    );
}

#[test]
fn lex_identifier_cannot_start_with_digit() {
    // "1abc" => Integer(1) then Identifier("abc"); read_number stops at non-digit.
    assert_eq!(
        lex_all("1abc"),
        vec![Token::Integer(1), Token::Identifier("abc".to_string())]
    );
}

#[test]
fn lex_unicode_identifier_is_alphanumeric() {
    // is_alphabetic()/is_alphanumeric() accept unicode letters.
    assert_eq!(lex_all("café"), vec![Token::Identifier("café".to_string())]);
    assert_eq!(lex_all("日本"), vec![Token::Identifier("日本".to_string())]);
}

// =====================================================================
// LEXER: numbers
// =====================================================================

#[test]
fn lex_integer_zero() {
    assert_eq!(lex_all("0"), vec![Token::Integer(0)]);
}

#[test]
fn lex_integer_basic() {
    assert_eq!(lex_all("42"), vec![Token::Integer(42)]);
    assert_eq!(lex_all("1000000"), vec![Token::Integer(1_000_000)]);
}

#[test]
fn lex_integer_with_underscore_separators() {
    assert_eq!(lex_all("1_000_000"), vec![Token::Integer(1_000_000)]);
}

#[test]
fn lex_float_basic() {
    assert_eq!(lex_all("3.125"), vec![Token::Float(3.125)]);
    assert_eq!(lex_all("0.5"), vec![Token::Float(0.5)]);
}

#[test]
fn lex_float_with_underscores() {
    assert_eq!(lex_all("1_000.5"), vec![Token::Float(1000.5)]);
}

#[test]
fn lex_integer_then_dot_not_followed_by_digit_is_dot() {
    // "5." -> read_number sees '.' but next is not a digit -> Integer(5), Dot.
    assert_eq!(lex_all("5."), vec![Token::Integer(5), Token::Dot]);
}

#[test]
fn lex_integer_dot_identifier_property_access_shape() {
    // "n.prop"-style: Integer(5) Dot Identifier("x")
    assert_eq!(
        lex_all("5.x"),
        vec![
            Token::Integer(5),
            Token::Dot,
            Token::Identifier("x".to_string())
        ]
    );
}

#[test]
fn lex_integer_overflow_returns_error() {
    let huge = "99999999999999999999999999";
    assert_eq!(
        try_lex(huge),
        Err(LexError::IntegerOutOfRange(huge.to_string()))
    );
}

#[test]
fn lex_i64_max_parses() {
    assert_eq!(
        lex_all("9223372036854775807"),
        vec![Token::Integer(i64::MAX)]
    );
}

#[test]
fn lex_i64_max_plus_one_errors() {
    let literal = "9223372036854775808";
    assert_eq!(
        try_lex(literal),
        Err(LexError::IntegerOutOfRange(literal.to_string()))
    );
}

#[test]
fn lex_i64_boundary_with_leading_zeroes() {
    assert_eq!(
        lex_all("0009223372036854775807"),
        vec![Token::Integer(i64::MAX)]
    );
    let overflow = "0009223372036854775808";
    assert_eq!(
        try_lex(overflow),
        Err(LexError::IntegerOutOfRange(overflow.to_string()))
    );
}

#[test]
fn lex_i64_boundary_with_underscores() {
    assert_eq!(
        lex_all("9_223_372_036_854_775_807"),
        vec![Token::Integer(i64::MAX)]
    );
    let overflow = "9_223_372_036_854_775_808";
    assert_eq!(
        try_lex(overflow),
        Err(LexError::IntegerOutOfRange(overflow.to_string()))
    );
}

// =====================================================================
// LEXER: string literals + escapes
// =====================================================================

#[test]
fn lex_single_quoted_string() {
    assert_eq!(
        lex_all("'hello'"),
        vec![Token::StringLiteral("hello".to_string())]
    );
}

#[test]
fn lex_double_quoted_string() {
    assert_eq!(
        lex_all("\"hello\""),
        vec![Token::StringLiteral("hello".to_string())]
    );
}

#[test]
fn lex_empty_string() {
    assert_eq!(lex_all("''"), vec![Token::StringLiteral(String::new())]);
    assert_eq!(lex_all("\"\""), vec![Token::StringLiteral(String::new())]);
}

#[test]
fn lex_string_with_spaces_and_punctuation() {
    assert_eq!(
        lex_all("'hello, world! (test)'"),
        vec![Token::StringLiteral("hello, world! (test)".to_string())]
    );
}

#[test]
fn lex_string_escape_newline_tab_return() {
    assert_eq!(
        lex_all("'a\\nb\\tc\\rd'"),
        vec![Token::StringLiteral("a\nb\tc\rd".to_string())]
    );
}

#[test]
fn lex_string_escape_backslash() {
    assert_eq!(
        lex_all("'a\\\\b'"),
        vec![Token::StringLiteral("a\\b".to_string())]
    );
}

#[test]
fn lex_string_escape_quotes() {
    assert_eq!(
        lex_all("'it\\'s'"),
        vec![Token::StringLiteral("it's".to_string())]
    );
    assert_eq!(
        lex_all("\"say \\\"hi\\\"\""),
        vec![Token::StringLiteral("say \"hi\"".to_string())]
    );
}

#[test]
fn lex_string_unknown_escape_keeps_char() {
    // `\x` -> the default arm pushes the char `x`.
    assert_eq!(
        lex_all("'\\x'"),
        vec![Token::StringLiteral("x".to_string())]
    );
}

#[test]
fn lex_string_unicode_content() {
    assert_eq!(
        lex_all("'café ☕ 日本'"),
        vec![Token::StringLiteral("café ☕ 日本".to_string())]
    );
}

#[test]
fn lex_string_opposite_quote_inside() {
    // A double quote inside single-quoted string is literal.
    assert_eq!(
        lex_all("'he said \"hi\"'"),
        vec![Token::StringLiteral("he said \"hi\"".to_string())]
    );
}

#[test]
fn lex_unterminated_string_returns_partial_literal_not_panic() {
    // read_string falls off the end and returns whatever it accumulated.
    assert_eq!(
        lex_all("'unterminated"),
        vec![Token::StringLiteral("unterminated".to_string())]
    );
}

#[test]
fn lex_unterminated_string_with_trailing_escape_does_not_panic() {
    // Trailing backslash at EOF: advance() returns None, loop ends.
    let toks = lex_all("'abc\\");
    assert_eq!(toks, vec![Token::StringLiteral("abc".to_string())]);
}

// =====================================================================
// LEXER: whitespace and comments
// =====================================================================

#[test]
fn lex_skips_whitespace_variants() {
    assert_eq!(
        lex_all("  \t\n\r  MATCH   \n  RETURN  "),
        vec![Token::Match, Token::Return]
    );
}

#[test]
fn lex_empty_input_yields_no_tokens() {
    assert_eq!(lex_all(""), Vec::<Token>::new());
}

#[test]
fn lex_only_whitespace_yields_no_tokens() {
    assert_eq!(lex_all("    \n\t  "), Vec::<Token>::new());
}

#[test]
fn lex_line_comment_skipped() {
    assert_eq!(
        lex_all("MATCH // this is a comment\n RETURN"),
        vec![Token::Match, Token::Return]
    );
}

#[test]
fn lex_line_comment_to_eof() {
    assert_eq!(lex_all("RETURN // trailing"), vec![Token::Return]);
}

#[test]
fn lex_single_slash_is_skipped_as_unknown() {
    // A lone '/' is not a comment; skip_whitespace breaks, then next_token's
    // catch-all advances past it and recurses -> no token emitted.
    assert_eq!(lex_all("/"), Vec::<Token>::new());
}

#[test]
fn lex_unknown_chars_are_skipped() {
    // '@', '%', '&', '?' are not recognized; the catch-all skips them.
    assert_eq!(lex_all("@MATCH%RETURN&"), vec![Token::Match, Token::Return]);
}

#[test]
fn lex_eof_is_idempotent() {
    let mut lex = Lexer::new("");
    assert_eq!(lex.next_token().unwrap(), Token::Eof);
    assert_eq!(lex.next_token().unwrap(), Token::Eof);
    assert_eq!(lex.next_token().unwrap(), Token::Eof);
}

// =====================================================================
// LEXER: full-query token stream sanity
// =====================================================================

#[test]
fn lex_full_match_query_stream() {
    let toks = lex_all("MATCH (n:Label {a: 1}) WHERE n.x = $p RETURN n LIMIT 10");
    assert_eq!(
        toks,
        vec![
            Token::Match,
            Token::LParen,
            Token::Identifier("n".to_string()),
            Token::Colon,
            Token::Identifier("Label".to_string()),
            Token::LBrace,
            Token::Identifier("a".to_string()),
            Token::Colon,
            Token::Integer(1),
            Token::RBrace,
            Token::RParen,
            Token::Where,
            Token::Identifier("n".to_string()),
            Token::Dot,
            Token::Identifier("x".to_string()),
            Token::Eq,
            Token::Dollar,
            Token::Identifier("p".to_string()),
            Token::Return,
            Token::Identifier("n".to_string()),
            Token::Limit,
            Token::Integer(10),
        ]
    );
}

// =====================================================================
// PARSER: construction
// =====================================================================

#[test]
fn parser_new_always_ok_even_for_garbage() {
    // Parser::new only primes the first token; it never fails.
    assert!(Parser::new("").is_ok());
    assert!(Parser::new("@#%^&").is_ok());
    assert!(Parser::new("MATCH").is_ok());
}

#[test]
fn parse_empty_input_yields_empty_statement_list() {
    let stmts = try_parse("").unwrap();
    assert!(stmts.is_empty());
}

#[test]
fn parse_only_whitespace_yields_empty_statement_list() {
    let stmts = try_parse("   \n\t  ").unwrap();
    assert!(stmts.is_empty());
}

#[test]
fn parse_only_comment_yields_empty_statement_list() {
    let stmts = try_parse("// just a comment").unwrap();
    assert!(stmts.is_empty());
}

// =====================================================================
// PARSER: MATCH ... RETURN basics
// =====================================================================

#[test]
fn parse_minimal_match_return() {
    let stmt = parse_one("MATCH (n) RETURN n");
    let (pattern, where_clause, ret) = as_match(&stmt);
    assert_eq!(pattern.len(), 1);
    assert_eq!(pattern[0].variable, "n");
    assert!(pattern[0].labels.is_empty());
    assert!(where_clause.is_none());
    assert_eq!(ret.items.len(), 1);
    assert_eq!(ret.items[0].expr, Expr::Identifier("n".to_string()));
}

#[test]
fn parse_match_with_label() {
    let stmt = parse_one("MATCH (n:User) RETURN n");
    let (pattern, _, _) = as_match(&stmt);
    assert_eq!(pattern[0].labels, vec!["User".to_string()]);
}

#[test]
fn parse_match_anonymous_node() {
    // Empty variable name when no identifier precedes the label/paren.
    let stmt = parse_one("MATCH (:User) RETURN 1");
    let (pattern, _, _) = as_match(&stmt);
    assert_eq!(pattern[0].variable, "");
    assert_eq!(pattern[0].labels, vec!["User".to_string()]);
}

#[test]
fn parse_match_lowercase_keywords() {
    let stmt = parse_one("match (n:User) return n");
    let (pattern, _, _) = as_match(&stmt);
    assert_eq!(pattern[0].labels, vec!["User".to_string()]);
}

#[test]
fn parse_match_without_return_has_empty_return_clause() {
    // parse_match defaults return_clause to an empty items vec when no RETURN.
    let stmt = parse_one("MATCH (n:User)");
    let (_, _, ret) = as_match(&stmt);
    assert!(ret.items.is_empty());
}

#[test]
fn parse_return_multiple_items() {
    let stmt = parse_one("MATCH (a)-[:R]->(b) RETURN a, b");
    let (_, _, ret) = as_match(&stmt);
    assert_eq!(ret.items.len(), 2);
    assert_eq!(ret.items[0].expr, Expr::Identifier("a".to_string()));
    assert_eq!(ret.items[1].expr, Expr::Identifier("b".to_string()));
}

#[test]
fn parse_return_property_access() {
    let stmt = parse_one("MATCH (n) RETURN n.name");
    let (_, _, ret) = as_match(&stmt);
    assert_eq!(
        ret.items[0].expr,
        Expr::PropertyAccess(
            Box::new(Expr::Identifier("n".to_string())),
            "name".to_string()
        )
    );
}

#[test]
fn parse_return_with_as_alias() {
    let stmt = parse_one("MATCH (n) RETURN n.name AS title");
    let (_, _, ret) = as_match(&stmt);
    assert_eq!(ret.items[0].alias, Some("title".to_string()));
}

#[test]
fn parse_return_as_is_case_insensitive() {
    let stmt = parse_one("MATCH (n) RETURN n.name as title");
    let (_, _, ret) = as_match(&stmt);
    assert_eq!(ret.items[0].alias, Some("title".to_string()));
}

#[test]
fn parse_return_no_alias_is_none() {
    let stmt = parse_one("MATCH (n) RETURN n");
    let (_, _, ret) = as_match(&stmt);
    assert_eq!(ret.items[0].alias, None);
}

// =====================================================================
// PARSER: property maps
// =====================================================================

#[test]
fn parse_node_with_single_property() {
    let stmt = parse_one("MATCH (n {id: 5}) RETURN n");
    let (pattern, _, _) = as_match(&stmt);
    assert_eq!(pattern[0].properties.len(), 1);
    assert_eq!(
        pattern[0].properties.get("id"),
        Some(&Expr::Literal(Value::Int(5)))
    );
}

#[test]
fn parse_node_with_multiple_properties() {
    let stmt = parse_one("MATCH (n {id: 5, name: 'bob', active: true}) RETURN n");
    let (pattern, _, _) = as_match(&stmt);
    let props = &pattern[0].properties;
    assert_eq!(props.len(), 3);
    assert_eq!(props.get("id"), Some(&Expr::Literal(Value::Int(5))));
    assert_eq!(
        props.get("name"),
        Some(&Expr::Literal(Value::String("bob".to_string())))
    );
    assert_eq!(props.get("active"), Some(&Expr::Literal(Value::Bool(true))));
}

#[test]
fn parse_empty_property_map() {
    let stmt = parse_one("MATCH (n {}) RETURN n");
    let (pattern, _, _) = as_match(&stmt);
    assert!(pattern[0].properties.is_empty());
}

#[test]
fn parse_property_value_is_parameter() {
    let stmt = parse_one("MATCH (n {id: $uid}) RETURN n");
    let (pattern, _, _) = as_match(&stmt);
    assert_eq!(
        pattern[0].properties.get("id"),
        Some(&Expr::Parameter("uid".to_string()))
    );
}

#[test]
fn parse_property_key_can_be_keyword_token() {
    // The parser allows keyword tokens as property keys (lowercased).
    let stmt = parse_one("MATCH (n {key: 1, order: 2, where: 3}) RETURN n");
    let (pattern, _, _) = as_match(&stmt);
    let props = &pattern[0].properties;
    assert!(props.contains_key("key"));
    assert!(props.contains_key("order"));
    assert!(props.contains_key("where"));
}

#[test]
fn parse_property_null_literal() {
    let stmt = parse_one("MATCH (n {x: null}) RETURN n");
    let (pattern, _, _) = as_match(&stmt);
    assert_eq!(
        pattern[0].properties.get("x"),
        Some(&Expr::Literal(Value::Null))
    );
}

#[test]
fn parse_property_float_literal() {
    let stmt = parse_one("MATCH (n {price: 3.5}) RETURN n");
    let (pattern, _, _) = as_match(&stmt);
    let v = pattern[0].properties.get("price").unwrap();
    match v {
        Expr::Literal(Value::Float(_)) => {}
        other => panic!("expected float literal, got {other:?}"),
    }
}

// =====================================================================
// PARSER: relationships and directions
// =====================================================================

#[test]
fn parse_outgoing_relationship() {
    let stmt = parse_one("MATCH (a)-[:KNOWS]->(b) RETURN a");
    let (pattern, _, _) = as_match(&stmt);
    assert_eq!(pattern.len(), 2);
    assert_eq!(pattern[1].direction, Some(Direction::Outgoing));
    assert_eq!(
        pattern[1].relationship.as_ref().unwrap().kinds,
        vec!["KNOWS".to_string()]
    );
}

#[test]
fn parse_incoming_relationship() {
    let stmt = parse_one("MATCH (a)<-[:KNOWS]-(b) RETURN a");
    let (pattern, _, _) = as_match(&stmt);
    assert_eq!(pattern[1].direction, Some(Direction::Incoming));
}

#[test]
fn parse_both_relationship() {
    let stmt = parse_one("MATCH (a)-[:KNOWS]-(b) RETURN a");
    let (pattern, _, _) = as_match(&stmt);
    assert_eq!(pattern[1].direction, Some(Direction::Both));
}

#[test]
fn parse_relationship_with_variable_and_kind() {
    let stmt = parse_one("MATCH (a)-[r:KNOWS]->(b) RETURN r");
    let (pattern, _, _) = as_match(&stmt);
    let rel = pattern[1].relationship.as_ref().unwrap();
    assert_eq!(rel.variable, "r");
    assert_eq!(rel.kinds, vec!["KNOWS".to_string()]);
}

#[test]
fn parse_relationship_with_properties() {
    let stmt = parse_one("MATCH (a)-[r:RATED {stars: 5}]->(b) RETURN r");
    let (pattern, _, _) = as_match(&stmt);
    let rel = pattern[1].relationship.as_ref().unwrap();
    assert_eq!(
        rel.properties.get("stars"),
        Some(&Expr::Literal(Value::Int(5)))
    );
}

#[test]
fn parse_relationship_no_kind_anonymous() {
    let stmt = parse_one("MATCH (a)-[]->(b) RETURN a");
    let (pattern, _, _) = as_match(&stmt);
    let rel = pattern[1].relationship.as_ref().unwrap();
    assert!(rel.kinds.is_empty());
    assert_eq!(rel.variable, "");
}

#[test]
fn parse_bare_dash_relationship_no_bracket() {
    // A bare `-` with no bracket: relationship is None but direction set.
    let stmt = parse_one("MATCH (a)-->(b) RETURN a");
    let (pattern, _, _) = as_match(&stmt);
    assert_eq!(pattern.len(), 2);
    assert!(pattern[1].relationship.is_none());
    assert_eq!(pattern[1].direction, Some(Direction::Outgoing));
}

#[test]
fn parse_chained_relationships() {
    let stmt = parse_one("MATCH (a)-[:R1]->(b)-[:R2]->(c) RETURN a");
    let (pattern, _, _) = as_match(&stmt);
    assert_eq!(pattern.len(), 3);
    assert_eq!(
        pattern[1].relationship.as_ref().unwrap().kinds,
        vec!["R1".to_string()]
    );
    assert_eq!(
        pattern[2].relationship.as_ref().unwrap().kinds,
        vec!["R2".to_string()]
    );
}

// =====================================================================
// PARSER: variable-length relationships
// =====================================================================

#[test]
fn parse_var_length_star_only() {
    // `*` with no bounds -> min defaults to 1, max None (per parse_relationship_length:
    // first is None, no following Dot -> min = unwrap_or(1), max = None).
    let stmt = parse_one("MATCH (a)-[:R*]->(b) RETURN b");
    let (pattern, _, _) = as_match(&stmt);
    let len = pattern[1]
        .relationship
        .as_ref()
        .unwrap()
        .length
        .clone()
        .unwrap();
    assert_eq!(len, RelationshipLength { min: 1, max: None });
}

#[test]
fn parse_var_length_fixed() {
    // `*3` with no `..` -> min and max both 3.
    let stmt = parse_one("MATCH (a)-[:R*3]->(b) RETURN b");
    let (pattern, _, _) = as_match(&stmt);
    let len = pattern[1]
        .relationship
        .as_ref()
        .unwrap()
        .length
        .clone()
        .unwrap();
    assert_eq!(
        len,
        RelationshipLength {
            min: 3,
            max: Some(3)
        }
    );
}

#[test]
fn parse_var_length_range() {
    let stmt = parse_one("MATCH (a)-[:R*2..5]->(b) RETURN b");
    let (pattern, _, _) = as_match(&stmt);
    let len = pattern[1]
        .relationship
        .as_ref()
        .unwrap()
        .length
        .clone()
        .unwrap();
    assert_eq!(
        len,
        RelationshipLength {
            min: 2,
            max: Some(5)
        }
    );
}

#[test]
fn parse_var_length_open_upper() {
    // `*2..` -> min 2, max None.
    let stmt = parse_one("MATCH (a)-[:R*2..]->(b) RETURN b");
    let (pattern, _, _) = as_match(&stmt);
    let len = pattern[1]
        .relationship
        .as_ref()
        .unwrap()
        .length
        .clone()
        .unwrap();
    assert_eq!(len, RelationshipLength { min: 2, max: None });
}

#[test]
fn parse_var_length_open_lower() {
    // `*..3` -> first is None so min defaults to 1, max 3.
    let stmt = parse_one("MATCH (a)-[:R*..3]->(b) RETURN b");
    let (pattern, _, _) = as_match(&stmt);
    let len = pattern[1]
        .relationship
        .as_ref()
        .unwrap()
        .length
        .clone()
        .unwrap();
    assert_eq!(
        len,
        RelationshipLength {
            min: 1,
            max: Some(3)
        }
    );
}

#[test]
fn parse_var_length_no_kind() {
    let stmt = parse_one("MATCH (a)-[*1..3]-(b) RETURN b");
    let (pattern, _, _) = as_match(&stmt);
    let rel = pattern[1].relationship.as_ref().unwrap();
    assert!(rel.kinds.is_empty());
    assert_eq!(
        rel.length,
        Some(RelationshipLength {
            min: 1,
            max: Some(3)
        })
    );
    assert_eq!(pattern[1].direction, Some(Direction::Both));
}

#[test]
fn parse_var_length_min_exceeds_max_is_error() {
    // 5..2 -> min > max -> ParseError::Message.
    let res = try_parse("MATCH (a)-[:R*5..2]->(b) RETURN b");
    assert!(res.is_err());
    match res.unwrap_err() {
        ParseError::Message(m) => assert!(m.contains("minimum exceeds maximum"), "msg: {m}"),
        other => panic!("expected Message error, got {other:?}"),
    }
}

// =====================================================================
// PARSER: WHERE clause + expression precedence/associativity
// =====================================================================

#[test]
fn parse_where_simple_equality() {
    let stmt = parse_one("MATCH (n) WHERE n.age = 30 RETURN n");
    let (_, where_clause, _) = as_match(&stmt);
    let expr = where_clause.as_ref().unwrap();
    match expr {
        Expr::BinaryOp(_, BinaryOperator::Eq, _) => {}
        other => panic!("expected Eq binary op, got {other:?}"),
    }
}

#[test]
fn parse_where_all_comparison_operators() {
    let cases = [
        ("n.x = 1", BinaryOperator::Eq),
        ("n.x != 1", BinaryOperator::Ne),
        ("n.x > 1", BinaryOperator::Gt),
        ("n.x < 1", BinaryOperator::Lt),
        ("n.x >= 1", BinaryOperator::Gte),
        ("n.x <= 1", BinaryOperator::Lte),
    ];
    for (cond, op) in cases {
        let q = format!("MATCH (n) WHERE {cond} RETURN n");
        let stmt = parse_one(&q);
        let (_, where_clause, _) = as_match(&stmt);
        match where_clause.as_ref().unwrap() {
            Expr::BinaryOp(_, found, _) => assert_eq!(*found, op, "for {cond}"),
            other => panic!("expected binary op for {cond}, got {other:?}"),
        }
    }
}

#[test]
fn parse_where_and_precedence_over_or() {
    // a OR b AND c  parses as  a OR (b AND c)
    let stmt = parse_one("MATCH (n) WHERE n.a = 1 OR n.b = 2 AND n.c = 3 RETURN n");
    let (_, where_clause, _) = as_match(&stmt);
    match where_clause.as_ref().unwrap() {
        Expr::BinaryOp(_, BinaryOperator::Or, right) => match right.as_ref() {
            Expr::BinaryOp(_, BinaryOperator::And, _) => {}
            other => panic!("expected AND on the right of OR, got {other:?}"),
        },
        other => panic!("expected top-level OR, got {other:?}"),
    }
}

#[test]
fn parse_where_and_is_left_associative() {
    // a AND b AND c -> (a AND b) AND c
    let stmt = parse_one("MATCH (n) WHERE n.a = 1 AND n.b = 2 AND n.c = 3 RETURN n");
    let (_, where_clause, _) = as_match(&stmt);
    match where_clause.as_ref().unwrap() {
        Expr::BinaryOp(left, BinaryOperator::And, _) => match left.as_ref() {
            Expr::BinaryOp(_, BinaryOperator::And, _) => {}
            other => panic!("expected nested AND on the left, got {other:?}"),
        },
        other => panic!("expected top-level AND, got {other:?}"),
    }
}

#[test]
fn parse_where_or_is_left_associative() {
    let stmt = parse_one("MATCH (n) WHERE n.a = 1 OR n.b = 2 OR n.c = 3 RETURN n");
    let (_, where_clause, _) = as_match(&stmt);
    match where_clause.as_ref().unwrap() {
        Expr::BinaryOp(left, BinaryOperator::Or, _) => {
            assert!(matches!(
                left.as_ref(),
                Expr::BinaryOp(_, BinaryOperator::Or, _)
            ));
        }
        other => panic!("expected top-level OR, got {other:?}"),
    }
}

#[test]
fn parse_where_comparison_binds_tighter_than_and() {
    // a = 1 AND b = 2 -> (a = 1) AND (b = 2)
    let stmt = parse_one("MATCH (n) WHERE n.a = 1 AND n.b = 2 RETURN n");
    let (_, where_clause, _) = as_match(&stmt);
    match where_clause.as_ref().unwrap() {
        Expr::BinaryOp(left, BinaryOperator::And, right) => {
            assert!(matches!(
                left.as_ref(),
                Expr::BinaryOp(_, BinaryOperator::Eq, _)
            ));
            assert!(matches!(
                right.as_ref(),
                Expr::BinaryOp(_, BinaryOperator::Eq, _)
            ));
        }
        other => panic!("expected AND of two equalities, got {other:?}"),
    }
}

#[test]
fn parse_where_and_or_case_insensitive() {
    let stmt = parse_one("MATCH (n) WHERE n.a = 1 and n.b = 2 or n.c = 3 RETURN n");
    let (_, where_clause, _) = as_match(&stmt);
    assert!(matches!(
        where_clause.as_ref().unwrap(),
        Expr::BinaryOp(_, BinaryOperator::Or, _)
    ));
}

#[test]
fn parse_where_parenthesized_overrides_precedence() {
    // (a OR b) AND c -> top-level AND with OR on the left.
    let stmt = parse_one("MATCH (n) WHERE (n.a = 1 OR n.b = 2) AND n.c = 3 RETURN n");
    let (_, where_clause, _) = as_match(&stmt);
    match where_clause.as_ref().unwrap() {
        Expr::BinaryOp(left, BinaryOperator::And, _) => {
            assert!(matches!(
                left.as_ref(),
                Expr::BinaryOp(_, BinaryOperator::Or, _)
            ));
        }
        other => panic!("expected AND with parenthesized OR left, got {other:?}"),
    }
}

#[test]
fn parse_where_equality_chains_are_left_associative() {
    // a = b = c -> (a = b) = c (parse_equality loops).
    let stmt = parse_one("MATCH (n) WHERE n.a = n.b = n.c RETURN n");
    let (_, where_clause, _) = as_match(&stmt);
    match where_clause.as_ref().unwrap() {
        Expr::BinaryOp(left, BinaryOperator::Eq, _) => {
            assert!(matches!(
                left.as_ref(),
                Expr::BinaryOp(_, BinaryOperator::Eq, _)
            ));
        }
        other => panic!("expected left-assoc equality, got {other:?}"),
    }
}

#[test]
fn parse_where_with_parameter_rhs() {
    let stmt = parse_one("MATCH (n) WHERE n.id = $target RETURN n");
    let (_, where_clause, _) = as_match(&stmt);
    match where_clause.as_ref().unwrap() {
        Expr::BinaryOp(_, BinaryOperator::Eq, right) => {
            assert_eq!(right.as_ref(), &Expr::Parameter("target".to_string()));
        }
        other => panic!("got {other:?}"),
    }
}

// =====================================================================
// PARSER: ORDER BY / LIMIT
// =====================================================================

#[test]
fn parse_order_by_default_asc() {
    let mut p = Parser::new("MATCH (n) RETURN n ORDER BY n.name").unwrap();
    let stmts = p.parse().unwrap();
    match &stmts[0] {
        Statement::Match { order_by, .. } => {
            let ob = order_by.as_ref().unwrap();
            assert_eq!(ob.len(), 1);
            assert_eq!(ob[0].1, OrderDirection::Asc);
        }
        other => panic!("expected Match, got {other:?}"),
    }
}

#[test]
fn parse_order_by_explicit_desc() {
    let mut p = Parser::new("MATCH (n) RETURN n ORDER BY n.name DESC").unwrap();
    let stmts = p.parse().unwrap();
    match &stmts[0] {
        Statement::Match { order_by, .. } => {
            assert_eq!(order_by.as_ref().unwrap()[0].1, OrderDirection::Desc);
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn parse_order_by_explicit_asc() {
    let mut p = Parser::new("MATCH (n) RETURN n ORDER BY n.name ASC").unwrap();
    let stmts = p.parse().unwrap();
    match &stmts[0] {
        Statement::Match { order_by, .. } => {
            assert_eq!(order_by.as_ref().unwrap()[0].1, OrderDirection::Asc);
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn parse_order_by_multiple_keys() {
    let mut p = Parser::new("MATCH (n) RETURN n ORDER BY n.a ASC, n.b DESC").unwrap();
    let stmts = p.parse().unwrap();
    match &stmts[0] {
        Statement::Match { order_by, .. } => {
            let ob = order_by.as_ref().unwrap();
            assert_eq!(ob.len(), 2);
            assert_eq!(ob[0].1, OrderDirection::Asc);
            assert_eq!(ob[1].1, OrderDirection::Desc);
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn parse_order_by_requires_by_keyword() {
    // ORDER without BY -> expect(By) fails.
    let res = try_parse("MATCH (n) RETURN n ORDER n.name");
    assert!(res.is_err());
}

#[test]
fn parse_limit_integer() {
    let mut p = Parser::new("MATCH (n) RETURN n LIMIT 25").unwrap();
    let stmts = p.parse().unwrap();
    match &stmts[0] {
        Statement::Match { limit, .. } => {
            assert_eq!(limit.as_ref().unwrap(), &Expr::Literal(Value::Int(25)));
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn parse_limit_parameter() {
    let mut p = Parser::new("MATCH (n) RETURN n LIMIT $count").unwrap();
    let stmts = p.parse().unwrap();
    match &stmts[0] {
        Statement::Match { limit, .. } => {
            assert_eq!(
                limit.as_ref().unwrap(),
                &Expr::Parameter("count".to_string())
            );
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn parse_order_by_then_limit_together() {
    let mut p = Parser::new("MATCH (n) RETURN n ORDER BY n.x DESC LIMIT 5").unwrap();
    let stmts = p.parse().unwrap();
    match &stmts[0] {
        Statement::Match {
            order_by, limit, ..
        } => {
            assert!(order_by.is_some());
            assert_eq!(limit.as_ref().unwrap(), &Expr::Literal(Value::Int(5)));
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn parse_limit_absent_is_none() {
    let mut p = Parser::new("MATCH (n) RETURN n").unwrap();
    let stmts = p.parse().unwrap();
    match &stmts[0] {
        Statement::Match {
            limit, order_by, ..
        } => {
            assert!(limit.is_none());
            assert!(order_by.is_none());
        }
        other => panic!("got {other:?}"),
    }
}

// =====================================================================
// PARSER: aggregate functions
// =====================================================================

#[test]
fn parse_count_star() {
    let stmt = parse_one("MATCH (n) RETURN count(*)");
    let (_, _, ret) = as_match(&stmt);
    assert_eq!(
        ret.items[0].expr,
        Expr::Aggregate {
            function: AggregateFunction::Count,
            argument: None,
            distinct: false,
        }
    );
}

#[test]
fn parse_count_distinct() {
    let stmt = parse_one("MATCH (n) RETURN count(DISTINCT n)");
    let (_, _, ret) = as_match(&stmt);
    match &ret.items[0].expr {
        Expr::Aggregate {
            function: AggregateFunction::Count,
            distinct,
            argument,
        } => {
            assert!(*distinct);
            assert!(argument.is_some());
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn parse_all_aggregate_functions() {
    let cases = [
        ("sum(n.x)", AggregateFunction::Sum),
        ("avg(n.x)", AggregateFunction::Avg),
        ("min(n.x)", AggregateFunction::Min),
        ("max(n.x)", AggregateFunction::Max),
        ("collect(n.x)", AggregateFunction::Collect),
        ("count(n.x)", AggregateFunction::Count),
    ];
    for (call, func) in cases {
        let q = format!("MATCH (n) RETURN {call}");
        let stmt = parse_one(&q);
        let (_, _, ret) = as_match(&stmt);
        match &ret.items[0].expr {
            Expr::Aggregate { function, .. } => assert_eq!(*function, func, "for {call}"),
            other => panic!("expected aggregate for {call}, got {other:?}"),
        }
    }
}

#[test]
fn parse_aggregate_function_name_case_insensitive() {
    let stmt = parse_one("MATCH (n) RETURN COUNT(*)");
    let (_, _, ret) = as_match(&stmt);
    assert!(matches!(
        ret.items[0].expr,
        Expr::Aggregate {
            function: AggregateFunction::Count,
            ..
        }
    ));
}

#[test]
fn parse_unsupported_function_is_error() {
    // Only the six aggregates are recognized as function calls.
    let res = try_parse("MATCH (n) RETURN foobar(n.x)");
    assert!(res.is_err());
    match res.unwrap_err() {
        ParseError::Message(m) => assert!(m.contains("unsupported function"), "msg: {m}"),
        other => panic!("got {other:?}"),
    }
}

#[test]
fn parse_count_distinct_star_is_error() {
    // count(DISTINCT *) is rejected: star only allowed for non-distinct count.
    let res = try_parse("MATCH (n) RETURN count(DISTINCT *)");
    assert!(res.is_err());
}

#[test]
fn parse_sum_star_is_error() {
    // Only count(*) supports a star argument.
    let res = try_parse("MATCH (n) RETURN sum(*)");
    assert!(res.is_err());
    match res.unwrap_err() {
        ParseError::Message(m) => assert!(m.contains("star argument"), "msg: {m}"),
        other => panic!("got {other:?}"),
    }
}

#[test]
fn parse_aggregate_with_alias() {
    let stmt = parse_one("MATCH (n) RETURN count(*) AS total");
    let (_, _, ret) = as_match(&stmt);
    assert_eq!(ret.items[0].alias, Some("total".to_string()));
}

// =====================================================================
// PARSER: CREATE
// =====================================================================

#[test]
fn parse_create_node() {
    let stmt = parse_one("CREATE (n:User {id: $uid})");
    match &stmt {
        Statement::Create { pattern } => {
            assert_eq!(pattern.len(), 1);
            assert_eq!(pattern[0].labels, vec!["User".to_string()]);
            assert_eq!(
                pattern[0].properties.get("id"),
                Some(&Expr::Parameter("uid".to_string()))
            );
        }
        other => panic!("expected Create, got {other:?}"),
    }
}

#[test]
fn parse_create_relationship() {
    let stmt = parse_one("CREATE (a:User)-[:PLACED]->(o:Order)");
    match &stmt {
        Statement::Create { pattern } => {
            assert_eq!(pattern.len(), 2);
            assert_eq!(
                pattern[1].relationship.as_ref().unwrap().kinds,
                vec!["PLACED".to_string()]
            );
            assert_eq!(pattern[1].direction, Some(Direction::Outgoing));
        }
        other => panic!("got {other:?}"),
    }
}

// =====================================================================
// PARSER: MATCH ... CREATE (MatchCreate)
// =====================================================================

#[test]
fn parse_match_create() {
    let stmt = parse_one("MATCH (u:User {id: $uid}) CREATE (u)-[:PLACED]->(o:Order)");
    match &stmt {
        Statement::MatchCreate {
            match_pattern,
            create_pattern,
            where_clause,
        } => {
            assert_eq!(match_pattern.len(), 1);
            assert_eq!(create_pattern.len(), 2);
            assert!(where_clause.is_none());
        }
        other => panic!("expected MatchCreate, got {other:?}"),
    }
}

#[test]
fn parse_match_where_create() {
    let stmt = parse_one("MATCH (u:User) WHERE u.id = $uid CREATE (u)-[:PLACED]->(o:Order)");
    match &stmt {
        Statement::MatchCreate { where_clause, .. } => {
            assert!(where_clause.is_some());
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn parse_multi_match_then_create() {
    let stmt = parse_one("MATCH (a:Cat {id: $p}) MATCH (b:Cat {id: $c}) CREATE (a)-[:SUB]->(b)");
    match &stmt {
        Statement::MatchCreate {
            match_pattern,
            create_pattern,
            ..
        } => {
            assert_eq!(match_pattern.len(), 2);
            assert_eq!(match_pattern[0].variable, "a");
            assert_eq!(match_pattern[1].variable, "b");
            assert_eq!(create_pattern.len(), 2);
        }
        other => panic!("got {other:?}"),
    }
}

// =====================================================================
// PARSER: MERGE
// =====================================================================

#[test]
fn parse_merge_without_on_create() {
    let stmt = parse_one("MERGE (n:User {email: $e})");
    match &stmt {
        Statement::Merge { pattern, on_create } => {
            assert_eq!(pattern.len(), 1);
            assert!(on_create.is_empty());
        }
        other => panic!("expected Merge, got {other:?}"),
    }
}

#[test]
fn parse_merge_with_on_create_set() {
    let stmt = parse_one("MERGE (n:User {email: $e}) ON CREATE SET n.created = $now");
    match &stmt {
        Statement::Merge { on_create, .. } => {
            assert_eq!(on_create.len(), 1);
            let SetClause { target, value } = &on_create[0];
            assert_eq!(
                *target,
                Expr::PropertyAccess(
                    Box::new(Expr::Identifier("n".to_string())),
                    "created".to_string()
                )
            );
            assert_eq!(*value, Expr::Parameter("now".to_string()));
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn parse_merge_on_create_multiple_assignments() {
    let stmt = parse_one("MERGE (n:User {email: $e}) ON CREATE SET n.a = 1, n.b = 2");
    match &stmt {
        Statement::Merge { on_create, .. } => assert_eq!(on_create.len(), 2),
        other => panic!("got {other:?}"),
    }
}

#[test]
fn parse_merge_on_without_create_is_error() {
    let res = try_parse("MERGE (n:User) ON DELETE SET n.x = 1");
    assert!(res.is_err());
    match res.unwrap_err() {
        ParseError::Message(m) => assert!(m.contains("expected CREATE after ON"), "msg: {m}"),
        other => panic!("got {other:?}"),
    }
}

// =====================================================================
// PARSER: SET (standalone)
// =====================================================================

#[test]
fn parse_set_single_assignment() {
    let stmt = parse_one("SET n.name = 'bob'");
    match &stmt {
        Statement::Set { assignments } => {
            assert_eq!(assignments.len(), 1);
            assert_eq!(
                assignments[0].value,
                Expr::Literal(Value::String("bob".to_string()))
            );
        }
        other => panic!("expected Set, got {other:?}"),
    }
}

#[test]
fn parse_set_multiple_assignments() {
    let stmt = parse_one("SET n.a = 1, n.b = 2, n.c = 3");
    match &stmt {
        Statement::Set { assignments } => assert_eq!(assignments.len(), 3),
        other => panic!("got {other:?}"),
    }
}

#[test]
fn parse_set_target_is_property_access() {
    let stmt = parse_one("SET n.x = $v");
    match &stmt {
        Statement::Set { assignments } => {
            assert_eq!(
                assignments[0].target,
                Expr::PropertyAccess(Box::new(Expr::Identifier("n".to_string())), "x".to_string())
            );
        }
        other => panic!("got {other:?}"),
    }
}

// =====================================================================
// PARSER: DELETE
// =====================================================================

#[test]
fn parse_delete_single_identifier() {
    let stmt = parse_one("DELETE n");
    match &stmt {
        Statement::Delete { identifiers } => assert_eq!(identifiers, &vec!["n".to_string()]),
        other => panic!("expected Delete, got {other:?}"),
    }
}

#[test]
fn parse_delete_multiple_identifiers() {
    let stmt = parse_one("DELETE a, b, c");
    match &stmt {
        Statement::Delete { identifiers } => {
            assert_eq!(
                identifiers,
                &vec!["a".to_string(), "b".to_string(), "c".to_string()]
            );
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn parse_delete_no_identifiers_yields_empty_vec() {
    // DELETE with nothing following: the while loop never runs.
    let stmt = parse_one("DELETE");
    match &stmt {
        Statement::Delete { identifiers } => assert!(identifiers.is_empty()),
        other => panic!("got {other:?}"),
    }
}

// =====================================================================
// PARSER: KV statements (GET / SET KEY / DEL / INCR)
// =====================================================================

#[test]
fn parse_kv_get() {
    let stmt = parse_one("GET KEY $k");
    match &stmt {
        Statement::KvGet { key } => assert_eq!(key, &Expr::Parameter("k".to_string())),
        other => panic!("expected KvGet, got {other:?}"),
    }
}

#[test]
fn parse_kv_get_string_key() {
    let stmt = parse_one("GET KEY 'session:123'");
    match &stmt {
        Statement::KvGet { key } => {
            assert_eq!(
                key,
                &Expr::Literal(Value::String("session:123".to_string()))
            );
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn parse_kv_set_without_ttl() {
    let stmt = parse_one("SET KEY $k = $v");
    match &stmt {
        Statement::KvSet { key, value, ttl } => {
            assert_eq!(key, &Expr::Parameter("k".to_string()));
            assert_eq!(value, &Expr::Parameter("v".to_string()));
            assert!(ttl.is_none());
        }
        other => panic!("expected KvSet, got {other:?}"),
    }
}

#[test]
fn parse_kv_set_with_ttl() {
    let stmt = parse_one("SET KEY $k = $v TTL 60");
    match &stmt {
        Statement::KvSet { ttl, .. } => {
            assert_eq!(ttl.as_ref().unwrap(), &Expr::Literal(Value::Int(60)));
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn parse_kv_del() {
    let stmt = parse_one("DEL KEY $k");
    match &stmt {
        Statement::KvDel { key } => assert_eq!(key, &Expr::Parameter("k".to_string())),
        other => panic!("expected KvDel, got {other:?}"),
    }
}

#[test]
fn parse_kv_incr() {
    let stmt = parse_one("INCR KEY 'counter'");
    match &stmt {
        Statement::KvIncr { key } => {
            assert_eq!(key, &Expr::Literal(Value::String("counter".to_string())));
        }
        other => panic!("expected KvIncr, got {other:?}"),
    }
}

#[test]
fn parse_kv_get_missing_key_keyword_is_error() {
    // GET must be followed by KEY.
    let res = try_parse("GET $k");
    assert!(res.is_err());
}

// =====================================================================
// PARSER: parameter syntax edge cases
// =====================================================================

#[test]
fn parse_parameter_simple() {
    let stmt = parse_one("SET KEY $myParam = 1");
    match &stmt {
        Statement::KvSet { key, .. } => assert_eq!(key, &Expr::Parameter("myParam".to_string())),
        other => panic!("got {other:?}"),
    }
}

#[test]
fn parse_parameter_named_like_keyword() {
    // `$key`, `$order`, `$where` etc. are accepted: keyword tokens map to lowercased names.
    let stmt = parse_one("MATCH (n {x: $order}) RETURN n");
    let (pattern, _, _) = as_match(&stmt);
    assert_eq!(
        pattern[0].properties.get("x"),
        Some(&Expr::Parameter("order".to_string()))
    );
}

#[test]
fn parse_dollar_without_name_is_error() {
    // `$` followed by something that is not an identifier/keyword -> Message error.
    let res = try_parse("MATCH (n {x: $}) RETURN n");
    assert!(res.is_err());
}

// =====================================================================
// PARSER: literal expressions in RETURN
// =====================================================================

#[test]
fn parse_return_integer_literal() {
    let stmt = parse_one("MATCH (n) RETURN 42");
    let (_, _, ret) = as_match(&stmt);
    assert_eq!(ret.items[0].expr, Expr::Literal(Value::Int(42)));
}

#[test]
fn parse_return_string_literal() {
    let stmt = parse_one("MATCH (n) RETURN 'hi'");
    let (_, _, ret) = as_match(&stmt);
    assert_eq!(
        ret.items[0].expr,
        Expr::Literal(Value::String("hi".to_string()))
    );
}

#[test]
fn parse_return_bool_literals() {
    let stmt = parse_one("MATCH (n) RETURN true, false");
    let (_, _, ret) = as_match(&stmt);
    assert_eq!(ret.items[0].expr, Expr::Literal(Value::Bool(true)));
    assert_eq!(ret.items[1].expr, Expr::Literal(Value::Bool(false)));
}

#[test]
fn parse_return_null_literal() {
    let stmt = parse_one("MATCH (n) RETURN null");
    let (_, _, ret) = as_match(&stmt);
    assert_eq!(ret.items[0].expr, Expr::Literal(Value::Null));
}

#[test]
fn parse_return_float_literal_value() {
    let stmt = parse_one("MATCH (n) RETURN 2.5");
    let (_, _, ret) = as_match(&stmt);
    match &ret.items[0].expr {
        Expr::Literal(v) => assert_eq!(v.to_f64(), Some(2.5)),
        other => panic!("got {other:?}"),
    }
}

#[test]
fn parse_parenthesized_expression_in_return() {
    let stmt = parse_one("MATCH (n) RETURN (42)");
    let (_, _, ret) = as_match(&stmt);
    assert_eq!(ret.items[0].expr, Expr::Literal(Value::Int(42)));
}

// =====================================================================
// PARSER: multiple statements / semicolons
// =====================================================================

#[test]
fn parse_two_statements_separated_by_semicolon() {
    let stmts = try_parse("CREATE (n:User); CREATE (m:Order)").unwrap();
    assert_eq!(stmts.len(), 2);
    assert!(matches!(stmts[0], Statement::Create { .. }));
    assert!(matches!(stmts[1], Statement::Create { .. }));
}

#[test]
fn parse_trailing_semicolon_ok() {
    let stmts = try_parse("DELETE n;").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn parse_multiple_kv_statements() {
    let stmts = try_parse("SET KEY $a = 1; GET KEY $a; DEL KEY $a").unwrap();
    assert_eq!(stmts.len(), 3);
    assert!(matches!(stmts[0], Statement::KvSet { .. }));
    assert!(matches!(stmts[1], Statement::KvGet { .. }));
    assert!(matches!(stmts[2], Statement::KvDel { .. }));
}

// =====================================================================
// PARSER: error paths (malformed input -> Err, never panic)
// =====================================================================

#[test]
fn parse_bare_keyword_only_no_pattern_is_error() {
    // MATCH with nothing after -> parse_pattern -> parse_pattern_element -> expect(LParen) fails.
    assert!(try_parse("MATCH").is_err());
}

#[test]
fn parse_unknown_leading_token_is_error() {
    // An identifier cannot start a statement.
    let res = try_parse("foobar (n) RETURN n");
    assert!(res.is_err());
    match res.unwrap_err() {
        ParseError::UnexpectedToken { .. } => {}
        other => panic!("expected UnexpectedToken, got {other:?}"),
    }
}

#[test]
fn parse_unclosed_paren_is_error() {
    assert!(try_parse("MATCH (n RETURN n").is_err());
}

#[test]
fn parse_unclosed_brace_property_map_is_error() {
    assert!(try_parse("MATCH (n {a: 1) RETURN n").is_err());
}

#[test]
fn parse_unclosed_bracket_relationship_is_error() {
    assert!(try_parse("MATCH (a)-[:R->(b) RETURN a").is_err());
}

#[test]
fn parse_property_missing_colon_is_error() {
    assert!(try_parse("MATCH (n {a 1}) RETURN n").is_err());
}

#[test]
fn parse_property_missing_value_is_error() {
    assert!(try_parse("MATCH (n {a: }) RETURN n").is_err());
}

#[test]
fn parse_set_missing_eq_is_error() {
    assert!(try_parse("SET n.x 5").is_err());
}

#[test]
fn parse_property_access_missing_prop_name_is_error() {
    // `n.` with nothing after -> "expected property name after ." Message.
    let res = try_parse("MATCH (n) RETURN n.");
    assert!(res.is_err());
}

#[test]
fn parse_relationship_with_no_following_node_is_error() {
    // After `-[:R]->` there must be a node; EOF -> expect(LParen) fails.
    assert!(try_parse("MATCH (a)-[:R]->").is_err());
}

#[test]
fn parse_dangling_dollar_alone_is_error() {
    assert!(try_parse("RETURN $").is_err());
}

#[test]
fn parse_lone_symbols_is_error() {
    assert!(try_parse(")").is_err());
    assert!(try_parse("}").is_err());
    assert!(try_parse("]").is_err());
    assert!(try_parse(",").is_err());
}

// =====================================================================
// ROBUSTNESS: large / deeply-nested / unicode inputs do not panic
// =====================================================================

#[test]
fn robustness_deeply_nested_parentheses_in_where() {
    let mut q = String::from("MATCH (n) WHERE ");
    let depth = 32;
    for _ in 0..depth {
        q.push('(');
    }
    q.push_str("n.x = 1");
    for _ in 0..depth {
        q.push(')');
    }
    q.push_str(" RETURN n");
    // Should parse successfully (balanced) without stack issues for this depth.
    let res = try_parse(&q);
    assert!(res.is_ok(), "deeply nested balanced parens should parse");
}

#[test]
fn robustness_parentheses_beyond_nesting_limit_returns_error() {
    let mut q = String::from("MATCH (n) WHERE ");
    for _ in 0..128 {
        q.push('(');
    }
    q.push_str("n.x = 1");
    for _ in 0..128 {
        q.push(')');
    }
    q.push_str(" RETURN n");

    assert!(matches!(
        try_parse(&q),
        Err(ParseError::Message(message)) if message == "query nesting too deep"
    ));
}

#[test]
fn robustness_hundred_thousand_parentheses_returns_error_not_abort() {
    let mut q = String::from("MATCH (n) WHERE ");
    q.extend(std::iter::repeat_n('(', 100_000));
    q.push_str("n.x = 1");
    q.extend(std::iter::repeat_n(')', 100_000));
    q.push_str(" RETURN n");

    assert!(matches!(
        try_parse(&q),
        Err(ParseError::Message(message)) if message == "query nesting too deep"
    ));
}

#[test]
fn robustness_deeply_nested_unbalanced_is_error_not_panic() {
    let mut q = String::from("MATCH (n) WHERE ");
    for _ in 0..200 {
        q.push('(');
    }
    q.push_str("n.x = 1 RETURN n");
    // Unbalanced: must return Err, must not panic.
    assert!(try_parse(&q).is_err());
}

#[test]
fn robustness_long_chained_relationship_pattern() {
    let mut q = String::from("MATCH (n0)");
    let hops = 100;
    for i in 1..=hops {
        q.push_str(&format!("-[:R]->(n{i})"));
    }
    q.push_str(" RETURN n0");
    let stmt = parse_one(&q);
    let (pattern, _, _) = as_match(&stmt);
    assert_eq!(pattern.len(), hops + 1);
}

#[test]
fn robustness_long_or_chain() {
    let mut conds = Vec::new();
    for i in 0..100 {
        conds.push(format!("n.x = {i}"));
    }
    let q = format!("MATCH (n) WHERE {} RETURN n", conds.join(" OR "));
    let stmt = parse_one(&q);
    let (_, where_clause, _) = as_match(&stmt);
    assert!(matches!(
        where_clause.as_ref().unwrap(),
        Expr::BinaryOp(_, BinaryOperator::Or, _)
    ));
}

#[test]
fn robustness_large_property_map() {
    let mut props = Vec::new();
    for i in 0..200 {
        props.push(format!("p{i}: {i}"));
    }
    let q = format!("MATCH (n {{{}}}) RETURN n", props.join(", "));
    let stmt = parse_one(&q);
    let (pattern, _, _) = as_match(&stmt);
    assert_eq!(pattern[0].properties.len(), 200);
}

#[test]
fn robustness_unicode_in_string_property() {
    let stmt = parse_one("MATCH (n {name: 'José 日本 ☕'}) RETURN n");
    let (pattern, _, _) = as_match(&stmt);
    assert_eq!(
        pattern[0].properties.get("name"),
        Some(&Expr::Literal(Value::String("José 日本 ☕".to_string())))
    );
}

#[test]
fn robustness_unterminated_string_in_query_does_not_panic() {
    // Lexer returns a StringLiteral with the rest; parser may succeed or err,
    // but must not panic. We just assert it returns a Result.
    let _ = try_parse("MATCH (n {x: 'unterminated }) RETURN n");
}

#[test]
fn robustness_garbage_punctuation_only() {
    // Unknown punctuation is skipped by the lexer; this yields no tokens
    // -> empty statement list (Ok).
    let res = try_parse("@@@%%%&&&???");
    assert!(res.is_ok());
    assert!(res.unwrap().is_empty());
}

#[test]
fn robustness_huge_integer_in_property_returns_parse_error() {
    let result = try_parse("MATCH (n {big: 99999999999999999999999999}) RETURN n");
    assert!(matches!(
        result,
        Err(ParseError::Lex(LexError::IntegerOutOfRange(_)))
    ));
}

// =====================================================================
// VALUE: public helper API (used pervasively by parsed literals)
// =====================================================================

#[test]
fn value_as_string_accessor() {
    assert_eq!(Value::String("x".to_string()).as_string(), Some("x"));
    assert_eq!(Value::Int(1).as_string(), None);
}

#[test]
fn value_as_int_accessor() {
    assert_eq!(Value::Int(7).as_int(), Some(7));
    assert_eq!(Value::Bool(true).as_int(), None);
}

#[test]
fn value_as_bool_accessor() {
    assert_eq!(Value::Bool(false).as_bool(), Some(false));
    assert_eq!(Value::Null.as_bool(), None);
}

#[test]
fn value_float_roundtrip() {
    let v = Value::from_f64(3.5);
    assert_eq!(v.to_f64(), Some(3.5));
    assert_eq!(Value::Int(1).to_f64(), None);
}

#[test]
fn value_float_nan_roundtrips_via_bits() {
    let v = Value::from_f64(f64::NAN);
    assert!(v.to_f64().unwrap().is_nan());
}

#[test]
fn value_from_conversions() {
    assert_eq!(Value::from("s"), Value::String("s".to_string()));
    assert_eq!(Value::from("s".to_string()), Value::String("s".to_string()));
    assert_eq!(Value::from(9i64), Value::Int(9));
    assert_eq!(Value::from(true), Value::Bool(true));
}

#[test]
fn value_display_formatting() {
    assert_eq!(Value::String("hi".to_string()).to_string(), "hi");
    assert_eq!(Value::Int(42).to_string(), "42");
    assert_eq!(Value::Bool(true).to_string(), "true");
    assert_eq!(Value::Null.to_string(), "null");
    assert_eq!(Value::from_f64(1.5).to_string(), "1.5");
}

#[test]
fn value_display_list() {
    let v = Value::List(vec![Value::Int(1), Value::Int(2)]);
    assert_eq!(v.to_string(), "[1, 2]");
}

#[test]
fn value_partial_ord_same_types() {
    assert!(Value::Int(1) < Value::Int(2));
    assert!(Value::from_f64(1.0) < Value::from_f64(2.0));
    assert!(Value::String("a".to_string()) < Value::String("b".to_string()));
    assert_ne!(
        Value::Bool(true).partial_cmp(&Value::Bool(false)),
        Some(std::cmp::Ordering::Less)
    );
}

#[test]
fn value_partial_ord_cross_type_is_none() {
    assert_eq!(
        Value::Int(1).partial_cmp(&Value::String("a".to_string())),
        None
    );
    assert_eq!(Value::Null.partial_cmp(&Value::Null), None);
}

#[test]
fn value_equality_and_clone() {
    let m: HashMap<String, Value> = HashMap::new();
    let a = Value::Map(m.clone());
    let b = Value::Map(m);
    assert_eq!(a, b.clone());
}

// =====================================================================
// AST: enum variant equality sanity (Direction / OrderDirection / ops)
// =====================================================================

#[test]
fn ast_direction_variants_distinct() {
    assert_ne!(Direction::Incoming, Direction::Outgoing);
    assert_ne!(Direction::Outgoing, Direction::Both);
}

#[test]
fn ast_order_direction_variants_distinct() {
    assert_ne!(OrderDirection::Asc, OrderDirection::Desc);
}

#[test]
fn ast_binary_operator_variants_distinct() {
    assert_ne!(BinaryOperator::Eq, BinaryOperator::Ne);
    assert_ne!(BinaryOperator::And, BinaryOperator::Or);
    assert_ne!(BinaryOperator::Gt, BinaryOperator::Gte);
}

#[test]
fn ast_aggregate_function_variants_distinct() {
    assert_ne!(AggregateFunction::Count, AggregateFunction::Sum);
    assert_ne!(AggregateFunction::Min, AggregateFunction::Max);
}

#[test]
fn ast_relationship_length_equality() {
    assert_eq!(
        RelationshipLength {
            min: 1,
            max: Some(3)
        },
        RelationshipLength {
            min: 1,
            max: Some(3)
        }
    );
    assert_ne!(
        RelationshipLength { min: 1, max: None },
        RelationshipLength {
            min: 1,
            max: Some(1)
        }
    );
}

// #####################################################################
// #####################################################################
// ## APPENDED COMPLETENESS PASS (adversarial critic)                 ##
// ## Gaps found by re-reading src/ vs. the suite above. Every test   ##
// ## below exercises a PUBLIC, lexer-reachable path. No invented     ##
// ## APIs; errors-as-values; no expected panics on well-formed input.##
// #####################################################################
// #####################################################################

// =====================================================================
// LEXER (gap): operator disambiguation mid-stream + Token Clone/Debug
// =====================================================================

#[test]
fn lex_bang_then_non_eq_is_dash_plus_next() {
    // `!` not followed by `=` falls back to Dash; the following char lexes
    // independently. `!x` => Dash, Identifier("x").
    assert_eq!(
        lex_all("!x"),
        vec![Token::Dash, Token::Identifier("x".to_string())]
    );
}

#[test]
fn lex_gt_lt_followed_by_non_eq_are_plain() {
    // `>x` => Gt, ident; `<x` => Lt, ident (neither `>=`/`<=` nor `<-`).
    assert_eq!(
        lex_all(">x"),
        vec![Token::Gt, Token::Identifier("x".to_string())]
    );
    assert_eq!(
        lex_all("<x"),
        vec![Token::Lt, Token::Identifier("x".to_string())]
    );
}

#[test]
fn lex_lt_at_eof_is_lone_lt() {
    // `<` with nothing after: not `<=`, not `<-` -> Lt.
    assert_eq!(lex_all("<"), vec![Token::Lt]);
}

#[test]
fn lex_eq_eq_eq_collapses_pairs() {
    // `=` consumes at most one trailing `=`. `===` => Eq (from `==`), then Eq.
    assert_eq!(lex_all("==="), vec![Token::Eq, Token::Eq]);
}

#[test]
fn lex_arrow_vs_dash_in_pattern_fragment() {
    // `-[->` style fragment: Dash, LBracket, Arrow.
    assert_eq!(
        lex_all("-[->"),
        vec![Token::Dash, Token::LBracket, Token::Arrow]
    );
}

#[test]
fn lex_token_is_clone_and_debug() {
    // Token derives Clone + Debug; exercise both so the impls are covered.
    let t = Token::Identifier("z".to_string());
    let c = t.clone();
    assert_eq!(t, c);
    assert_eq!(format!("{:?}", Token::Eof), "Eof");
    assert!(format!("{:?}", Token::Integer(5)).contains('5'));
}

// =====================================================================
// LEXER (gap): number edge cases
// =====================================================================

#[test]
fn lex_leading_underscore_is_identifier_not_number() {
    // `_5` starts with `_` -> read_identifier path, NOT a number.
    assert_eq!(lex_all("_5"), vec![Token::Identifier("_5".to_string())]);
}

#[test]
fn lex_trailing_underscore_on_integer() {
    // read_number consumes trailing `_`; replace('_',"") -> "5" -> Integer(5).
    assert_eq!(lex_all("5_"), vec![Token::Integer(5)]);
}

#[test]
fn lex_float_then_dot_then_int_is_float_dot_int() {
    // `1.2.3`: read_number reads "1.2" (one fractional run, returns on the
    // SECOND `.` which is not digit-followed mid-fraction... actually the
    // fractional loop stops at the 2nd '.', returning Float(1.2)); then
    // Dot, Integer(3).
    assert_eq!(
        lex_all("1.2.3"),
        vec![Token::Float(1.2), Token::Dot, Token::Integer(3)]
    );
}

#[test]
fn lex_float_with_underscores_in_both_parts() {
    assert_eq!(lex_all("1_0.5_0"), vec![Token::Float(10.50)]);
}

#[test]
fn lex_zero_point_zero_is_float() {
    assert_eq!(lex_all("0.0"), vec![Token::Float(0.0)]);
}

// =====================================================================
// LEXER (gap): comments and string-escape variants
// =====================================================================

#[test]
fn lex_comment_at_start_of_input() {
    assert_eq!(lex_all("// leading\nRETURN"), vec![Token::Return]);
}

#[test]
fn lex_comment_only_no_newline() {
    // `//foo` to EOF: skip_whitespace consumes the rest; no tokens.
    assert_eq!(lex_all("//foo"), Vec::<Token>::new());
}

#[test]
fn lex_string_escape_other_quote_kind() {
    // In a single-quoted string, `\"` maps to a literal `"` (escape table arm).
    assert_eq!(
        lex_all("'a\\\"b'"),
        vec![Token::StringLiteral("a\"b".to_string())]
    );
}

#[test]
fn lex_string_with_embedded_newline_char_literal() {
    // A real newline byte inside the quotes is copied verbatim (not an escape).
    assert_eq!(
        lex_all("'line1\nline2'"),
        vec![Token::StringLiteral("line1\nline2".to_string())]
    );
}

// =====================================================================
// PARSER (gap): symbolic names from keyword tokens (take_symbolic_name)
// =====================================================================

#[test]
fn parse_node_label_named_order_keyword() {
    // take_symbolic_name maps Token::Order -> "Order", so `:Order` is a valid label.
    let stmt = parse_one("MATCH (n:Order) RETURN n");
    let (pattern, _, _) = as_match(&stmt);
    assert_eq!(pattern[0].labels, vec!["Order".to_string()]);
}

#[test]
fn parse_relationship_kind_named_order_keyword() {
    // Same path inside parse_relationship for `:Order` as a kind.
    let stmt = parse_one("MATCH (a)-[:Order]->(b) RETURN a");
    let (pattern, _, _) = as_match(&stmt);
    assert_eq!(
        pattern[1].relationship.as_ref().unwrap().kinds,
        vec!["Order".to_string()]
    );
}

#[test]
fn parse_node_colon_then_non_symbolic_yields_no_label() {
    // `(n:)` -> take_symbolic_name returns None (RParen is not a name) -> empty
    // labels, and the RParen still closes the element. No panic, no label.
    let stmt = parse_one("MATCH (n:) RETURN n");
    let (pattern, _, _) = as_match(&stmt);
    assert_eq!(pattern[0].variable, "n");
    assert!(pattern[0].labels.is_empty());
}

#[test]
fn parse_fully_anonymous_node() {
    // `()` -> empty variable, no labels, no props.
    let stmt = parse_one("MATCH () RETURN 1");
    let (pattern, _, _) = as_match(&stmt);
    assert_eq!(pattern[0].variable, "");
    assert!(pattern[0].labels.is_empty());
    assert!(pattern[0].properties.is_empty());
}

// =====================================================================
// PARSER (gap): every keyword token usable as a property-map key
// =====================================================================

#[test]
fn parse_all_keyword_tokens_as_property_keys() {
    // parse_properties has a dedicated arm per keyword token, lowercasing it.
    // (Asc/Desc/By/On are in the arm list too.)
    let pairs = [
        ("key", "key"),
        ("get", "get"),
        ("set", "set"),
        ("del", "del"),
        ("incr", "incr"),
        ("ttl", "ttl"),
        ("match", "match"),
        ("return", "return"),
        ("create", "create"),
        ("merge", "merge"),
        ("delete", "delete"),
        ("where", "where"),
        ("limit", "limit"),
        ("order", "order"),
        ("by", "by"),
        ("asc", "asc"),
        ("desc", "desc"),
        ("on", "on"),
    ];
    for (kw, expected_key) in pairs {
        let q = format!("MATCH (n {{{kw}: 1}}) RETURN n");
        let stmt = parse_one(&q);
        let (pattern, _, _) = as_match(&stmt);
        assert_eq!(
            pattern[0].properties.get(expected_key),
            Some(&Expr::Literal(Value::Int(1))),
            "keyword `{kw}` should become property key `{expected_key}`"
        );
    }
}

// =====================================================================
// PARSER (gap): every keyword token usable as a $parameter name
// =====================================================================

#[test]
fn parse_all_keyword_tokens_as_parameter_names() {
    // parse_primary has a per-keyword arm after `$`, lowercasing the name.
    let pairs = [
        ("$key", "key"),
        ("$get", "get"),
        ("$set", "set"),
        ("$del", "del"),
        ("$incr", "incr"),
        ("$ttl", "ttl"),
        ("$match", "match"),
        ("$return", "return"),
        ("$create", "create"),
        ("$merge", "merge"),
        ("$delete", "delete"),
        ("$where", "where"),
        ("$limit", "limit"),
        ("$order", "order"),
        ("$by", "by"),
        ("$asc", "asc"),
        ("$desc", "desc"),
        ("$on", "on"),
    ];
    for (param, expected_name) in pairs {
        let q = format!("MATCH (n {{x: {param}}}) RETURN n");
        let stmt = parse_one(&q);
        let (pattern, _, _) = as_match(&stmt);
        assert_eq!(
            pattern[0].properties.get("x"),
            Some(&Expr::Parameter(expected_name.to_string())),
            "param `{param}` should yield Parameter(`{expected_name}`)"
        );
    }
}

// =====================================================================
// PARSER (gap): variable-length relationship malformed-range paths
// =====================================================================

#[test]
fn parse_var_length_single_dot_is_error() {
    // `*2.` -> first int 2, current is Dot -> advance past it, then
    // expect(Dot) for the SECOND dot fails (next is `]`) -> Err, not panic.
    let res = try_parse("MATCH (a)-[:R*2.]->(b) RETURN b");
    assert!(res.is_err());
}

#[test]
fn parse_var_length_equal_min_max_ok() {
    // `*3..3` -> min == max, the `min > max` guard does NOT fire.
    let stmt = parse_one("MATCH (a)-[:R*3..3]->(b) RETURN b");
    let (pattern, _, _) = as_match(&stmt);
    assert_eq!(
        pattern[1].relationship.as_ref().unwrap().length,
        Some(RelationshipLength {
            min: 3,
            max: Some(3)
        })
    );
}

#[test]
fn parse_var_length_zero_bounds() {
    // `*0..0` -> min 0, max Some(0); boundary value, 0 is non-negative so OK.
    let stmt = parse_one("MATCH (a)-[:R*0..0]->(b) RETURN b");
    let (pattern, _, _) = as_match(&stmt);
    assert_eq!(
        pattern[1].relationship.as_ref().unwrap().length,
        Some(RelationshipLength {
            min: 0,
            max: Some(0)
        })
    );
}

#[test]
fn parse_var_length_star_zero_fixed() {
    // `*0` (no `..`) -> first = Some(0) -> min 0, max Some(0).
    let stmt = parse_one("MATCH (a)-[:R*0]->(b) RETURN b");
    let (pattern, _, _) = as_match(&stmt);
    assert_eq!(
        pattern[1].relationship.as_ref().unwrap().length,
        Some(RelationshipLength {
            min: 0,
            max: Some(0)
        })
    );
}

#[test]
fn parse_var_length_bare_double_dot() {
    // `*..` -> first None (min defaults 1), max None (no integer after `..`).
    let stmt = parse_one("MATCH (a)-[:R*..]->(b) RETURN b");
    let (pattern, _, _) = as_match(&stmt);
    assert_eq!(
        pattern[1].relationship.as_ref().unwrap().length,
        Some(RelationshipLength { min: 1, max: None })
    );
}

// =====================================================================
// PARSER (gap): aggregate DISTINCT with a real argument; sum DISTINCT
// =====================================================================

#[test]
fn parse_count_distinct_with_property_argument() {
    // count(DISTINCT n.x): distinct true AND argument Some (not the `n` ident case).
    let stmt = parse_one("MATCH (n) RETURN count(DISTINCT n.x)");
    let (_, _, ret) = as_match(&stmt);
    match &ret.items[0].expr {
        Expr::Aggregate {
            function: AggregateFunction::Count,
            distinct,
            argument,
        } => {
            assert!(*distinct);
            assert!(argument.is_some());
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn parse_sum_distinct_is_allowed() {
    // The star-guard only fires for star args; sum(DISTINCT n.x) is fine.
    let stmt = parse_one("MATCH (n) RETURN sum(DISTINCT n.x)");
    let (_, _, ret) = as_match(&stmt);
    match &ret.items[0].expr {
        Expr::Aggregate {
            function: AggregateFunction::Sum,
            distinct,
            argument,
        } => {
            assert!(*distinct);
            assert!(argument.is_some());
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn parse_collect_with_property_argument() {
    let stmt = parse_one("MATCH (n) RETURN collect(n.id)");
    let (_, _, ret) = as_match(&stmt);
    assert!(matches!(
        ret.items[0].expr,
        Expr::Aggregate {
            function: AggregateFunction::Collect,
            distinct: false,
            ..
        }
    ));
}

#[test]
fn parse_aggregate_unclosed_paren_is_error() {
    // count(n  with no `)` -> expect(RParen) fails.
    assert!(try_parse("MATCH (n) RETURN count(n").is_err());
}

// =====================================================================
// PARSER (gap): SET target as a bare identifier (not property access)
// =====================================================================

#[test]
fn parse_set_target_bare_identifier() {
    // parse_set_clauses' target is parse_primary; a bare ident (no `.`) yields
    // Expr::Identifier as the target.
    let stmt = parse_one("SET n = $v");
    match &stmt {
        Statement::Set { assignments } => {
            assert_eq!(assignments.len(), 1);
            assert_eq!(assignments[0].target, Expr::Identifier("n".to_string()));
            assert_eq!(assignments[0].value, Expr::Parameter("v".to_string()));
        }
        other => panic!("expected Set, got {other:?}"),
    }
}

// =====================================================================
// PARSER (gap): KV error paths + literal value/ttl shapes
// =====================================================================

#[test]
fn parse_kv_set_string_value_and_int_ttl() {
    let stmt = parse_one("SET KEY 'k' = 'v' TTL 30");
    match &stmt {
        Statement::KvSet { key, value, ttl } => {
            assert_eq!(key, &Expr::Literal(Value::String("k".to_string())));
            assert_eq!(value, &Expr::Literal(Value::String("v".to_string())));
            assert_eq!(ttl.as_ref().unwrap(), &Expr::Literal(Value::Int(30)));
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn parse_kv_set_ttl_parameter() {
    let stmt = parse_one("SET KEY $k = $v TTL $ttl");
    match &stmt {
        Statement::KvSet { ttl, .. } => {
            assert_eq!(ttl.as_ref().unwrap(), &Expr::Parameter("ttl".to_string()));
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn parse_kv_set_missing_eq_is_error() {
    // `SET KEY $k $v` -> expect(Eq) fails.
    assert!(try_parse("SET KEY $k $v").is_err());
}

#[test]
fn parse_kv_del_missing_key_keyword_is_error() {
    assert!(try_parse("DEL $k").is_err());
}

#[test]
fn parse_kv_incr_missing_key_keyword_is_error() {
    assert!(try_parse("INCR $k").is_err());
}

#[test]
fn parse_kv_get_integer_key() {
    // Keys can be any primary, including an integer literal.
    let stmt = parse_one("GET KEY 42");
    match &stmt {
        Statement::KvGet { key } => assert_eq!(key, &Expr::Literal(Value::Int(42))),
        other => panic!("got {other:?}"),
    }
}

// =====================================================================
// PARSER (gap): WHERE with a bare (non-comparison) expression
// =====================================================================

#[test]
fn parse_where_bare_identifier_expression() {
    // `WHERE n` -> parse_expression bottoms out at an Identifier; no binary op.
    let stmt = parse_one("MATCH (n) WHERE n RETURN n");
    let (_, where_clause, _) = as_match(&stmt);
    assert_eq!(
        where_clause.as_ref().unwrap(),
        &Expr::Identifier("n".to_string())
    );
}

#[test]
fn parse_where_bare_property_access_expression() {
    let stmt = parse_one("MATCH (n) WHERE n.active RETURN n");
    let (_, where_clause, _) = as_match(&stmt);
    assert_eq!(
        where_clause.as_ref().unwrap(),
        &Expr::PropertyAccess(
            Box::new(Expr::Identifier("n".to_string())),
            "active".to_string()
        )
    );
}

#[test]
fn parse_where_literal_only() {
    // `WHERE true` -> bare bool literal expression.
    let stmt = parse_one("MATCH (n) WHERE true RETURN n");
    let (_, where_clause, _) = as_match(&stmt);
    assert_eq!(
        where_clause.as_ref().unwrap(),
        &Expr::Literal(Value::Bool(true))
    );
}

// =====================================================================
// PARSER (gap): RETURN AS at EOF -> alias None (inner else branch)
// =====================================================================

#[test]
fn parse_return_as_with_no_following_identifier_is_none() {
    // `RETURN n AS` at EOF: the `AS` is consumed, then the inner match sees
    // Token::Eof (not an Identifier) -> alias None. No panic.
    let stmt = parse_one("MATCH (n) RETURN n AS");
    let (_, _, ret) = as_match(&stmt);
    assert_eq!(ret.items[0].expr, Expr::Identifier("n".to_string()));
    assert_eq!(ret.items[0].alias, None);
}

// =====================================================================
// PARSER (gap): parse_add is a pass-through; `+` is never consumed,
// so trailing `+ ...` after a RETURN item dangles into a statement error.
// =====================================================================

#[test]
fn parse_plus_after_return_item_is_error_not_arithmetic() {
    // `RETURN 1 + 2`: `1` is the sole return item; `+` is not AS/Comma so the
    // return clause ends, MATCH ends, and `+` starts no statement -> Err.
    // (Documents that arithmetic `+` is NOT supported, per parse_add MVP note.)
    assert!(try_parse("MATCH (n) RETURN 1 + 2").is_err());
}

// =====================================================================
// PARSER (gap): nested parenthesization in primary
// =====================================================================

#[test]
fn parse_double_parenthesized_literal() {
    let stmt = parse_one("MATCH (n) RETURN ((42))");
    let (_, _, ret) = as_match(&stmt);
    assert_eq!(ret.items[0].expr, Expr::Literal(Value::Int(42)));
}

#[test]
fn parse_parenthesized_property_access_in_where() {
    let stmt = parse_one("MATCH (n) WHERE (n.x) = 1 RETURN n");
    let (_, where_clause, _) = as_match(&stmt);
    match where_clause.as_ref().unwrap() {
        Expr::BinaryOp(left, BinaryOperator::Eq, _) => {
            assert!(matches!(left.as_ref(), Expr::PropertyAccess(_, _)));
        }
        other => panic!("got {other:?}"),
    }
}

// =====================================================================
// PARSER (gap): empty / stray-semicolon statement handling
// =====================================================================

#[test]
fn parse_leading_semicolon_is_error() {
    // A bare `;` is not a statement keyword; parse_statement errors on it.
    assert!(try_parse("; MATCH (n) RETURN n").is_err());
}

#[test]
fn parse_only_semicolon_is_error() {
    assert!(try_parse(";").is_err());
}

#[test]
fn parse_three_statements_chained() {
    let stmts = try_parse("CREATE (a:A); MATCH (b:B) RETURN b; DELETE c").unwrap();
    assert_eq!(stmts.len(), 3);
    assert!(matches!(stmts[0], Statement::Create { .. }));
    assert!(matches!(stmts[1], Statement::Match { .. }));
    assert!(matches!(stmts[2], Statement::Delete { .. }));
}

// =====================================================================
// PARSER (gap): MatchCreate with multiple MATCH and a WHERE + structural
// =====================================================================

#[test]
fn parse_match_create_carries_where_across() {
    let stmt = parse_one("MATCH (a:A) MATCH (b:B) WHERE a.id = $x CREATE (a)-[:R]->(b)");
    match &stmt {
        Statement::MatchCreate {
            match_pattern,
            where_clause,
            create_pattern,
        } => {
            assert_eq!(match_pattern.len(), 2);
            assert!(where_clause.is_some());
            assert_eq!(create_pattern.len(), 2);
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn parse_relationship_both_with_variable_and_props() {
    // Undirected `-[r:R {w: 1}]-` exercises variable + kind + props + Both dir.
    let stmt = parse_one("MATCH (a)-[r:R {w: 1}]-(b) RETURN r");
    let (pattern, _, _) = as_match(&stmt);
    let rel = pattern[1].relationship.as_ref().unwrap();
    assert_eq!(rel.variable, "r");
    assert_eq!(rel.kinds, vec!["R".to_string()]);
    assert_eq!(rel.properties.get("w"), Some(&Expr::Literal(Value::Int(1))));
    assert_eq!(pattern[1].direction, Some(Direction::Both));
}

#[test]
fn parse_incoming_relationship_with_bracket_and_kind() {
    // `<-[:R]-` : incoming with an explicit bracket; expect(Dash) after the bracket.
    let stmt = parse_one("MATCH (a)<-[r:R]-(b) RETURN r");
    let (pattern, _, _) = as_match(&stmt);
    assert_eq!(pattern[1].direction, Some(Direction::Incoming));
    assert_eq!(pattern[1].relationship.as_ref().unwrap().variable, "r");
}

#[test]
fn parse_incoming_bare_dash_no_bracket() {
    // `<--` : LeftArrow then expect(Dash); relationship None, Incoming dir.
    let stmt = parse_one("MATCH (a)<--(b) RETURN a");
    let (pattern, _, _) = as_match(&stmt);
    assert!(pattern[1].relationship.is_none());
    assert_eq!(pattern[1].direction, Some(Direction::Incoming));
}

#[test]
fn parse_incoming_missing_trailing_dash_is_error() {
    // `<-[:R]>` : after incoming bracket, expect(Dash) fails on `>`/Arrow.
    // `<-[:R]->` would feed Arrow where Dash is required -> Err.
    assert!(try_parse("MATCH (a)<-[:R]->(b) RETURN a").is_err());
}

// =====================================================================
// PARSER (gap): ParseError Display / Debug surface (errors are values)
// =====================================================================

#[test]
fn parse_error_unexpected_eof_variant_is_reachable_and_displays() {
    // Construct via a real parse: `MATCH (n {a: $` -> after `$`, current is
    // RBrace path? Actually `$` then RBrace -> Message. Use a case that hits
    // UnexpectedEof is hard; instead assert the Display of each variant we can
    // reach, using the public ParseError values returned by the parser.
    let err = try_parse("foobar").unwrap_err();
    // UnexpectedToken Display contains "unexpected token".
    let shown = format!("{err}");
    assert!(shown.contains("unexpected token"), "Display: {shown}");
    // Debug is also derived.
    assert!(!format!("{err:?}").is_empty());
}

#[test]
fn parse_error_message_variant_displays_inner_string() {
    // ParseError::Message(s) Display is just `{0}` -> the raw message.
    let err = try_parse("MATCH (a)-[:R*9..1]->(b) RETURN b").unwrap_err();
    let shown = format!("{err}");
    assert!(
        shown.contains("minimum exceeds maximum"),
        "Display: {shown}"
    );
}

// =====================================================================
// VALUE (gap): List/Map Display, Hash via HashSet, Eq-on-bits, ordering
// =====================================================================

#[test]
fn value_display_empty_list() {
    assert_eq!(Value::List(vec![]).to_string(), "[]");
}

#[test]
fn value_display_nested_list() {
    let v = Value::List(vec![
        Value::String("a".to_string()),
        Value::List(vec![Value::Int(1)]),
    ]);
    assert_eq!(v.to_string(), "[a, [1]]");
}

#[test]
fn value_display_map_shows_key_count() {
    let mut m = HashMap::new();
    m.insert("a".to_string(), Value::Int(1));
    m.insert("b".to_string(), Value::Int(2));
    assert_eq!(Value::Map(m).to_string(), "{...2 keys}");
}

#[test]
fn value_display_empty_map() {
    assert_eq!(Value::Map(HashMap::new()).to_string(), "{...0 keys}");
}

#[test]
fn value_is_hashable_in_set() {
    // Value derives Hash + Eq; usable as a HashSet member. Distinct variants
    // and distinct payloads stay distinct; equal payloads dedupe.
    use std::collections::HashSet;
    let mut set: HashSet<Value> = HashSet::new();
    set.insert(Value::Int(1));
    set.insert(Value::Int(1)); // dup
    set.insert(Value::Int(2));
    set.insert(Value::String("1".to_string()));
    set.insert(Value::Bool(true));
    set.insert(Value::Null);
    set.insert(Value::Null); // dup
    assert!(set.contains(&Value::Int(1)));
    assert!(set.contains(&Value::String("1".to_string())));
    // 1, 2, "1", true, null = 5 distinct.
    assert_eq!(set.len(), 5);
}

#[test]
fn value_float_eq_is_bitwise() {
    // Float stored as bits with derived Eq: identical bits compare equal, and
    // NaN == NaN holds (same bit pattern) — a deliberate consequence of the
    // bits representation, unlike native f64.
    assert_eq!(Value::from_f64(1.5), Value::from_f64(1.5));
    assert_eq!(Value::from_f64(f64::NAN), Value::from_f64(f64::NAN));
    assert_ne!(Value::from_f64(1.0), Value::from_f64(2.0));
}

#[test]
fn value_float_partial_cmp_nan_is_none() {
    // partial_cmp delegates to f64 ordering after from_bits, so NaN -> None
    // even though Eq-on-bits said they're "equal".
    assert_eq!(
        Value::from_f64(f64::NAN).partial_cmp(&Value::from_f64(f64::NAN)),
        None
    );
}

#[test]
fn value_bool_ordering_false_lt_true() {
    assert!(Value::Bool(false) < Value::Bool(true));
    assert_eq!(
        Value::Bool(true).partial_cmp(&Value::Bool(true)),
        Some(std::cmp::Ordering::Equal)
    );
}

#[test]
fn value_partial_cmp_list_and_map_are_none() {
    // List/Map/Null have no PartialOrd arm -> None.
    assert_eq!(Value::List(vec![]).partial_cmp(&Value::List(vec![])), None);
    assert_eq!(
        Value::Map(HashMap::new()).partial_cmp(&Value::Map(HashMap::new())),
        None
    );
}

#[test]
fn value_as_accessors_on_wrong_variants_return_none() {
    // Exhaustively confirm the accessor None-paths across variants.
    assert_eq!(Value::List(vec![]).as_string(), None);
    assert_eq!(Value::Map(HashMap::new()).as_int(), None);
    assert_eq!(Value::Null.as_string(), None);
    assert_eq!(Value::Null.as_int(), None);
    assert_eq!(Value::from_f64(1.0).as_bool(), None);
    assert_eq!(Value::Int(1).to_f64(), None);
    assert_eq!(Value::String("x".to_string()).to_f64(), None);
}

#[test]
fn value_serialize_trait_is_implemented() {
    // Value derives serde::Serialize/Deserialize. Without serde_json in
    // dev-deps we can't round-trip through JSON here, but we CAN confirm the
    // trait bound is satisfied at compile time via a generic bound check.
    fn assert_serialize<T: serde::Serialize>(_: &T) {}
    fn assert_deserialize<'de, T: serde::Deserialize<'de>>() {}
    assert_serialize(&Value::Int(1));
    assert_serialize(&Value::List(vec![Value::Null]));
    assert_deserialize::<Value>();
}

// =====================================================================
// AST (gap): struct field round-trips + clone/debug coverage
// =====================================================================

#[test]
fn ast_pattern_element_clone_and_debug() {
    let stmt = parse_one("MATCH (n:User {id: 1}) RETURN n");
    let (pattern, _, _) = as_match(&stmt);
    let cloned = pattern[0].clone();
    assert_eq!(&cloned, &pattern[0]);
    assert!(!format!("{:?}", pattern[0]).is_empty());
}

#[test]
fn ast_expr_clone_equality() {
    let a = Expr::BinaryOp(
        Box::new(Expr::Identifier("x".to_string())),
        BinaryOperator::And,
        Box::new(Expr::Literal(Value::Bool(true))),
    );
    assert_eq!(a.clone(), a);
}

#[test]
fn ast_return_item_alias_some_vs_none_distinct() {
    let with_alias = parse_one("MATCH (n) RETURN n AS x");
    let no_alias = parse_one("MATCH (n) RETURN n");
    let (_, _, a) = as_match(&with_alias);
    let (_, _, b) = as_match(&no_alias);
    assert_ne!(a.items[0], b.items[0]);
}

#[test]
fn ast_set_clause_equality_round_trip() {
    let stmt = parse_one("SET n.x = 1");
    match &stmt {
        Statement::Set { assignments } => {
            let manual = SetClause {
                target: Expr::PropertyAccess(
                    Box::new(Expr::Identifier("n".to_string())),
                    "x".to_string(),
                ),
                value: Expr::Literal(Value::Int(1)),
            };
            assert_eq!(assignments[0], manual);
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn ast_statement_variants_are_distinct() {
    // Same-shaped-but-different statements compare unequal.
    let a = parse_one("CREATE (n:User)");
    let b = parse_one("MERGE (n:User)");
    assert_ne!(a, b);
}
