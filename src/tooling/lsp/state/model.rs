
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

/// Mutable LSP database. The v0 implementation recomputes the changed
/// document against one workspace snapshot.
#[derive(Clone, Debug)]
pub struct LspState {
    pub(super) documents: BTreeMap<String, OpenDocument>,
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
            target_python: PythonVersion::DEFAULT_TARGET,
            display_locale: "en".to_owned(),
            site_roots: Vec::new(),
            analysis_runs: 0,
            shutdown_requested: false,
        }
    }
}
