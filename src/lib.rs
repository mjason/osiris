pub mod artifact;
pub mod ast;
pub mod backend;
pub mod cli;
pub mod compiler;
pub mod dependency;
pub mod diagnostic;
pub mod extension;
pub mod hir;
pub mod interface;
pub mod interface_graph;
pub mod lexer;
pub mod lsp;
pub mod lsp_stdio;
pub mod macro_expand;
pub mod module_graph;
pub mod name;
pub mod printer;
pub mod project;
pub mod python_ast;
pub mod reader;
pub mod records;
pub mod semantic;
pub mod source;
pub mod source_map;
pub mod syntax;
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
#[pymodule]
fn _core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(python_version, m)?)?;
    m.add_function(wrap_pyfunction!(python_run_cli, m)?)?;
    m.add_function(wrap_pyfunction!(python_run_lsp_stdio, m)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_comes_from_cargo_metadata() {
        assert_eq!(super::version(), env!("CARGO_PKG_VERSION"));
    }
}
