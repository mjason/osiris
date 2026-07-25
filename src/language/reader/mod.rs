//! Recoverable S-expression grammar built over the lossless token stream.
//!
//! `nom` provides the parser contract and composition layer. Recovery is kept
//! inside individual grammar productions so an incomplete editor buffer still
//! produces forms, metadata, and stable diagnostics.

use std::collections::{BTreeMap, VecDeque};

use nom::{
    IResult, Parser,
    branch::alt,
    error::{Error, ErrorKind},
};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

use crate::{
    diagnostic::Diagnostic,
    lexer::lex,
    source::Span,
    syntax::{
        Document, Form, FormKind, METADATA_TARGET_LIMITS, MetadataEntry, Name, NodeId,
        NodeIdentity, NodePath, NodePathSegment, ReaderMacroKind, SyntaxNodeKind, Token, TokenKind,
        check_metadata_resources, datum_eq, metadata_datum_is_serializable, source_form_eq,
    },
};

// Keep recursive reader productions below the default Rust thread stack while
// still allowing realistic generated forms.  Inputs beyond this bound become
// recoverable error forms instead of risking a process-level stack overflow.
const MAX_DEPTH: usize = 96;
const MAX_DIAGNOSTICS: usize = 100;

type TokenInput<'source> = &'source [&'source Token];
type ParseResult<'source> = IResult<TokenInput<'source>, ParsedForm>;

#[derive(Debug)]
struct ParsedForm {
    form: Form,
    diagnostics: Vec<Diagnostic>,
}

impl ParsedForm {
    fn new(form: Form) -> Self {
        Self {
            form,
            diagnostics: Vec::new(),
        }
    }

    fn error(form: Form, diagnostic: Diagnostic) -> Self {
        Self {
            form,
            diagnostics: vec![diagnostic],
        }
    }
}

/// Reads a complete source file into a lossless token stream and recoverable form tree.
#[must_use]
pub fn read(source: &str) -> Document {
    read_snapshot(source, None)
}

/// Reads a new source snapshot while retaining identities for unchanged forms
/// from `previous`. Parsing remains fully recoverable; this API never mutates
/// or borrows the previous snapshot in the returned document.
#[must_use]
pub fn read_incremental(source: &str, previous: &Document) -> Document {
    read_snapshot(source, Some(previous))
}

/// Map each normalized embedded-body line to the byte offset of its first
/// content byte in the host source.
#[must_use]
pub fn embedded_line_source_offsets(raw: &str, span: Span) -> Vec<usize> {
    let first_break = if raw.starts_with("\r\n") {
        Some(2)
    } else if raw.starts_with(['\n', '\r']) {
        Some(1)
    } else {
        None
    };
    let Some(first_break) = first_break else {
        return vec![span.start];
    };
    let Some(last_break_char) = raw.rfind(['\n', '\r']) else {
        return vec![span.start];
    };
    let closing_indent = &raw[last_break_char + 1..];
    if !closing_indent
        .chars()
        .all(|character| matches!(character, ' ' | '\t'))
    {
        return vec![span.start];
    }
    let content_end = if raw.as_bytes()[last_break_char] == b'\n'
        && last_break_char > 0
        && raw.as_bytes()[last_break_char - 1] == b'\r'
    {
        last_break_char - 1
    } else {
        last_break_char
    };
    let content = &raw[first_break..content_end];
    let mut offsets = Vec::new();
    let mut cursor = 0;
    loop {
        let line_end = content[cursor..]
            .find(['\n', '\r'])
            .map_or(content.len(), |offset| cursor + offset);
        let line = &content[cursor..line_end];
        let stripped = if line.is_empty() || !line.starts_with(closing_indent) {
            0
        } else {
            closing_indent.len()
        };
        offsets.push(span.start + first_break + cursor + stripped);
        if line_end == content.len() {
            break;
        }
        cursor = if content.as_bytes()[line_end] == b'\r'
            && content.as_bytes().get(line_end + 1) == Some(&b'\n')
        {
            line_end + 2
        } else {
            line_end + 1
        };
    }
    offsets
}

/// Map one normalized embedded-body byte offset into the host source.
#[must_use]
pub fn embedded_source_offset(raw: &str, body: &str, span: Span, offset: usize) -> usize {
    let bounded = offset.min(body.len());
    let line = body[..bounded]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count();
    let line_start = body[..bounded].rfind('\n').map_or(0, |index| index + 1);
    embedded_line_source_offsets(raw, span)
        .get(line)
        .copied()
        .unwrap_or(span.end)
        .saturating_add(bounded - line_start)
        .min(span.end)
}

fn read_snapshot(source: &str, previous: Option<&Document>) -> Document {
    let lexed = lex(source);
    let significant = lexed
        .tokens
        .iter()
        .filter(|token| !token.kind.is_trivia())
        .collect::<Vec<_>>();
    let mut input = significant.as_slice();
    let mut forms = Vec::new();
    let mut diagnostics = lexed.diagnostics;

    while !input.is_empty() {
        match parse_form(input, 0, source.len()) {
            Ok((rest, parsed)) if rest.len() < input.len() => {
                input = rest;
                forms.push(parsed.form);
                diagnostics.extend(parsed.diagnostics);
            }
            Ok(_) | Err(_) => {
                let token = input[0];
                diagnostics.push(Diagnostic::error(
                    "OSR-R0013",
                    "reader could not make progress at this token",
                    token.span,
                ));
                forms.push(Form::new(
                    FormKind::Error("unreadable token".to_owned()),
                    token.span,
                ));
                input = &input[1..];
            }
        }
    }

    diagnostics
        .sort_by_key(|diagnostic| (diagnostic.span.start, diagnostic.span.end, diagnostic.code));
    diagnostics.truncate(MAX_DIAGNOSTICS);

    drop(significant);
    let nodes = build_node_identities(source, &forms, previous);
    Document {
        format_version: 1,
        source_len: source.len(),
        tokens: lexed.tokens,
        forms,
        nodes,
        diagnostics,
    }
}

mod datum;
mod identity;
mod parser;

use datum::*;
use identity::*;
use parser::*;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
