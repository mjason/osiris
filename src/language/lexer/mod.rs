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
#[path = "tests.rs"]
mod tests;
