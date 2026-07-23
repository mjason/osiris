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
