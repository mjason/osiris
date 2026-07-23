#[path = "packaging/artifact/mod.rs"]
pub mod artifact;
#[path = "language/ast/mod.rs"]
pub mod ast;
#[path = "backend/python/mod.rs"]
pub mod backend;
#[path = "tooling/cli/mod.rs"]
pub mod cli;
#[path = "compiler/mod.rs"]
pub mod compiler;
#[path = "language/core_forms.rs"]
pub mod core_forms;
#[path = "packaging/dependency/mod.rs"]
pub mod dependency;
#[path = "language/diagnostic.rs"]
pub mod diagnostic;
#[path = "packaging/extension/mod.rs"]
pub mod extension;
#[path = "support/hash.rs"]
mod hash;
#[path = "compiler/hir/mod.rs"]
pub mod hir;
#[path = "packaging/interface/mod.rs"]
pub mod interface;
#[path = "packaging/interface_graph/mod.rs"]
pub mod interface_graph;
#[path = "language/lexer/mod.rs"]
pub mod lexer;
#[path = "tooling/lsp/mod.rs"]
pub mod lsp;
#[path = "tooling/lsp_stdio/mod.rs"]
pub mod lsp_stdio;
#[path = "compiler/macro/mod.rs"]
pub mod macro_expand;
#[path = "compiler/module_graph/mod.rs"]
pub mod module_graph;
#[path = "language/name/mod.rs"]
pub mod name;
#[path = "tooling/printer/mod.rs"]
pub mod printer;
#[path = "packaging/project/mod.rs"]
pub mod project;
#[path = "backend/python/ast/mod.rs"]
pub mod python_ast;
#[path = "language/reader/mod.rs"]
pub mod reader;
#[path = "extensions/static_data/mod.rs"]
pub mod records;
#[path = "tooling/semantic/mod.rs"]
pub mod semantic;
#[path = "language/source/mod.rs"]
pub mod source;
#[path = "backend/python/source_map/mod.rs"]
pub mod source_map;
#[path = "language/syntax/mod.rs"]
pub mod syntax;
#[path = "language/types/mod.rs"]
pub mod types;

/// Returns the compiler version from the Cargo package metadata.
#[must_use]
pub const fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(feature = "python")]
use pyo3::prelude::*;

#[cfg(feature = "python")]
use pyo3::exceptions::PyOSError;

#[cfg(feature = "python")]
#[pyfunction]
#[pyo3(name = "version")]
fn python_version() -> &'static str {
    version()
}

#[cfg(feature = "python")]
#[pyfunction]
#[pyo3(name = "_run_cli")]
fn python_run_cli(arguments: Vec<String>) -> (u8, String, String) {
    let outcome = cli::run_cli(&arguments);
    (outcome.exit_code, outcome.stdout, outcome.stderr)
}

#[cfg(feature = "python")]
#[pyfunction]
#[pyo3(name = "_run_lsp_stdio")]
fn python_run_lsp_stdio() -> PyResult<()> {
    lsp_stdio::run_stdio().map_err(|error| PyOSError::new_err(error.to_string()))
}

#[cfg(feature = "python")]
#[pyfunction]
#[pyo3(name = "_run_watch_stdio")]
fn python_run_watch_stdio(py: Python<'_>, arguments: Vec<String>) -> PyResult<()> {
    py.detach(|| cli::run_watch_stdio(&arguments))
        .map_err(PyOSError::new_err)
}

#[cfg(feature = "python")]
#[pymodule]
fn _core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(python_version, m)?)?;
    m.add_function(wrap_pyfunction!(python_run_cli, m)?)?;
    m.add_function(wrap_pyfunction!(python_run_lsp_stdio, m)?)?;
    m.add_function(wrap_pyfunction!(python_run_watch_stdio, m)?)?;
    Ok(())
}

#[cfg(test)]
#[path = "lib/tests.rs"]
mod tests;
