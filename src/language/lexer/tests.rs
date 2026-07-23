use super::{INVALID_ESCAPE, LINE_BREAK_IN_STRING, UNCLOSED_STRING, UNSUPPORTED_DISPATCH, lex};
use crate::{source::Span, syntax::TokenKind};

fn kinds(source: &str) -> Vec<TokenKind> {
    lex(source)
        .tokens
        .into_iter()
        .map(|token| token.kind)
        .collect()
}

#[test]
fn preserves_trivia_and_fixed_reader_punctuation() {
    let result = lex("  ; hi\n(->> [x] #{:a} '~@y)\n");
    assert_eq!(
        result
            .tokens
            .iter()
            .map(|token| token.kind)
            .collect::<Vec<_>>(),
        vec![
            TokenKind::Whitespace,
            TokenKind::Comment,
            TokenKind::Whitespace,
            TokenKind::LeftParen,
            TokenKind::Atom,
            TokenKind::Whitespace,
            TokenKind::LeftBracket,
            TokenKind::Atom,
            TokenKind::RightBracket,
            TokenKind::Whitespace,
            TokenKind::SetStart,
            TokenKind::Atom,
            TokenKind::RightBrace,
            TokenKind::Whitespace,
            TokenKind::Quote,
            TokenKind::UnquoteSplicing,
            TokenKind::Atom,
            TokenKind::RightParen,
            TokenKind::Whitespace,
        ]
    );
}

#[test]
fn recognizes_every_fixed_delimiter_and_prefix() {
    assert_eq!(
        kinds("()[]{}#{}'`~~@^"),
        vec![
            TokenKind::LeftParen,
            TokenKind::RightParen,
            TokenKind::LeftBracket,
            TokenKind::RightBracket,
            TokenKind::LeftBrace,
            TokenKind::RightBrace,
            TokenKind::SetStart,
            TokenKind::RightBrace,
            TokenKind::Quote,
            TokenKind::SyntaxQuote,
            TokenKind::Unquote,
            TokenKind::UnquoteSplicing,
            TokenKind::Metadata,
        ]
    );
}

#[test]
fn token_text_round_trips_every_source_byte() {
    let source = "(中文, #bad ; note\r\n  \"ok\\n\")";
    let rebuilt = lex(source)
        .tokens
        .into_iter()
        .map(|token| token.text)
        .collect::<String>();
    assert_eq!(rebuilt, source);
}

#[test]
fn token_text_round_trips_recoverable_lexical_errors() {
    let source = "#tag \"unterminated\\q";
    let result = lex(source);
    let rebuilt = result
        .tokens
        .iter()
        .map(|token| token.text.as_str())
        .collect::<String>();
    assert_eq!(rebuilt, source);
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == UNSUPPORTED_DISPATCH)
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == INVALID_ESCAPE)
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == UNCLOSED_STRING)
    );
}

#[test]
fn token_spans_are_utf8_byte_ranges() {
    let result = lex("中 名");
    assert_eq!(result.tokens[0].span, Span::new(0, 3));
    assert_eq!(result.tokens[0].text, "中");
    assert_eq!(result.tokens[2].span, Span::new(4, 7));
    assert_eq!(result.tokens[2].text, "名");
}

#[test]
fn strings_keep_raw_spelling_and_accept_common_escapes() {
    let result = lex(r#""a\n\u4e2d\U0001f600\x41\141""#);
    assert_eq!(result.diagnostics, Vec::new());
    assert_eq!(result.tokens[0].kind, TokenKind::String);
    assert_eq!(result.tokens[0].text, r#""a\n\u4e2d\U0001f600\x41\141""#);
}

#[test]
fn invalid_escape_becomes_error_token() {
    let result = lex(r#""bad\q""#);
    assert_eq!(kinds(r#""bad\q""#), vec![TokenKind::Error]);
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].code, INVALID_ESCAPE);
}

#[test]
fn malformed_unicode_escape_recovers_at_quote() {
    let result = lex(r#""bad\u12" tail"#);
    assert_eq!(result.tokens[0].kind, TokenKind::Error);
    assert_eq!(result.tokens[1].kind, TokenKind::Whitespace);
    assert_eq!(result.tokens[2].kind, TokenKind::Atom);
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == INVALID_ESCAPE)
    );
}

#[test]
fn unclosed_string_is_one_error_token() {
    let result = lex("\"unterminated");
    assert_eq!(result.tokens.len(), 1);
    assert_eq!(result.tokens[0].kind, TokenKind::Error);
    assert_eq!(result.diagnostics[0].code, UNCLOSED_STRING);
}

#[test]
fn unclosed_string_recovers_at_the_first_raw_line_break() {
    let source = "\"unterminated\n(tail)";
    let result = lex(source);

    assert_eq!(
        result
            .tokens
            .iter()
            .map(|token| token.text.as_str())
            .collect::<String>(),
        source
    );
    assert_eq!(result.tokens[0].kind, TokenKind::Error);
    assert_eq!(result.tokens[0].text, "\"unterminated\n");
    assert_eq!(result.tokens[1].kind, TokenKind::LeftParen);
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == LINE_BREAK_IN_STRING)
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == UNCLOSED_STRING)
    );
}

#[test]
fn raw_line_break_marks_the_complete_string_as_error() {
    let result = lex("\"first\r\nsecond\" tail");
    assert_eq!(result.tokens[0].kind, TokenKind::Error);
    assert_eq!(result.tokens[0].text, "\"first\r\nsecond\"");
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].code, LINE_BREAK_IN_STRING);
    assert_eq!(result.diagnostics[0].span, Span::new(6, 8));
}

#[test]
fn escaped_line_break_is_accepted() {
    let result = lex("\"first\\\nsecond\"");
    assert_eq!(result.tokens[0].kind, TokenKind::String);
    assert!(result.diagnostics.is_empty());
}

#[test]
fn unsupported_dispatch_does_not_swallow_following_atom() {
    let result = lex("#foo #{bar}");
    assert_eq!(result.tokens[0].kind, TokenKind::Error);
    assert_eq!(result.tokens[1].kind, TokenKind::Atom);
    assert_eq!(result.tokens[3].kind, TokenKind::SetStart);
    assert_eq!(result.diagnostics[0].code, UNSUPPORTED_DISPATCH);
}

#[test]
fn comma_is_lossless_whitespace() {
    let result = lex("a,b");
    assert_eq!(result.tokens[0].kind, TokenKind::Atom);
    assert_eq!(result.tokens[1].kind, TokenKind::Whitespace);
    assert_eq!(result.tokens[1].text, ",");
    assert_eq!(result.tokens[2].kind, TokenKind::Atom);
}

#[test]
fn hash_inside_an_atom_is_not_a_dispatch() {
    let result = lex("value# other#name");
    assert!(result.diagnostics.is_empty());
    assert_eq!(result.tokens[0].kind, TokenKind::Atom);
    assert_eq!(result.tokens[0].text, "value#");
    assert_eq!(result.tokens[2].text, "other#name");
}

#[test]
fn no_panic_on_unicode_and_delimiters() {
    let result = lex("😀^名~@值");
    assert!(result.diagnostics.is_empty());
    assert_eq!(result.tokens.len(), 5);
}
