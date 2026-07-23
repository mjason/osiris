//! Structured Python backend for the typed HIR.
//!
//! The backend intentionally has no source-string templates for generated
//! Python.  It lowers HIR into [`crate::python_ast`] first and delegates all
//! syntax, escaping, and precedence decisions to that module's printer.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::{
    hir::{self, ExprKind, ItemKind, Operator},
    name::python_identifier,
    python_ast as py,
    source::Span,
    types::{PythonVersion, Type, nominal_short_name, python_builtin_exception_from_binding},
};

/// A fully rendered backend result.  Keeping the AST alongside its rendering
/// lets the compiler, source-map writer, and tests inspect the same result.
#[derive(Clone, Debug, PartialEq)]
pub struct GeneratedPython {
    pub module: py::Module,
    pub source: String,
}

/// An error raised while lowering a semantically valid HIR to Python.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackendError {
    pub message: String,
    pub span: Option<Span>,
}

impl BackendError {
    fn new(message: impl Into<String>, span: Option<Span>) -> Self {
        Self {
            message: message.into(),
            span,
        }
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    #[must_use]
    pub const fn span(&self) -> Option<Span> {
        self.span
    }
}

impl fmt::Display for BackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for BackendError {}

/// Lower a typed HIR module to a deterministic Python module and source.
pub fn compile_module(
    module: &hir::Module,
    target: impl Into<PythonVersion>,
) -> Result<GeneratedPython, BackendError> {
    let mut backend = Backend::new(module, target.into());
    let body = backend.lower_items(module)?;
    let mut imports = backend.imports();
    imports.extend(backend.typing_imports());

    // Future annotations keeps nominal/generic references readable and makes
    // forward references between generated declarations legal on Python 3.9.
    let mut final_body = Vec::with_capacity(imports.len() + body.len());
    final_body.push(py::Stmt::Import(py::Import::From {
        module: Some("__future__".to_owned()),
        names: vec![py::ImportAlias::new("annotations")],
        level: 0,
    }));
    final_body.extend(imports);
    final_body.extend(backend.typevar_declarations());
    final_body.extend(body);
    let python_module = py::Module::new(final_body);
    let source = python_module
        .to_source()
        .map_err(|error| BackendError::new(error.to_string(), None))?;
    Ok(GeneratedPython {
        module: python_module,
        source,
    })
}

/// Alias kept explicit for callers that prefer the verb used by other
/// backends.  It also makes the public API pleasant to discover in docs.
pub fn emit_module(
    module: &hir::Module,
    target: impl Into<PythonVersion>,
) -> Result<GeneratedPython, BackendError> {
    compile_module(module, target)
}

struct Backend<'hir> {
    target: PythonVersion,
    bindings: BTreeMap<crate::name::BindingId, &'hir hir::Binding>,
    names: BTreeMap<crate::name::BindingId, String>,
    reserved_names: BTreeSet<String>,
    temporary_counter: usize,
    helper_counter: usize,
    direct_imports: BTreeMap<String, Option<String>>,
    from_imports: BTreeMap<String, BTreeMap<String, Option<String>>>,
    typing: BTreeSet<String>,
    need_dataclass: bool,
    need_dataclass_field: bool,
    typevars: BTreeMap<String, String>,
    typevar_names: BTreeMap<crate::types::TypeVarId, String>,
    active_type_parameters: BTreeMap<String, String>,
    binding_overrides: Vec<BTreeMap<crate::name::BindingId, py::Expr>>,
}

mod bindings;
mod control;
mod declarations;
mod expressions;
mod setup;
mod support;

use support::*;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
