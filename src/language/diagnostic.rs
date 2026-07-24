use serde::Serialize;

use crate::source::{LineIndex, Span};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Diagnostic {
    pub code: &'static str,
    pub severity: Severity,
    pub message: String,
    pub span: Span,
}

impl Diagnostic {
    #[must_use]
    pub fn error(code: &'static str, message: impl Into<String>, span: Span) -> Self {
        Self {
            code,
            severity: Severity::Error,
            message: message.into(),
            span,
        }
    }
}

#[must_use]
pub fn render(source_name: &str, source: &str, diagnostic: &Diagnostic) -> String {
    render_message(
        source_name,
        source,
        diagnostic.span,
        "error",
        diagnostic.code,
        &diagnostic.message,
    )
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
