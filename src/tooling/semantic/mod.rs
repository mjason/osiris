//! Versioned semantic data for editors, agents, and interface tooling.
//!
//! This module is a projection of compiler::Analysis. It never parses or
//! infers types itself.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::{Value as JsonValue, json};

use crate::{
    ast,
    compiler::Analysis,
    diagnostic::Diagnostic,
    hir::{self, Expr, ExprKind, ItemKind},
    macro_expand::ExpansionTrace,
    name::{BindingKind, contains_cjk},
    source::Span,
    syntax::{Form, FormKind, MetadataEntry},
    types::{CallSummaries, DataProperties, EffectRow, TemporalSummary, Type},
};

/// Bumped when the JSON shape of SemanticDocument changes incompatibly.
pub const SEMANTIC_DOCUMENT_VERSION: u32 = 1;

/// A localized label. Locale only affects presentation and completion order.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct LocalizedLabel {
    #[serde(rename = "zh-CN")]
    pub zh_cn: String,
    pub en: String,
}

impl LocalizedLabel {
    #[must_use]
    pub fn new(canonical: impl Into<String>, chinese: Option<String>) -> Self {
        let en = canonical.into();
        Self {
            zh_cn: chinese.unwrap_or_else(|| en.clone()),
            en,
        }
    }

    #[must_use]
    pub fn for_locale(&self, locale: &str) -> &str {
        if locale.eq_ignore_ascii_case("zh-cn") || locale.eq_ignore_ascii_case("zh") {
            &self.zh_cn
        } else {
            &self.en
        }
    }
}

/// Author-written or phase-1 macro metadata, retaining the reader datum.
#[derive(Clone, Debug, Serialize)]
pub struct AuthoredMetadata {
    pub key: JsonValue,
    pub value: JsonValue,
    pub key_text: String,
    pub value_text: String,
    pub span: Span,
    pub raw: JsonValue,
}

/// A static record projected without extension-specific interpretation.
#[derive(Clone, Debug, Serialize)]
pub struct SemanticRecord {
    pub schema: String,
    pub owner: String,
    pub fields: JsonValue,
    pub span: Span,
    pub metadata: Vec<AuthoredMetadata>,
    pub raw: JsonValue,
}

/// A fact with explicit provenance and trust class.
#[derive(Clone, Debug, Serialize)]
pub struct SemanticFact {
    pub kind: String,
    pub value: JsonValue,
    pub provenance: Vec<FactOrigin>,
    pub trust: String,
    pub span: Span,
}

#[derive(Clone, Debug, Serialize)]
pub struct FactOrigin {
    pub kind: String,
    pub span: Span,
    pub detail: Option<String>,
}

/// The four intentionally separate metadata/fact layers.
#[derive(Clone, Debug, Default, Serialize)]
pub struct SemanticLayers {
    pub authored: Vec<AuthoredMetadata>,
    pub records: Vec<SemanticRecord>,
    pub declared: Vec<SemanticFact>,
    pub verified: Vec<SemanticFact>,
}

/// JSON-stable effect, temporal, and data summaries.
#[derive(Clone, Debug, Serialize)]
pub struct SemanticSummary {
    pub effects: EffectRow,
    /// Compatibility spelling for clients using the singular term.
    pub effect: EffectRow,
    pub temporal: TemporalSummary,
    pub data: DataProperties,
}

impl SemanticSummary {
    #[must_use]
    pub fn from_call(summary: &CallSummaries) -> Self {
        Self {
            effects: summary.effects.clone(),
            effect: summary.effects.clone(),
            temporal: summary.temporal.clone(),
            data: summary.data.clone(),
        }
    }

    #[must_use]
    pub fn unknown() -> Self {
        Self::from_call(&CallSummaries::unknown())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SemanticAlias {
    pub spelling: String,
    pub canonical: String,
    pub public: bool,
    pub preferred: bool,
    pub span: Span,
    pub labels: LocalizedLabel,
}

/// One resolved binding, including locals and parameters.
#[derive(Clone, Debug, Serialize)]
pub struct SemanticSymbol {
    pub binding_id: String,
    pub canonical: String,
    pub source: String,
    pub source_spelling: String,
    pub python: String,
    pub kind: BindingKind,
    pub aliases: Vec<SemanticAlias>,
    pub public: bool,
    #[serde(rename = "type")]
    pub ty: Type,
    pub metadata: SemanticLayers,
    pub summary: SemanticSummary,
    pub labels: LocalizedLabel,
    pub span: Span,
    pub definition: Span,
    pub references: Vec<Span>,
    pub occurrences: Vec<Span>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MacroTraceView {
    pub macro_name: String,
    pub macro_binding_id: String,
    pub call_span: Span,
    pub expansion_span: Span,
    pub depth: usize,
    pub origin: Vec<Span>,
}

impl From<&ExpansionTrace> for MacroTraceView {
    fn from(trace: &ExpansionTrace) -> Self {
        Self {
            macro_name: trace.macro_name.clone(),
            macro_binding_id: trace.macro_binding_id.clone(),
            call_span: trace.call_span,
            expansion_span: trace.expansion_span,
            depth: trace.depth,
            origin: trace.origin.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct OperationNode {
    pub id: String,
    pub kind: String,
    pub span: Span,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding_id: Option<String>,
    #[serde(rename = "type")]
    pub ty: Type,
    pub summary: SemanticSummary,
    pub labels: LocalizedLabel,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub macro_origins: Vec<Span>,
}

#[derive(Clone, Debug, Serialize)]
pub struct OperationEdge {
    pub from: String,
    pub to: String,
    pub kind: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct OperationGraph {
    pub nodes: Vec<OperationNode>,
    pub edges: Vec<OperationEdge>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SemanticDiagnostic {
    pub code: String,
    pub severity: String,
    pub message: String,
    pub span: Span,
}

impl From<&Diagnostic> for SemanticDiagnostic {
    fn from(diagnostic: &Diagnostic) -> Self {
        Self {
            code: diagnostic.code.to_owned(),
            severity: format!("{:?}", diagnostic.severity).to_lowercase(),
            message: diagnostic.message.clone(),
            span: diagnostic.span,
        }
    }
}

/// Versioned semantic document consumed by LSP, Agent tools, and inspect.
#[derive(Clone, Debug, Serialize)]
pub struct SemanticDocument {
    pub version: u32,
    pub document_version: i64,
    pub source: String,
    pub source_len: usize,
    pub module: String,
    pub symbols: Vec<SemanticSymbol>,
    pub authored: Vec<AuthoredMetadata>,
    pub records: Vec<SemanticRecord>,
    pub declared: Vec<SemanticFact>,
    pub verified: Vec<SemanticFact>,
    pub macro_traces: Vec<MacroTraceView>,
    pub operation_graph: OperationGraph,
    /// Flat aliases retained for early Agent clients.
    pub operations: Vec<OperationNode>,
    pub operation_edges: Vec<OperationEdge>,
    pub diagnostics: Vec<SemanticDiagnostic>,
}

/// Semantic view is the public name used by Agent integrations.
pub type SemanticView = SemanticDocument;

mod analysis;
mod metadata;
mod operations;
mod projection;

use analysis::*;
use metadata::*;
use operations::*;
pub use projection::project;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
