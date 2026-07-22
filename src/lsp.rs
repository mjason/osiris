//! Language Server Protocol state and JSON-RPC dispatch.
//!
//! The state owns one compiler Analysis per open document. Project documents
//! are analyzed against their source-root workspace, while editor queries are
//! projections of the cached Analysis and SemanticDocument.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use unicode_normalization::UnicodeNormalization;

use crate::{
    compiler::{self, Analysis, CompileInput, CompileOptions},
    dependency, hir,
    interface::{self, Interface},
    name::{BindingKind, IdentifierLint, lint_forms_strict},
    printer::render_document_text,
    project::{ProjectConfig, PythonVersion},
    reader,
    semantic::{MacroTraceView, SemanticDocument, SemanticSymbol},
    source::Span,
    syntax::{Form, FormKind},
    types::Type,
};

pub const JSON_RPC_VERSION: &str = "2.0";
pub const LSP_SERVER_NAME: &str = "osiris";

pub const PARSE_ERROR: i64 = -32_700;
pub const INVALID_REQUEST: i64 = -32_600;
pub const METHOD_NOT_FOUND: i64 = -32_601;
pub const INVALID_PARAMS: i64 = -32_602;
pub const INTERNAL_ERROR: i64 = -32_603;
pub const DOCUMENT_NOT_FOUND: i64 = -32_002;
pub const STALE_DOCUMENT_VERSION: i64 = -32_003;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Location {
    pub uri: String,
    pub range: Range,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextEdit {
    pub range: Range,
    pub new_text: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceEdit {
    pub changes: BTreeMap<String, Vec<TextEdit>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LspDiagnostic {
    pub range: Range,
    pub severity: u8,
    pub code: String,
    pub source: String,
    pub message: String,
    pub data: JsonValue,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishDiagnosticsParams {
    pub uri: String,
    pub version: i64,
    pub diagnostics: Vec<LspDiagnostic>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarkupContent {
    pub kind: String,
    pub value: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Hover {
    pub contents: MarkupContent,
    pub range: Option<Range>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionItem {
    pub label: String,
    pub kind: u8,
    pub detail: String,
    pub insert_text: String,
    pub sort_text: String,
    pub filter_text: String,
    pub data: JsonValue,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureHelp {
    pub signatures: Vec<SignatureInformation>,
    pub active_signature: u32,
    pub active_parameter: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureInformation {
    pub label: String,
    pub parameters: Vec<ParameterInformation>,
    pub active_parameter: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ParameterInformation {
    pub label: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpandPreview {
    pub uri: String,
    pub version: i64,
    pub text: String,
    pub macro_traces: Vec<MacroTraceView>,
    pub diagnostics: Vec<LspDiagnostic>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentContentChangeEvent {
    #[serde(default)]
    pub range: Option<Range>,
    #[serde(default)]
    pub range_length: Option<u32>,
    pub text: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LspStateError {
    pub code: i64,
    pub message: String,
}

impl LspStateError {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for LspStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for LspStateError {}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<JsonValue>,
    pub method: String,
    #[serde(default)]
    pub params: Option<JsonValue>,
}

/// An open document and its one cached frontend analysis.
#[derive(Clone, Debug)]
pub struct OpenDocument {
    pub uri: String,
    pub version: i64,
    pub text: String,
    pub analysis: Analysis,
    pub semantic: SemanticDocument,
    pub identifier_lints: Vec<IdentifierLint>,
    function_interfaces: BTreeMap<String, interface::FunctionInterface>,
    macro_interfaces: BTreeMap<String, interface::MacroInterface>,
    workspace_symbols: WorkspaceSymbolIndex,
}

impl OpenDocument {
    fn from_analysis(
        uri: String,
        version: i64,
        text: String,
        identifier_lints: Vec<IdentifierLint>,
        frontend: ProjectDocumentAnalysis,
    ) -> Self {
        let ProjectDocumentAnalysis {
            analysis,
            function_interfaces,
            macro_interfaces,
            workspace_symbols,
        } = frontend;
        let semantic = SemanticDocument::from_analysis_at_version(&analysis, uri.clone(), version);
        Self {
            uri,
            version,
            text,
            analysis,
            semantic,
            identifier_lints,
            function_interfaces,
            macro_interfaces,
            workspace_symbols,
        }
    }
}

struct ProjectDocumentAnalysis {
    analysis: Analysis,
    function_interfaces: BTreeMap<String, interface::FunctionInterface>,
    macro_interfaces: BTreeMap<String, interface::MacroInterface>,
    workspace_symbols: WorkspaceSymbolIndex,
}

#[derive(Clone, Debug, Default)]
struct WorkspaceSymbolIndex {
    source_uris: BTreeSet<String>,
    sources: BTreeMap<String, String>,
    definitions: BTreeMap<String, Location>,
    ambiguous_definitions: BTreeSet<String>,
    references: BTreeMap<String, Vec<Location>>,
    rename_occurrences: BTreeMap<String, Vec<RenameOccurrence>>,
    binding_kinds: BTreeMap<String, BindingKind>,
    provider_names: BTreeMap<(String, String), String>,
    ambiguous_provider_names: BTreeSet<(String, String)>,
    pending_import_members: Vec<PendingImportMember>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RenameOccurrence {
    uri: String,
    span: Span,
    spelling: String,
    declaration: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingImportMember {
    uri: String,
    provider: String,
    spelling: String,
    span: Span,
}

/// Mutable LSP database. The v0 implementation recomputes the changed
/// document against one workspace snapshot.
#[derive(Clone, Debug)]
pub struct LspState {
    documents: BTreeMap<String, OpenDocument>,
    target_python: PythonVersion,
    display_locale: String,
    site_roots: Vec<PathBuf>,
    analysis_runs: u64,
    shutdown_requested: bool,
}

impl Default for LspState {
    fn default() -> Self {
        Self {
            documents: BTreeMap::new(),
            target_python: PythonVersion::MINIMUM,
            display_locale: "en".to_owned(),
            site_roots: Vec::new(),
            analysis_runs: 0,
            shutdown_requested: false,
        }
    }
}

impl LspState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_target_python(target_python: PythonVersion) -> Self {
        Self {
            target_python,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn display_locale(&self) -> &str {
        &self.display_locale
    }

    pub fn set_display_locale(&mut self, locale: impl Into<String>) {
        self.display_locale = normalize_locale(locale.into());
    }

    pub fn set_site_roots(&mut self, roots: impl IntoIterator<Item = PathBuf>) {
        self.site_roots = roots.into_iter().collect();
        self.site_roots.sort();
        self.site_roots.dedup();
    }

    #[must_use]
    pub const fn analysis_runs(&self) -> u64 {
        self.analysis_runs
    }

    #[must_use]
    pub const fn shutdown_requested(&self) -> bool {
        self.shutdown_requested
    }

    pub fn request_shutdown(&mut self) {
        self.shutdown_requested = true;
    }

    #[must_use]
    pub fn document(&self, uri: &str) -> Option<&OpenDocument> {
        self.documents.get(uri)
    }

    #[must_use]
    pub fn semantic_document(&self, uri: &str) -> Option<&SemanticDocument> {
        self.document(uri).map(|document| &document.semantic)
    }

    #[must_use]
    pub fn document_version(&self, uri: &str) -> Option<i64> {
        self.document(uri).map(|document| document.version)
    }

    /// Opens or replaces a document and runs the frontend exactly once.
    pub fn did_open(
        &mut self,
        uri: impl Into<String>,
        version: i64,
        text: impl Into<String>,
    ) -> PublishDiagnosticsParams {
        let uri = uri.into();
        let text = text.into();
        let document = self.analyze_document(uri.clone(), version, text);
        self.analysis_runs += 1;
        self.refresh_workspace_symbols(&document);
        self.documents.insert(uri.clone(), document);
        self.diagnostics(&uri)
            .expect("the opened document was just inserted")
    }

    pub fn open_document(
        &mut self,
        uri: impl Into<String>,
        version: i64,
        text: impl Into<String>,
    ) -> PublishDiagnosticsParams {
        self.did_open(uri, version, text)
    }

    /// Applies all changes and runs the frontend once for the resulting text.
    pub fn did_change(
        &mut self,
        uri: &str,
        version: i64,
        changes: &[TextDocumentContentChangeEvent],
    ) -> Result<PublishDiagnosticsParams, LspStateError> {
        let Some(current) = self.documents.get(uri) else {
            return Err(LspStateError::new(
                DOCUMENT_NOT_FOUND,
                format!("document {uri} is not open"),
            ));
        };
        if version <= current.version {
            return Err(LspStateError::new(
                STALE_DOCUMENT_VERSION,
                format!(
                    "document version {version} is not newer than {}",
                    current.version
                ),
            ));
        }
        let mut text = current.text.clone();
        for change in changes {
            apply_content_change(&mut text, change)?;
        }
        let document = self.analyze_document(uri.to_owned(), version, text);
        self.analysis_runs += 1;
        self.refresh_workspace_symbols(&document);
        self.documents.insert(uri.to_owned(), document);
        self.diagnostics(uri)
            .ok_or_else(|| LspStateError::new(DOCUMENT_NOT_FOUND, "changed document disappeared"))
    }

    /// Convenience API for full document synchronization.
    pub fn did_change_full(
        &mut self,
        uri: &str,
        version: i64,
        text: impl Into<String>,
    ) -> Result<PublishDiagnosticsParams, LspStateError> {
        self.did_change(
            uri,
            version,
            &[TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: text.into(),
            }],
        )
    }

    pub fn change_document(
        &mut self,
        uri: &str,
        version: i64,
        text: impl Into<String>,
    ) -> Result<PublishDiagnosticsParams, LspStateError> {
        self.did_change_full(uri, version, text)
    }

    pub fn did_close(&mut self, uri: &str) -> bool {
        self.documents.remove(uri).is_some()
    }

    fn refresh_workspace_symbols(&mut self, updated: &OpenDocument) {
        let index = updated.workspace_symbols.clone();
        for document in self.documents.values_mut() {
            if index.source_uris.contains(&document.uri) {
                document.workspace_symbols = index.clone();
            }
        }
    }

    fn analyze_document(&self, uri: String, version: i64, text: String) -> OpenDocument {
        let snapshot = self.documents.get(&uri).map_or_else(
            || reader::read(&text),
            |previous| reader::read_incremental(&text, &previous.analysis.document),
        );
        let identifier_lints = lint_forms_strict(&snapshot.forms);
        let mut frontend = self
            .analyze_project_document(&uri, &text)
            .unwrap_or_else(|| {
                let fallback = fallback_module_name(&uri);
                let options =
                    CompileOptions::new(fallback, self.target_python).with_source_name(uri.clone());
                let analysis = compiler::analyze(&text, &options);
                let workspace_symbols = build_single_symbol_index(&analysis, &uri, &text);
                ProjectDocumentAnalysis {
                    analysis,
                    function_interfaces: BTreeMap::new(),
                    macro_interfaces: BTreeMap::new(),
                    workspace_symbols,
                }
            });
        frontend.analysis.document = snapshot;
        OpenDocument::from_analysis(uri, version, text, identifier_lints, frontend)
    }

    fn analyze_project_document(&self, uri: &str, text: &str) -> Option<ProjectDocumentAnalysis> {
        let source_path = file_uri_to_path(uri)?;
        let project = ProjectConfig::discover(&source_path).ok()?;
        let target_path = fs::canonicalize(&source_path).ok()?;
        let target_module = project.module_name_for_source(&source_path).ok()?;

        let open_texts = self
            .documents
            .values()
            .filter_map(|document| {
                let path = file_uri_to_path(&document.uri)?;
                let path = fs::canonicalize(path).ok()?;
                Some((path, document.text.clone()))
            })
            .collect::<BTreeMap<_, _>>();
        let mut paths = Vec::new();
        for root in &project.source_roots {
            collect_workspace_sources(root, &mut paths).ok()?;
        }
        paths.sort();
        paths.dedup();

        let mut buffers = Vec::with_capacity(paths.len());
        let mut target_index = None;
        for path in paths {
            let canonical = fs::canonicalize(&path).ok()?;
            let module_name = project.module_name_for_source(&path).ok()?;
            let source = if canonical == target_path {
                target_index = Some(buffers.len());
                text.to_owned()
            } else if let Some(open) = open_texts.get(&canonical) {
                open.clone()
            } else {
                fs::read_to_string(&path).ok()?
            };
            buffers.push(WorkspaceBuffer {
                uri: if canonical == target_path {
                    uri.to_owned()
                } else {
                    format!("file://{}", canonical.display())
                },
                options: project_options(&project, &path, module_name),
                source,
            });
        }
        let target_index = target_index?;
        let inputs = buffers
            .iter()
            .map(|buffer| CompileInput::new(&buffer.source, &buffer.options))
            .collect::<Vec<_>>();
        let external_interfaces = load_project_interfaces(&project, &self.site_roots)?;
        let workspace = compiler::compile_workspace(&inputs, &external_interfaces);
        let recovering = workspace.has_errors();
        let (analyses, workspace_diagnostics) = if recovering {
            (
                compiler::analyze_workspace_recovering(&inputs, &external_interfaces),
                workspace.diagnostics,
            )
        } else {
            (
                workspace
                    .units
                    .into_iter()
                    .map(|unit| unit.analysis)
                    .collect(),
                Vec::new(),
            )
        };
        let function_interfaces = collect_function_interfaces(&analyses, &external_interfaces);
        let macro_interfaces = collect_macro_interfaces(&analyses, &external_interfaces);
        let workspace_symbols = build_project_symbol_index(&analyses, &buffers);
        let mut analysis = analyses.into_iter().nth(target_index)?;
        analysis.diagnostics.extend(
            workspace_diagnostics
                .into_iter()
                .filter(|located| located.input_index == target_index)
                .map(|located| located.diagnostic),
        );
        analysis.diagnostics.sort_by(|left, right| {
            (left.span.start, left.span.end, left.code, &left.message).cmp(&(
                right.span.start,
                right.span.end,
                right.code,
                &right.message,
            ))
        });
        analysis.diagnostics.dedup_by(|left, right| {
            left.span == right.span && left.code == right.code && left.message == right.message
        });
        if !recovering {
            debug_assert_eq!(analysis.hir.name, target_module);
        }
        Some(ProjectDocumentAnalysis {
            analysis,
            function_interfaces,
            macro_interfaces,
            workspace_symbols,
        })
    }

    #[must_use]
    pub fn diagnostics(&self, uri: &str) -> Option<PublishDiagnosticsParams> {
        let document = self.document(uri)?;
        let mut diagnostics = document
            .analysis
            .diagnostics
            .iter()
            .map(|diagnostic| LspDiagnostic {
                range: span_to_range(&document.text, diagnostic.span),
                severity: 1,
                code: diagnostic.code.to_owned(),
                source: LSP_SERVER_NAME.to_owned(),
                message: diagnostic.message.clone(),
                data: json!({
                    "span": diagnostic.span,
                    "nodeId": node_id_for_span(document, diagnostic.span),
                    "documentVersion": document.version,
                }),
            })
            .collect::<Vec<_>>();
        diagnostics.extend(document.identifier_lints.iter().map(|lint| LspDiagnostic {
            range: span_to_range(&document.text, lint.span),
            severity: 2,
            code: lint.code.to_owned(),
            source: LSP_SERVER_NAME.to_owned(),
            message: lint.message.clone(),
            data: json!({
                "span": lint.span,
                "nodeId": node_id_for_span(document, lint.span),
                "documentVersion": document.version,
                "lintKind": lint.kind,
                "strictUnicode": true,
            }),
        }));
        diagnostics.sort_by(|left, right| {
            (
                left.range.start.line,
                left.range.start.character,
                left.range.end.line,
                left.range.end.character,
                left.severity,
                &left.code,
            )
                .cmp(&(
                    right.range.start.line,
                    right.range.start.character,
                    right.range.end.line,
                    right.range.end.character,
                    right.severity,
                    &right.code,
                ))
        });
        Some(PublishDiagnosticsParams {
            uri: uri.to_owned(),
            version: document.version,
            diagnostics,
        })
    }

    #[must_use]
    pub fn hover(&self, uri: &str, position: Position, locale: Option<&str>) -> Option<Hover> {
        let document = self.document(uri)?;
        let offset = position_to_offset(&document.text, position)?;
        let symbol = document.semantic.symbol_at(offset)?;
        let locale = locale.unwrap_or(&self.display_locale);
        let label = symbol.labels.for_locale(locale);
        let aliases = symbol
            .aliases
            .iter()
            .map(|alias| alias.spelling.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let effects = serde_json::to_string(&symbol.summary.effects).unwrap_or_default();
        let temporal = serde_json::to_string(&symbol.summary.temporal).unwrap_or_default();
        let data = serde_json::to_string(&symbol.summary.data).unwrap_or_default();
        let mut value = format!(
            "**{}** ({})\n\nType: {}  \nPython: {}  \nBinding: {}",
            escape_markdown(label),
            escape_markdown(&symbol.canonical),
            escape_markdown(&symbol.ty.to_string()),
            escape_markdown(&symbol.python),
            escape_markdown(&symbol.binding_id),
        );
        if !aliases.is_empty() {
            value.push_str(&format!("\n\nAliases: {}", escape_markdown(&aliases)));
        }
        value.push_str(&format!(
            "\n\nEffects: {}  \nTemporal: {}  \nData: {}",
            escape_markdown(&effects),
            escape_markdown(&temporal),
            escape_markdown(&data),
        ));
        Some(Hover {
            contents: MarkupContent {
                kind: "markdown".to_owned(),
                value,
            },
            range: occurrence_at(symbol, offset).map(|span| span_to_range(&document.text, span)),
        })
    }

    #[must_use]
    pub fn completion(
        &self,
        uri: &str,
        position: Position,
        locale: Option<&str>,
    ) -> Vec<CompletionItem> {
        let Some(document) = self.document(uri) else {
            return Vec::new();
        };
        let offset = position_to_offset(&document.text, position).unwrap_or(document.text.len());
        let prefix = completion_prefix(&document.text, offset);
        let locale = locale.unwrap_or(&self.display_locale);
        let chinese = is_chinese_locale(locale);
        let mut items = document
            .semantic
            .symbols
            .iter()
            .filter(|symbol| symbol_matches_prefix(symbol, &prefix))
            .map(|symbol| completion_item(symbol, chinese))
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            (&left.sort_text, &left.label, &left.insert_text).cmp(&(
                &right.sort_text,
                &right.label,
                &right.insert_text,
            ))
        });
        items
    }

    #[must_use]
    pub fn signature_help(
        &self,
        uri: &str,
        position: Position,
        locale: Option<&str>,
    ) -> Option<SignatureHelp> {
        let document = self.document(uri)?;
        let offset = position_to_offset(&document.text, position)?;
        let macro_trace = document
            .analysis
            .expansion_traces
            .iter()
            .filter(|trace| span_contains(trace.call_span, offset))
            .min_by_key(|trace| trace.call_span.end.saturating_sub(trace.call_span.start));
        let runtime_call = document
            .semantic
            .operation_graph
            .nodes
            .iter()
            .filter(|operation| {
                operation.kind == "call"
                    && operation.binding_id.is_some()
                    && span_contains(operation.span, offset)
            })
            .min_by_key(|operation| operation.span.end.saturating_sub(operation.span.start));
        if let Some(trace) = macro_trace
            && runtime_call.is_none_or(|call| {
                trace.call_span.end.saturating_sub(trace.call_span.start)
                    <= call.span.end.saturating_sub(call.span.start)
            })
        {
            return macro_signature_help(
                document,
                trace,
                offset,
                locale.unwrap_or(&self.display_locale),
            );
        }
        let call = runtime_call?;
        let binding_id = call.binding_id.as_deref()?;
        let signature = callable_signature(document, binding_id)?;
        let call_form = find_call_form(&document.analysis.document.forms, call.span)?;
        let FormKind::List(items) = &call_form.kind else {
            return None;
        };
        let invoked_name = items
            .first()
            .and_then(form_name)
            .unwrap_or(&signature.canonical);
        let arguments = source_arguments(items);
        let source_argument = active_source_argument(items, &arguments, offset);
        let active_parameter = active_parameter(&signature.parameters, &arguments, source_argument)
            .map(|index| index as u32);
        let chinese = is_chinese_locale(locale.unwrap_or(&self.display_locale));
        let parameter_labels = signature
            .parameters
            .iter()
            .map(|parameter| signature_parameter_label(parameter, chinese))
            .collect::<Vec<_>>();
        let label = format!(
            "{}({}) -> {}",
            invoked_name,
            parameter_labels.join(", "),
            signature.return_type
        );
        Some(SignatureHelp {
            signatures: vec![SignatureInformation {
                label,
                parameters: parameter_labels
                    .into_iter()
                    .map(|label| ParameterInformation { label })
                    .collect(),
                active_parameter,
            }],
            active_signature: 0,
            active_parameter,
        })
    }

    #[must_use]
    pub fn definition(&self, uri: &str, position: Position) -> Option<Location> {
        let document = self.document(uri)?;
        let offset = position_to_offset(&document.text, position)?;
        let symbol = document.semantic.symbol_at(offset)?;
        document
            .workspace_symbols
            .definitions
            .get(&symbol.binding_id)
            .cloned()
    }

    #[must_use]
    pub fn references(&self, uri: &str, position: Position) -> Vec<Location> {
        let Some(document) = self.document(uri) else {
            return Vec::new();
        };
        let Some(offset) = position_to_offset(&document.text, position) else {
            return Vec::new();
        };
        let Some(symbol) = document.semantic.symbol_at(offset) else {
            return Vec::new();
        };
        document
            .workspace_symbols
            .references
            .get(&symbol.binding_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Returns the exact source spelling which can be renamed at `position`.
    /// Qualified references deliberately expose only their member component.
    #[must_use]
    pub fn prepare_rename(&self, uri: &str, position: Position) -> Option<Range> {
        let document = self.document(uri)?;
        let offset = position_to_offset(&document.text, position)?;
        let (binding_id, occurrence) = rename_target(&document.workspace_symbols, uri, offset)?;
        (document
            .workspace_symbols
            .definitions
            .contains_key(binding_id)
            && rename_kind_supported(&document.workspace_symbols, binding_id)
            && rename_group_has_declaration(
                &document.workspace_symbols,
                binding_id,
                &occurrence.spelling,
            ))
        .then(|| span_to_range(&document.text, occurrence.span))
    }

    /// Builds a deterministic, source-only workspace edit for one spelling of
    /// a stable binding. Aliases are independent spelling groups even though
    /// they resolve to the same `BindingId`.
    pub fn rename(
        &self,
        uri: &str,
        position: Position,
        new_name: &str,
    ) -> Result<Option<WorkspaceEdit>, LspStateError> {
        let Some(document) = self.document(uri) else {
            return Err(document_not_found(uri));
        };
        let Some(offset) = position_to_offset(&document.text, position) else {
            return Err(LspStateError::new(
                INVALID_PARAMS,
                "rename position is outside the document",
            ));
        };
        let Some((binding_id, selected)) = rename_target(&document.workspace_symbols, uri, offset)
        else {
            return Ok(None);
        };
        if !document
            .workspace_symbols
            .definitions
            .contains_key(binding_id)
            || !rename_kind_supported(&document.workspace_symbols, binding_id)
        {
            return Ok(None);
        }

        let new_name = normalize_rename_name(new_name)?;
        if is_reserved_rename_name(&new_name)
            || document
                .macro_interfaces
                .values()
                .any(|macro_| macro_.canonical == new_name)
            || document_declares_phase_name(document, &new_name)
        {
            return Err(LspStateError::new(
                INVALID_PARAMS,
                format!("newName `{new_name}` is reserved by Osiris syntax or a macro"),
            ));
        }
        let selected_spelling = selected.spelling.nfc().collect::<String>();
        reject_rename_collision(
            &document.workspace_symbols,
            binding_id,
            &selected_spelling,
            &new_name,
        )?;

        let mut spans = BTreeSet::<(String, usize, usize)>::new();
        let mut grouped = BTreeMap::<String, Vec<(Span, TextEdit)>>::new();
        for occurrence in document
            .workspace_symbols
            .rename_occurrences
            .get(binding_id)
            .into_iter()
            .flatten()
        {
            if occurrence.spelling.nfc().collect::<String>() != selected_spelling
                || !document
                    .workspace_symbols
                    .source_uris
                    .contains(&occurrence.uri)
            {
                continue;
            }
            let Some(source) = document.workspace_symbols.sources.get(&occurrence.uri) else {
                continue;
            };
            if occurrence.span.end > source.len()
                || !source.is_char_boundary(occurrence.span.start)
                || !source.is_char_boundary(occurrence.span.end)
            {
                continue;
            }
            if !spans.insert((
                occurrence.uri.clone(),
                occurrence.span.start,
                occurrence.span.end,
            )) {
                continue;
            }
            grouped.entry(occurrence.uri.clone()).or_default().push((
                occurrence.span,
                TextEdit {
                    range: span_to_range(source, occurrence.span),
                    new_text: new_name.clone(),
                },
            ));
        }

        let mut changes = BTreeMap::new();
        for (edit_uri, mut edits) in grouped {
            edits.sort_by_key(|(span, _)| (span.start, span.end));
            if edits.windows(2).any(|pair| pair[0].0.end > pair[1].0.start) {
                return Err(LspStateError::new(
                    INTERNAL_ERROR,
                    "rename produced overlapping source edits",
                ));
            }
            changes.insert(edit_uri, edits.into_iter().map(|(_, edit)| edit).collect());
        }
        Ok((!changes.is_empty()).then_some(WorkspaceEdit { changes }))
    }

    #[must_use]
    pub fn expand_preview(&self, uri: &str) -> Option<ExpandPreview> {
        let document = self.document(uri)?;
        Some(ExpandPreview {
            uri: uri.to_owned(),
            version: document.version,
            text: render_document_text(&document.analysis.expanded_document),
            macro_traces: document.semantic.macro_traces.clone(),
            diagnostics: self.diagnostics(uri)?.diagnostics,
        })
    }

    #[must_use]
    pub fn inspect(&self, uri: &str, query: Option<&str>) -> Option<JsonValue> {
        let document = self.document(uri)?;
        let Some(query) = query.filter(|query| !query.is_empty()) else {
            return serde_json::to_value(&document.semantic).ok();
        };
        document
            .semantic
            .symbols
            .iter()
            .find(|symbol| {
                symbol.binding_id == query
                    || symbol.canonical == query
                    || symbol.source_spelling == query
                    || symbol
                        .aliases
                        .iter()
                        .any(|alias| alias.spelling == query || alias.canonical == query)
            })
            .and_then(|symbol| serde_json::to_value(symbol).ok())
    }
}

#[derive(Clone, Debug)]
struct CallableSignature {
    canonical: String,
    parameters: Vec<CallableSignatureParameter>,
    return_type: Type,
}

#[derive(Clone, Debug)]
struct CallableSignatureParameter {
    canonical: String,
    source_spelling: String,
    python: String,
    aliases: Vec<String>,
    ty: Type,
    has_default: bool,
    default_text: Option<String>,
    variadic: bool,
}

#[derive(Clone, Copy)]
struct SourceArgument<'form> {
    span: Span,
    keyword: Option<&'form str>,
}

struct MacroSignature<'form> {
    canonical: &'form str,
    parameters: &'form Form,
    variadic: bool,
}

fn macro_signature_help(
    document: &OpenDocument,
    trace: &crate::macro_expand::ExpansionTrace,
    offset: usize,
    locale: &str,
) -> Option<SignatureHelp> {
    let signature = macro_signature(document, &trace.macro_binding_id)?;
    let call_form = find_call_form(&document.analysis.document.forms, trace.call_span)?;
    let FormKind::List(items) = &call_form.kind else {
        return None;
    };
    let invoked_name = items
        .first()
        .and_then(form_name)
        .unwrap_or(signature.canonical);
    let parameters = phase_parameter_labels(
        signature.parameters,
        is_chinese_locale(locale),
        signature.variadic,
    )?;
    let arguments = source_macro_arguments(items);
    let active_argument = active_source_argument(items, &arguments, offset);
    let active_parameter =
        (!parameters.is_empty()).then(|| active_argument.min(parameters.len() - 1) as u32);
    let label = format!("{}({})", invoked_name, parameters.join(", "));
    Some(SignatureHelp {
        signatures: vec![SignatureInformation {
            label,
            parameters: parameters
                .into_iter()
                .map(|label| ParameterInformation { label })
                .collect(),
            active_parameter,
        }],
        active_signature: 0,
        active_parameter,
    })
}

fn macro_signature<'document>(
    document: &'document OpenDocument,
    binding_id: &str,
) -> Option<MacroSignature<'document>> {
    for item in &document.analysis.surface.items {
        let crate::ast::ItemKind::Defmacro(macro_) = &item.kind else {
            continue;
        };
        let local_id = crate::name::BindingId::new(
            &document.analysis.hir.name,
            &macro_.name.canonical,
            BindingKind::Macro,
        );
        if local_id.as_str() == binding_id {
            return Some(MacroSignature {
                canonical: &macro_.name.spelling,
                parameters: phase_parameter_form(&macro_.phase_form)?,
                variadic: macro_
                    .params
                    .last()
                    .is_some_and(|parameter| parameter.variadic),
            });
        }
    }
    document
        .macro_interfaces
        .get(binding_id)
        .map(|macro_| MacroSignature {
            canonical: &macro_.canonical,
            parameters: &macro_.parameters,
            variadic: macro_.variadic,
        })
}

fn phase_parameter_form(declaration: &Form) -> Option<&Form> {
    let FormKind::List(items) = &declaration.kind else {
        return None;
    };
    let mut index = 2;
    if matches!(
        items.get(index).map(|item| &item.kind),
        Some(FormKind::String(_))
    ) {
        index += 1;
    }
    items
        .get(index)
        .filter(|parameters| matches!(parameters.kind, FormKind::Vector(_)))
}

fn phase_parameter_labels(
    parameters: &Form,
    chinese: bool,
    declared_variadic: bool,
) -> Option<Vec<String>> {
    let FormKind::Vector(items) = &parameters.kind else {
        return None;
    };
    let mut labels = Vec::new();
    let mut variadic = false;
    for item in items {
        if form_name(item).is_some_and(|name| name == "&") {
            variadic = true;
            continue;
        }
        let localized = chinese
            .then(|| {
                metadata_aliases(&item.metadata, "")
                    .into_iter()
                    .find(|alias| contains_cjk(alias))
            })
            .flatten();
        let label = localized.unwrap_or_else(|| display_form(item));
        labels.push(if variadic {
            format!("& {label}")
        } else {
            label
        });
        variadic = false;
    }
    debug_assert_eq!(
        labels.last().is_some_and(|label| label.starts_with("& ")),
        declared_variadic
    );
    Some(labels)
}

fn display_form(form: &Form) -> String {
    match &form.kind {
        FormKind::None => "none".to_owned(),
        FormKind::Bool(value) => value.to_string(),
        FormKind::Integer(value) | FormKind::Float(value) => value.clone(),
        FormKind::String(value) => {
            serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_owned())
        }
        FormKind::Keyword(name) | FormKind::Symbol(name) => name.spelling.clone(),
        FormKind::List(items) => format!("({})", display_forms(items)),
        FormKind::Vector(items) => format!("[{}]", display_forms(items)),
        FormKind::Map(items) => format!("{{{}}}", display_forms(items)),
        FormKind::Set(items) => format!("#{{{}}}", display_forms(items)),
        FormKind::ReaderMacro { macro_kind, form } => format!(
            "{}{}",
            match macro_kind {
                crate::syntax::ReaderMacroKind::Quote => "'",
                crate::syntax::ReaderMacroKind::SyntaxQuote => "`",
                crate::syntax::ReaderMacroKind::Unquote => "~",
                crate::syntax::ReaderMacroKind::UnquoteSplicing => "~@",
            },
            display_form(form)
        ),
        FormKind::Error(message) => format!("#<error:{message}>"),
    }
}

fn display_forms(forms: &[Form]) -> String {
    forms.iter().map(display_form).collect::<Vec<_>>().join(" ")
}

fn callable_signature(document: &OpenDocument, binding_id: &str) -> Option<CallableSignature> {
    for item in &document.analysis.hir.items {
        let hir::ItemKind::Function(function) = &item.kind else {
            continue;
        };
        if function.binding.as_str() == binding_id {
            return Some(local_callable_signature(
                document,
                binding_id,
                &function.parameters,
                &function.return_type,
            ));
        }
    }
    for function in &document.analysis.hir.extern_functions {
        if function.binding.as_str() == binding_id {
            return Some(local_callable_signature(
                document,
                binding_id,
                &function.parameters,
                &function.return_type,
            ));
        }
    }
    document
        .function_interfaces
        .get(binding_id)
        .map(interface_callable_signature)
}

fn local_callable_signature(
    document: &OpenDocument,
    binding_id: &str,
    parameters: &[hir::Parameter],
    return_type: &Type,
) -> CallableSignature {
    let interface = document.function_interfaces.get(binding_id);
    let parameters = parameters
        .iter()
        .enumerate()
        .map(|(index, parameter)| {
            let binding = document
                .analysis
                .hir
                .bindings
                .iter()
                .find(|binding| binding.name.id == parameter.binding);
            let published = interface.and_then(|interface| interface.parameters.get(index));
            let canonical = binding.map_or_else(
                || {
                    published.map_or_else(
                        || format!("arg{}", index + 1),
                        |parameter| parameter.canonical.clone(),
                    )
                },
                |binding| binding.name.canonical.clone(),
            );
            let mut aliases = binding
                .map(|binding| metadata_aliases(&binding.metadata, &canonical))
                .unwrap_or_default();
            if let Some(published) = published {
                aliases.extend(published.aliases.iter().cloned());
            }
            aliases.sort();
            aliases.dedup();
            CallableSignatureParameter {
                source_spelling: binding.map_or_else(
                    || canonical.clone(),
                    |binding| binding.source_spelling.clone(),
                ),
                python: binding.map_or_else(
                    || crate::name::python_identifier(&canonical),
                    |binding| binding.name.python.clone(),
                ),
                canonical,
                aliases,
                ty: parameter.ty.clone(),
                has_default: parameter.default.is_some(),
                default_text: parameter.default.as_ref().and_then(|default| {
                    source_slice(&document.text, default.span).map(normalize_inline_source)
                }),
                variadic: parameter.variadic,
            }
        })
        .collect();
    CallableSignature {
        canonical: binding_id
            .rsplit("::")
            .next()
            .unwrap_or(binding_id)
            .to_owned(),
        parameters,
        return_type: return_type.clone(),
    }
}

fn interface_callable_signature(function: &interface::FunctionInterface) -> CallableSignature {
    CallableSignature {
        canonical: function
            .binding
            .rsplit("::")
            .next()
            .unwrap_or(&function.binding)
            .to_owned(),
        parameters: function
            .parameters
            .iter()
            .map(|parameter| CallableSignatureParameter {
                canonical: parameter.canonical.clone(),
                source_spelling: parameter.canonical.clone(),
                python: crate::name::python_identifier(&parameter.canonical),
                aliases: parameter.aliases.clone(),
                ty: parameter.ty.clone(),
                has_default: parameter.has_default,
                default_text: None,
                variadic: parameter.variadic,
            })
            .collect(),
        return_type: function.return_type.clone(),
    }
}

fn signature_parameter_label(parameter: &CallableSignatureParameter, chinese: bool) -> String {
    let name = if chinese {
        parameter
            .aliases
            .iter()
            .find(|alias| contains_cjk(alias))
            .unwrap_or(&parameter.source_spelling)
    } else {
        &parameter.source_spelling
    };
    let variadic = if parameter.variadic { "& " } else { "" };
    let default = if let Some(value) = &parameter.default_text {
        format!(" = {value}")
    } else if parameter.has_default {
        " = ...".to_owned()
    } else {
        String::new()
    };
    format!("{variadic}{name}: {}{default}", parameter.ty)
}

fn source_slice(source: &str, span: Span) -> Option<&str> {
    (span.start <= span.end
        && span.end <= source.len()
        && source.is_char_boundary(span.start)
        && source.is_char_boundary(span.end))
    .then(|| &source[span.start..span.end])
    .filter(|value| !value.trim().is_empty())
}

fn normalize_inline_source(source: &str) -> String {
    source.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn metadata_aliases(metadata: &[crate::syntax::MetadataEntry], canonical: &str) -> Vec<String> {
    let mut aliases = Vec::new();
    for entry in metadata {
        if form_name(&entry.key).is_some_and(|name| name.trim_start_matches(':') == "osiris/names")
        {
            collect_metadata_names(&entry.value, &mut aliases);
        }
    }
    aliases.retain(|alias| alias != canonical);
    aliases.sort();
    aliases.dedup();
    aliases
}

fn collect_metadata_names(form: &Form, names: &mut Vec<String>) {
    let FormKind::Map(entries) = &form.kind else {
        return;
    };
    for pair in entries.chunks_exact(2) {
        match form_name(&pair[0])
            .unwrap_or_default()
            .trim_start_matches(':')
        {
            "preferred" => {
                if let Some(name) = form_name(&pair[1]) {
                    names.push(name.to_owned());
                }
            }
            "aliases" => {
                if let FormKind::Vector(values) = &pair[1].kind {
                    names.extend(values.iter().filter_map(form_name).map(str::to_owned));
                }
            }
            _ => collect_metadata_names(&pair[1], names),
        }
    }
}

fn find_call_form(forms: &[Form], span: Span) -> Option<&Form> {
    forms
        .iter()
        .filter_map(|form| find_call_form_in(form, span))
        .min_by_key(|form| form.span.end.saturating_sub(form.span.start))
}

fn find_call_form_in(form: &Form, span: Span) -> Option<&Form> {
    if form.span.start > span.start || form.span.end < span.end {
        return None;
    }
    let children = match &form.kind {
        FormKind::List(items)
        | FormKind::Vector(items)
        | FormKind::Map(items)
        | FormKind::Set(items) => items.as_slice(),
        FormKind::ReaderMacro { form, .. } => std::slice::from_ref(form.as_ref()),
        FormKind::None
        | FormKind::Bool(_)
        | FormKind::Integer(_)
        | FormKind::Float(_)
        | FormKind::String(_)
        | FormKind::Keyword(_)
        | FormKind::Symbol(_)
        | FormKind::Error(_) => &[],
    };
    children
        .iter()
        .filter_map(|child| find_call_form_in(child, span))
        .min_by_key(|child| child.span.end.saturating_sub(child.span.start))
        .or_else(|| matches!(form.kind, FormKind::List(_)).then_some(form))
}

fn form_name(form: &Form) -> Option<&str> {
    match &form.kind {
        FormKind::Keyword(name) | FormKind::Symbol(name) => Some(&name.spelling),
        _ => None,
    }
}

fn source_arguments(items: &[Form]) -> Vec<SourceArgument<'_>> {
    let mut arguments = Vec::new();
    let mut index = 1;
    while index < items.len() {
        if let FormKind::Keyword(keyword) = &items[index].kind {
            let end = items
                .get(index + 1)
                .map_or(items[index].span.end, |value| value.span.end);
            arguments.push(SourceArgument {
                span: Span::new(items[index].span.start, end),
                keyword: Some(keyword.canonical.trim_start_matches(':')),
            });
            index += usize::from(index + 1 < items.len()) + 1;
        } else {
            arguments.push(SourceArgument {
                span: items[index].span,
                keyword: None,
            });
            index += 1;
        }
    }
    arguments
}

fn source_macro_arguments(items: &[Form]) -> Vec<SourceArgument<'_>> {
    items
        .iter()
        .skip(1)
        .map(|argument| SourceArgument {
            span: argument.span,
            keyword: None,
        })
        .collect()
}

fn active_source_argument(
    items: &[Form],
    arguments: &[SourceArgument<'_>],
    offset: usize,
) -> usize {
    let Some(callee) = items.first() else {
        return 0;
    };
    if offset <= callee.span.end {
        return 0;
    }
    let mut previous_end = callee.span.end;
    for (index, argument) in arguments.iter().enumerate() {
        if previous_end <= offset && offset <= argument.span.end {
            return index;
        }
        previous_end = argument.span.end;
    }
    arguments.len()
}

fn active_parameter(
    parameters: &[CallableSignatureParameter],
    arguments: &[SourceArgument<'_>],
    active_argument: usize,
) -> Option<usize> {
    if parameters.is_empty() {
        return None;
    }
    if let Some(argument) = arguments.get(active_argument) {
        if let Some(keyword) = argument.keyword {
            return parameters.iter().position(|parameter| {
                parameter.canonical == keyword
                    || parameter.python == keyword
                    || parameter.aliases.iter().any(|alias| alias == keyword)
            });
        }
        let positional = arguments[..=active_argument]
            .iter()
            .filter(|argument| argument.keyword.is_none())
            .count()
            .saturating_sub(1);
        return Some(positional.min(parameters.len() - 1));
    }

    let positional = arguments
        .iter()
        .filter(|argument| argument.keyword.is_none())
        .count();
    let keyword_parameters = arguments
        .iter()
        .filter_map(|argument| argument.keyword)
        .collect::<Vec<_>>();
    parameters
        .iter()
        .enumerate()
        .skip(positional)
        .find(|(_, parameter)| {
            !keyword_parameters.iter().any(|keyword| {
                parameter.canonical == *keyword
                    || parameter.python == *keyword
                    || parameter.aliases.iter().any(|alias| alias == keyword)
            })
        })
        .map(|(index, _)| index)
        .or(Some(parameters.len() - 1))
}

const fn span_contains(span: Span, offset: usize) -> bool {
    span.start <= offset && offset <= span.end
}

fn occurrence_at(symbol: &SemanticSymbol, offset: usize) -> Option<Span> {
    symbol
        .occurrences
        .iter()
        .copied()
        .filter(|span| span.start <= offset && offset <= span.end)
        .min_by_key(|span| span.end.saturating_sub(span.start))
}

fn rename_target<'index>(
    index: &'index WorkspaceSymbolIndex,
    uri: &str,
    offset: usize,
) -> Option<(&'index str, &'index RenameOccurrence)> {
    index
        .rename_occurrences
        .iter()
        .flat_map(|(binding_id, occurrences)| {
            occurrences
                .iter()
                .map(move |occurrence| (binding_id.as_str(), occurrence))
        })
        .filter(|(_, occurrence)| {
            occurrence.uri == uri
                && occurrence.span.start <= offset
                && offset <= occurrence.span.end
        })
        .min_by_key(|(_, occurrence)| occurrence.span.end.saturating_sub(occurrence.span.start))
}

fn normalize_rename_name(new_name: &str) -> Result<String, LspStateError> {
    let normalized = new_name.nfc().collect::<String>();
    let parsed = reader::read(&normalized);
    let valid = parsed.diagnostics.is_empty()
        && parsed.forms.len() == 1
        && parsed.forms[0].metadata.is_empty()
        && parsed.forms[0].span == Span::new(0, normalized.len())
        && parsed.forms[0].datum_span == parsed.forms[0].span
        && matches!(parsed.forms[0].kind, FormKind::Symbol(_))
        && !normalized.contains(['/', '.']);
    if !valid {
        return Err(LspStateError::new(
            INVALID_PARAMS,
            "newName must be one non-qualified Osiris symbol",
        ));
    }
    Ok(normalized)
}

fn is_reserved_rename_name(name: &str) -> bool {
    // Exhaustive parser heads from `ast::Lowerer::{lower_item,lower_list_expr,
    // lower_try_expr}`, plus the bootstrap macros declared in
    // `macro_expand::BOOTSTRAP_PRELUDE`. Keep this list aligned when either
    // grammar surface changes; unlike ordinary prelude functions these names
    // capture list-head syntax before runtime binding resolution.
    matches!(
        name,
        "module"
            | "import"
            | "import-for-syntax"
            | "py/import"
            | "export"
            | "alias"
            | "def"
            | "defn"
            | "defstruct"
            | "defstatic-schema"
            | "static-record"
            | "extern"
            | "defmacro"
            | "defn-for-syntax"
            | "fn"
            | "let"
            | "if"
            | "do"
            | "try"
            | "raise"
            | "catch"
            | "finally"
            | "->"
            | "->>"
            | "cond->"
            | "cond->>"
            | "as->"
            | "doto"
            | "defn-"
            | "and"
            | "or"
            | "when"
            | "if-not"
            | "when-not"
            | "cond"
            | "if-let"
            | "when-let"
            | "if-some"
            | "when-some"
            | "nil?"
            | "some?"
            | "some->"
            | "some->>"
            | "condp"
            | "case"
            | "for"
            | "doseq"
            | "when-first"
            | "loop"
            | "recur"
            | "dotimes"
            | "while"
            | "letfn"
            | "trampoline"
            | "lazy-seq"
            | "lazy-cat"
            | "delay"
            | "force"
            | "realized?"
            | "deref"
            | "future"
            | "future-call"
            | "future-done?"
            | "future-cancelled?"
            | "future-cancel"
            | "pmap"
            | "pvalues"
            | "pcalls"
            | "promise"
            | "deliver"
            | "lock"
            | "locking"
            | "time"
            | "binding"
            | "with-open"
            | "throw"
            | "assert"
            | "comment"
    )
}

fn reject_rename_collision(
    index: &WorkspaceSymbolIndex,
    binding_id: &str,
    selected_spelling: &str,
    new_name: &str,
) -> Result<(), LspStateError> {
    if selected_spelling == new_name {
        return Ok(());
    }
    let declaration_uris = index
        .rename_occurrences
        .get(binding_id)
        .into_iter()
        .flatten()
        .filter(|occurrence| {
            occurrence.declaration
                && occurrence.spelling.nfc().collect::<String>() == selected_spelling
        })
        .map(|occurrence| occurrence.uri.as_str())
        .collect::<BTreeSet<_>>();
    if declaration_uris.is_empty() {
        return Err(LspStateError::new(
            INVALID_PARAMS,
            "selected spelling has no editable source declaration",
        ));
    }
    let collision = index
        .rename_occurrences
        .iter()
        .any(|(candidate_id, occurrences)| {
            occurrences.iter().any(|occurrence| {
                occurrence.declaration
                    && declaration_uris.contains(occurrence.uri.as_str())
                    && occurrence.spelling.nfc().collect::<String>() == new_name
                    && (candidate_id != binding_id
                        || occurrence.spelling.nfc().collect::<String>() != selected_spelling)
            })
        });
    if collision {
        return Err(LspStateError::new(
            INVALID_PARAMS,
            format!("newName `{new_name}` collides with an existing declaration"),
        ));
    }
    Ok(())
}

fn completion_item(symbol: &SemanticSymbol, chinese: bool) -> CompletionItem {
    let actual_alias = chinese
        .then(|| {
            symbol
                .aliases
                .iter()
                .filter(|alias| contains_cjk(&alias.spelling))
                .min_by_key(|alias| (!alias.public, !alias.preferred, &alias.spelling))
        })
        .flatten();
    let insert_text = actual_alias.map_or_else(
        || symbol.source_spelling.clone(),
        |alias| alias.spelling.clone(),
    );
    let label = if chinese {
        actual_alias.map_or_else(
            || symbol.labels.zh_cn.clone(),
            |alias| alias.spelling.clone(),
        )
    } else {
        symbol.labels.en.clone()
    };
    let localized = chinese && (contains_cjk(&label) || contains_cjk(&insert_text));
    CompletionItem {
        label,
        kind: completion_kind(symbol.kind),
        detail: format!("{} : {}", symbol.canonical, symbol.ty),
        insert_text: insert_text.clone(),
        sort_text: format!("{}:{}", if localized { 0 } else { 1 }, symbol.canonical),
        filter_text: format!(
            "{} {} {}",
            symbol.canonical,
            symbol.source_spelling,
            symbol
                .aliases
                .iter()
                .map(|alias| alias.spelling.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        ),
        data: json!({
            "bindingId": symbol.binding_id,
            "canonical": symbol.canonical,
            "insertedAlias": actual_alias.map(|alias| alias.spelling.as_str()),
        }),
    }
}

const fn completion_kind(kind: BindingKind) -> u8 {
    match kind {
        BindingKind::Function | BindingKind::Macro => 3,
        BindingKind::Type => 7,
        BindingKind::Module | BindingKind::PythonModule => 9,
        BindingKind::Field => 5,
        BindingKind::Parameter | BindingKind::Value => 6,
    }
}

fn symbol_matches_prefix(symbol: &SemanticSymbol, prefix: &str) -> bool {
    prefix.is_empty()
        || symbol.canonical.starts_with(prefix)
        || symbol.source_spelling.starts_with(prefix)
        || symbol
            .aliases
            .iter()
            .any(|alias| alias.spelling.starts_with(prefix))
}

fn completion_prefix(source: &str, offset: usize) -> String {
    let offset = offset.min(source.len());
    source[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, character)| {
            !character.is_whitespace()
                && !matches!(character, '(' | ')' | '[' | ']' | '{' | '}' | '"' | ',')
        })
        .last()
        .map_or_else(String::new, |(start, _)| source[start..offset].to_owned())
}

fn build_single_symbol_index(analysis: &Analysis, uri: &str, source: &str) -> WorkspaceSymbolIndex {
    let mut index = WorkspaceSymbolIndex::default();
    index_analysis_symbols(&mut index, analysis, uri, source);
    finish_symbol_index(&mut index);
    index
}

fn build_project_symbol_index(
    analyses: &[Analysis],
    buffers: &[WorkspaceBuffer],
) -> WorkspaceSymbolIndex {
    let mut index = WorkspaceSymbolIndex::default();
    for (analysis, buffer) in analyses.iter().zip(buffers) {
        index_analysis_symbols(&mut index, analysis, &buffer.uri, &buffer.source);
    }
    finish_symbol_index(&mut index);
    index
}

fn index_analysis_symbols(
    index: &mut WorkspaceSymbolIndex,
    analysis: &Analysis,
    uri: &str,
    source: &str,
) {
    index.source_uris.insert(uri.to_owned());
    index.sources.insert(uri.to_owned(), source.to_owned());
    let semantic = SemanticDocument::from_analysis(analysis, uri);
    let local_prefix = format!("{}::", analysis.hir.name);
    for symbol in &semantic.symbols {
        index
            .binding_kinds
            .entry(symbol.binding_id.clone())
            .or_insert(symbol.kind);
        if symbol.binding_id.starts_with(&local_prefix)
            && !index.ambiguous_definitions.contains(&symbol.binding_id)
        {
            let definition = Location {
                uri: uri.to_owned(),
                range: span_to_range(source, symbol.definition),
            };
            match index.definitions.get(&symbol.binding_id) {
                Some(existing) if existing != &definition => {
                    index.definitions.remove(&symbol.binding_id);
                    index
                        .ambiguous_definitions
                        .insert(symbol.binding_id.clone());
                }
                Some(_) => {}
                None => {
                    index
                        .definitions
                        .insert(symbol.binding_id.clone(), definition);
                }
            }
        }
        index
            .references
            .entry(symbol.binding_id.clone())
            .or_default()
            .extend(symbol.occurrences.iter().copied().map(|span| Location {
                uri: uri.to_owned(),
                range: span_to_range(source, span),
            }));
        index_symbol_rename_occurrences(index, analysis, symbol, uri, source);
        if symbol.public && symbol.binding_id.starts_with(&local_prefix) {
            record_provider_name(
                index,
                &analysis.hir.name,
                &symbol.canonical,
                &symbol.binding_id,
            );
            record_provider_name(
                index,
                &analysis.hir.name,
                &symbol.source_spelling,
                &symbol.binding_id,
            );
            for alias in symbol.aliases.iter().filter(|alias| alias.public) {
                record_provider_name(
                    index,
                    &analysis.hir.name,
                    &alias.spelling,
                    &symbol.binding_id,
                );
            }
        }
    }
    index_declaration_references(index, analysis, &semantic, uri, source);
}

fn index_symbol_rename_occurrences(
    index: &mut WorkspaceSymbolIndex,
    analysis: &Analysis,
    symbol: &SemanticSymbol,
    uri: &str,
    source: &str,
) {
    let local_prefix = format!("{}::", analysis.hir.name);
    if symbol.binding_id.starts_with(&local_prefix)
        && let Some(form) = definition_name_form(
            &analysis.document.forms,
            symbol.definition,
            &symbol.source_spelling,
        )
        && let Some((span, spelling)) = rename_member_from_form(source, form)
    {
        push_rename_occurrence(
            index,
            &symbol.binding_id,
            RenameOccurrence {
                uri: uri.to_owned(),
                span,
                spelling,
                declaration: true,
            },
        );
    }
    for reference in &symbol.references {
        let Some(form) = exact_symbol_form(&analysis.document.forms, *reference) else {
            continue;
        };
        let Some((span, spelling)) = rename_member_from_form(source, form) else {
            continue;
        };
        push_rename_occurrence(
            index,
            &symbol.binding_id,
            RenameOccurrence {
                uri: uri.to_owned(),
                span,
                spelling,
                declaration: false,
            },
        );
    }
    for alias in &symbol.aliases {
        let Some(form) = exact_container_form(&analysis.document.forms, alias.span) else {
            continue;
        };
        let Some(local) =
            list_item(form, 1).filter(|form| symbol_form_matches(form, &alias.spelling))
        else {
            continue;
        };
        let Some((span, spelling)) = rename_member_from_form(source, local) else {
            continue;
        };
        push_rename_occurrence(
            index,
            &symbol.binding_id,
            RenameOccurrence {
                uri: uri.to_owned(),
                span,
                spelling,
                declaration: true,
            },
        );
    }
}

fn index_declaration_references(
    index: &mut WorkspaceSymbolIndex,
    analysis: &Analysis,
    semantic: &SemanticDocument,
    uri: &str,
    source: &str,
) {
    for item in &analysis.surface.items {
        let Some(form) = exact_container_form(&analysis.document.forms, item.span) else {
            continue;
        };
        match &item.kind {
            crate::ast::ItemKind::Alias(alias) => {
                let Some(resolved) = analysis.hir.aliases.iter().find(|resolved| {
                    resolved.span == alias.span
                        && resolved.spelling.nfc().eq(alias.local.spelling.nfc())
                }) else {
                    continue;
                };
                let Some(target) = list_item(form, 2) else {
                    continue;
                };
                if let Some((span, spelling)) = rename_member_from_form(source, target) {
                    push_rename_occurrence(
                        index,
                        resolved.target.as_str(),
                        RenameOccurrence {
                            uri: uri.to_owned(),
                            span,
                            spelling,
                            declaration: false,
                        },
                    );
                }
            }
            crate::ast::ItemKind::Export(export) => {
                let Some(names) = list_item(form, 1).and_then(collection_items) else {
                    continue;
                };
                for (name, name_form) in export.names.iter().zip(names) {
                    let mut bindings = semantic
                        .symbols
                        .iter()
                        .filter(|symbol| {
                            symbol.public && semantic_symbol_accepts(symbol, &name.spelling)
                        })
                        .map(|symbol| symbol.binding_id.as_str());
                    let Some(binding_id) = bindings.next() else {
                        continue;
                    };
                    if bindings.any(|candidate| candidate != binding_id) {
                        continue;
                    }
                    if let Some((span, spelling)) = rename_member_from_form(source, name_form) {
                        push_rename_occurrence(
                            index,
                            binding_id,
                            RenameOccurrence {
                                uri: uri.to_owned(),
                                span,
                                spelling,
                                declaration: false,
                            },
                        );
                    }
                }
            }
            crate::ast::ItemKind::Import(import) => {
                let members = import_member_forms(form);
                for (_name, name_form) in import.members.iter().zip(members) {
                    let Some((span, spelling)) = rename_member_from_form(source, name_form) else {
                        continue;
                    };
                    index.pending_import_members.push(PendingImportMember {
                        uri: uri.to_owned(),
                        provider: import.module.canonical.clone(),
                        spelling,
                        span,
                    });
                }
            }
            _ => {}
        }
    }
}

fn record_provider_name(
    index: &mut WorkspaceSymbolIndex,
    module: &str,
    spelling: &str,
    binding_id: &str,
) {
    let key = (module.to_owned(), spelling.nfc().collect::<String>());
    if index.ambiguous_provider_names.contains(&key) {
        return;
    }
    match index.provider_names.get(&key) {
        Some(existing) if existing != binding_id => {
            index.provider_names.remove(&key);
            index.ambiguous_provider_names.insert(key);
        }
        Some(_) => {}
        None => {
            index.provider_names.insert(key, binding_id.to_owned());
        }
    }
}

fn push_rename_occurrence(
    index: &mut WorkspaceSymbolIndex,
    binding_id: &str,
    occurrence: RenameOccurrence,
) {
    index
        .rename_occurrences
        .entry(binding_id.to_owned())
        .or_default()
        .push(occurrence);
}

fn semantic_symbol_accepts(symbol: &SemanticSymbol, spelling: &str) -> bool {
    let spelling = spelling.nfc().collect::<String>();
    symbol.canonical == spelling
        || symbol.source_spelling.nfc().eq(spelling.chars())
        || symbol
            .aliases
            .iter()
            .any(|alias| alias.spelling.nfc().eq(spelling.chars()) || alias.canonical == spelling)
}

fn exact_container_form(forms: &[Form], span: Span) -> Option<&Form> {
    for form in forms {
        if form.span == span || form.datum_span == span {
            return Some(form);
        }
        if form.span.start <= span.start
            && span.end <= form.span.end
            && let Some(found) = exact_container_form(form_children(form), span)
        {
            return Some(found);
        }
    }
    None
}

fn exact_symbol_form(forms: &[Form], span: Span) -> Option<&Form> {
    let form = exact_container_form(forms, span)?;
    matches!(form.kind, FormKind::Symbol(_)).then_some(form)
}

fn definition_name_form<'form>(
    forms: &'form [Form],
    span: Span,
    expected: &str,
) -> Option<&'form Form> {
    let container = exact_container_form(forms, span)?;
    if symbol_form_matches(container, expected) {
        return Some(container);
    }
    if let FormKind::List(items) = &container.kind {
        let declaration = items.first().and_then(form_name).is_some_and(|head| {
            matches!(
                head,
                "def" | "defn" | "defstruct" | "defstatic-schema" | "defmacro" | "defn-for-syntax"
            )
        });
        if declaration
            && let Some(name) = items
                .get(1)
                .filter(|form| symbol_form_matches(form, expected))
        {
            return Some(name);
        }
    }
    form_children(container)
        .iter()
        .find(|form| symbol_form_matches(form, expected))
}

fn symbol_form_matches(form: &Form, expected: &str) -> bool {
    let FormKind::Symbol(name) = &form.kind else {
        return false;
    };
    name.spelling.nfc().eq(expected.nfc())
}

fn rename_member_from_form(source: &str, form: &Form) -> Option<(Span, String)> {
    if !matches!(form.kind, FormKind::Symbol(_))
        || form.datum_span.end > source.len()
        || !source.is_char_boundary(form.datum_span.start)
        || !source.is_char_boundary(form.datum_span.end)
    {
        return None;
    }
    let raw = &source[form.datum_span.start..form.datum_span.end];
    let relative = raw
        .char_indices()
        .filter(|(_, character)| matches!(character, '/' | '.'))
        .map(|(offset, character)| offset + character.len_utf8())
        .next_back()
        .unwrap_or(0);
    let spelling = raw.get(relative..)?.to_owned();
    if spelling.is_empty() {
        return None;
    }
    Some((
        Span::new(form.datum_span.start + relative, form.datum_span.end),
        spelling,
    ))
}

fn list_item(form: &Form, index: usize) -> Option<&Form> {
    let FormKind::List(items) = &form.kind else {
        return None;
    };
    items.get(index)
}

fn collection_items(form: &Form) -> Option<&[Form]> {
    match &form.kind {
        FormKind::List(items) | FormKind::Vector(items) => Some(items),
        _ => None,
    }
}

fn import_member_forms(form: &Form) -> Vec<&Form> {
    let FormKind::List(items) = &form.kind else {
        return Vec::new();
    };
    let mut members = Vec::new();
    let mut index = 2;
    while index + 1 < items.len() {
        let is_members = matches!(
            &items[index].kind,
            FormKind::Keyword(name) if matches!(name.canonical.as_str(), ":refer" | ":only")
        );
        if is_members && let Some(values) = collection_items(&items[index + 1]) {
            members.extend(values);
        }
        index += 2;
    }
    members
}

fn form_children(form: &Form) -> &[Form] {
    match &form.kind {
        FormKind::List(items)
        | FormKind::Vector(items)
        | FormKind::Map(items)
        | FormKind::Set(items) => items,
        FormKind::ReaderMacro { form, .. } => std::slice::from_ref(form),
        _ => &[],
    }
}

fn finish_symbol_index(index: &mut WorkspaceSymbolIndex) {
    let pending = std::mem::take(&mut index.pending_import_members);
    for member in pending {
        let key = (
            member.provider.clone(),
            member.spelling.nfc().collect::<String>(),
        );
        if index.ambiguous_provider_names.contains(&key) {
            continue;
        }
        let Some(binding_id) = index.provider_names.get(&key).cloned() else {
            continue;
        };
        push_rename_occurrence(
            index,
            &binding_id,
            RenameOccurrence {
                uri: member.uri,
                span: member.span,
                spelling: member.spelling,
                declaration: false,
            },
        );
    }
    for references in index.references.values_mut() {
        references.sort_by(|left, right| {
            (
                &left.uri,
                left.range.start.line,
                left.range.start.character,
                left.range.end.line,
                left.range.end.character,
            )
                .cmp(&(
                    &right.uri,
                    right.range.start.line,
                    right.range.start.character,
                    right.range.end.line,
                    right.range.end.character,
                ))
        });
        references.dedup();
    }
    for occurrences in index.rename_occurrences.values_mut() {
        occurrences.sort_by(|left, right| {
            (
                &left.uri,
                left.span.start,
                left.span.end,
                &left.spelling,
                !left.declaration,
            )
                .cmp(&(
                    &right.uri,
                    right.span.start,
                    right.span.end,
                    &right.spelling,
                    !right.declaration,
                ))
        });
        occurrences.dedup_by(|left, right| {
            left.uri == right.uri
                && left.span == right.span
                && left.spelling == right.spelling
                && left.declaration == right.declaration
        });
    }
}

fn collect_function_interfaces(
    analyses: &[Analysis],
    external_interfaces: &BTreeMap<String, Interface>,
) -> BTreeMap<String, interface::FunctionInterface> {
    let mut functions = external_interfaces
        .values()
        .flat_map(|interface| interface.functions.iter())
        .map(|function| (function.binding.clone(), function.clone()))
        .collect::<BTreeMap<_, _>>();
    for analysis in analyses {
        let Ok(interface) = interface::build_provisional(&analysis.surface) else {
            continue;
        };
        functions.extend(
            interface
                .functions
                .into_iter()
                .map(|function| (function.binding.clone(), function)),
        );
    }
    functions
}

fn collect_macro_interfaces(
    analyses: &[Analysis],
    external_interfaces: &BTreeMap<String, Interface>,
) -> BTreeMap<String, interface::MacroInterface> {
    let mut macros = external_interfaces
        .values()
        .flat_map(|interface| interface.macros.iter())
        .map(|macro_| (macro_.id.clone(), macro_.clone()))
        .collect::<BTreeMap<_, _>>();
    for analysis in analyses {
        let Ok(interface) = interface::build_provisional(&analysis.surface) else {
            continue;
        };
        macros.extend(
            interface
                .macros
                .into_iter()
                .map(|macro_| (macro_.id.clone(), macro_)),
        );
    }
    macros
}

struct WorkspaceBuffer {
    uri: String,
    source: String,
    options: CompileOptions,
}

fn project_options(project: &ProjectConfig, path: &Path, module_name: String) -> CompileOptions {
    CompileOptions::new(&module_name, project.target_python)
        .with_source_name(path.display().to_string())
        .with_expected_module_name(module_name)
        .with_provider(
            project.distribution.clone(),
            project.distribution_version.clone(),
        )
}

fn collect_workspace_sources(directory: &Path, paths: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_workspace_sources(&entry.path(), paths)?;
        } else if file_type.is_file()
            && entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                == Some("osr")
        {
            paths.push(entry.path());
        }
    }
    Ok(())
}

fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let encoded = uri.strip_prefix("file://")?;
    let encoded = if let Some(path) = encoded.strip_prefix("localhost/") {
        format!("/{path}")
    } else if encoded.starts_with('/') {
        encoded.to_owned()
    } else {
        return None;
    };
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex_digit(*bytes.get(index + 1)?)?;
            let low = hex_digit(*bytes.get(index + 2)?)?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok().map(PathBuf::from)
}

fn load_project_interfaces(
    project: &ProjectConfig,
    site_roots: &[PathBuf],
) -> Option<BTreeMap<String, Interface>> {
    if project.extensions.is_empty() {
        return Some(BTreeMap::new());
    }
    if site_roots.is_empty() {
        return None;
    }
    let lock = project.load_lock().ok()?;
    let graph = dependency::resolve_effective_extensions(project, &lock, site_roots).ok()?;
    let mut interfaces = BTreeMap::<String, Interface>::new();
    for distribution in graph.extensions {
        for extension in distribution.extensions {
            let source = fs::read_to_string(&extension.interface).ok()?;
            let parsed = interface::read(&source).ok()?;
            if parsed.module != extension.module
                || parsed.semantic_interface_hash() != extension.semantic_interface_hash
            {
                return None;
            }
            if let Some(existing) = interfaces.get(&parsed.module) {
                if existing.semantic_interface_hash() != parsed.semantic_interface_hash() {
                    return None;
                }
            } else {
                interfaces.insert(parsed.module.clone(), parsed);
            }
        }
    }
    Some(interfaces)
}

const fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn fallback_module_name(uri: &str) -> String {
    let path = uri.rsplit('/').next().unwrap_or(uri);
    Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("main")
        .to_owned()
}

fn normalize_locale(locale: String) -> String {
    if is_chinese_locale(&locale) {
        "zh-CN".to_owned()
    } else if locale.is_empty() {
        "en".to_owned()
    } else {
        locale
    }
}

fn is_chinese_locale(locale: &str) -> bool {
    locale.eq_ignore_ascii_case("zh")
        || locale.eq_ignore_ascii_case("zh-cn")
        || locale.to_ascii_lowercase().starts_with("zh-")
}

fn contains_cjk(value: &str) -> bool {
    value.chars().any(|character| {
        matches!(
            character as u32,
            0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff
        )
    })
}

fn rename_group_has_declaration(
    index: &WorkspaceSymbolIndex,
    binding_id: &str,
    spelling: &str,
) -> bool {
    let spelling = spelling.nfc().collect::<String>();
    index
        .rename_occurrences
        .get(binding_id)
        .into_iter()
        .flatten()
        .any(|occurrence| {
            occurrence.declaration && occurrence.spelling.nfc().collect::<String>() == spelling
        })
}

fn rename_kind_supported(index: &WorkspaceSymbolIndex, binding_id: &str) -> bool {
    // The current semantic projection records every runtime value occurrence,
    // but it does not yet retain nominal type, field, module, or phase-1 macro
    // references. Refuse those categories instead of emitting a partial edit.
    matches!(
        index.binding_kinds.get(binding_id),
        Some(BindingKind::Function | BindingKind::Value | BindingKind::Parameter)
    )
}

fn document_declares_phase_name(document: &OpenDocument, name: &str) -> bool {
    document.analysis.document.forms.iter().any(|form| {
        let FormKind::List(items) = &form.kind else {
            return false;
        };
        let Some(head) = items.first().and_then(form_name) else {
            return false;
        };
        matches!(head, "defmacro" | "defn-for-syntax")
            && items
                .get(1)
                .and_then(form_name)
                .is_some_and(|declared| declared.nfc().eq(name.nfc()))
    })
}

fn node_id_for_span(document: &OpenDocument, span: Span) -> Option<u64> {
    document
        .analysis
        .document
        .nodes
        .iter()
        .filter(|node| node.span.start <= span.start && span.end <= node.span.end)
        .min_by(|left, right| {
            let left_width = left.span.end.saturating_sub(left.span.start);
            let right_width = right.span.end.saturating_sub(right.span.start);
            left_width
                .cmp(&right_width)
                .then_with(|| right.path.segments().len().cmp(&left.path.segments().len()))
                .then_with(|| left.span.start.cmp(&right.span.start))
                .then_with(|| left.id.cmp(&right.id))
        })
        .map(|node| node.id.get())
}

fn escape_markdown(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('*', "\\*")
        .replace('_', "\\_")
}

fn apply_content_change(
    source: &mut String,
    change: &TextDocumentContentChangeEvent,
) -> Result<(), LspStateError> {
    let Some(range) = change.range else {
        source.clone_from(&change.text);
        return Ok(());
    };
    let Some(start) = position_to_offset(source, range.start) else {
        return Err(LspStateError::new(
            INVALID_PARAMS,
            "change start is outside the document",
        ));
    };
    let Some(end) = position_to_offset(source, range.end) else {
        return Err(LspStateError::new(
            INVALID_PARAMS,
            "change end is outside the document",
        ));
    };
    if start > end || !source.is_char_boundary(start) || !source.is_char_boundary(end) {
        return Err(LspStateError::new(
            INVALID_PARAMS,
            "change range is not a valid UTF-8 boundary",
        ));
    }
    source.replace_range(start..end, &change.text);
    Ok(())
}

/// Converts an LSP UTF-16 position to a UTF-8 byte offset.
#[must_use]
pub fn position_to_offset(source: &str, position: Position) -> Option<usize> {
    let mut line = 0_u32;
    let mut line_start = 0_usize;
    for (offset, byte) in source.bytes().enumerate() {
        if line == position.line {
            break;
        }
        if byte == b'\n' {
            line += 1;
            line_start = offset + 1;
        }
    }
    if line != position.line {
        return None;
    }
    let line_end = source[line_start..]
        .find('\n')
        .map_or(source.len(), |relative| line_start + relative);
    let line_text = source[line_start..line_end]
        .strip_suffix('\r')
        .unwrap_or(&source[line_start..line_end]);
    let mut utf16 = 0_u32;
    for (relative, character) in line_text.char_indices() {
        if utf16 == position.character {
            return Some(line_start + relative);
        }
        let width = character.len_utf16() as u32;
        if utf16 + width > position.character {
            return None;
        }
        utf16 += width;
    }
    (utf16 == position.character).then_some(line_start + line_text.len())
}

/// Converts a UTF-8 byte offset to an LSP UTF-16 position.
#[must_use]
pub fn offset_to_position(source: &str, offset: usize) -> Position {
    let offset = offset.min(source.len());
    let offset = if source.is_char_boundary(offset) {
        offset
    } else {
        (0..offset)
            .rev()
            .find(|candidate| source.is_char_boundary(*candidate))
            .unwrap_or(0)
    };
    let prefix = &source[..offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() as u32;
    let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
    let character = source[line_start..offset].encode_utf16().count() as u32;
    Position { line, character }
}

#[must_use]
pub fn span_to_range(source: &str, span: Span) -> Range {
    Range {
        start: offset_to_position(source, span.start),
        end: offset_to_position(source, span.end),
    }
}

#[derive(Clone, Debug, Default)]
pub struct JsonRpcOutcome {
    pub response: Option<JsonValue>,
    pub notifications: Vec<JsonValue>,
}

impl JsonRpcOutcome {
    #[must_use]
    pub fn messages(&self) -> Vec<String> {
        self.notifications
            .iter()
            .chain(self.response.iter())
            .filter_map(|message| serde_json::to_string(message).ok())
            .collect()
    }

    #[must_use]
    pub fn response_text(&self) -> Option<String> {
        self.response
            .as_ref()
            .and_then(|response| serde_json::to_string(response).ok())
    }
}

/// Thin state-machine wrapper useful to a future stdio transport.
#[derive(Clone, Debug, Default)]
pub struct JsonRpcMachine {
    pub state: LspState,
}

pub type LspServer = JsonRpcMachine;

impl JsonRpcMachine {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn handle(&mut self, input: &str) -> JsonRpcOutcome {
        handle_json_rpc(&mut self.state, input)
    }

    pub fn handle_json(&mut self, input: &str) -> Vec<String> {
        self.handle(input).messages()
    }
}

impl LspState {
    pub fn handle_json_rpc(&mut self, input: &str) -> JsonRpcOutcome {
        handle_json_rpc(self, input)
    }
}

/// Parses and dispatches one JSON-RPC message without performing any IO.
pub fn handle_json_rpc(state: &mut LspState, input: &str) -> JsonRpcOutcome {
    let request = match serde_json::from_str::<JsonValue>(input) {
        Ok(request) => request,
        Err(error) => {
            return JsonRpcOutcome {
                response: Some(rpc_error(
                    JsonValue::Null,
                    PARSE_ERROR,
                    "parse error",
                    Some(json!({ "detail": error.to_string() })),
                )),
                notifications: Vec::new(),
            };
        }
    };
    let Some(object) = request.as_object() else {
        return JsonRpcOutcome {
            response: Some(rpc_error(
                JsonValue::Null,
                INVALID_REQUEST,
                "request must be a JSON object",
                None,
            )),
            notifications: Vec::new(),
        };
    };
    if object.get("jsonrpc").and_then(JsonValue::as_str) != Some(JSON_RPC_VERSION) {
        return JsonRpcOutcome {
            response: Some(rpc_error(
                object.get("id").cloned().unwrap_or(JsonValue::Null),
                INVALID_REQUEST,
                "jsonrpc must be 2.0",
                None,
            )),
            notifications: Vec::new(),
        };
    }
    let Some(method) = object.get("method").and_then(JsonValue::as_str) else {
        return JsonRpcOutcome {
            response: Some(rpc_error(
                object.get("id").cloned().unwrap_or(JsonValue::Null),
                INVALID_REQUEST,
                "request method must be a string",
                None,
            )),
            notifications: Vec::new(),
        };
    };
    let id = object.get("id").cloned();
    let params = object.get("params").cloned().unwrap_or(JsonValue::Null);
    match dispatch(state, method, &params) {
        Ok(dispatch) => JsonRpcOutcome {
            response: id.map(|id| rpc_success(id, dispatch.result.unwrap_or(JsonValue::Null))),
            notifications: dispatch.notifications,
        },
        Err(error) => JsonRpcOutcome {
            response: id.map(|id| {
                rpc_error(
                    id,
                    error.code,
                    &error.message,
                    Some(json!({ "method": method })),
                )
            }),
            notifications: Vec::new(),
        },
    }
}

pub fn handle_request(state: &mut LspState, request: &JsonRpcRequest) -> JsonRpcOutcome {
    match serde_json::to_string(request) {
        Ok(input) => handle_json_rpc(state, &input),
        Err(error) => JsonRpcOutcome {
            response: Some(rpc_error(
                request.id.clone().unwrap_or(JsonValue::Null),
                INTERNAL_ERROR,
                "could not encode request",
                Some(json!({ "detail": error.to_string() })),
            )),
            notifications: Vec::new(),
        },
    }
}

#[derive(Default)]
struct DispatchOutcome {
    result: Option<JsonValue>,
    notifications: Vec<JsonValue>,
}

fn dispatch(
    state: &mut LspState,
    method: &str,
    params: &JsonValue,
) -> Result<DispatchOutcome, LspStateError> {
    match method {
        "initialize" => {
            if let Some(locale) = find_string(params, &["initializationOptions", "displayLocale"])
                .or_else(|| find_string(params, &["locale"]))
            {
                state.set_display_locale(locale);
            }
            if let Some(site_roots) = find_value(params, &["initializationOptions", "siteRoots"]) {
                let roots = site_roots.as_array().ok_or_else(|| {
                    LspStateError::new(
                        INVALID_PARAMS,
                        "initializationOptions.siteRoots must be an array of paths",
                    )
                })?;
                let roots = roots
                    .iter()
                    .map(|root| {
                        root.as_str().map(PathBuf::from).ok_or_else(|| {
                            LspStateError::new(
                                INVALID_PARAMS,
                                "initializationOptions.siteRoots must contain only paths",
                            )
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                state.set_site_roots(roots);
            }
            Ok(DispatchOutcome {
                result: Some(initialize_result()),
                notifications: Vec::new(),
            })
        }
        "initialized" | "$/cancelRequest" | "exit" => Ok(DispatchOutcome::default()),
        "shutdown" => {
            state.request_shutdown();
            Ok(DispatchOutcome {
                result: Some(JsonValue::Null),
                notifications: Vec::new(),
            })
        }
        "textDocument/didOpen" => {
            let params: DidOpenParams = decode_params(params)?;
            let diagnostics = state.did_open(
                params.text_document.uri,
                params.text_document.version,
                params.text_document.text,
            );
            Ok(DispatchOutcome {
                result: None,
                notifications: vec![publish_diagnostics_notification(diagnostics)],
            })
        }
        "textDocument/didChange" => {
            let params: DidChangeParams = decode_params(params)?;
            let diagnostics = state.did_change(
                &params.text_document.uri,
                params.text_document.version,
                &params.content_changes,
            )?;
            Ok(DispatchOutcome {
                result: None,
                notifications: vec![publish_diagnostics_notification(diagnostics)],
            })
        }
        "textDocument/didClose" => {
            let params: DidCloseParams = decode_params(params)?;
            state.did_close(&params.text_document.uri);
            Ok(DispatchOutcome {
                result: None,
                notifications: vec![json!({
                    "jsonrpc": JSON_RPC_VERSION,
                    "method": "textDocument/publishDiagnostics",
                    "params": {
                        "uri": params.text_document.uri,
                        "diagnostics": [],
                    },
                })],
            })
        }
        "textDocument/hover" => {
            let params: PositionParams = decode_params(params)?;
            ensure_document(state, &params.text_document.uri)?;
            let result = match state.hover(
                &params.text_document.uri,
                params.position,
                params.locale.as_deref(),
            ) {
                Some(hover) => serialize_value(hover)?,
                None => JsonValue::Null,
            };
            Ok(result_outcome(result))
        }
        "textDocument/completion" => {
            let params: PositionParams = decode_params(params)?;
            ensure_document(state, &params.text_document.uri)?;
            let result = serialize_value(state.completion(
                &params.text_document.uri,
                params.position,
                params.locale.as_deref(),
            ))?;
            Ok(result_outcome(result))
        }
        "textDocument/signatureHelp" => {
            let params: PositionParams = decode_params(params)?;
            ensure_document(state, &params.text_document.uri)?;
            let result = state
                .signature_help(
                    &params.text_document.uri,
                    params.position,
                    params.locale.as_deref(),
                )
                .map_or(Ok(JsonValue::Null), serialize_value)?;
            Ok(result_outcome(result))
        }
        "textDocument/definition" => {
            let params: PositionParams = decode_params(params)?;
            ensure_document(state, &params.text_document.uri)?;
            let result = match state.definition(&params.text_document.uri, params.position) {
                Some(location) => serialize_value(location)?,
                None => JsonValue::Null,
            };
            Ok(result_outcome(result))
        }
        "textDocument/references" => {
            let params: PositionParams = decode_params(params)?;
            ensure_document(state, &params.text_document.uri)?;
            let result =
                serialize_value(state.references(&params.text_document.uri, params.position))?;
            Ok(result_outcome(result))
        }
        "textDocument/prepareRename" => {
            let params: PositionParams = decode_params(params)?;
            ensure_document(state, &params.text_document.uri)?;
            let result = state
                .prepare_rename(&params.text_document.uri, params.position)
                .map_or(Ok(JsonValue::Null), serialize_value)?;
            Ok(result_outcome(result))
        }
        "textDocument/rename" => {
            let params: RenameParams = decode_params(params)?;
            ensure_document(state, &params.text_document.uri)?;
            let result = state
                .rename(&params.text_document.uri, params.position, &params.new_name)?
                .map_or(Ok(JsonValue::Null), serialize_value)?;
            Ok(result_outcome(result))
        }
        "osiris/diagnostics" => {
            let uri = required_uri(params)?;
            let diagnostics = state
                .diagnostics(uri)
                .ok_or_else(|| document_not_found(uri))?;
            Ok(result_outcome(serialize_value(diagnostics)?))
        }
        "textDocument/diagnostic" => {
            let uri = required_uri(params)?;
            let diagnostics = state
                .diagnostics(uri)
                .ok_or_else(|| document_not_found(uri))?;
            Ok(result_outcome(json!({
                "kind": "full",
                "items": diagnostics.diagnostics,
            })))
        }
        "osiris/expand" | "osiris/expandPreview" | "textDocument/expandPreview" => {
            let uri = required_uri(params)?;
            let preview = state
                .expand_preview(uri)
                .ok_or_else(|| document_not_found(uri))?;
            Ok(result_outcome(serialize_value(preview)?))
        }
        "osiris/semanticView" => {
            let uri = required_uri(params)?;
            if let Some(locale) = find_string(params, &["locale"]) {
                state.set_display_locale(locale);
            }
            let semantic = state
                .semantic_document(uri)
                .ok_or_else(|| document_not_found(uri))?;
            Ok(result_outcome(serialize_value(semantic)?))
        }
        "osiris/syntaxSnapshot" => {
            let uri = required_uri(params)?;
            let document = state.document(uri).ok_or_else(|| document_not_found(uri))?;
            Ok(result_outcome(json!({
                "version": document.analysis.document.format_version,
                "documentVersion": document.version,
                "sourceLen": document.analysis.document.source_len,
                "nodes": &document.analysis.document.nodes,
            })))
        }
        "osiris/inspect" => {
            let uri = required_uri(params)?;
            ensure_document(state, uri)?;
            let query = find_string(params, &["bindingId"])
                .or_else(|| find_string(params, &["symbol"]))
                .or_else(|| find_string(params, &["query"]));
            let inspected = state
                .inspect(uri, query.as_deref())
                .unwrap_or(JsonValue::Null);
            Ok(result_outcome(inspected))
        }
        _ => Err(LspStateError::new(
            METHOD_NOT_FOUND,
            format!("method not found: {method}"),
        )),
    }
}

fn result_outcome(result: JsonValue) -> DispatchOutcome {
    DispatchOutcome {
        result: Some(result),
        notifications: Vec::new(),
    }
}

fn ensure_document(state: &LspState, uri: &str) -> Result<(), LspStateError> {
    state
        .document(uri)
        .map(|_| ())
        .ok_or_else(|| document_not_found(uri))
}

fn document_not_found(uri: &str) -> LspStateError {
    LspStateError::new(DOCUMENT_NOT_FOUND, format!("document {uri} is not open"))
}

fn serialize_value<T: Serialize>(value: T) -> Result<JsonValue, LspStateError> {
    serde_json::to_value(value).map_err(|error| {
        LspStateError::new(
            INTERNAL_ERROR,
            format!("failed to serialize response: {error}"),
        )
    })
}

fn decode_params<T: for<'de> Deserialize<'de>>(params: &JsonValue) -> Result<T, LspStateError> {
    serde_json::from_value(params.clone())
        .map_err(|error| LspStateError::new(INVALID_PARAMS, format!("invalid params: {error}")))
}

fn required_uri(params: &JsonValue) -> Result<&str, LspStateError> {
    find_string_ref(params, &["uri"])
        .or_else(|| find_string_ref(params, &["textDocument", "uri"]))
        .ok_or_else(|| LspStateError::new(INVALID_PARAMS, "params require a document uri"))
}

fn find_string(value: &JsonValue, path: &[&str]) -> Option<String> {
    find_string_ref(value, path).map(str::to_owned)
}

fn find_string_ref<'a>(value: &'a JsonValue, path: &[&str]) -> Option<&'a str> {
    find_value(value, path)?.as_str()
}

fn find_value<'a>(value: &'a JsonValue, path: &[&str]) -> Option<&'a JsonValue> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    Some(current)
}

fn publish_diagnostics_notification(params: PublishDiagnosticsParams) -> JsonValue {
    json!({
        "jsonrpc": JSON_RPC_VERSION,
        "method": "textDocument/publishDiagnostics",
        "params": params,
    })
}

fn rpc_success(id: JsonValue, result: JsonValue) -> JsonValue {
    json!({
        "jsonrpc": JSON_RPC_VERSION,
        "id": id,
        "result": result,
    })
}

fn rpc_error(id: JsonValue, code: i64, message: &str, data: Option<JsonValue>) -> JsonValue {
    let mut error = json!({
        "code": code,
        "message": message,
    });
    if let Some(data) = data {
        error["data"] = data;
    }
    json!({
        "jsonrpc": JSON_RPC_VERSION,
        "id": id,
        "error": error,
    })
}

fn initialize_result() -> JsonValue {
    json!({
        "capabilities": {
            "positionEncoding": "utf-16",
            "textDocumentSync": {
                "openClose": true,
                "change": 2,
            },
            "hoverProvider": true,
            "completionProvider": {
                "triggerCharacters": ["/", ":", "."],
            },
            "signatureHelpProvider": {
                "triggerCharacters": [" ", ":"],
                "retriggerCharacters": [" ", ":"],
            },
            "definitionProvider": true,
            "referencesProvider": true,
            "renameProvider": {
                "prepareProvider": true,
            },
            "diagnosticProvider": {
                "interFileDependencies": true,
                "workspaceDiagnostics": false,
            },
            "experimental": {
                "osirisSemanticView": true,
                "osirisSyntaxSnapshot": true,
                "osirisInspect": true,
                "osirisExpandPreview": true,
            },
        },
        "serverInfo": {
            "name": LSP_SERVER_NAME,
            "version": crate::version(),
        },
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TextDocumentItem {
    uri: String,
    version: i64,
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DidOpenParams {
    text_document: TextDocumentItem,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VersionedTextDocumentIdentifier {
    uri: String,
    version: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DidChangeParams {
    text_document: VersionedTextDocumentIdentifier,
    content_changes: Vec<TextDocumentContentChangeEvent>,
}

#[derive(Debug, Deserialize)]
struct TextDocumentIdentifier {
    uri: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DidCloseParams {
    text_document: TextDocumentIdentifier,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PositionParams {
    text_document: TextDocumentIdentifier,
    position: Position,
    #[serde(default)]
    locale: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenameParams {
    text_document: TextDocumentIdentifier,
    position: Position,
    new_name: String,
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fs,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use serde_json::{Value as JsonValue, json};

    use crate::{
        compiler::{self, CompileInput, CompileOptions},
        interface,
        name::{BindingKind, CONFUSABLE_IDENTIFIER, MIXED_SCRIPT_IDENTIFIER},
        project::PythonVersion,
        syntax::NodePath,
        types::Type,
    };

    use super::{
        JsonRpcMachine, LspState, OpenDocument, PARSE_ERROR, Position, ProjectDocumentAnalysis,
        Range, TextDocumentContentChangeEvent, build_single_symbol_index,
        collect_function_interfaces, collect_macro_interfaces, handle_json_rpc, offset_to_position,
        position_to_offset,
    };

    const URI: &str = "file:///workspace/demo.osr";
    static NEXT_WORKSPACE: AtomicUsize = AtomicUsize::new(0);

    fn source() -> &'static str {
        r#"(module demo)
(defn add-one [[x Int]] -> Int (+ x 1))
(alias 加一 add-one)
(export [add-one 加一])
(def result (加一 2))
"#
    }

    #[test]
    fn state_reuses_one_analysis_for_all_queries() {
        let mut state = LspState::new();
        let diagnostics = state.did_open(URI, 1, source());
        assert!(diagnostics.diagnostics.is_empty());
        assert_eq!(state.analysis_runs(), 1);

        let position = Position {
            line: 4,
            character: 14,
        };
        let hover = state
            .hover(URI, position, Some("zh-CN"))
            .expect("hover on alias");
        assert!(hover.contents.value.contains("add-one"));
        let definition = state
            .definition(URI, position)
            .expect("definition from alias");
        assert_eq!(definition.range.start.line, 1);
        assert!(state.references(URI, position).len() >= 2);

        let completion = state.completion(
            URI,
            Position {
                line: 5,
                character: 0,
            },
            Some("zh-CN"),
        );
        let localized = completion
            .iter()
            .find(|item| item.label == "加一")
            .expect("Chinese alias completion");
        assert_eq!(localized.insert_text, "加一");
        assert_eq!(state.analysis_runs(), 1);
    }

    #[test]
    fn rename_keeps_canonical_and_alias_spelling_groups_separate() {
        let mut state = LspState::new();
        let diagnostics = state.did_open(URI, 1, source());
        assert!(diagnostics.diagnostics.is_empty(), "{diagnostics:?}");

        let canonical_position = offset_to_position(
            source(),
            source().find("add-one [[x").expect("canonical declaration"),
        );
        let prepared = state
            .prepare_rename(URI, canonical_position)
            .expect("canonical prepare range");
        assert_eq!(
            &source()[position_to_offset(source(), prepared.start).expect("start")
                ..position_to_offset(source(), prepared.end).expect("end")],
            "add-one"
        );
        let canonical = state
            .rename(URI, canonical_position, "increment")
            .expect("valid rename")
            .expect("workspace edit");
        let canonical_edits = canonical.changes.get(URI).expect("same document edits");
        assert_eq!(canonical_edits.len(), 3);
        assert!(canonical_edits.iter().all(|edit| {
            let start = position_to_offset(source(), edit.range.start).expect("edit start");
            let end = position_to_offset(source(), edit.range.end).expect("edit end");
            &source()[start..end] == "add-one" && edit.new_text == "increment"
        }));

        let alias_position = offset_to_position(
            source(),
            source().find("加一 2").expect("Chinese alias call"),
        );
        let alias = state
            .rename(URI, alias_position, "增加")
            .expect("valid alias rename")
            .expect("alias workspace edit");
        let alias_edits = alias.changes.get(URI).expect("same document edits");
        assert_eq!(alias_edits.len(), 3);
        assert!(alias_edits.iter().all(|edit| {
            let start = position_to_offset(source(), edit.range.start).expect("edit start");
            let end = position_to_offset(source(), edit.range.end).expect("edit end");
            &source()[start..end] == "加一" && edit.new_text == "增加"
        }));

        for invalid in [
            "if",
            "defn",
            "cond",
            "when-first",
            "loop",
            "recur",
            "time",
            "lazy-seq",
            "future-call",
            "binding",
            "with-open",
            "two names",
            "alpha/score",
            ":keyword",
        ] {
            let error = state
                .rename(URI, canonical_position, invalid)
                .expect_err("invalid or reserved rename target");
            assert_eq!(error.code, super::INVALID_PARAMS, "{invalid}");
        }
    }

    #[test]
    fn json_rpc_advertises_and_dispatches_prepare_and_rename() {
        let mut machine = JsonRpcMachine::new();
        let initialized = machine.handle(
            &json!({
                "jsonrpc": "2.0",
                "id": "initialize",
                "method": "initialize",
                "params": {}
            })
            .to_string(),
        );
        assert_eq!(
            initialized.response.as_ref().expect("initialize response")["result"]["capabilities"]["renameProvider"]
                ["prepareProvider"],
            true
        );
        machine.state.did_open(URI, 1, source());
        let position = offset_to_position(source(), source().find("加一 2").expect("alias call"));

        let prepared = machine.handle(
            &json!({
                "jsonrpc": "2.0",
                "id": "prepare",
                "method": "textDocument/prepareRename",
                "params": {
                    "textDocument": { "uri": URI },
                    "position": position,
                }
            })
            .to_string(),
        );
        assert!(prepared.response.as_ref().expect("prepare response")["result"].is_object());

        let renamed = machine.handle(
            &json!({
                "jsonrpc": "2.0",
                "id": "rename",
                "method": "textDocument/rename",
                "params": {
                    "textDocument": { "uri": URI },
                    "position": position,
                    "newName": "增加",
                }
            })
            .to_string(),
        );
        let response = renamed.response.as_ref().expect("rename response");
        assert_eq!(
            response["result"]["changes"][URI].as_array().map(Vec::len),
            Some(3)
        );
    }

    #[test]
    fn rename_validates_collisions_normalizes_nfc_and_emits_utf16_ranges() {
        let source = r#"(module demo)
(defn first [[x Int]] -> Int x)
(alias 第一 first)
(export [first 第一])
(defn second [[x Int]] -> Int x)
(def value (do "😀" (第一 1)))
"#;
        let mut state = LspState::new();
        let diagnostics = state.did_open(URI, 1, source);
        assert!(diagnostics.diagnostics.is_empty(), "{diagnostics:?}");
        let first = offset_to_position(source, source.find("first [[x").expect("first"));
        let alias_offset = source.rfind("第一 1").expect("alias call");
        let alias = offset_to_position(source, alias_offset);

        for (position, collision) in [(first, "second"), (first, "第一"), (alias, "first")] {
            let error = state
                .rename(URI, position, collision)
                .expect_err("obvious declaration collision");
            assert_eq!(error.code, super::INVALID_PARAMS);
        }

        let prepared = state
            .prepare_rename(URI, alias)
            .expect("UTF-16 prepare range");
        let line_start = source[..alias_offset]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        assert_eq!(
            prepared.start.character,
            source[line_start..alias_offset].encode_utf16().count() as u32
        );
        assert_eq!(prepared.end.character, prepared.start.character + 2);

        let edit = state
            .rename(URI, alias, "e\u{301}")
            .expect("NFC rename")
            .expect("NFC edits");
        let edits = edit.changes.get(URI).expect("document edits");
        assert_eq!(edits.len(), 3);
        assert!(edits.iter().all(|edit| edit.new_text == "é"));
        assert!(edits.windows(2).all(|pair| {
            (pair[0].range.start.line, pair[0].range.start.character)
                < (pair[1].range.start.line, pair[1].range.start.character)
        }));
    }

    #[test]
    fn signature_help_capability_and_local_typed_call_use_localized_parameters() {
        let source = r#"(module demo)
(defn rolling
  [[values Float]
   ^{:osiris/names {"zh-CN" {:preferred 周期 :aliases [窗口]}}}
   [window Int = 14]]
  -> Float
  values)
(def result (rolling 1.0 ))
"#;
        let mut machine = JsonRpcMachine::new();
        let initialized = machine.handle(
            &json!({
                "jsonrpc": "2.0",
                "id": "initialize",
                "method": "initialize",
                "params": {}
            })
            .to_string(),
        );
        assert_eq!(
            initialized.response.as_ref().expect("initialize response")["result"]["capabilities"]["signatureHelpProvider"]
                ["triggerCharacters"],
            json!([" ", ":"])
        );
        let diagnostics = machine.state.did_open(URI, 1, source);
        assert!(diagnostics.diagnostics.is_empty(), "{diagnostics:?}");
        let cursor = source.find("(rolling 1.0 )").expect("call") + "(rolling 1.0 ".len();
        let position = offset_to_position(source, cursor);

        let response = machine.handle(
            &json!({
                "jsonrpc": "2.0",
                "id": "signature",
                "method": "textDocument/signatureHelp",
                "params": {
                    "textDocument": { "uri": URI },
                    "position": position,
                    "locale": "zh-CN"
                }
            })
            .to_string(),
        );

        let result = &response.response.as_ref().expect("signature response")["result"];
        assert_eq!(result["activeSignature"], 0);
        assert_eq!(result["activeParameter"], 1);
        assert_eq!(
            result["signatures"][0]["label"],
            "rolling(values: Float, 周期: Int = 14) -> Float"
        );
        assert_eq!(
            result["signatures"][0]["parameters"][1]["label"],
            "周期: Int = 14"
        );
    }

    #[test]
    fn project_document_uses_path_identity_and_dependency_interfaces() {
        let sequence = NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "osiris-lsp-workspace-{}-{sequence}",
            std::process::id()
        ));
        let source_root = root.join("src/demo");
        fs::create_dir_all(&source_root).expect("source root");
        fs::write(
            root.join("pyproject.toml"),
            "[project]\nname = \"lsp-workspace\"\nversion = \"1.0\"\n\n[tool.osiris]\nsource = [\"src\"]\n",
        )
        .expect("project configuration");
        fs::write(
            source_root.join("math.osr"),
            "(module demo.math)\n(export [add-one])\n(defn add-one [[x Int]] -> Int (+ x 1))\n",
        )
        .expect("dependency source");
        let app_source =
            "(module demo.app)\n(import demo.math :as math)\n(def answer (math/add-one 41))\n";
        let app = source_root.join("app.osr");
        fs::write(&app, app_source).expect("application source");
        let uri = format!("file://{}", app.display());
        let mut state = LspState::new();

        let diagnostics = state.did_open(&uri, 1, app_source);

        assert!(diagnostics.diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(
            state
                .document(&uri)
                .expect("open document")
                .analysis
                .hir
                .name,
            "demo.app"
        );
        drop(state);
        fs::remove_dir_all(root).expect("workspace cleanup");
    }

    #[test]
    fn workspace_navigation_uses_provider_locations_and_stable_binding_identity() {
        let sequence = NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "osiris-lsp-navigation-workspace-{}-{sequence}",
            std::process::id()
        ));
        let source_root = root.join("src/demo");
        fs::create_dir_all(&source_root).expect("source root");
        fs::write(
            root.join("pyproject.toml"),
            "[project]\nname = \"lsp-navigation\"\nversion = \"1.0\"\n\n[tool.osiris]\nsource = [\"src\"]\n",
        )
        .expect("project configuration");
        let alpha_source = r#"(module demo.alpha)
(export [score 得分])
(defn score [[value Int]] -> Int value)
(alias 得分 score)
"#;
        let beta_source = r#"(module demo.beta)
(export [score])
(defn score [[value Int]] -> Int value)
"#;
        let app_source = r#"(module demo.app)
(import demo.alpha :as alpha :refer [得分])
(import demo.beta :as beta)
(def alpha-result (alpha/score 1))
(def alias-result (得分 2))
(def beta-result (beta/score 3))
"#;
        let broken_source = r#"(module demo.broken)
(import demo.alpha :as alpha)
(def broken-result (alpha/score 4))
(defn invalid [[x Int]] -> Int)
"#;
        let alpha_path = source_root.join("alpha.osr");
        let beta_path = source_root.join("beta.osr");
        let app_path = source_root.join("app.osr");
        let broken_path = source_root.join("broken.osr");
        fs::write(&alpha_path, alpha_source).expect("alpha source");
        fs::write(&beta_path, beta_source).expect("beta source");
        fs::write(&app_path, app_source).expect("app source");
        fs::write(&broken_path, broken_source).expect("broken source");
        let alpha_uri = format!("file://{}", alpha_path.display());
        let beta_uri = format!("file://{}", beta_path.display());
        let app_uri = format!("file://{}", app_path.display());
        let broken_uri = format!("file://{}", broken_path.display());
        let mut state = LspState::new();

        let app_diagnostics = state.did_open(&app_uri, 1, app_source);
        assert!(
            app_diagnostics.diagnostics.is_empty(),
            "{app_diagnostics:?}"
        );
        let alpha_call = offset_to_position(
            app_source,
            app_source.find("alpha/score 1").expect("alpha call"),
        );
        let alias_call = offset_to_position(
            app_source,
            app_source.find("得分 2").expect("referred alias call"),
        );
        let beta_call = offset_to_position(
            app_source,
            app_source.find("beta/score 3").expect("beta call"),
        );

        let alpha_definition = state
            .definition(&app_uri, alpha_call)
            .expect("qualified alpha definition");
        let alias_definition = state
            .definition(&app_uri, alias_call)
            .expect("Chinese alias definition");
        let beta_definition = state
            .definition(&app_uri, beta_call)
            .expect("qualified beta definition");
        assert_eq!(alpha_definition.uri, alpha_uri);
        assert_eq!(alias_definition, alpha_definition);
        assert_eq!(beta_definition.uri, beta_uri);
        assert_ne!(beta_definition, alpha_definition);

        let alpha_references = state.references(&app_uri, alpha_call);
        assert!(
            alpha_references
                .iter()
                .any(|location| location.uri == alpha_uri)
        );
        assert!(
            alpha_references
                .iter()
                .any(|location| location.uri == app_uri)
        );
        assert!(
            alpha_references
                .iter()
                .any(|location| location.uri == broken_uri)
        );
        assert!(
            alpha_references
                .iter()
                .all(|location| location.uri != beta_uri)
        );

        let alpha_diagnostics = state.did_open(&alpha_uri, 1, alpha_source);
        assert!(
            alpha_diagnostics.diagnostics.is_empty(),
            "{alpha_diagnostics:?}"
        );
        let alpha_declaration = offset_to_position(
            alpha_source,
            alpha_source
                .find("score [[value")
                .expect("alpha declaration"),
        );
        let provider_references = state.references(&alpha_uri, alpha_declaration);
        assert!(
            provider_references
                .iter()
                .any(|location| location.uri == app_uri)
        );
        assert!(
            provider_references
                .iter()
                .any(|location| location.uri == broken_uri)
        );

        let broken_diagnostics = state.did_open(&broken_uri, 1, broken_source);
        assert!(!broken_diagnostics.diagnostics.is_empty());
        let recovered_call = offset_to_position(
            broken_source,
            broken_source
                .find("alpha/score 4")
                .expect("recovered alpha call"),
        );
        assert_eq!(
            state
                .definition(&broken_uri, recovered_call)
                .expect("definition survives recovery"),
            alpha_definition
        );

        let alpha_member_call = offset_to_position(
            app_source,
            app_source.find("alpha/score 1").expect("alpha call") + "alpha/".len(),
        );
        assert_eq!(state.prepare_rename(&app_uri, alpha_call), None);
        let prepared = state
            .prepare_rename(&app_uri, alpha_member_call)
            .expect("qualified member prepare range");
        let prepared_start = position_to_offset(app_source, prepared.start).expect("range start");
        let prepared_end = position_to_offset(app_source, prepared.end).expect("range end");
        assert_eq!(&app_source[prepared_start..prepared_end], "score");

        let renamed = state
            .rename(&app_uri, alpha_member_call, "rank")
            .expect("workspace rename")
            .expect("workspace edits");
        assert_eq!(renamed.changes.get(&alpha_uri).map(Vec::len), Some(3));
        assert_eq!(renamed.changes.get(&app_uri).map(Vec::len), Some(1));
        assert_eq!(renamed.changes.get(&broken_uri).map(Vec::len), Some(1));
        assert!(!renamed.changes.contains_key(&beta_uri));
        for (edit_uri, edit_source) in [
            (&alpha_uri, alpha_source),
            (&app_uri, app_source),
            (&broken_uri, broken_source),
        ] {
            for edit in renamed
                .changes
                .get(edit_uri)
                .expect("expected source edits")
            {
                let start = position_to_offset(edit_source, edit.range.start).expect("edit start");
                let end = position_to_offset(edit_source, edit.range.end).expect("edit end");
                assert_eq!(&edit_source[start..end], "score");
                assert_eq!(edit.new_text, "rank");
            }
        }

        let alias_renamed = state
            .rename(&app_uri, alias_call, "分数")
            .expect("workspace alias rename")
            .expect("alias edits");
        assert_eq!(alias_renamed.changes.get(&alpha_uri).map(Vec::len), Some(2));
        assert_eq!(alias_renamed.changes.get(&app_uri).map(Vec::len), Some(2));
        assert!(!alias_renamed.changes.contains_key(&beta_uri));
        assert!(!alias_renamed.changes.contains_key(&broken_uri));
        for (edit_uri, edit_source) in [(&alpha_uri, alpha_source), (&app_uri, app_source)] {
            for edit in alias_renamed
                .changes
                .get(edit_uri)
                .expect("expected alias edits")
            {
                let start = position_to_offset(edit_source, edit.range.start).expect("edit start");
                let end = position_to_offset(edit_source, edit.range.end).expect("edit end");
                assert_eq!(&edit_source[start..end], "得分");
                assert_eq!(edit.new_text, "分数");
            }
        }

        drop(state);
        fs::remove_dir_all(root).expect("workspace cleanup");
    }

    #[test]
    fn external_interface_without_source_has_no_definition_location() {
        let provider_source = r#"(module vendor.math)
(export [score])
(defn score [[value Int]] -> Int value)
"#;
        let provider_options = CompileOptions::new("vendor.math", PythonVersion::MINIMUM);
        let provider = compiler::analyze(provider_source, &provider_options);
        assert!(
            provider.diagnostics.is_empty(),
            "{:?}",
            provider.diagnostics
        );
        let provider_interface =
            interface::build_provisional(&provider.surface).expect("provider interface");
        let external_interfaces = BTreeMap::from([("vendor.math".to_owned(), provider_interface)]);
        let consumer_source = r#"(module demo.app)
(import vendor.math :as math)
(def result (math/score 1))
"#;
        let consumer_options = CompileOptions::new("demo.app", PythonVersion::MINIMUM);
        let inputs = [CompileInput::new(consumer_source, &consumer_options)];
        let mut analyses = compiler::analyze_workspace_recovering(&inputs, &external_interfaces);
        assert_eq!(analyses.len(), 1);
        assert!(
            analyses[0].diagnostics.is_empty(),
            "{:?}",
            analyses[0].diagnostics
        );
        let function_interfaces = collect_function_interfaces(&analyses, &external_interfaces);
        let macro_interfaces = collect_macro_interfaces(&analyses, &external_interfaces);
        let analysis = analyses.remove(0);
        let uri = "file:///workspace/external-consumer.osr";
        let workspace_symbols = build_single_symbol_index(&analysis, uri, consumer_source);
        let document = OpenDocument::from_analysis(
            uri.to_owned(),
            1,
            consumer_source.to_owned(),
            Vec::new(),
            ProjectDocumentAnalysis {
                analysis,
                function_interfaces,
                macro_interfaces,
                workspace_symbols,
            },
        );
        let mut state = LspState::new();
        state.documents.insert(uri.to_owned(), document);
        let call = offset_to_position(
            consumer_source,
            consumer_source.find("math/score 1").expect("external call"),
        );

        assert_eq!(state.definition(uri, call), None);
        assert!(
            state
                .references(uri, call)
                .iter()
                .all(|location| location.uri == uri)
        );
        let member = offset_to_position(
            consumer_source,
            consumer_source.find("math/score 1").expect("external call") + "math/".len(),
        );
        assert_eq!(state.prepare_rename(uri, member), None);
        assert_eq!(
            state.rename(uri, member, "rank").expect("rename result"),
            None
        );
    }

    #[test]
    fn project_errors_preserve_workspace_identity_imports_and_completion() {
        let sequence = NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "osiris-lsp-recovering-workspace-{}-{sequence}",
            std::process::id()
        ));
        let source_root = root.join("src/demo");
        fs::create_dir_all(&source_root).expect("source root");
        fs::write(
            root.join("pyproject.toml"),
            "[project]\nname = \"lsp-workspace\"\nversion = \"1.0\"\n\n[tool.osiris]\nsource = [\"src\"]\n",
        )
        .expect("project configuration");
        fs::write(
            source_root.join("math.osr"),
            "(module demo.math)\n(export [add-one])\n(defn add-one [[x Int]] -> Int (+ x 1))\n",
        )
        .expect("dependency source");
        let app_source =
            "(module demo.app)\n(import demo.math :as math)\n(def answer (math/add-one 41))\n";
        let app = source_root.join("app.osr");
        fs::write(&app, app_source).expect("application source");
        let broken_source =
            "(module demo.broken)\n(import demo.math :as math)\n(defn invalid [[x Int]] -> Int)\n";
        let broken = source_root.join("broken.osr");
        fs::write(&broken, broken_source).expect("broken source");
        let app_uri = format!("file://{}", app.display());
        let broken_uri = format!("file://{}", broken.display());
        let mut state = LspState::new();

        let app_diagnostics = state.did_open(&app_uri, 1, app_source);

        assert!(
            app_diagnostics.diagnostics.is_empty(),
            "{app_diagnostics:?}"
        );
        let app_document = state.document(&app_uri).expect("open app document");
        assert_eq!(app_document.analysis.hir.name, "demo.app");
        let imported = app_document
            .semantic
            .symbols
            .iter()
            .find(|symbol| symbol.binding_id == "demo.math::function::add-one")
            .expect("imported function should remain in app semantics");
        assert_eq!(imported.kind, BindingKind::Function);
        assert!(matches!(imported.ty, Type::Fn(_)));
        assert!(
            state
                .completion(
                    &app_uri,
                    Position {
                        line: 3,
                        character: 0,
                    },
                    None,
                )
                .iter()
                .any(|item| item.data["bindingId"] == "demo.math::function::add-one")
        );

        let broken_diagnostics = state.did_open(&broken_uri, 1, broken_source);

        assert!(!broken_diagnostics.diagnostics.is_empty());
        let broken_document = state.document(&broken_uri).expect("open broken document");
        assert_eq!(broken_document.analysis.hir.name, "demo.broken");
        assert!(broken_document.semantic.symbols.iter().any(|symbol| {
            symbol.binding_id == "demo.math::function::add-one"
                && symbol.kind == BindingKind::Function
                && matches!(symbol.ty, Type::Fn(_))
        }));
        assert!(
            state
                .completion(
                    &broken_uri,
                    Position {
                        line: 3,
                        character: 0,
                    },
                    None,
                )
                .iter()
                .any(|item| item.data["bindingId"] == "demo.math::function::add-one")
        );
        drop(state);
        fs::remove_dir_all(root).expect("workspace cleanup");
    }

    #[test]
    fn signature_help_uses_cross_module_types_aliases_and_default_presence() {
        let sequence = NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "osiris-lsp-signature-workspace-{}-{sequence}",
            std::process::id()
        ));
        let source_root = root.join("src/demo");
        fs::create_dir_all(&source_root).expect("source root");
        fs::write(
            root.join("pyproject.toml"),
            "[project]\nname = \"lsp-signature\"\nversion = \"1.0\"\n\n[tool.osiris]\nsource = [\"src\"]\n",
        )
        .expect("project configuration");
        fs::write(
            source_root.join("math.osr"),
            r#"(module demo.math)
(export [rolling])
(defn rolling
  [[values Float]
   ^{:osiris/names {"zh-CN" {:preferred 周期}}} [window Int = 14]]
  -> Float
  values)
"#,
        )
        .expect("dependency source");
        let app_source = r#"(module demo.app)
(import demo.math :as math)
(def answer (math/rolling 1.0 ))
"#;
        let app = source_root.join("app.osr");
        fs::write(&app, app_source).expect("application source");
        let uri = format!("file://{}", app.display());
        let mut state = LspState::new();
        let diagnostics = state.did_open(&uri, 1, app_source);
        assert!(diagnostics.diagnostics.is_empty(), "{diagnostics:?}");
        let cursor =
            app_source.find("(math/rolling 1.0 )").expect("call") + "(math/rolling 1.0 ".len();

        let signature = state
            .signature_help(&uri, offset_to_position(app_source, cursor), Some("zh-CN"))
            .expect("cross-module signature help");

        assert_eq!(signature.active_parameter, Some(1));
        assert_eq!(
            signature.signatures[0].label,
            "math/rolling(values: Float, 周期: Int = ...) -> Float"
        );
        assert_eq!(
            signature.signatures[0].parameters[1].label,
            "周期: Int = ..."
        );
        drop(state);
        fs::remove_dir_all(root).expect("workspace cleanup");
    }

    #[test]
    fn macro_signature_help_uses_stable_identity_for_qualified_referred_and_local_macros() {
        let sequence = NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "osiris-lsp-macro-signature-workspace-{}-{sequence}",
            std::process::id()
        ));
        let source_root = root.join("src/demo");
        fs::create_dir_all(&source_root).expect("source root");
        fs::write(
            root.join("pyproject.toml"),
            "[project]\nname = \"lsp-macro-signature\"\nversion = \"1.0\"\n\n[tool.osiris]\nsource = [\"src\"]\n",
        )
        .expect("project configuration");
        fs::write(
            source_root.join("first.osr"),
            "(module demo.first)\n(export [wrap])\n(defmacro wrap [第一值 & 其余] 第一值)\n",
        )
        .expect("first macro dependency");
        fs::write(
            source_root.join("second.osr"),
            "(module demo.second)\n(export [wrap])\n(defmacro wrap [第二值] 第二值)\n",
        )
        .expect("second macro dependency");
        let app_source = r#"(module demo.app)
(import-for-syntax demo.first :as first)
(import-for-syntax demo.second :refer [wrap])
(defmacro local-wrap [本地值] 本地值)
(def first-result (first/wrap 1 2))
(def second-result (wrap 2))
(def local-result (local-wrap 3))
"#;
        let app = source_root.join("app.osr");
        fs::write(&app, app_source).expect("application source");
        let uri = format!("file://{}", app.display());
        let mut state = LspState::new();
        let diagnostics = state.did_open(&uri, 1, app_source);
        assert!(diagnostics.diagnostics.is_empty(), "{diagnostics:?}");

        let qualified_position = offset_to_position(
            app_source,
            app_source.find("(first/wrap 1 2)").expect("qualified call") + "(first/wrap 1 ".len(),
        );
        let referred_position = offset_to_position(
            app_source,
            app_source.find("(wrap 2)").expect("referred call") + "(wrap ".len(),
        );
        let local_position = offset_to_position(
            app_source,
            app_source.find("(local-wrap 3)").expect("local call") + "(local-wrap ".len(),
        );

        let qualified = state
            .signature_help(&uri, qualified_position, Some("zh-CN"))
            .expect("qualified macro signature");
        let referred = state
            .signature_help(&uri, referred_position, Some("zh-CN"))
            .expect("referred macro signature");
        let local = state
            .signature_help(&uri, local_position, Some("zh-CN"))
            .expect("local macro signature");

        assert_eq!(qualified.signatures[0].label, "first/wrap(第一值, & 其余)");
        assert_eq!(qualified.active_parameter, Some(1));
        assert_eq!(referred.signatures[0].label, "wrap(第二值)");
        assert_eq!(local.signatures[0].label, "local-wrap(本地值)");
        let trace_ids = state
            .document(&uri)
            .expect("open app")
            .semantic
            .macro_traces
            .iter()
            .map(|trace| trace.macro_binding_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            trace_ids,
            [
                "demo.first::macro::wrap",
                "demo.second::macro::wrap",
                "demo.app::macro::local-wrap"
            ]
        );
        drop(state);
        fs::remove_dir_all(root).expect("workspace cleanup");
    }

    #[test]
    fn full_and_ranged_changes_update_versions_once() {
        let mut state = LspState::new();
        state.did_open(URI, 4, "(module demo)\n(def value 1)\n");
        let changed = state
            .did_change_full(URI, 5, "(module demo)\n(def value missing)\n")
            .expect("new version");
        assert!(!changed.diagnostics.is_empty());
        assert_eq!(state.analysis_runs(), 2);
        assert_eq!(state.document_version(URI), Some(5));

        let stale = state.did_change_full(URI, 5, "(module demo)");
        assert!(stale.is_err());
        assert_eq!(state.analysis_runs(), 2);

        let repaired = state
            .did_change(
                URI,
                6,
                &[TextDocumentContentChangeEvent {
                    range: Some(Range {
                        start: Position {
                            line: 1,
                            character: 11,
                        },
                        end: Position {
                            line: 1,
                            character: 18,
                        },
                    }),
                    range_length: Some(7),
                    text: "1".to_owned(),
                }],
            )
            .expect("range change");
        assert!(repaired.diagnostics.is_empty());
        assert_eq!(state.analysis_runs(), 3);
    }

    #[test]
    fn document_changes_reuse_incremental_reader_node_identities() {
        let uri = "file:///workspace/incremental.osr";
        let mut state = LspState::new();
        state.did_open(uri, 1, "(def first 1)\n(def second 2)\n");
        let retained = state
            .document(uri)
            .expect("open document")
            .analysis
            .document
            .node_id(&NodePath::top_level(1))
            .expect("second form identity");

        state
            .did_change_full(uri, 2, "; note\n(def first 10)\n(def second 2)\n")
            .expect("changed document");
        let changed = state.document(uri).expect("changed document");
        assert_eq!(
            changed.analysis.document.node_id(&NodePath::top_level(1)),
            Some(retained)
        );
    }

    #[test]
    fn strict_unicode_lints_are_lsp_warnings_not_compiler_errors() {
        let uri = "file:///workspace/unicode-lint.osr";
        let mut state = LspState::new();
        let diagnostics = state.did_open(uri, 1, "(def pаypal 1)\n");
        assert!(
            !state
                .document(uri)
                .expect("open document")
                .analysis
                .has_errors()
        );
        assert!(diagnostics.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == CONFUSABLE_IDENTIFIER && diagnostic.severity == 2
        }));
        assert!(diagnostics.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == MIXED_SCRIPT_IDENTIFIER && diagnostic.severity == 2
        }));
        assert!(
            diagnostics
                .diagnostics
                .iter()
                .filter(|diagnostic| {
                    diagnostic.code == CONFUSABLE_IDENTIFIER
                        || diagnostic.code == MIXED_SCRIPT_IDENTIFIER
                })
                .all(|diagnostic| diagnostic.data["nodeId"].is_u64())
        );
    }

    #[test]
    fn positions_use_lsp_utf16_units() {
        let source = "a😀中\nvalue";
        let offset = source.find('中').expect("Chinese character");
        let position = offset_to_position(source, offset);
        assert_eq!(
            position,
            Position {
                line: 0,
                character: 3,
            }
        );
        assert_eq!(position_to_offset(source, position), Some(offset));
        assert_eq!(
            position_to_offset(
                source,
                Position {
                    line: 0,
                    character: 2,
                }
            ),
            None
        );
    }

    #[test]
    fn json_rpc_transcript_recovers_and_exposes_semantics() {
        let mut machine = JsonRpcMachine::new();
        let initialize = machine.handle(
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "initializationOptions": { "displayLocale": "zh-CN" }
                }
            })
            .to_string(),
        );
        assert_eq!(initialize.response.as_ref().expect("response")["id"], 1);

        let opened = machine.handle(
            &json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didOpen",
                "params": {
                    "textDocument": {
                        "uri": URI,
                        "languageId": "osiris",
                        "version": 1,
                        "text": source(),
                    }
                }
            })
            .to_string(),
        );
        assert!(opened.response.is_none());
        assert_eq!(
            opened.notifications[0]["method"],
            "textDocument/publishDiagnostics"
        );

        let semantic = machine.handle(
            &json!({
                "jsonrpc": "2.0",
                "id": "semantic",
                "method": "osiris/semanticView",
                "params": { "textDocument": { "uri": URI }, "locale": "zh-CN" }
            })
            .to_string(),
        );
        let result = &semantic.response.as_ref().expect("response")["result"];
        assert_eq!(result["version"], 1);
        assert_eq!(result["document_version"], 1);
        assert!(result["symbols"].is_array());
        assert!(result["operation_graph"]["nodes"].is_array());

        let inspected = machine.handle(
            &json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "osiris/inspect",
                "params": { "uri": URI, "symbol": "加一" }
            })
            .to_string(),
        );
        assert_eq!(
            inspected.response.as_ref().expect("response")["result"]["canonical"],
            "add-one"
        );

        let malformed = handle_json_rpc(&mut machine.state, "{broken");
        assert_eq!(
            malformed.response.as_ref().expect("parse response")["error"]["code"],
            PARSE_ERROR
        );
        assert_eq!(machine.state.analysis_runs(), 1);

        let syntax = machine.handle(
            &json!({
                "jsonrpc": "2.0",
                "id": "syntax",
                "method": "osiris/syntaxSnapshot",
                "params": { "textDocument": { "uri": URI } }
            })
            .to_string(),
        );
        let syntax_result = &syntax.response.as_ref().expect("response")["result"];
        assert_eq!(syntax_result["version"], 1);
        assert_eq!(syntax_result["documentVersion"], 1);
        assert!(
            syntax_result["nodes"]
                .as_array()
                .is_some_and(|nodes| !nodes.is_empty())
        );
        assert!(syntax_result["nodes"][0]["id"].is_u64());
    }

    #[test]
    fn expand_preview_includes_macro_trace() {
        let mut state = LspState::new();
        state.did_open(URI, 1, "(module demo)\n(def result (-> 1 (+ 2)))\n");
        let preview = state.expand_preview(URI).expect("preview");
        assert!(preview.text.contains("(+ 1 2)"));
        assert!(!preview.macro_traces.is_empty());
        assert_eq!(
            preview.macro_traces[0].macro_binding_id,
            "osiris.prelude::macro::->"
        );
        let serialized = serde_json::to_value(preview).expect("preview serializes");
        assert_eq!(
            serialized["macroTraces"][0]["macro_binding_id"],
            "osiris.prelude::macro::->"
        );
    }

    #[test]
    fn malformed_request_has_json_rpc_error_shape() {
        let mut state = LspState::new();
        let output = handle_json_rpc(&mut state, "[]");
        let response: &JsonValue = output.response.as_ref().expect("error response");
        assert_eq!(response["jsonrpc"], "2.0");
        assert!(response["error"]["code"].is_number());
    }
}
