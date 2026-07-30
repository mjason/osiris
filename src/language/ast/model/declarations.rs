use super::*;

pub type Metadata = Vec<MetadataEntry>;

pub const OPERATOR_METADATA_KEY: &str = "osiris/operator";

/// Shape errors for the closed static operator declaration metadata.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperatorMetadataError {
    Duplicate,
    ExpectedName,
}

/// Read `^{:osiris/operator :add}` without assigning it semantic authority.
/// Ownership and signature validation happen while building the typed public
/// interface; the AST only exposes the authored declaration deterministically.
pub fn operator_declaration(
    metadata: &[MetadataEntry],
) -> Result<Option<String>, OperatorMetadataError> {
    let mut declaration = None;
    for entry in metadata {
        let key = match &entry.key.kind {
            FormKind::Keyword(name) | FormKind::Symbol(name) => {
                name.canonical.trim_start_matches(':')
            }
            _ => continue,
        };
        if key != OPERATOR_METADATA_KEY {
            continue;
        }
        if declaration.is_some() {
            return Err(OperatorMetadataError::Duplicate);
        }
        let value = match &entry.value.kind {
            FormKind::Keyword(name) | FormKind::Symbol(name) => {
                name.canonical.trim_start_matches(':').to_owned()
            }
            _ => return Err(OperatorMetadataError::ExpectedName),
        };
        declaration = Some(value);
    }
    Ok(declaration)
}

/// Common source information for clients that want to inspect a node without
/// matching its kind.  Public AST structs also expose `span` and `metadata`
/// directly because that is more convenient for LSP consumers.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct NodeInfo {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
}

impl NodeInfo {
    pub(in crate::ast) fn from_form(form: &Form) -> Self {
        Self {
            span: form.span,
            metadata: form.metadata.clone(),
        }
    }
}

/// Lowering output.  A module is always returned, including when the source
/// has malformed declarations, so editor tooling can still inspect later
/// forms.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LowerResult {
    pub module: Module,
    pub diagnostics: Vec<Diagnostic>,
}

/// A source module.  `name` is `None` for a source file without a module
/// header; no implicit name is invented by this pass.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Module {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
    pub name: Option<Name>,
    pub items: Vec<Item>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub embedded_content_references: Vec<EmbeddedContentReference>,
}

/// One statically resolved Rich Metadata reference to an embedded text block.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EmbeddedContentReference {
    pub field: String,
    pub reference_span: Span,
    pub language: String,
    pub label: String,
    pub content: String,
    pub source_span: Span,
    pub body_span: Span,
    pub content_hash: String,
}

impl Module {
    #[must_use]
    pub fn is_named(&self) -> bool {
        self.name.is_some()
    }
}

/// A top-level item.  Header declarations are kept as explicit variants so
/// later dependency-graph construction does not need to inspect raw forms.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Item {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
    pub kind: ItemKind,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
#[allow(clippy::large_enum_variant)]
pub enum ItemKind {
    Import(Import),
    ImportForSyntax(Import),
    PyImport(PyImport),
    PyDecorate(PyDecorate),
    Export(Export),
    Alias(Alias),
    Def(Def),
    Defn(Function),
    Defstruct(Defstruct),
    DefstaticSchema(DefstaticSchema),
    StaticRecord(StaticRecord),
    EmbeddedText(EmbeddedText),
    EmbeddedPython(EmbeddedPython),
    Extern(Extern),
    Defmacro(Macro),
    DefnForSyntax(Function),
    /// A top-level expression is legal in a source file and remains visible
    /// to later validation/code generation.
    Expr(Expr),
    Error(String),
}

impl Item {
    pub(in crate::ast) fn new(form: &Form, kind: ItemKind) -> Self {
        let info = NodeInfo::from_form(form);
        Self {
            span: info.span,
            metadata: info.metadata,
            kind,
        }
    }

    #[must_use]
    pub fn is_declaration(&self) -> bool {
        !matches!(self.kind, ItemKind::Expr(_) | ItemKind::Error(_))
    }

    /// The name this declaration publishes through the per-item `^:export`
    /// marker, if it carries one.
    ///
    /// `^:export` reads as `^{:export true}`, and the lowerer merges metadata
    /// written before the form with metadata written on the declared name, so
    /// `^:export (def x 1)` and `(def ^:export x 1)` are the same declaration.
    /// Any other value stays ordinary authored metadata and publishes nothing.
    ///
    /// This is the second explicit way to publish a name; the module-level
    /// `(export [...])` manifest is the first, and the public surface is their
    /// union. Unlike the manifest, a marker rides on the declaration itself, so
    /// a declaration macro can generate one.
    #[must_use]
    pub fn export_marker(&self) -> Option<&Name> {
        if !self.carries_export_marker() {
            return None;
        }
        match &self.kind {
            ItemKind::Def(definition) => Some(&definition.name),
            ItemKind::Defn(function) | ItemKind::DefnForSyntax(function) => function.name.as_ref(),
            ItemKind::Defstruct(structure) => Some(&structure.name),
            ItemKind::DefstaticSchema(schema) => Some(&schema.name),
            ItemKind::Defmacro(declaration) => Some(&declaration.name),
            // A generic embedded block declares an ordinary `Str` binding with
            // ordinary module visibility. Embedded Python is deliberately
            // absent: its label is a private provider handle, not a binding a
            // module can publish.
            ItemKind::EmbeddedText(text) => Some(&text.label),
            _ => None,
        }
    }

    /// Whether the `:export` key is present and true anywhere on this item,
    /// regardless of whether this kind of item can be published.
    ///
    /// A marker that publishes nothing is a mistake worth reporting rather than
    /// ignoring, so the two questions are asked separately.
    #[must_use]
    pub fn carries_export_marker(&self) -> bool {
        let declaration: &[MetadataEntry] = match &self.kind {
            ItemKind::Def(definition) => &definition.metadata,
            ItemKind::Defn(function) | ItemKind::DefnForSyntax(function) => &function.metadata,
            ItemKind::Defstruct(structure) => &structure.metadata,
            ItemKind::DefstaticSchema(schema) => &schema.metadata,
            ItemKind::Defmacro(declaration) => &declaration.metadata,
            ItemKind::Extern(external) => &external.metadata,
            _ => &[],
        };
        declares_export(declaration) || declares_export(&self.metadata)
    }
}

/// Every item a module declares, descending into the declarations an `extern`
/// block nests so that both publication paths see the same set.
#[must_use]
pub fn declared_items(items: &[Item]) -> Vec<&Item> {
    let mut flattened = Vec::new();
    collect_declared_items(items, &mut flattened);
    flattened
}

fn collect_declared_items<'a>(items: &'a [Item], flattened: &mut Vec<&'a Item>) {
    for item in items {
        flattened.push(item);
        if let ItemKind::Extern(external) = &item.kind {
            collect_declared_items(&external.items, flattened);
        }
    }
}

/// The names a module publishes through per-item `^:export` markers.
#[must_use]
pub fn export_markers(items: &[Item]) -> Vec<&Name> {
    declared_items(items)
        .into_iter()
        .filter_map(Item::export_marker)
        .collect()
}

/// The metadata key that publishes one declaration, standardized by the
/// language as a peer of the module-level `(export [...])` manifest.
pub const EXPORT_METADATA_KEY: &str = "export";

fn declares_export(metadata: &[MetadataEntry]) -> bool {
    metadata.iter().any(|entry| {
        matches!(
            &entry.key.kind,
            FormKind::Keyword(key) | FormKind::Symbol(key)
                if key.canonical.trim_start_matches(':') == EXPORT_METADATA_KEY
        ) && matches!(entry.value.kind, FormKind::Bool(true))
    })
}

/// Import phase is kept explicit even though the public item enum also has
/// dedicated runtime/compile-time variants.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImportPhase {
    Runtime,
    Syntax,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Import {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
    pub module: Name,
    pub alias: Option<Name>,
    pub members: Vec<Name>,
    pub refer_all: bool,
    pub excluded: Vec<Name>,
    pub renamed: Vec<ImportRename>,
    pub phase: ImportPhase,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ImportRename {
    pub canonical: Name,
    pub local: Name,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PyImport {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
    /// Python module paths are retained as a string because they may contain
    /// dots or names that are not Osiris identifiers.
    pub module: String,
    pub alias: Option<Name>,
}

/// Explicit Python decorators attached to one generated declaration.
///
/// Decorators are executable Python expressions, so they are deliberately
/// separate from immutable Rich Metadata.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PyDecorate {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
    pub target: Name,
    pub target_span: Span,
    pub decorators: Vec<Expr>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Export {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
    pub names: Vec<Name>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Alias {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
    pub local: Name,
    pub target: Name,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Def {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
    pub name: Name,
    pub type_annotation: Option<TypeExpr>,
    pub value: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Function {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
    pub name: Option<Name>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub type_params: Vec<Name>,
    pub params: Vec<Param>,
    pub return_type: Option<TypeExpr>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contract: Option<ExternContract>,
    pub body: Vec<Expr>,
    pub phase: FunctionPhase,
    /// Original declaration retained for phase-1 interface emission. Runtime
    /// functions do not need it, and it is intentionally omitted from JSON.
    #[serde(skip)]
    pub phase_form: Option<Form>,
}

/// A closed, data-only declaration attached to an `extern` function.
///
/// Omitted summary sections remain conservative (`unknown`). The contract id
/// is an opaque stable identity used by interfaces and, later, local trust
/// policy; it does not grant trust by itself.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ExternContract {
    pub span: Span,
    pub id: String,
    pub summaries: CallSummaries,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FunctionPhase {
    Runtime,
    Syntax,
    Macro,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Macro {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
    pub name: Name,
    pub params: Vec<Param>,
    pub return_type: Option<TypeExpr>,
    pub body: Vec<Expr>,
    /// Exact reader form used as the replayable phase-1 interface IR.
    #[serde(skip)]
    pub phase_form: Form,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Defstruct {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
    pub name: Name,
    pub type_params: Vec<Name>,
    pub doc: Option<String>,
    pub fields: Vec<Field>,
    pub checks: Vec<StructCheck>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StructCheck {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
    pub condition: Expr,
    pub message: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Field {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
    pub name: Name,
    pub type_annotation: Option<TypeExpr>,
    pub default: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DefstaticSchema {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
    pub name: Name,
    pub body: Vec<Expr>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StaticRecord {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
    pub schema: Name,
    pub owner: Name,
    pub fields: Vec<(Name, Expr)>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EmbeddedText {
    pub span: Span,
    pub body_span: Span,
    pub language: String,
    pub label: Name,
    pub body: String,
    pub runtime_reachable: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EmbeddedPython {
    pub span: Span,
    pub body_span: Span,
    pub handle: Name,
    pub raw_body: String,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logical_module: Option<String>,
    /// Set when `py/embed` named a file rather than carrying the body inline.
    ///
    /// The body arrives empty and the caller fills it, because compilation is a
    /// function of source text and options: the compiler core does not read the
    /// filesystem (OEP-0001-R006CC). Everything downstream sees an ordinary
    /// provider either way.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Extern {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
    pub backend: Name,
    pub module: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_handle: Option<Name>,
    pub items: Vec<Item>,
}

// Declaration aliases keep the public API readable for clients that prefer a
// Decl suffix while preserving the compact enum payload names.
pub type ImportDecl = Import;
pub type PyImportDecl = PyImport;
pub type ExportDecl = Export;
pub type AliasDecl = Alias;
pub type DefDecl = Def;
pub type FunctionDecl = Function;
pub type StructDecl = Defstruct;
pub type ExternDecl = Extern;
