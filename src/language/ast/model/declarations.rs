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
pub struct Extern {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
    pub backend: Name,
    pub module: String,
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
