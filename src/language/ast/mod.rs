//! Surface AST lowering for Osiris.
//!
//! The reader deliberately knows very little about the language.  This module
//! is the boundary where the reader's lossless [`Form`] tree becomes a small,
//! typed-enough surface tree that later name resolution and HIR lowering can
//! consume.  Lowering is intentionally non-binding: a symbol is still just a
//! symbol and imports/aliases are declarations, not lookups.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::{
    core_forms::{DeclarationForm, ExpressionForm},
    diagnostic::Diagnostic,
    source::Span,
    syntax::{Document, Form, FormKind, MetadataEntry, Name, datum_eq},
    types::{
        Alignment, Availability, CallSummaries, DataProperties, Effect, EffectRow, TemporalBound,
        TemporalSummary, parse_type,
    },
};

mod lower;
mod model;

pub use lower::lower_document;
pub use model::*;

/// Stable diagnostics emitted by this lowering pass.
pub const AST_EXPECTED_LIST: &str = "OSR-A0001";
pub const AST_MISSING_NAME: &str = "OSR-A0002";
pub const AST_INVALID_NAME: &str = "OSR-A0003";
pub const AST_WRONG_SHAPE: &str = "OSR-A0004";
pub const AST_EXPECTED_VECTOR: &str = "OSR-A0005";
pub const AST_EXPECTED_PAIR: &str = "OSR-A0006";
pub const AST_INVALID_KEYWORD_ARGS: &str = "OSR-A0007";
pub const AST_UNKNOWN_CLAUSE: &str = "OSR-A0008";
pub const AST_INVALID_CONTRACT: &str = "OSR-A0009";
pub const AST_CONFLICTING_TYPE_ANNOTATION: &str = "OSR-A0010";
pub const AST_INVALID_TYPE_METADATA: &str = "OSR-A0011";

/// Metadata attached to every surface node is copied from the reader form.
fn list_parts(form: &Form) -> Option<&[Form]> {
    match &form.kind {
        FormKind::List(parts) => Some(parts),
        _ => None,
    }
}

fn destructured_parameter_parts(form: &Form, phase: FunctionPhase) -> Option<(&Form, &[Form])> {
    match &form.kind {
        FormKind::Map(_) => Some((form, &[] as &[Form])),
        FormKind::Vector(parts) if phase != FunctionPhase::Runtime => Some((form, &parts[..0])),
        FormKind::Vector(parts) if !ordinary_runtime_parameter_declaration(parts) => {
            Some((form, &parts[..0]))
        }
        _ => None,
    }
}

fn ordinary_runtime_parameter_declaration(parts: &[Form]) -> bool {
    matches!(parts, [name, equals, _]
        if template_symbol_name(name).is_some() && is_equal_symbol(equals))
}

/// Returns a symbol used directly or supplied through a syntax-quote
/// unquote.  The latter is still syntax data while a macro declaration is
/// lowered, but it denotes the generated declaration's name.
fn template_symbol_name(form: &Form) -> Option<Name> {
    match &form.kind {
        FormKind::Symbol(name) => Some(name.clone()),
        FormKind::ReaderMacro {
            macro_kind: crate::syntax::ReaderMacroKind::Unquote,
            form: inner,
        } => template_symbol_name(inner),
        _ => None,
    }
}

fn symbol_name(form: &Form) -> Option<Name> {
    match &form.kind {
        FormKind::Symbol(name) => Some(name.clone()),
        _ => None,
    }
}

fn keyword_name(form: &Form) -> Option<Name> {
    match &form.kind {
        FormKind::Keyword(name) => Some(name.clone()),
        _ => None,
    }
}

fn keyword_span(form: &Form) -> Span {
    form.span
}

fn contract_name(form: &Form) -> Option<String> {
    match &form.kind {
        FormKind::Keyword(name) => Some(name.canonical.trim_start_matches(':').to_owned()),
        FormKind::Symbol(name) => Some(name.canonical.clone()),
        FormKind::String(value) => Some(value.clone()),
        _ => None,
    }
}

fn contract_optional_name(form: &Form) -> Result<Option<String>, ()> {
    if matches!(&form.kind, FormKind::None) {
        return Ok(None);
    }
    contract_name(form)
        .filter(|name| !name.is_empty())
        .map(Some)
        .ok_or(())
}

fn contract_optional_names(form: &Form) -> Result<Option<Vec<String>>, ()> {
    if matches!(&form.kind, FormKind::None) {
        return Ok(None);
    }
    let FormKind::Vector(values) = &form.kind else {
        return Err(());
    };
    values
        .iter()
        .map(|value| {
            contract_name(value)
                .filter(|name| !name.is_empty())
                .ok_or(())
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn contract_optional_bool(form: &Form) -> Result<Option<bool>, ()> {
    match &form.kind {
        FormKind::None => Ok(None),
        FormKind::Bool(value) => Ok(Some(*value)),
        _ => Err(()),
    }
}

fn error_name() -> Name {
    Name {
        spelling: "<error>".to_owned(),
        canonical: "<error>".to_owned(),
    }
}

fn is_equal_symbol(form: &Form) -> bool {
    matches!(&form.kind, FormKind::Symbol(name) if name.canonical == "=")
}

fn is_ampersand_symbol(form: &Form) -> bool {
    matches!(&form.kind, FormKind::Symbol(name) if name.canonical == "&")
}

fn metadata_key(form: &Form) -> Option<&str> {
    match &form.kind {
        FormKind::Keyword(name) | FormKind::Symbol(name) => {
            Some(name.canonical.trim_start_matches(':'))
        }
        _ => None,
    }
}

fn merge_declaration_metadata(
    mut declaration: Vec<MetadataEntry>,
    name: &[MetadataEntry],
) -> Vec<MetadataEntry> {
    for entry in name {
        if let Some(existing) = declaration
            .iter_mut()
            .find(|existing| datum_eq(&existing.key, &entry.key))
        {
            *existing = entry.clone();
        } else {
            declaration.push(entry.clone());
        }
    }
    declaration
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
