//! Language Server Protocol state and JSON-RPC dispatch.
//!
//! The state owns one compiler Analysis per open document. Project documents
//! are analyzed against their source-root workspace, while editor queries are
//! projections of the cached Analysis and SemanticDocument.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use unicode_normalization::UnicodeNormalization;

use crate::{
    compiler::{self, Analysis, CompileInput, CompileOptions},
    dependency, hir,
    interface::{self, Interface},
    name::{BindingKind, IdentifierLint, contains_cjk, lint_forms_strict},
    printer::render_document_text,
    project::{ProjectConfig, PythonVersion},
    reader,
    semantic::{SemanticDocument, SemanticSymbol},
    source::Span,
    syntax::{Form, FormKind, metadata_aliases},
    types::Type,
};

mod protocol;
mod rpc;
mod signature;
mod state;
mod symbols;
mod text;
mod workspace;

pub use protocol::*;
use rpc::document_not_found;
pub use rpc::{JsonRpcMachine, JsonRpcOutcome, LspServer, handle_json_rpc, handle_request};
use signature::*;
#[cfg(test)]
use state::ProjectDocumentAnalysis;
pub use state::{LspState, OpenDocument};
use symbols::*;
use text::{apply_content_change, escape_markdown, node_id_for_span};
pub use text::{offset_to_position, position_to_offset, span_to_range};
use workspace::*;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
