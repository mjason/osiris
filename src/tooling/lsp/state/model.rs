
/// An open document and its one cached frontend analysis.
#[derive(Clone, Debug)]
pub struct OpenDocument {
    pub uri: String,
    pub version: i64,
    pub text: String,
    pub analysis: Analysis,
    pub semantic: SemanticDocument,
    pub identifier_lints: Vec<IdentifierLint>,
    pub(super) function_interfaces: BTreeMap<String, interface::FunctionInterface>,
    pub(super) macro_interfaces: BTreeMap<String, interface::MacroInterface>,
    pub(super) display_locale: Option<String>,
    workspace_symbols: WorkspaceSymbolIndex,
}

impl OpenDocument {
    pub(super) fn from_analysis(
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
            display_locale,
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
            display_locale,
            workspace_symbols,
        }
    }
}

pub(super) struct ProjectDocumentAnalysis {
    pub(super) analysis: Analysis,
    pub(super) function_interfaces: BTreeMap<String, interface::FunctionInterface>,
    pub(super) macro_interfaces: BTreeMap<String, interface::MacroInterface>,
    pub(super) display_locale: Option<String>,
    pub(super) workspace_symbols: WorkspaceSymbolIndex,
}

/// Reusable result of analyzing one complete project snapshot. The source
/// buffers are still rebuilt on each notification so edits on disk and open
/// editor buffers are detected without rerunning the compiler on a cache hit.
#[derive(Clone, Debug)]
pub(super) struct WorkspaceAnalysisCache {
    pub(super) project_root: PathBuf,
    pub(super) fingerprint: String,
    pub(super) buffers: Vec<super::WorkspaceBuffer>,
    pub(super) analyses: Vec<Analysis>,
    pub(super) workspace_diagnostics: Vec<crate::compiler::LocatedDiagnostic>,
    pub(super) function_interfaces:
        BTreeMap<String, interface::FunctionInterface>,
    pub(super) macro_interfaces: BTreeMap<String, interface::MacroInterface>,
    pub(super) workspace_symbols: super::WorkspaceSymbolIndex,
    pub(super) display_locale: Option<String>,
}

/// Mutable LSP database. Project documents share an analysis cache keyed by
/// the complete source/configuration snapshot; a changed snapshot is rebuilt
/// once and then reused by all editor queries.
#[derive(Clone, Debug)]
pub struct LspState {
    pub(super) documents: BTreeMap<String, OpenDocument>,
    target_python: PythonVersion,
    display_locale: String,
    session_locale: Option<String>,
    site_roots: Vec<PathBuf>,
    analysis_runs: u64,
    shutdown_requested: bool,
    pub(super) workspace_cache: Option<WorkspaceAnalysisCache>,
}

impl Default for LspState {
    fn default() -> Self {
        Self {
            documents: BTreeMap::new(),
            target_python: PythonVersion::DEFAULT_TARGET,
            display_locale: "zh-CN".to_owned(),
            session_locale: None,
            site_roots: Vec::new(),
            analysis_runs: 0,
            shutdown_requested: false,
            workspace_cache: None,
        }
    }
}
