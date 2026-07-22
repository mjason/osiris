//! The lossless, UTF-8 aware lexer for Osiris source files.
//!
//! The lexer deliberately does not interpret atoms (numbers, names, keywords,
//! and so on).  That belongs to the reader.  Its job is to identify the small
//! set of fixed reader punctuation and preserve every source byte in a token.

use crate::{
    diagnostic::Diagnostic,
    source::Span,
    syntax::{Token, TokenKind},
};

/// A lexical result.  Diagnostics are non-fatal so the reader can continue and
/// provide useful errors for the rest of an incomplete editor buffer.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct LexResult {
    pub tokens: Vec<Token>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Lex one Osiris source buffer while preserving trivia.
#[must_use]
pub fn lex(source: &str) -> LexResult {
    Lexer::new(source).run()
}

const INVALID_ESCAPE: &str = "OSR-L001";
const UNCLOSED_STRING: &str = "OSR-L002";
const UNSUPPORTED_DISPATCH: &str = "OSR-L003";
const LINE_BREAK_IN_STRING: &str = "OSR-L004";

struct Lexer<'a> {
    source: &'a str,
    offset: usize,
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            offset: 0,
            tokens: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn run(mut self) -> LexResult {
        while self.offset < self.source.len() {
            let Some(character) = self.peek() else {
                break;
            };

            if character.is_whitespace() || character == ',' {
                self.lex_whitespace();
            } else if character == ';' {
                self.lex_comment();
            } else {
                self.lex_non_trivia();
            }
        }

        LexResult {
            tokens: self.tokens,
            diagnostics: self.diagnostics,
        }
    }

    fn peek(&self) -> Option<char> {
        self.source[self.offset..].chars().next()
    }

    fn advance(&mut self) -> Option<char> {
        let character = self.peek()?;
        self.offset += character.len_utf8();
        Some(character)
    }

    fn push(&mut self, kind: TokenKind, start: usize, end: usize) {
        debug_assert!(start <= end);
        // All offsets are advanced on `char` boundaries, so this slice cannot
        // split a UTF-8 code point.
        self.tokens.push(Token {
            kind,
            text: self.source[start..end].to_owned(),
            span: Span::new(start, end),
        });
    }

    fn error(&mut self, code: &'static str, message: impl Into<String>, span: Span) {
        self.diagnostics
            .push(Diagnostic::error(code, message, span));
    }

    fn lex_whitespace(&mut self) {
        let start = self.offset;
        while let Some(character) = self.peek() {
            if character.is_whitespace() || character == ',' {
                self.advance();
            } else {
                break;
            }
        }
        self.push(TokenKind::Whitespace, start, self.offset);
    }

    fn lex_comment(&mut self) {
        let start = self.offset;
        self.advance(); // `;`
        while let Some(character) = self.peek() {
            if character == '\n' || character == '\r' {
                break;
            }
            self.advance();
        }
        self.push(TokenKind::Comment, start, self.offset);
    }

    fn lex_non_trivia(&mut self) {
        let start = self.offset;
        let Some(character) = self.peek() else {
            return;
        };

        let kind = match character {
            '(' => Some(TokenKind::LeftParen),
            ')' => Some(TokenKind::RightParen),
            '[' => Some(TokenKind::LeftBracket),
            ']' => Some(TokenKind::RightBracket),
            '{' => Some(TokenKind::LeftBrace),
            '}' => Some(TokenKind::RightBrace),
            '\'' => Some(TokenKind::Quote),
            '`' => Some(TokenKind::SyntaxQuote),
            '^' => Some(TokenKind::Metadata),
            '"' => {
                self.lex_string();
                return;
            }
            '~' => {
                self.advance();
                if self.peek() == Some('@') {
                    self.advance();
                    self.push(TokenKind::UnquoteSplicing, start, self.offset);
                } else {
                    self.push(TokenKind::Unquote, start, self.offset);
                }
                return;
            }
            '#' => {
                self.advance();
                if self.peek() == Some('{') {
                    self.advance();
                    self.push(TokenKind::SetStart, start, self.offset);
                } else {
                    let is_end_of_input = self.peek().is_none();
                    self.push(TokenKind::Error, start, self.offset);
                    self.error(
                        UNSUPPORTED_DISPATCH,
                        if is_end_of_input {
                            "unsupported reader dispatch `#`"
                        } else {
                            "unsupported reader dispatch starting with `#`"
                        },
                        Span::new(start, self.offset),
                    );
                }
                return;
            }
            _ => None,
        };

        if let Some(kind) = kind {
            self.advance();
            self.push(kind, start, self.offset);
        } else {
            self.lex_atom();
        }
    }

    fn lex_atom(&mut self) {
        let start = self.offset;
        while let Some(character) = self.peek() {
            if is_atom_terminator(character) {
                break;
            }
            self.advance();
        }

        // `lex_non_trivia` is only called for a non-delimiter, but keep this
        // guard so malformed future additions cannot make the lexer loop.
        if self.offset == start {
            self.advance();
        }
        self.push(TokenKind::Atom, start, self.offset);
    }

    fn lex_string(&mut self) {
        let start = self.offset;
        self.advance(); // opening quote
        let mut valid = true;
        let mut closed = false;
        let mut saw_line_break = false;
        let mut line_recovery = None;

        while let Some(character) = self.peek() {
            match character {
                '"' => {
                    self.advance();
                    closed = true;
                    break;
                }
                '\\' => {
                    let escape_start = self.offset;
                    self.advance();
                    let Some(escape) = self.peek() else {
                        valid = false;
                        self.error(
                            INVALID_ESCAPE,
                            "string escape is missing a character",
                            Span::new(escape_start, self.offset),
                        );
                        break;
                    };

                    if is_simple_escape(escape) {
                        self.advance();
                    } else if escape == 'u' {
                        self.advance();
                        if !self.consume_hex_digits(4, escape_start) {
                            valid = false;
                        }
                    } else if escape == 'U' {
                        self.advance();
                        if !self.consume_hex_digits(8, escape_start) {
                            valid = false;
                        }
                    } else if escape == 'x' {
                        self.advance();
                        if !self.consume_hex_digits(2, escape_start) {
                            valid = false;
                        }
                    } else if escape == '\n' || escape == '\r' {
                        self.consume_line_break();
                    } else if ('0'..='7').contains(&escape) {
                        self.consume_octal_digits();
                    } else {
                        valid = false;
                        self.advance();
                        self.error(
                            INVALID_ESCAPE,
                            format!("unsupported string escape `\\{escape}`"),
                            Span::new(escape_start, self.offset),
                        );
                    }
                }
                '\n' | '\r' => {
                    valid = false;
                    let line_break_start = self.offset;
                    self.consume_line_break();
                    if !saw_line_break {
                        self.error(
                            LINE_BREAK_IN_STRING,
                            "unescaped line break in string literal",
                            Span::new(line_break_start, self.offset),
                        );
                        saw_line_break = true;
                        line_recovery = Some((self.offset, self.diagnostics.len()));
                    }
                }
                _ => {
                    self.advance();
                }
            }
        }

        if !closed {
            valid = false;
            if let Some((recovery_offset, diagnostic_count)) = line_recovery {
                self.offset = recovery_offset;
                self.diagnostics.truncate(diagnostic_count);
            }
            self.error(
                UNCLOSED_STRING,
                "unterminated string literal",
                Span::new(start, self.offset),
            );
        }

        self.push(
            if valid {
                TokenKind::String
            } else {
                TokenKind::Error
            },
            start,
            self.offset,
        );
    }

    fn consume_hex_digits(&mut self, count: usize, escape_start: usize) -> bool {
        let mut consumed = 0;
        while consumed < count {
            match self.peek() {
                Some(character) if character.is_ascii_hexdigit() => {
                    self.advance();
                    consumed += 1;
                }
                _ => break,
            }
        }

        if consumed == count {
            true
        } else {
            let end = self.offset.max(escape_start + 1);
            self.error(
                INVALID_ESCAPE,
                format!("expected {count} hexadecimal digits in string escape"),
                Span::new(escape_start, end),
            );
            false
        }
    }

    fn consume_octal_digits(&mut self) {
        // The first octal digit is still at the current offset.  Python and
        // Clojure both accept up to three octal digits in a string escape.
        let mut consumed = 0;
        while consumed < 3 {
            let Some(character) = self.peek() else {
                break;
            };
            if ('0'..='7').contains(&character) {
                self.advance();
                consumed += 1;
            } else {
                break;
            }
        }
    }

    fn consume_line_break(&mut self) {
        if self.peek() == Some('\r') {
            self.advance();
            if self.peek() == Some('\n') {
                self.advance();
            }
        } else if self.peek() == Some('\n') {
            self.advance();
        }
    }
}

fn is_simple_escape(character: char) -> bool {
    matches!(
        character,
        'a' | 'b' | 'f' | 'n' | 'r' | 't' | 'v' | '\\' | '\'' | '"' | '/'
    )
}

fn is_atom_terminator(character: char) -> bool {
    character.is_whitespace()
        || character == ','
        || matches!(
            character,
            ';' | '(' | ')' | '[' | ']' | '{' | '}' | '\'' | '`' | '~' | '^' | '"'
        )
}

#[cfg(test)]
mod tests {
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
}
