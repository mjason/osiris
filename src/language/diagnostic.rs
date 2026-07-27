use serde::Serialize;

use crate::source::{LineIndex, Span};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
}

/// Why a related location is attached to a diagnostic.
///
/// The kind is the stable machine-readable part; `message` is the localized
/// human sentence and MUST NOT be used for identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RelatedKind {
    /// The macro call whose expansion produced the reported syntax.
    MacroCallSite,
    /// The `defmacro` the expanded syntax was written in.
    MacroDefinition,
}

/// One additional location a diagnostic points at.
///
/// `module` names the module `span` belongs to when that is not the module the
/// diagnostic was raised in. `None` means the diagnostic's own module, which is
/// the only case where a renderer holding a single source buffer can resolve
/// the span to a line and column.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Related {
    pub kind: RelatedKind,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    pub span: Span,
    /// Macro binding id when the entry describes a macro, so tooling can join
    /// the diagnostic to an expansion trace without matching spans.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding_id: Option<String>,
}

impl Related {
    #[must_use]
    pub fn new(kind: RelatedKind, message: impl Into<String>, span: Span) -> Self {
        Self {
            kind,
            message: message.into(),
            module: None,
            span,
            binding_id: None,
        }
    }

    #[must_use]
    pub fn in_module(mut self, module: Option<String>) -> Self {
        self.module = module;
        self
    }

    #[must_use]
    pub fn for_macro(mut self, binding_id: impl Into<String>) -> Self {
        self.binding_id = Some(binding_id.into());
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Diagnostic {
    pub code: &'static str,
    pub severity: Severity,
    pub message: String,
    pub span: Span,
    /// Ordered supporting locations, outermost cause first. Empty for a
    /// diagnostic that needs no more than its primary span.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<Related>,
}

impl Diagnostic {
    #[must_use]
    pub fn error(code: &'static str, message: impl Into<String>, span: Span) -> Self {
        Self {
            code,
            severity: Severity::Error,
            message: message.into(),
            span,
            related: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_related(mut self, related: impl IntoIterator<Item = Related>) -> Self {
        self.related.extend(related);
        self
    }
}

#[must_use]
pub fn render(source_name: &str, source: &str, diagnostic: &Diagnostic) -> String {
    let mut rendered = render_message(
        source_name,
        source,
        diagnostic.span,
        "error",
        diagnostic.code,
        &diagnostic.message,
    );
    let index = LineIndex::new(source);
    for related in &diagnostic.related {
        // A span from another module is only a byte range here, and repeating
        // the primary span adds nothing, so both render as a bare note.
        let location = if related.module.is_some() || related.span == diagnostic.span {
            String::new()
        } else {
            let start = related.span.start.min(source.len());
            let (line, column) = index.line_column(source, start);
            format!(" ({source_name}:{line}:{column})")
        };
        rendered.push_str(&format!("  = note: {}{location}\n", related.message));
    }
    rendered
}

#[must_use]
pub fn render_warning(
    source_name: &str,
    source: &str,
    span: Span,
    code: &str,
    message: &str,
) -> String {
    render_message(source_name, source, span, "warning", code, message)
}

fn render_message(
    source_name: &str,
    source: &str,
    span: Span,
    severity: &str,
    code: &str,
    message: &str,
) -> String {
    let index = LineIndex::new(source);
    let start = span.start.min(source.len());
    let (line, column) = index.line_column(source, start);
    let bounds = index.line_bounds(source, line);
    let text = &source[bounds.start..bounds.end];
    let marker_offset = source[bounds.start..start].chars().count();
    let marker_end = span.end.min(bounds.end).max(start);
    let marker_width = source[start..marker_end].chars().count().max(1);

    format!(
        "{source_name}:{line}:{column}: {severity}[{code}]: {message}\n  |\n{line:>2} | {text}\n  | {}{}\n",
        " ".repeat(marker_offset),
        "^".repeat(marker_width)
    )
}

#[must_use]
pub fn render_all(source_name: &str, source: &str, diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| render(source_name, source, diagnostic))
        .collect::<Vec<_>>()
        .join("")
}
