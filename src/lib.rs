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

#[cfg(test)]
#[path = "lib/tests.rs"]
mod tests;
