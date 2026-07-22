//! Surface AST lowering for Osiris.
//!
//! The reader deliberately knows very little about the language.  This module
//! is the boundary where the reader's lossless [`Form`] tree becomes a small,
//! typed-enough surface tree that later name resolution and HIR lowering can
//! consume.  Lowering is intentionally non-binding: a symbol is still just a
//! symbol and imports/aliases are declarations, not lookups.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::{
    diagnostic::Diagnostic,
    source::Span,
    syntax::{Document, Form, FormKind, MetadataEntry, Name, datum_eq},
    types::{
        Alignment, Availability, CallSummaries, DataProperties, Effect, EffectRow, TemporalBound,
        TemporalSummary, parse_type,
    },
};

/// Stable diagnostics emitted by this lowering pass.
pub const AST_EXPECTED_LIST: &str = "OSR-A0001";
pub const AST_MISSING_NAME: &str = "OSR-A0002";
pub const AST_INVALID_NAME: &str = "OSR-A0003";
pub const AST_WRONG_SHAPE: &str = "OSR-A0004";
pub const AST_EXPECTED_VECTOR: &str = "OSR-A0005";
pub const AST_EXPECTED_PAIR: &str = "OSR-A0006";
pub const AST_INVALID_KEYWORD_ARGS: &str = "OSR-A0007";
pub const AST_UNKNOWN_CLAUSE: &str = "OSR-A0008";
pub const AST_INVALID_CONTRACT: &str = "OSR-A0009";
pub const AST_CONFLICTING_TYPE_ANNOTATION: &str = "OSR-A0010";
pub const AST_INVALID_TYPE_METADATA: &str = "OSR-A0011";

/// Metadata attached to every surface node is copied from the reader form.
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
    fn from_form(form: &Form) -> Self {
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
    fn new(form: &Form, kind: ItemKind) -> Self {
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
    pub phase: ImportPhase,
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

/// Function/lambda parameter.  `variadic` marks the parameter following `&`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Param {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
    pub name: Name,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<Pattern>,
    pub type_annotation: Option<TypeExpr>,
    pub default: Option<Expr>,
    pub variadic: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TypeExpr {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
    pub kind: TypeExprKind,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum TypeExprKind {
    Name(Name),
    Apply {
        constructor: Box<TypeExpr>,
        args: Vec<TypeExpr>,
    },
    /// Function types keep their parameter vector structural.  A vector in a
    /// generic type application is a literal, but in `(Fn [A B] -> C)` it is
    /// the function parameter list and must not be collapsed into a literal.
    Function {
        parameters: Vec<TypeExpr>,
        return_type: Box<TypeExpr>,
    },
    Tuple(Vec<TypeExpr>),
    Union(Vec<TypeExpr>),
    /// A type-level vector/map or an otherwise extension-defined form.  The
    /// original reader datum is retained until an extension type checker sees
    /// it.
    Literal(Form),
    Error(String),
}

/// A pattern used by `let` bindings.  Destructuring is represented now so the
/// later HIR pass can add field/key resolution without changing the surface
/// API.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Pattern {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
    pub kind: PatternKind,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum PatternKind {
    Name(Name),
    Ignore,
    Vector(Vec<Pattern>),
    Map(Vec<(Pattern, Pattern)>),
    Literal(Form),
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Binding {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
    pub pattern: Pattern,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_annotation: Option<TypeExpr>,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct KeywordArg {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
    pub key: Name,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum CallArg {
    Positional(Expr),
    Keyword(KeywordArg),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CallExpr {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
    pub callee: Box<Expr>,
    /// Source order, including duplicate keyword arguments.
    pub args: Vec<CallArg>,
    /// Convenience projections of `args`; these retain their source order and
    /// are intentionally not deduplicated.
    pub positional: Vec<Expr>,
    pub keywords: Vec<KeywordArg>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FnExpr {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
    pub params: Vec<Param>,
    pub return_type: Option<TypeExpr>,
    pub body: Vec<Expr>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TryExpr {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
    pub body: Vec<Expr>,
    pub catches: Vec<CatchClause>,
    pub finally_body: Option<Vec<Expr>>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CatchClause {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
    pub exception_type: Option<TypeExpr>,
    pub binding: Option<Param>,
    pub body: Vec<Expr>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Expr {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Metadata,
    pub kind: ExprKind,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum ExprKind {
    None,
    Bool(bool),
    Integer(String),
    Float(String),
    String(String),
    Keyword(Name),
    Name(Name),
    List(Vec<Expr>),
    Vector(Vec<Expr>),
    Map(Vec<(Expr, Expr)>),
    Set(Vec<Expr>),
    Call(CallExpr),
    Fn(FnExpr),
    Let {
        bindings: Vec<Binding>,
        body: Vec<Expr>,
    },
    If {
        condition: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Option<Box<Expr>>,
    },
    Do(Vec<Expr>),
    Try(TryExpr),
    Raise(Option<Box<Expr>>),
    Quote(Box<Expr>),
    SyntaxQuote(Box<Expr>),
    Unquote(Box<Expr>),
    UnquoteSplicing(Box<Expr>),
    Error(String),
}

impl Expr {
    fn from_form(form: &Form, kind: ExprKind) -> Self {
        let info = NodeInfo::from_form(form);
        Self {
            span: info.span,
            metadata: info.metadata,
            kind,
        }
    }

    #[must_use]
    pub fn name(&self) -> Option<&Name> {
        match &self.kind {
            ExprKind::Name(name) => Some(name),
            _ => None,
        }
    }
}

impl TypeExpr {
    fn from_form(form: &Form, kind: TypeExprKind) -> Self {
        let info = NodeInfo::from_form(form);
        Self {
            span: info.span,
            metadata: info.metadata,
            kind,
        }
    }
}

/// Lower a reader document into the surface AST.
#[must_use]
pub fn lower_document(document: &Document) -> LowerResult {
    let mut lowerer = Lowerer {
        diagnostics: document.diagnostics.clone(),
        next_pattern_parameter: 0,
    };
    let mut module = Module {
        span: if document.forms.is_empty() {
            Span::new(0, document.source_len)
        } else {
            document
                .forms
                .iter()
                .map(|form| form.span)
                .reduce(Span::cover)
                .unwrap_or(Span::new(0, document.source_len))
        },
        metadata: Vec::new(),
        name: None,
        items: Vec::new(),
    };
    let mut saw_non_header = false;
    let mut saw_module_header = false;

    for form in &document.forms {
        if lowerer.is_head(form, "module") {
            if saw_module_header {
                lowerer.error(
                    AST_WRONG_SHAPE,
                    "module header may appear only once",
                    form.span,
                );
                continue;
            }
            saw_module_header = true;
            if saw_non_header {
                lowerer.error(
                    AST_WRONG_SHAPE,
                    "module header must precede top-level items",
                    form.span,
                );
            }
            let (name, metadata) = lowerer.lower_module_header(form);
            module.name = name;
            module.metadata = metadata;
            continue;
        }
        saw_non_header = true;
        module.items.push(lowerer.lower_item(form));
    }

    LowerResult {
        module,
        diagnostics: lowerer.diagnostics,
    }
}

struct Lowerer {
    diagnostics: Vec<Diagnostic>,
    next_pattern_parameter: usize,
}

#[derive(Default)]
struct MetadataTypeAnnotation {
    present: bool,
    annotation: Option<TypeExpr>,
}

impl Lowerer {
    fn error(&mut self, code: &'static str, message: impl Into<String>, span: Span) {
        self.diagnostics
            .push(Diagnostic::error(code, message, span));
    }

    fn lower_metadata_type(
        &mut self,
        metadata: &[MetadataEntry],
        context: &str,
    ) -> MetadataTypeAnnotation {
        let declared = metadata
            .iter()
            .find(|entry| metadata_key(&entry.key) == Some("type"));
        let tagged = metadata
            .iter()
            .find(|entry| metadata_key(&entry.key) == Some("tag"));
        let present = declared.is_some() || tagged.is_some();

        if let (Some(declared), Some(_)) = (declared, tagged) {
            self.error(
                AST_CONFLICTING_TYPE_ANNOTATION,
                format!(
                    "{context} has both `:type` and Clojure `:tag` metadata; `:type` takes precedence"
                ),
                declared.key.span,
            );
        }

        let Some(entry) = declared.or(tagged) else {
            return MetadataTypeAnnotation::default();
        };
        if matches!(entry.value.kind, FormKind::Bool(true)) {
            let message = if declared.is_some() {
                format!(
                    "`^:type` on a {context} is only a marker and does not name a type; use `^{{:type T}}` or `^T`"
                )
            } else {
                format!("Clojure `:tag` metadata on a {context} must name a type")
            };
            self.error(AST_INVALID_TYPE_METADATA, message, entry.value.span);
            return MetadataTypeAnnotation {
                present,
                annotation: None,
            };
        }
        if let Err(error) = parse_type(&entry.value, &BTreeMap::new()) {
            self.error(
                AST_INVALID_TYPE_METADATA,
                format!("invalid {context} type metadata: {error}"),
                error.span,
            );
            return MetadataTypeAnnotation {
                present,
                annotation: None,
            };
        }
        let mut annotation = self.lower_metadata_type_form(&entry.value);
        annotation.metadata = metadata.to_vec();
        MetadataTypeAnnotation {
            present,
            annotation: Some(annotation),
        }
    }

    fn lower_metadata_type_form(&mut self, form: &Form) -> TypeExpr {
        let FormKind::Symbol(name) = &form.kind else {
            return self.lower_type(form);
        };
        let arity = match name.canonical.as_str() {
            "Vector" | "List" | "Set" | "Option" => 1,
            "Map" => 2,
            _ => return self.lower_type(form),
        };
        let any = TypeExpr {
            span: form.span,
            metadata: Vec::new(),
            kind: TypeExprKind::Name(Name {
                spelling: "Any".to_owned(),
                canonical: "Any".to_owned(),
            }),
        };
        TypeExpr {
            span: form.span,
            metadata: Vec::new(),
            kind: TypeExprKind::Apply {
                constructor: Box::new(self.lower_type(form)),
                args: vec![any; arity],
            },
        }
    }

    fn report_type_annotation_conflict(&mut self, context: &str, span: Span) {
        self.error(
            AST_CONFLICTING_TYPE_ANNOTATION,
            format!(
                "{context} has both explicit and metadata type annotations; the explicit annotation takes precedence"
            ),
            span,
        );
    }

    fn is_head(&self, form: &Form, expected: &str) -> bool {
        list_parts(form)
            .and_then(|parts| parts.first())
            .and_then(symbol_name)
            .is_some_and(|name| name.canonical == expected)
    }

    fn lower_module_header(&mut self, form: &Form) -> (Option<Name>, Metadata) {
        let Some(parts) = list_parts(form) else {
            self.error(
                AST_EXPECTED_LIST,
                "module declaration must be a list",
                form.span,
            );
            return (None, form.metadata.clone());
        };
        if parts.len() != 2 {
            self.error(
                AST_WRONG_SHAPE,
                "module declaration expects exactly one module name",
                form.span,
            );
        }
        let name = match parts.get(1) {
            Some(part) => self.require_name(part, "module name"),
            None => {
                self.error(
                    AST_MISSING_NAME,
                    "module declaration requires a name",
                    form.span,
                );
                None
            }
        };
        (name, form.metadata.clone())
    }

    fn lower_item(&mut self, form: &Form) -> Item {
        let Some(parts) = list_parts(form) else {
            return Item::new(form, ItemKind::Expr(self.lower_expr(form)));
        };
        let Some(head) = parts.first().and_then(symbol_name) else {
            return Item::new(form, ItemKind::Expr(self.lower_expr(form)));
        };
        match head.canonical.as_str() {
            "import" => Item::new(form, ItemKind::Import(self.lower_import(form, false))),
            "import-for-syntax" => Item::new(
                form,
                ItemKind::ImportForSyntax(self.lower_import(form, true)),
            ),
            "py/import" => Item::new(form, ItemKind::PyImport(self.lower_py_import(form))),
            "export" => Item::new(form, ItemKind::Export(self.lower_export(form))),
            "alias" => Item::new(form, ItemKind::Alias(self.lower_alias(form))),
            "def" => Item::new(form, ItemKind::Def(self.lower_def(form))),
            "defn" => Item::new(
                form,
                ItemKind::Defn(self.lower_function(
                    form,
                    FunctionPhase::Runtime,
                    true,
                    true,
                    false,
                )),
            ),
            "defstruct" => Item::new(form, ItemKind::Defstruct(self.lower_defstruct(form))),
            "defstatic-schema" => Item::new(
                form,
                ItemKind::DefstaticSchema(self.lower_defstatic_schema(form)),
            ),
            "static-record" => {
                Item::new(form, ItemKind::StaticRecord(self.lower_static_record(form)))
            }
            "extern" => Item::new(form, ItemKind::Extern(self.lower_extern(form))),
            "defmacro" => Item::new(form, ItemKind::Defmacro(self.lower_macro(form))),
            "defn-for-syntax" => Item::new(
                form,
                ItemKind::DefnForSyntax(self.lower_function(
                    form,
                    FunctionPhase::Syntax,
                    true,
                    true,
                    false,
                )),
            ),
            _ => Item::new(form, ItemKind::Expr(self.lower_expr(form))),
        }
    }

    fn lower_import(&mut self, form: &Form, syntax: bool) -> Import {
        let info = NodeInfo::from_form(form);
        let parts = list_parts(form).unwrap_or_default();
        let module = match parts.get(1) {
            Some(part) => self.require_name(part, "import module"),
            None => {
                self.error(AST_MISSING_NAME, "import requires a module name", form.span);
                None
            }
        }
        .unwrap_or_else(error_name);
        let mut alias = None;
        let mut members = Vec::new();
        let mut index = 2;
        while index < parts.len() {
            let Some(keyword) = keyword_name(&parts[index]) else {
                self.error(
                    AST_INVALID_KEYWORD_ARGS,
                    "import options must use keyword clauses",
                    parts[index].span,
                );
                index += 1;
                continue;
            };
            let key = keyword.canonical.as_str();
            let Some(value) = parts.get(index + 1) else {
                self.error(
                    AST_EXPECTED_PAIR,
                    format!("import option `{}` requires a value", keyword.spelling),
                    parts[index].span,
                );
                break;
            };
            match key {
                ":as" => {
                    if alias.is_some() {
                        self.error(
                            AST_INVALID_KEYWORD_ARGS,
                            "duplicate `:as` option",
                            keyword_span(&parts[index]),
                        );
                    }
                    alias = self.require_name(value, "import alias");
                }
                ":refer" | ":only" => {
                    members.extend(self.lower_name_collection(value, "import member"));
                }
                _ => self.error(
                    AST_UNKNOWN_CLAUSE,
                    format!("unknown import option `{}`", keyword.spelling),
                    keyword_span(&parts[index]),
                ),
            }
            index += 2;
        }
        Import {
            span: info.span,
            metadata: info.metadata,
            module,
            alias,
            members,
            phase: if syntax {
                ImportPhase::Syntax
            } else {
                ImportPhase::Runtime
            },
        }
    }

    fn lower_py_import(&mut self, form: &Form) -> PyImport {
        let info = NodeInfo::from_form(form);
        let parts = list_parts(form).unwrap_or_default();
        let module = parts
            .get(1)
            .and_then(|part| match &part.kind {
                FormKind::String(value) => Some(value.clone()),
                FormKind::Symbol(name) => Some(name.spelling.clone()),
                _ => {
                    self.error(
                        AST_INVALID_NAME,
                        "Python import module must be a string or symbol",
                        part.span,
                    );
                    None
                }
            })
            .unwrap_or_else(|| {
                if parts.get(1).is_none() {
                    self.error(
                        AST_MISSING_NAME,
                        "Python import requires a module",
                        form.span,
                    );
                }
                String::new()
            });
        let mut alias = None;
        let mut index = 2;
        while index < parts.len() {
            let Some(keyword) = keyword_name(&parts[index]) else {
                self.error(
                    AST_INVALID_KEYWORD_ARGS,
                    "Python import options must use keyword clauses",
                    parts[index].span,
                );
                index += 1;
                continue;
            };
            let Some(value) = parts.get(index + 1) else {
                self.error(
                    AST_EXPECTED_PAIR,
                    format!(
                        "Python import option `{}` requires a value",
                        keyword.spelling
                    ),
                    parts[index].span,
                );
                break;
            };
            if keyword.canonical == ":as" {
                alias = self.require_name(value, "Python import alias");
            } else {
                self.error(
                    AST_UNKNOWN_CLAUSE,
                    format!("unknown Python import option `{}`", keyword.spelling),
                    parts[index].span,
                );
            }
            index += 2;
        }
        PyImport {
            span: info.span,
            metadata: info.metadata,
            module,
            alias,
        }
    }

    fn lower_export(&mut self, form: &Form) -> Export {
        let info = NodeInfo::from_form(form);
        let parts = list_parts(form).unwrap_or_default();
        let names = parts
            .get(1)
            .map(|value| self.lower_name_collection(value, "exported name"))
            .unwrap_or_else(|| {
                self.error(
                    AST_EXPECTED_VECTOR,
                    "export expects a vector or list of names",
                    form.span,
                );
                Vec::new()
            });
        if parts.len() > 2 {
            self.error(
                AST_WRONG_SHAPE,
                "export accepts one name collection",
                form.span,
            );
        }
        Export {
            span: info.span,
            metadata: info.metadata,
            names,
        }
    }

    fn lower_alias(&mut self, form: &Form) -> Alias {
        let info = NodeInfo::from_form(form);
        let parts = list_parts(form).unwrap_or_default();
        if parts.len() != 3 {
            self.error(
                AST_WRONG_SHAPE,
                "alias expects a local and target name",
                form.span,
            );
        }
        let local = parts
            .get(1)
            .and_then(|part| self.require_name(part, "alias local name"))
            .unwrap_or_else(error_name);
        let target = parts
            .get(2)
            .and_then(|part| self.require_name(part, "alias target name"))
            .unwrap_or_else(error_name);
        Alias {
            span: info.span,
            metadata: info.metadata,
            local,
            target,
        }
    }

    fn lower_def(&mut self, form: &Form) -> Def {
        let info = NodeInfo::from_form(form);
        let parts = list_parts(form).unwrap_or_default();
        if parts.len() < 2 || parts.len() > 4 {
            self.error(
                AST_WRONG_SHAPE,
                "def expects a name, optional type, and value",
                form.span,
            );
        }
        let name_form = parts.get(1);
        let name = name_form
            .and_then(|part| self.require_name(part, "def name"))
            .unwrap_or_else(error_name);
        let metadata = merge_declaration_metadata(
            info.metadata,
            name_form.map_or(&[], |name| name.metadata.as_slice()),
        );
        let (type_annotation, value) = match parts.len() {
            0..=2 => (None, parts.get(2).map(|part| self.lower_expr(part))),
            3 => (Some(self.lower_type(&parts[2])), None),
            _ => (
                Some(self.lower_type(&parts[2])),
                Some(self.lower_expr(&parts[3])),
            ),
        };
        if parts.len() == 3 {
            // A three-form declaration is overwhelmingly the common `(def n
            // value)` shape.  Treat it as a value unless the middle datum is
            // recognisably type-like. A call is also a list, so collection
            // shape alone cannot distinguish `(Array Float)` from
            // `(Point :x 1)`.
            let candidate = &parts[2];
            if !looks_like_type(candidate) {
                return Def {
                    span: info.span,
                    metadata,
                    name,
                    type_annotation: None,
                    value: Some(self.lower_expr(candidate)),
                };
            }
        }
        Def {
            span: info.span,
            metadata,
            name,
            type_annotation,
            value,
        }
    }

    fn lower_function(
        &mut self,
        form: &Form,
        phase: FunctionPhase,
        named: bool,
        body_required: bool,
        extern_declaration: bool,
    ) -> Function {
        let info = NodeInfo::from_form(form);
        let parts = list_parts(form).unwrap_or_default();
        let mut index = 1;
        let name_form = if named { parts.get(index) } else { None };
        let name = if named {
            let result = match parts.get(index) {
                Some(part) => self.require_name(part, "function name"),
                None => {
                    self.error(
                        AST_MISSING_NAME,
                        "function declaration requires a name",
                        form.span,
                    );
                    None
                }
            };
            index += 1;
            Some(result.unwrap_or_else(error_name))
        } else {
            None
        };
        let params_form = parts.get(index);
        if params_form.is_none() {
            self.error(
                AST_EXPECTED_VECTOR,
                "function expects a parameter vector",
                form.span,
            );
        }
        let params = params_form
            .map(|part| self.lower_params(part, phase))
            .unwrap_or_default();
        index += usize::from(params_form.is_some());

        let metadata_return_type = name_form
            .map(|name| self.lower_metadata_type(&name.metadata, "function return"))
            .unwrap_or_default();
        let explicit_return_type = if parts
            .get(index)
            .and_then(symbol_name)
            .is_some_and(|arrow| arrow.canonical == "->")
        {
            index += 1;
            match parts.get(index) {
                Some(type_form) => {
                    index += 1;
                    Some(self.lower_type(type_form))
                }
                None => {
                    self.error(AST_WRONG_SHAPE, "`->` requires a return type", form.span);
                    None
                }
            }
        } else {
            None
        };
        if explicit_return_type.is_some() && metadata_return_type.present {
            self.report_type_annotation_conflict(
                "function return",
                name_form.map_or(form.span, |name| name.span),
            );
        }
        let return_type = explicit_return_type.or(metadata_return_type.annotation);
        let contract = if extern_declaration
            && parts
                .get(index)
                .and_then(keyword_name)
                .is_some_and(|name| name.canonical.trim_start_matches(':') == "contract")
        {
            let clause = &parts[index];
            index += 1;
            match parts.get(index) {
                Some(contract) => {
                    index += 1;
                    self.lower_extern_contract(contract)
                }
                None => {
                    self.error(
                        AST_INVALID_CONTRACT,
                        "`:contract` requires a declaration map",
                        clause.span,
                    );
                    None
                }
            }
        } else {
            None
        };
        let body = if extern_declaration {
            if let Some(unexpected) = parts.get(index) {
                self.error(
                    AST_WRONG_SHAPE,
                    "extern function declaration cannot contain a body or extra clauses",
                    unexpected.span,
                );
            }
            Vec::new()
        } else {
            parts[index..]
                .iter()
                .map(|part| self.lower_expr(part))
                .collect::<Vec<_>>()
        };
        if body_required && body.is_empty() {
            self.error(AST_WRONG_SHAPE, "function body cannot be empty", form.span);
        }
        Function {
            span: info.span,
            metadata: info.metadata,
            name,
            params,
            return_type,
            contract,
            body,
            phase,
            phase_form: (phase != FunctionPhase::Runtime).then(|| form.clone()),
        }
    }

    fn lower_extern_contract(&mut self, form: &Form) -> Option<ExternContract> {
        let (entries, mut valid) = self.contract_entries(form, "extern contract");
        let mut id = None;
        let mut summaries = CallSummaries::unknown();
        for (key, value) in entries {
            match key.as_str() {
                "id" => match &value.kind {
                    FormKind::String(value)
                        if !value.is_empty()
                            && value.trim() == value
                            && !value.chars().any(char::is_control) =>
                    {
                        id = Some(value.clone());
                    }
                    _ => {
                        valid = false;
                        self.error(
                            AST_INVALID_CONTRACT,
                            "extern contract `:id` must be a non-empty stable string",
                            value.span,
                        );
                    }
                },
                "effects" => match self.lower_contract_effects(value) {
                    Some(effects) => summaries.effects = effects,
                    None => valid = false,
                },
                "temporal" => match self.lower_contract_temporal(value) {
                    Some(temporal) => summaries.temporal = temporal,
                    None => valid = false,
                },
                "data" => match self.lower_contract_data(value) {
                    Some(data) => summaries.data = data,
                    None => valid = false,
                },
                _ => {
                    valid = false;
                    self.error(
                        AST_UNKNOWN_CLAUSE,
                        format!("unknown extern contract field `:{key}`"),
                        value.span,
                    );
                }
            }
        }
        let Some(id) = id else {
            self.error(
                AST_INVALID_CONTRACT,
                "extern contract requires a stable `:id`",
                form.span,
            );
            return None;
        };
        valid.then_some(ExternContract {
            span: form.span,
            id,
            summaries,
        })
    }

    fn lower_contract_effects(&mut self, form: &Form) -> Option<EffectRow> {
        if let FormKind::Keyword(name) = &form.kind {
            return match name.canonical.trim_start_matches(':') {
                "pure" => Some(EffectRow::pure()),
                "unknown" => Some(EffectRow::unknown()),
                _ => {
                    self.error(
                        AST_INVALID_CONTRACT,
                        "contract `:effects` must be `:pure`, `:unknown`, or a vector",
                        form.span,
                    );
                    None
                }
            };
        }
        let FormKind::Vector(values) = &form.kind else {
            self.error(
                AST_INVALID_CONTRACT,
                "contract `:effects` must be `:pure`, `:unknown`, or a vector",
                form.span,
            );
            return None;
        };
        let mut effects = BTreeSet::new();
        let mut valid = true;
        for value in values {
            let Some(name) = contract_name(value) else {
                valid = false;
                self.error(
                    AST_INVALID_CONTRACT,
                    "contract effects must be keyword or symbol names",
                    value.span,
                );
                continue;
            };
            let effect = match name.as_str() {
                "io" => Effect::Io,
                "throw" => Effect::Throw,
                "mutation" => Effect::Mutation,
                "hidden-state" => Effect::HiddenState,
                "python-dynamic" => Effect::PythonDynamic,
                custom if custom.contains('/') => Effect::Custom(custom.to_owned()),
                _ => {
                    valid = false;
                    self.error(
                        AST_INVALID_CONTRACT,
                        format!("unknown contract effect `{name}`"),
                        value.span,
                    );
                    continue;
                }
            };
            if !effects.insert(effect) {
                valid = false;
                self.error(
                    AST_INVALID_CONTRACT,
                    format!("duplicate contract effect `{name}`"),
                    value.span,
                );
            }
        }
        valid.then_some(EffectRow {
            effects,
            open: false,
        })
    }

    fn lower_contract_temporal(&mut self, form: &Form) -> Option<TemporalSummary> {
        let (entries, mut valid) = self.contract_entries(form, "temporal contract");
        let mut summary = TemporalSummary::unknown();
        for (key, value) in entries {
            match key.as_str() {
                "past" => match self.lower_contract_bound(value) {
                    Some(bound) => summary.past = bound,
                    None => valid = false,
                },
                "future" => match self.lower_contract_bound(value) {
                    Some(bound) => summary.future = bound,
                    None => valid = false,
                },
                "availability" => match self.lower_contract_availability(value) {
                    Some(availability) => summary.availability = availability,
                    None => valid = false,
                },
                _ => {
                    valid = false;
                    self.error(
                        AST_UNKNOWN_CLAUSE,
                        format!("unknown temporal contract field `:{key}`"),
                        value.span,
                    );
                }
            }
        }
        valid.then_some(summary)
    }

    fn lower_contract_bound(&mut self, form: &Form) -> Option<TemporalBound> {
        match &form.kind {
            FormKind::Integer(value) => match value.parse::<u64>() {
                Ok(value) => Some(TemporalBound::Finite(value)),
                Err(_) => {
                    self.error(
                        AST_INVALID_CONTRACT,
                        "temporal bounds must be non-negative integers",
                        form.span,
                    );
                    None
                }
            },
            FormKind::String(value) if !value.is_empty() => {
                Some(TemporalBound::Symbolic(value.clone()))
            }
            FormKind::Symbol(name) if !name.canonical.is_empty() => {
                Some(TemporalBound::Symbolic(name.canonical.clone()))
            }
            FormKind::Keyword(name) => match name.canonical.trim_start_matches(':') {
                "unbounded" => Some(TemporalBound::Unbounded),
                "unknown" => Some(TemporalBound::Unknown),
                _ => {
                    self.error(
                        AST_INVALID_CONTRACT,
                        "temporal bound keyword must be `:unbounded` or `:unknown`",
                        form.span,
                    );
                    None
                }
            },
            _ => {
                self.error(
                    AST_INVALID_CONTRACT,
                    "temporal bound must be a non-negative integer, symbol, string, `:unbounded`, or `:unknown`",
                    form.span,
                );
                None
            }
        }
    }

    fn lower_contract_availability(&mut self, form: &Form) -> Option<Availability> {
        match &form.kind {
            FormKind::Keyword(name) => match name.canonical.trim_start_matches(':') {
                "immediate" => Some(Availability::Immediate),
                "unknown" => Some(Availability::Unknown),
                value if !value.is_empty() => Some(Availability::Named(value.to_owned())),
                _ => None,
            },
            FormKind::Symbol(name) if !name.canonical.is_empty() => {
                Some(Availability::Named(name.canonical.clone()))
            }
            FormKind::String(value) if !value.is_empty() => {
                Some(Availability::Named(value.clone()))
            }
            _ => {
                self.error(
                    AST_INVALID_CONTRACT,
                    "availability must be `:immediate`, `:unknown`, or a non-empty static name",
                    form.span,
                );
                None
            }
        }
    }

    fn lower_contract_data(&mut self, form: &Form) -> Option<DataProperties> {
        let (entries, mut valid) = self.contract_entries(form, "data contract");
        let mut data = DataProperties::unknown();
        for (key, value) in entries {
            match key.as_str() {
                "schema" => match contract_optional_name(value) {
                    Ok(schema) => data.schema = schema,
                    Err(()) => {
                        valid = false;
                        self.error(
                            AST_INVALID_CONTRACT,
                            "data `:schema` must be none or a static name",
                            value.span,
                        );
                    }
                },
                "axes" => match contract_optional_names(value) {
                    Ok(axes) => data.axes = axes,
                    Err(()) => {
                        valid = false;
                        self.error(
                            AST_INVALID_CONTRACT,
                            "data `:axes` must be none or a vector of static names",
                            value.span,
                        );
                    }
                },
                "alignment" => match contract_name(value).as_deref() {
                    Some("positional") => data.alignment = Alignment::Positional,
                    Some("labelled") => data.alignment = Alignment::Labelled,
                    Some("as-of") => data.alignment = Alignment::AsOf,
                    Some("unknown") => data.alignment = Alignment::Unknown,
                    _ => {
                        valid = false;
                        self.error(
                            AST_INVALID_CONTRACT,
                            "data `:alignment` must be :positional, :labelled, :as-of, or :unknown",
                            value.span,
                        );
                    }
                },
                "ordered-by" => match contract_optional_names(value) {
                    Ok(keys) => data.ordered_by = keys,
                    Err(()) => {
                        valid = false;
                        self.error(
                            AST_INVALID_CONTRACT,
                            "data `:ordered-by` must be none or a vector of static names",
                            value.span,
                        );
                    }
                },
                "unique-by" => match contract_optional_names(value) {
                    Ok(keys) => data.unique_by = keys,
                    Err(()) => {
                        valid = false;
                        self.error(
                            AST_INVALID_CONTRACT,
                            "data `:unique-by` must be none or a vector of static names",
                            value.span,
                        );
                    }
                },
                "preserves-length" => match contract_optional_bool(value) {
                    Ok(flag) => data.preserves_length = flag,
                    Err(()) => {
                        valid = false;
                        self.error(
                            AST_INVALID_CONTRACT,
                            "data `:preserves-length` must be Bool or none",
                            value.span,
                        );
                    }
                },
                "materializes" => match contract_optional_bool(value) {
                    Ok(flag) => data.materializes = flag,
                    Err(()) => {
                        valid = false;
                        self.error(
                            AST_INVALID_CONTRACT,
                            "data `:materializes` must be Bool or none",
                            value.span,
                        );
                    }
                },
                "reshapes" => match contract_optional_bool(value) {
                    Ok(flag) => data.reshapes = flag,
                    Err(()) => {
                        valid = false;
                        self.error(
                            AST_INVALID_CONTRACT,
                            "data `:reshapes` must be Bool or none",
                            value.span,
                        );
                    }
                },
                "nulls-possible" => match contract_optional_bool(value) {
                    Ok(flag) => data.nulls_possible = flag,
                    Err(()) => {
                        valid = false;
                        self.error(
                            AST_INVALID_CONTRACT,
                            "data `:nulls-possible` must be Bool or none",
                            value.span,
                        );
                    }
                },
                "nan-possible" => match contract_optional_bool(value) {
                    Ok(flag) => data.nan_possible = flag,
                    Err(()) => {
                        valid = false;
                        self.error(
                            AST_INVALID_CONTRACT,
                            "data `:nan-possible` must be Bool or none",
                            value.span,
                        );
                    }
                },
                "nonfinite-possible" => match contract_optional_bool(value) {
                    Ok(flag) => data.nonfinite_possible = flag,
                    Err(()) => {
                        valid = false;
                        self.error(
                            AST_INVALID_CONTRACT,
                            "data `:nonfinite-possible` must be Bool or none",
                            value.span,
                        );
                    }
                },
                "nonfinite-policy" => match contract_optional_name(value) {
                    Ok(policy) => data.nonfinite_policy = policy,
                    Err(()) => {
                        valid = false;
                        self.error(
                            AST_INVALID_CONTRACT,
                            "data `:nonfinite-policy` must be none or a static name",
                            value.span,
                        );
                    }
                },
                _ => {
                    valid = false;
                    self.error(
                        AST_UNKNOWN_CLAUSE,
                        format!("unknown data contract field `:{key}`"),
                        value.span,
                    );
                }
            }
        }
        valid.then_some(data)
    }

    fn contract_entries<'form>(
        &mut self,
        form: &'form Form,
        context: &str,
    ) -> (Vec<(String, &'form Form)>, bool) {
        let FormKind::Map(parts) = &form.kind else {
            self.error(
                AST_INVALID_CONTRACT,
                format!("{context} must be a map"),
                form.span,
            );
            return (Vec::new(), false);
        };
        let mut valid = true;
        if parts.len() % 2 != 0 {
            valid = false;
            self.error(
                AST_INVALID_CONTRACT,
                format!("{context} requires key/value pairs"),
                form.span,
            );
        }
        let mut seen = BTreeSet::new();
        let mut entries = Vec::new();
        for pair in parts.chunks_exact(2) {
            let FormKind::Keyword(name) = &pair[0].kind else {
                valid = false;
                self.error(
                    AST_INVALID_CONTRACT,
                    format!("{context} keys must be keywords"),
                    pair[0].span,
                );
                continue;
            };
            let key = name.canonical.trim_start_matches(':').to_owned();
            if !seen.insert(key.clone()) {
                valid = false;
                self.error(
                    AST_INVALID_CONTRACT,
                    format!("duplicate {context} field `:{key}`"),
                    pair[0].span,
                );
                continue;
            }
            entries.push((key, &pair[1]));
        }
        (entries, valid)
    }

    fn lower_macro(&mut self, form: &Form) -> Macro {
        let function = self.lower_function(form, FunctionPhase::Macro, true, true, false);
        Macro {
            span: function.span,
            metadata: function.metadata,
            name: function.name.unwrap_or_else(error_name),
            params: function.params,
            return_type: function.return_type,
            body: function.body,
            phase_form: function
                .phase_form
                .expect("macro lowering retains its phase-1 form"),
        }
    }

    fn lower_defstruct(&mut self, form: &Form) -> Defstruct {
        let info = NodeInfo::from_form(form);
        let parts = list_parts(form).unwrap_or_default();
        let (name, type_params, mut index) = match parts.get(1) {
            Some(Form {
                kind: FormKind::List(header),
                ..
            }) if !header.is_empty() => {
                let name = header
                    .first()
                    .and_then(|part| self.require_name(part, "struct name"))
                    .unwrap_or_else(error_name);
                let params = header[1..]
                    .iter()
                    .filter_map(|part| self.require_name(part, "struct type parameter"))
                    .collect::<Vec<_>>();
                (name, params, 2)
            }
            Some(part) => (
                self.require_name(part, "struct name")
                    .unwrap_or_else(error_name),
                Vec::new(),
                2,
            ),
            None => {
                self.error(AST_MISSING_NAME, "defstruct requires a name", form.span);
                (error_name(), Vec::new(), 1)
            }
        };
        let mut doc = None;
        let mut fields = Vec::new();
        let mut checks = Vec::new();
        while let Some(part) = parts.get(index) {
            match &part.kind {
                FormKind::String(value) if doc.is_none() => doc = Some(value.clone()),
                FormKind::String(_) => self.error(
                    AST_WRONG_SHAPE,
                    "defstruct accepts at most one documentation string",
                    part.span,
                ),
                FormKind::Vector(_) => fields.push(self.lower_field(part)),
                FormKind::List(clause)
                    if clause
                        .first()
                        .and_then(symbol_name)
                        .is_some_and(|name| name.canonical == "check") =>
                {
                    if clause.len() < 2 {
                        self.error(
                            AST_WRONG_SHAPE,
                            "struct check requires a condition",
                            part.span,
                        );
                    } else {
                        checks.push(StructCheck {
                            span: part.span,
                            metadata: part.metadata.clone(),
                            condition: self.lower_expr(&clause[1]),
                            message: clause.get(2).map(|message| self.lower_expr(message)),
                        });
                        if clause.len() > 3 {
                            self.error(
                                AST_WRONG_SHAPE,
                                "struct check accepts a condition and optional message",
                                part.span,
                            );
                        }
                    }
                }
                _ => self.error(
                    AST_UNKNOWN_CLAUSE,
                    "expected a field vector, documentation string, or check clause",
                    part.span,
                ),
            }
            index += 1;
        }
        Defstruct {
            span: info.span,
            metadata: info.metadata,
            name,
            type_params,
            doc,
            fields,
            checks,
        }
    }

    fn lower_field(&mut self, form: &Form) -> Field {
        let info = NodeInfo::from_form(form);
        let parts = match &form.kind {
            FormKind::Vector(parts) => parts.as_slice(),
            _ => &[],
        };
        if parts.is_empty() || parts.len() > 4 {
            self.error(
                AST_WRONG_SHAPE,
                "field expects a name, type, and optional default",
                form.span,
            );
        }
        let name = parts
            .first()
            .and_then(|part| self.require_name(part, "field name"))
            .unwrap_or_else(error_name);
        let mut type_annotation = None;
        let mut default = None;
        let mut index = 1;
        if let Some(part) = parts.get(index) {
            if is_equal_symbol(part) {
                self.error(
                    AST_WRONG_SHAPE,
                    "field default requires a type before `=`",
                    part.span,
                );
            } else {
                type_annotation = Some(self.lower_type(part));
                index += 1;
            }
        }
        if parts.get(index).is_some_and(is_equal_symbol) {
            index += 1;
            if let Some(value) = parts.get(index) {
                default = Some(self.lower_expr(value));
                index += 1;
            } else {
                self.error(
                    AST_WRONG_SHAPE,
                    "field `=` requires a default expression",
                    form.span,
                );
            }
        }
        if index < parts.len() {
            self.error(
                AST_WRONG_SHAPE,
                "unexpected forms after field declaration",
                form.span,
            );
        }
        Field {
            span: info.span,
            metadata: info.metadata,
            name,
            type_annotation,
            default,
        }
    }

    fn lower_defstatic_schema(&mut self, form: &Form) -> DefstaticSchema {
        let info = NodeInfo::from_form(form);
        let parts = list_parts(form).unwrap_or_default();
        let name = parts
            .get(1)
            .and_then(|part| self.require_name(part, "schema name"))
            .unwrap_or_else(error_name);
        if parts.len() < 2 {
            self.error(
                AST_MISSING_NAME,
                "defstatic-schema requires a name",
                form.span,
            );
        }
        let body = parts[2..]
            .iter()
            .map(|part| self.lower_expr(part))
            .collect();
        DefstaticSchema {
            span: info.span,
            metadata: info.metadata,
            name,
            body,
        }
    }

    fn lower_static_record(&mut self, form: &Form) -> StaticRecord {
        let info = NodeInfo::from_form(form);
        let parts = list_parts(form).unwrap_or_default();
        let schema = parts
            .get(1)
            .and_then(|part| self.require_name(part, "record schema"))
            .unwrap_or_else(error_name);
        let owner = parts
            .get(2)
            .and_then(|part| self.require_name(part, "record owner"))
            .unwrap_or_else(error_name);
        let tail = parts.get(3..).unwrap_or_default();
        let fields = match tail {
            [
                Form {
                    kind: FormKind::Map(entries),
                    span,
                    ..
                },
            ] => self.lower_static_record_fields(entries, *span),
            [
                Form {
                    kind: FormKind::Map(entries),
                    span,
                    ..
                },
                ..,
            ] => {
                self.error(
                    AST_WRONG_SHAPE,
                    "static-record accepts exactly one field map",
                    form.span,
                );
                self.lower_static_record_fields(entries, *span)
            }
            [] => {
                self.error(
                    AST_WRONG_SHAPE,
                    "static-record expects a schema, owner, and one field map",
                    form.span,
                );
                Vec::new()
            }
            _ => {
                // Keep lowering the pre-map spelling so an editor can still
                // inspect its fields, while making the canonical shape clear.
                self.error(
                    AST_WRONG_SHAPE,
                    "static-record fields must be provided as a single map",
                    form.span,
                );
                self.lower_static_record_fields(tail, form.span)
            }
        };
        StaticRecord {
            span: info.span,
            metadata: info.metadata,
            schema,
            owner,
            fields,
        }
    }

    fn lower_static_record_fields(&mut self, entries: &[Form], span: Span) -> Vec<(Name, Expr)> {
        if entries.len() % 2 != 0 {
            self.error(
                AST_EXPECTED_PAIR,
                "static-record fields require key/value pairs",
                span,
            );
        }
        entries
            .chunks(2)
            .filter_map(|pair| {
                let key = pair
                    .first()
                    .and_then(|part| self.require_record_field(part))?;
                let value = pair.get(1).map(|part| self.lower_expr(part))?;
                Some((key, value))
            })
            .collect()
    }

    fn require_record_field(&mut self, form: &Form) -> Option<Name> {
        match &form.kind {
            FormKind::Keyword(name) | FormKind::Symbol(name) => Some(name.clone()),
            _ => {
                self.error(
                    AST_INVALID_NAME,
                    "record field must be a keyword or symbol",
                    form.span,
                );
                None
            }
        }
    }

    fn lower_extern(&mut self, form: &Form) -> Extern {
        let info = NodeInfo::from_form(form);
        let parts = list_parts(form).unwrap_or_default();
        if parts.len() < 3 {
            self.error(
                AST_WRONG_SHAPE,
                "extern expects a backend, module, and declarations",
                form.span,
            );
        }
        let backend = match parts.get(1) {
            Some(part) => self.require_name(part, "extern backend"),
            None => {
                self.error(AST_MISSING_NAME, "extern requires a backend", form.span);
                None
            }
        }
        .unwrap_or_else(error_name);
        let module = parts
            .get(2)
            .and_then(|part| match &part.kind {
                FormKind::String(value) => Some(value.clone()),
                FormKind::Symbol(name) => Some(name.spelling.clone()),
                _ => {
                    self.error(
                        AST_INVALID_NAME,
                        "extern module must be a string or symbol",
                        part.span,
                    );
                    None
                }
            })
            .unwrap_or_else(|| {
                if parts.get(2).is_none() {
                    self.error(AST_MISSING_NAME, "extern requires a module", form.span);
                }
                String::new()
            });
        let mut items = Vec::new();
        for declaration in parts.get(3..).unwrap_or_default() {
            let item = if self.is_head(declaration, "defn") {
                Item::new(
                    declaration,
                    ItemKind::Defn(self.lower_function(
                        declaration,
                        FunctionPhase::Runtime,
                        true,
                        false,
                        true,
                    )),
                )
            } else {
                self.lower_item(declaration)
            };
            items.push(item);
        }
        Extern {
            span: info.span,
            metadata: info.metadata,
            backend,
            module,
            items,
        }
    }

    fn lower_expr(&mut self, form: &Form) -> Expr {
        let kind = match &form.kind {
            FormKind::None => ExprKind::None,
            FormKind::Bool(value) => ExprKind::Bool(*value),
            FormKind::Integer(value) => ExprKind::Integer(value.clone()),
            FormKind::Float(value) => ExprKind::Float(value.clone()),
            FormKind::String(value) => ExprKind::String(value.clone()),
            FormKind::Keyword(name) => ExprKind::Keyword(name.clone()),
            FormKind::Symbol(name) => ExprKind::Name(name.clone()),
            FormKind::List(parts) => return self.lower_list_expr(form, parts),
            FormKind::Vector(parts) => {
                ExprKind::Vector(parts.iter().map(|part| self.lower_expr(part)).collect())
            }
            FormKind::Map(parts) => self.lower_map_expr(form, parts),
            FormKind::Set(parts) => {
                ExprKind::Set(parts.iter().map(|part| self.lower_expr(part)).collect())
            }
            FormKind::ReaderMacro {
                macro_kind,
                form: inner,
            } => {
                let expression = Box::new(self.lower_expr(inner));
                match macro_kind {
                    crate::syntax::ReaderMacroKind::Quote => ExprKind::Quote(expression),
                    crate::syntax::ReaderMacroKind::SyntaxQuote => {
                        ExprKind::SyntaxQuote(expression)
                    }
                    crate::syntax::ReaderMacroKind::Unquote => ExprKind::Unquote(expression),
                    crate::syntax::ReaderMacroKind::UnquoteSplicing => {
                        ExprKind::UnquoteSplicing(expression)
                    }
                }
            }
            FormKind::Error(message) => ExprKind::Error(message.clone()),
        };
        Expr::from_form(form, kind)
    }

    fn lower_list_expr(&mut self, form: &Form, parts: &[Form]) -> Expr {
        if parts.is_empty() {
            return Expr::from_form(
                form,
                ExprKind::List(parts.iter().map(|part| self.lower_expr(part)).collect()),
            );
        }
        let Some(head) = parts.first().and_then(symbol_name) else {
            return self.lower_call_expr(form, parts);
        };
        match head.canonical.as_str() {
            "fn" => self.lower_fn_expr(form, parts),
            "let" => self.lower_let_expr(form, parts),
            "if" => self.lower_if_expr(form, parts),
            "do" => Expr::from_form(
                form,
                ExprKind::Do(
                    parts[1..]
                        .iter()
                        .map(|part| self.lower_expr(part))
                        .collect(),
                ),
            ),
            "try" => self.lower_try_expr(form, parts),
            "raise" => self.lower_raise_expr(form, parts),
            _ => self.lower_call_expr(form, parts),
        }
    }

    fn lower_call_expr(&mut self, form: &Form, parts: &[Form]) -> Expr {
        let callee_form = parts.first().unwrap_or(form);
        let callee = Box::new(self.lower_expr(callee_form));
        let mut args = Vec::new();
        let mut positional = Vec::new();
        let mut keywords = Vec::new();
        let mut index = 1;
        while index < parts.len() {
            let part = &parts[index];
            if let Some(key) = keyword_name(part) {
                let Some(value_form) = parts.get(index + 1) else {
                    self.error(
                        AST_EXPECTED_PAIR,
                        format!("keyword argument `{}` requires a value", key.spelling),
                        part.span,
                    );
                    break;
                };
                let value = self.lower_expr(value_form);
                let argument = KeywordArg {
                    span: part.span.cover(value_form.span),
                    metadata: part.metadata.clone(),
                    key,
                    value,
                };
                keywords.push(argument.clone());
                args.push(CallArg::Keyword(argument));
                index += 2;
            } else {
                let value = self.lower_expr(part);
                positional.push(value.clone());
                args.push(CallArg::Positional(value));
                index += 1;
            }
        }
        let info = NodeInfo::from_form(form);
        Expr::from_form(
            form,
            ExprKind::Call(CallExpr {
                span: info.span,
                metadata: info.metadata,
                callee,
                args,
                positional,
                keywords,
            }),
        )
    }

    fn lower_fn_expr(&mut self, form: &Form, parts: &[Form]) -> Expr {
        let info = NodeInfo::from_form(form);
        let params_form = parts.get(1);
        let params = params_form
            .map(|part| self.lower_params(part, FunctionPhase::Runtime))
            .unwrap_or_else(|| {
                self.error(
                    AST_EXPECTED_VECTOR,
                    "fn expects a parameter vector",
                    form.span,
                );
                Vec::new()
            });
        let mut index = if params_form.is_some() { 2 } else { 1 };
        let metadata_return_type = params_form
            .map(|params| self.lower_metadata_type(&params.metadata, "function return"))
            .unwrap_or_default();
        let explicit_return_type = self.take_return_type(parts, &mut index);
        if explicit_return_type.is_some() && metadata_return_type.present {
            self.report_type_annotation_conflict(
                "function return",
                params_form.map_or(form.span, |params| params.span),
            );
        }
        let return_type = explicit_return_type.or(metadata_return_type.annotation);
        let body = parts[index..]
            .iter()
            .map(|part| self.lower_expr(part))
            .collect::<Vec<_>>();
        if body.is_empty() {
            self.error(AST_WRONG_SHAPE, "fn body cannot be empty", form.span);
        }
        Expr::from_form(
            form,
            ExprKind::Fn(FnExpr {
                span: info.span,
                metadata: info.metadata,
                params,
                return_type,
                body,
            }),
        )
    }

    fn take_return_type(&mut self, parts: &[Form], index: &mut usize) -> Option<TypeExpr> {
        if parts
            .get(*index)
            .and_then(symbol_name)
            .is_some_and(|name| name.canonical == "->")
        {
            *index += 1;
            match parts.get(*index) {
                Some(form) => {
                    *index += 1;
                    Some(self.lower_type(form))
                }
                None => {
                    self.error(
                        AST_WRONG_SHAPE,
                        "`->` requires a return type",
                        Span::default(),
                    );
                    None
                }
            }
        } else {
            None
        }
    }

    fn lower_let_expr(&mut self, form: &Form, parts: &[Form]) -> Expr {
        let bindings = match parts.get(1) {
            Some(Form {
                kind: FormKind::Vector(bindings),
                ..
            }) => self.lower_bindings(bindings),
            Some(binding_form) => {
                self.error(
                    AST_EXPECTED_VECTOR,
                    "let expects a binding vector",
                    binding_form.span,
                );
                Vec::new()
            }
            None => {
                self.error(
                    AST_EXPECTED_VECTOR,
                    "let expects a binding vector",
                    form.span,
                );
                Vec::new()
            }
        };
        let body_start = usize::from(parts.get(1).is_some()) + 1;
        let body = parts
            .get(body_start..)
            .unwrap_or_default()
            .iter()
            .map(|part| self.lower_expr(part))
            .collect::<Vec<_>>();
        if body.is_empty() {
            self.error(AST_WRONG_SHAPE, "let body cannot be empty", form.span);
        }
        Expr::from_form(form, ExprKind::Let { bindings, body })
    }

    fn lower_bindings(&mut self, forms: &[Form]) -> Vec<Binding> {
        if forms.len() % 2 != 0 {
            self.error(
                AST_EXPECTED_PAIR,
                "let bindings require pattern/value pairs",
                forms.last().map_or(Span::default(), |form| form.span),
            );
        }
        forms
            .chunks(2)
            .filter_map(|pair| {
                let pattern_form = pair.first()?;
                let value_form = pair.get(1)?;
                let value = self.lower_expr(value_form);
                let metadata_type =
                    self.lower_metadata_type(&pattern_form.metadata, "local binding");
                Some(Binding {
                    span: pattern_form.span.cover(value_form.span),
                    metadata: pattern_form.metadata.clone(),
                    pattern: self.lower_pattern(pattern_form),
                    type_annotation: metadata_type.annotation,
                    value,
                })
            })
            .collect()
    }

    fn lower_if_expr(&mut self, form: &Form, parts: &[Form]) -> Expr {
        if !(3..=4).contains(&parts.len()) {
            self.error(
                AST_WRONG_SHAPE,
                "if expects condition, then branch, and optional else branch",
                form.span,
            );
        }
        let condition = parts
            .get(1)
            .map(|part| self.lower_expr(part))
            .unwrap_or_else(|| self.error_expr(form.span, "missing if condition"));
        let then_branch = parts
            .get(2)
            .map(|part| self.lower_expr(part))
            .unwrap_or_else(|| self.error_expr(form.span, "missing if then branch"));
        let else_branch = parts.get(3).map(|part| Box::new(self.lower_expr(part)));
        Expr::from_form(
            form,
            ExprKind::If {
                condition: Box::new(condition),
                then_branch: Box::new(then_branch),
                else_branch,
            },
        )
    }

    fn lower_try_expr(&mut self, form: &Form, parts: &[Form]) -> Expr {
        let info = NodeInfo::from_form(form);
        let mut body = Vec::new();
        let mut catches = Vec::new();
        let mut finally_body = None;
        let mut saw_finally = false;
        for part in parts.iter().skip(1) {
            let Some(clause) = list_parts(part) else {
                body.push(self.lower_expr(part));
                continue;
            };
            let Some(kind) = clause.first().and_then(symbol_name) else {
                body.push(self.lower_expr(part));
                continue;
            };
            match kind.canonical.as_str() {
                "catch" => {
                    if saw_finally {
                        self.error(
                            AST_WRONG_SHAPE,
                            "catch clauses must appear before finally",
                            part.span,
                        );
                    }
                    catches.push(self.lower_catch(part, clause));
                }
                "finally" => {
                    if saw_finally {
                        self.error(
                            AST_WRONG_SHAPE,
                            "try accepts at most one finally clause",
                            part.span,
                        );
                        continue;
                    }
                    saw_finally = true;
                    let body = clause[1..]
                        .iter()
                        .map(|form| self.lower_expr(form))
                        .collect::<Vec<_>>();
                    if body.is_empty() {
                        self.error(AST_WRONG_SHAPE, "finally body cannot be empty", part.span);
                    }
                    finally_body = Some(body);
                }
                _ => body.push(self.lower_expr(part)),
            }
        }
        if body.is_empty() {
            self.error(AST_WRONG_SHAPE, "try body cannot be empty", form.span);
        }
        Expr::from_form(
            form,
            ExprKind::Try(TryExpr {
                span: info.span,
                metadata: info.metadata,
                body,
                catches,
                finally_body,
            }),
        )
    }

    fn lower_catch(&mut self, form: &Form, clause: &[Form]) -> CatchClause {
        let info = NodeInfo::from_form(form);
        if clause.len() < 3 {
            self.error(
                AST_WRONG_SHAPE,
                "catch expects exception type, binding, and body",
                form.span,
            );
        }
        let exception_type = clause.get(1).map(|part| self.lower_type(part));
        let binding = clause.get(2).and_then(|part| {
            self.require_name(part, "catch binding").map(|name| Param {
                span: part.span,
                metadata: part.metadata.clone(),
                name,
                pattern: None,
                type_annotation: None,
                default: None,
                variadic: false,
            })
        });
        if clause.get(2).is_some() && binding.is_none() {
            self.error(
                AST_INVALID_NAME,
                "catch binding must be a symbol",
                clause[2].span,
            );
        }
        // Keep malformed catch clauses recoverable.  The shape diagnostic
        // above is sufficient; never panic while slicing a short clause.
        let body = clause
            .get(3..)
            .unwrap_or(&[])
            .iter()
            .map(|part| self.lower_expr(part))
            .collect::<Vec<_>>();
        if body.is_empty() {
            self.error(AST_WRONG_SHAPE, "catch body cannot be empty", form.span);
        }
        CatchClause {
            span: info.span,
            metadata: info.metadata,
            exception_type,
            binding,
            body,
        }
    }

    fn lower_raise_expr(&mut self, form: &Form, parts: &[Form]) -> Expr {
        if parts.len() > 2 {
            self.error(
                AST_WRONG_SHAPE,
                "raise accepts at most one expression",
                form.span,
            );
        }
        Expr::from_form(
            form,
            ExprKind::Raise(parts.get(1).map(|part| Box::new(self.lower_expr(part)))),
        )
    }

    fn lower_params(&mut self, form: &Form, phase: FunctionPhase) -> Vec<Param> {
        let parts = match &form.kind {
            FormKind::Vector(parts) => parts.as_slice(),
            _ => {
                self.error(
                    AST_EXPECTED_VECTOR,
                    "parameters must be a vector",
                    form.span,
                );
                return Vec::new();
            }
        };
        let mut params = Vec::new();
        let mut index = 0;
        let mut next_is_variadic = false;
        let mut saw_variadic = false;
        while let Some(part) = parts.get(index) {
            if is_ampersand_symbol(part) {
                if saw_variadic || next_is_variadic {
                    self.error(
                        AST_WRONG_SHAPE,
                        "parameter vector contains duplicate &",
                        part.span,
                    );
                }
                saw_variadic = true;
                next_is_variadic = true;
                index += 1;
                if parts.get(index).is_none() {
                    self.error(
                        AST_WRONG_SHAPE,
                        "& requires a variadic parameter",
                        part.span,
                    );
                }
                continue;
            }
            let (param, consumed) = if has_type_marker(part) {
                let annotation = match parse_type(part, &BTreeMap::new()) {
                    Ok(_) => {
                        let mut annotation = self.lower_metadata_type_form(part);
                        annotation.metadata = part.metadata.clone();
                        Some(annotation)
                    }
                    Err(error) => {
                        self.error(
                            AST_INVALID_TYPE_METADATA,
                            format!("invalid parameter type after `^:type`: {error}"),
                            error.span,
                        );
                        None
                    }
                };
                match parts.get(index + 1) {
                    Some(name_form) if symbol_name(name_form).is_some() => {
                        let mut param = self.lower_param(name_form, next_is_variadic, phase);
                        if param.type_annotation.is_some() {
                            self.error(
                                AST_CONFLICTING_TYPE_ANNOTATION,
                                "parameter has both an adjacent `^:type Type` prefix and directly attached type metadata; directly attached metadata takes precedence",
                                name_form.span,
                            );
                        } else {
                            param.type_annotation = annotation;
                        }
                        (param, 2)
                    }
                    Some(unexpected) => {
                        self.error(
                            AST_INVALID_TYPE_METADATA,
                            "a `^:type Type` parameter prefix must be followed by a parameter name",
                            unexpected.span,
                        );
                        (self.lower_param(unexpected, next_is_variadic, phase), 2)
                    }
                    None => {
                        self.error(
                            AST_INVALID_TYPE_METADATA,
                            "a `^:type Type` parameter prefix must be followed by a parameter name",
                            part.span,
                        );
                        break;
                    }
                }
            } else {
                (self.lower_param(part, next_is_variadic, phase), 1)
            };
            params.push(param);
            if next_is_variadic && index + consumed < parts.len() {
                self.error(
                    AST_WRONG_SHAPE,
                    "variadic parameter must be the final parameter",
                    part.span,
                );
            }
            next_is_variadic = false;
            index += consumed;
        }
        params
    }

    fn lower_param(&mut self, form: &Form, variadic: bool, phase: FunctionPhase) -> Param {
        let info = NodeInfo::from_form(form);
        // Syntax-quoted macro templates may splice parameter names into a
        // generated runtime function, e.g. ``(fn [~time ~previous] ...)``.
        // The reader macro wrapper is intentional syntax data, not a runtime
        // value; retain the spliced symbol as the provisional parameter name
        // so AST validation does not reject an otherwise valid template.
        if phase == FunctionPhase::Runtime
            && let FormKind::ReaderMacro {
                macro_kind: crate::syntax::ReaderMacroKind::Unquote,
                form: inner,
            } = &form.kind
            && let Some(name) = symbol_name(inner)
        {
            return Param {
                span: info.span,
                metadata: info.metadata,
                name,
                pattern: None,
                type_annotation: None,
                default: None,
                variadic,
            };
        }
        if let Some((pattern_form, declaration_tail)) = destructured_parameter_parts(form, phase) {
            return self.lower_destructured_param(
                form,
                &info,
                pattern_form,
                declaration_tail,
                variadic,
                phase,
            );
        }
        let parts = match &form.kind {
            FormKind::Vector(parts) => parts.as_slice(),
            _ => std::slice::from_ref(form),
        };
        if parts.is_empty() || parts.len() > 4 {
            self.error(
                AST_WRONG_SHAPE,
                "parameter expects a name, type, and optional default",
                form.span,
            );
        }
        let name = parts
            .first()
            .and_then(|part| self.require_name(part, "parameter name"))
            .unwrap_or_else(error_name);
        let target_form = parts.first().unwrap_or(form);
        let mut metadata_type = self.lower_metadata_type(&target_form.metadata, "parameter");
        if !std::ptr::eq(target_form, form) {
            let wrapper_type = self.lower_metadata_type(&form.metadata, "parameter");
            if metadata_type.present && wrapper_type.present {
                self.error(
                    AST_CONFLICTING_TYPE_ANNOTATION,
                    "parameter type metadata appears on both its declaration and name; name metadata takes precedence",
                    target_form.span,
                );
            } else if wrapper_type.present {
                metadata_type = wrapper_type;
            }
        }
        let mut type_annotation = metadata_type.annotation;
        let mut default = None;
        let mut index = 1;
        if let Some(part) = parts.get(index) {
            if !is_equal_symbol(part) {
                if metadata_type.present {
                    self.report_type_annotation_conflict("parameter", part.span);
                }
                type_annotation = Some(self.lower_type(part));
                index += 1;
            }
        }
        if parts.get(index).is_some_and(is_equal_symbol) {
            index += 1;
            if let Some(value) = parts.get(index) {
                default = Some(self.lower_expr(value));
                index += 1;
            } else {
                self.error(
                    AST_WRONG_SHAPE,
                    "parameter = requires a default expression",
                    form.span,
                );
            }
        }
        if index < parts.len() {
            self.error(
                AST_WRONG_SHAPE,
                "unexpected forms after parameter declaration; wrap a runtime destructuring pattern in an extra vector layer",
                form.span,
            );
        }
        Param {
            span: info.span,
            metadata: info.metadata,
            name,
            pattern: None,
            type_annotation,
            default,
            variadic,
        }
    }

    fn lower_destructured_param(
        &mut self,
        form: &Form,
        info: &NodeInfo,
        pattern_form: &Form,
        parts: &[Form],
        variadic: bool,
        phase: FunctionPhase,
    ) -> Param {
        let mut metadata_type = self.lower_metadata_type(&pattern_form.metadata, "parameter");
        if !std::ptr::eq(pattern_form, form) {
            let wrapper_type = self.lower_metadata_type(&form.metadata, "parameter");
            if metadata_type.present && wrapper_type.present {
                self.error(
                    AST_CONFLICTING_TYPE_ANNOTATION,
                    "parameter type metadata appears on both its declaration and pattern; pattern metadata takes precedence",
                    pattern_form.span,
                );
            } else if wrapper_type.present {
                metadata_type = wrapper_type;
            }
        }
        let mut type_annotation = metadata_type.annotation;
        let mut default = None;
        let mut index = 0;
        if let Some(part) = parts.get(index)
            && !is_equal_symbol(part)
        {
            if metadata_type.present {
                self.report_type_annotation_conflict("parameter", part.span);
            }
            type_annotation = Some(self.lower_type(part));
            index += 1;
        }
        if parts.get(index).is_some_and(is_equal_symbol) {
            index += 1;
            if let Some(value) = parts.get(index) {
                default = Some(self.lower_expr(value));
                index += 1;
            } else {
                self.error(
                    AST_WRONG_SHAPE,
                    "destructured parameter = requires a default expression",
                    form.span,
                );
            }
        }
        if index < parts.len() {
            self.error(
                AST_WRONG_SHAPE,
                "unexpected forms after destructured parameter declaration",
                form.span,
            );
        }
        if phase == FunctionPhase::Runtime
            && matches!(pattern_form.kind, FormKind::Vector(_))
            && type_annotation.is_none()
        {
            self.error(
                AST_WRONG_SHAPE,
                "runtime vector destructuring requires an explicit type; use `[[...] (Vector T)]` or `[[...] Any]`",
                form.span,
            );
        }
        let parameter_index = self.next_pattern_parameter;
        self.next_pattern_parameter += 1;
        let spelling = format!("\0arg{parameter_index}");
        Param {
            span: info.span,
            metadata: info.metadata.clone(),
            name: Name {
                spelling: spelling.clone(),
                canonical: spelling,
            },
            pattern: Some(self.lower_pattern(pattern_form)),
            type_annotation,
            default,
            variadic,
        }
    }

    fn lower_type(&mut self, form: &Form) -> TypeExpr {
        let kind = match &form.kind {
            FormKind::Symbol(name) => TypeExprKind::Name(name.clone()),
            FormKind::List(parts) if parts.is_empty() => TypeExprKind::Literal(form.clone()),
            FormKind::List(parts) => {
                let constructor = Box::new(self.lower_type(&parts[0]));
                let args = parts[1..]
                    .iter()
                    .map(|part| self.lower_type(part))
                    .collect::<Vec<_>>();
                if matches!(
                    &constructor.kind,
                    TypeExprKind::Name(name) if name.canonical == "Fn"
                ) {
                    let parameter_form = parts.get(1);
                    let return_form = match parts.get(2..) {
                        Some([arrow, result])
                            if symbol_name(arrow).is_some_and(|name| name.canonical == "->") =>
                        {
                            Some(result)
                        }
                        Some([result]) => Some(result),
                        _ => None,
                    };
                    if let (
                        Some(Form {
                            kind: FormKind::Vector(parameters),
                            ..
                        }),
                        Some(return_form),
                    ) = (parameter_form, return_form)
                    {
                        return TypeExpr::from_form(
                            form,
                            TypeExprKind::Function {
                                parameters: parameters
                                    .iter()
                                    .map(|parameter| self.lower_type(parameter))
                                    .collect(),
                                return_type: Box::new(self.lower_type(return_form)),
                            },
                        );
                    }
                }
                match constructor.kind {
                    TypeExprKind::Name(ref name) if name.canonical == "Union" => {
                        TypeExprKind::Union(args)
                    }
                    TypeExprKind::Name(ref name) if name.canonical == "Tuple" => {
                        TypeExprKind::Tuple(args)
                    }
                    _ => TypeExprKind::Apply { constructor, args },
                }
            }
            FormKind::Vector(_) | FormKind::Map(_) | FormKind::Set(_) => {
                TypeExprKind::Literal(form.clone())
            }
            _ => TypeExprKind::Literal(form.clone()),
        };
        TypeExpr::from_form(form, kind)
    }

    fn lower_pattern(&mut self, form: &Form) -> Pattern {
        let kind = match &form.kind {
            FormKind::Symbol(name) if name.canonical == "_" => PatternKind::Ignore,
            FormKind::Symbol(name) => PatternKind::Name(name.clone()),
            FormKind::Vector(parts) => {
                PatternKind::Vector(parts.iter().map(|part| self.lower_pattern(part)).collect())
            }
            FormKind::Map(parts) => {
                if parts.len() % 2 != 0 {
                    self.error(
                        AST_EXPECTED_PAIR,
                        "map pattern requires key/value pairs",
                        form.span,
                    );
                }
                PatternKind::Map(
                    parts
                        .chunks(2)
                        .filter_map(|pair| {
                            Some((
                                self.lower_pattern(pair.first()?),
                                self.lower_pattern(pair.get(1)?),
                            ))
                        })
                        .collect(),
                )
            }
            _ => PatternKind::Literal(form.clone()),
        };
        Pattern {
            span: form.span,
            metadata: form.metadata.clone(),
            kind,
        }
    }

    fn lower_map_expr(&mut self, form: &Form, parts: &[Form]) -> ExprKind {
        if parts.len() % 2 != 0 {
            self.error(
                AST_EXPECTED_PAIR,
                "map expression requires key/value pairs",
                form.span,
            );
        }
        ExprKind::Map(
            parts
                .chunks(2)
                .filter_map(|pair| {
                    Some((
                        self.lower_expr(pair.first()?),
                        self.lower_expr(pair.get(1)?),
                    ))
                })
                .collect(),
        )
    }

    fn lower_name_collection(&mut self, form: &Form, what: &str) -> Vec<Name> {
        let parts = match &form.kind {
            FormKind::Vector(parts) | FormKind::List(parts) | FormKind::Set(parts) => parts,
            _ => {
                self.error(
                    AST_EXPECTED_VECTOR,
                    format!("{what} collection must be a vector or list"),
                    form.span,
                );
                return Vec::new();
            }
        };
        parts
            .iter()
            .filter_map(|part| self.require_name(part, what))
            .collect()
    }

    fn require_name(&mut self, form: &Form, what: &str) -> Option<Name> {
        if let Some(name) = template_symbol_name(form) {
            return Some(name);
        }
        self.error(
            AST_INVALID_NAME,
            format!("{what} must be a symbol"),
            form.span,
        );
        None
    }

    fn error_expr(&mut self, span: Span, message: &str) -> Expr {
        self.error(AST_WRONG_SHAPE, message, span);
        Expr {
            span,
            metadata: Vec::new(),
            kind: ExprKind::Error(message.to_owned()),
        }
    }
}

fn list_parts(form: &Form) -> Option<&[Form]> {
    match &form.kind {
        FormKind::List(parts) => Some(parts),
        _ => None,
    }
}

fn destructured_parameter_parts(form: &Form, phase: FunctionPhase) -> Option<(&Form, &[Form])> {
    match &form.kind {
        FormKind::Map(_) => Some((form, &[] as &[Form])),
        FormKind::Vector(parts) if phase != FunctionPhase::Runtime => Some((form, &parts[..0])),
        FormKind::Vector(parts) if !ordinary_runtime_parameter_declaration(parts) => {
            // Runtime destructuring uses one explicit wrapper layer. This
            // makes `[[left right] Type]` independent of whether `Type`
            // starts with an uppercase, a lowercase, or a non-Latin
            // character. Runtime vector patterns are required to provide the
            // tail type; phase-1 patterns remain implicitly Syntax-typed.
            if let Some(pattern) = parts
                .first()
                .filter(|part| matches!(part.kind, FormKind::Vector(_) | FormKind::Map(_)))
            {
                Some((pattern, &parts[1..]))
            } else {
                Some((form, &parts[..0]))
            }
        }
        _ => None,
    }
}

fn ordinary_runtime_parameter_declaration(parts: &[Form]) -> bool {
    parts
        .first()
        .is_some_and(|part| template_symbol_name(part).is_some())
}

/// Returns a symbol used directly or supplied through a syntax-quote
/// unquote.  The latter is still syntax data while a macro declaration is
/// lowered, but it denotes the generated declaration's name.
fn template_symbol_name(form: &Form) -> Option<Name> {
    match &form.kind {
        FormKind::Symbol(name) => Some(name.clone()),
        FormKind::ReaderMacro {
            macro_kind: crate::syntax::ReaderMacroKind::Unquote,
            form: inner,
        } => template_symbol_name(inner),
        _ => None,
    }
}

fn symbol_name(form: &Form) -> Option<Name> {
    match &form.kind {
        FormKind::Symbol(name) => Some(name.clone()),
        _ => None,
    }
}

fn keyword_name(form: &Form) -> Option<Name> {
    match &form.kind {
        FormKind::Keyword(name) => Some(name.clone()),
        _ => None,
    }
}

fn keyword_span(form: &Form) -> Span {
    form.span
}

fn contract_name(form: &Form) -> Option<String> {
    match &form.kind {
        FormKind::Keyword(name) => Some(name.canonical.trim_start_matches(':').to_owned()),
        FormKind::Symbol(name) => Some(name.canonical.clone()),
        FormKind::String(value) => Some(value.clone()),
        _ => None,
    }
}

fn contract_optional_name(form: &Form) -> Result<Option<String>, ()> {
    if matches!(&form.kind, FormKind::None) {
        return Ok(None);
    }
    contract_name(form)
        .filter(|name| !name.is_empty())
        .map(Some)
        .ok_or(())
}

fn contract_optional_names(form: &Form) -> Result<Option<Vec<String>>, ()> {
    if matches!(&form.kind, FormKind::None) {
        return Ok(None);
    }
    let FormKind::Vector(values) = &form.kind else {
        return Err(());
    };
    values
        .iter()
        .map(|value| {
            contract_name(value)
                .filter(|name| !name.is_empty())
                .ok_or(())
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn contract_optional_bool(form: &Form) -> Result<Option<bool>, ()> {
    match &form.kind {
        FormKind::None => Ok(None),
        FormKind::Bool(value) => Ok(Some(*value)),
        _ => Err(()),
    }
}

fn error_name() -> Name {
    Name {
        spelling: "<error>".to_owned(),
        canonical: "<error>".to_owned(),
    }
}

fn looks_like_type(form: &Form) -> bool {
    match &form.kind {
        FormKind::List(items) => {
            let Some(head) = items.first() else {
                return false;
            };
            looks_like_type_name(head) && items[1..].iter().all(looks_like_type_argument)
        }
        FormKind::Symbol(name) => name
            .canonical
            .chars()
            .next()
            .is_some_and(char::is_uppercase),
        _ => false,
    }
}

fn looks_like_type_name(form: &Form) -> bool {
    matches!(&form.kind, FormKind::Symbol(name)
        if name.canonical.chars().next().is_some_and(char::is_uppercase))
}

fn looks_like_type_argument(form: &Form) -> bool {
    looks_like_type(form)
        || matches!(
            form.kind,
            FormKind::Vector(_) | FormKind::Map(_) | FormKind::Set(_)
        )
}

fn is_equal_symbol(form: &Form) -> bool {
    matches!(&form.kind, FormKind::Symbol(name) if name.canonical == "=")
}

fn is_ampersand_symbol(form: &Form) -> bool {
    matches!(&form.kind, FormKind::Symbol(name) if name.canonical == "&")
}

fn metadata_key(form: &Form) -> Option<&str> {
    match &form.kind {
        FormKind::Keyword(name) | FormKind::Symbol(name) => {
            Some(name.canonical.trim_start_matches(':'))
        }
        _ => None,
    }
}

fn merge_declaration_metadata(
    mut declaration: Vec<MetadataEntry>,
    name: &[MetadataEntry],
) -> Vec<MetadataEntry> {
    for entry in name {
        if let Some(existing) = declaration
            .iter_mut()
            .find(|existing| datum_eq(&existing.key, &entry.key))
        {
            *existing = entry.clone();
        } else {
            declaration.push(entry.clone());
        }
    }
    declaration
}

fn has_type_marker(form: &Form) -> bool {
    form.metadata.iter().any(|entry| {
        metadata_key(&entry.key) == Some("type") && matches!(entry.value.kind, FormKind::Bool(true))
    })
}

#[cfg(test)]
mod tests {
    use super::{
        AST_WRONG_SHAPE, ExprKind, FunctionPhase, ItemKind, OperatorMetadataError, PatternKind,
        TypeExpr, TypeExprKind, lower_document, metadata_key, operator_declaration,
    };
    use crate::{
        reader::read,
        syntax::FormKind,
        types::{Alignment, Availability, Effect, TemporalBound},
    };

    #[test]
    fn lowers_module_header_and_declarations() {
        let document = read(
            "(module analytics.transforms.normalize)
             (import data.series :as series)
             (import-for-syntax osiris.syntax :as syntax)
             (py/import numpy :as np)
             (export [normalize])
             (alias 窗口均值 series/moving-average)
             (def scale Float 0.5)
             (defn normalize [[values Frame] [window PositiveInt = 8]] -> Float
               (let [x 1 y (+ x 2)] y))",
        );
        assert!(
            document.diagnostics.is_empty(),
            "{:?}",
            document.diagnostics
        );
        let lowered = lower_document(&document);
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
        assert_eq!(
            lowered
                .module
                .name
                .as_ref()
                .map(|name| name.canonical.as_str()),
            Some("analytics.transforms.normalize")
        );
        assert_eq!(lowered.module.items.len(), 7);
        assert!(matches!(lowered.module.items[0].kind, ItemKind::Import(_)));
        assert!(matches!(
            lowered.module.items[1].kind,
            ItemKind::ImportForSyntax(_)
        ));
        assert!(matches!(
            lowered.module.items[2].kind,
            ItemKind::PyImport(_)
        ));
        let function = match &lowered.module.items[6].kind {
            ItemKind::Defn(function) => function,
            other => panic!("expected defn, got {other:?}"),
        };
        assert_eq!(function.params.len(), 2);
        assert_eq!(
            function.params[1].default.as_ref().map(|value| &value.kind),
            Some(&ExprKind::Integer("8".to_owned()))
        );
        assert_eq!(function.phase, FunctionPhase::Runtime);
    }

    #[test]
    fn lowers_clojure_parameter_patterns_without_confusing_type_annotations() {
        let lowered = lower_document(&read(
            r#"(fn [{:keys [value]}
                    [[left right] Any]
                    [[first second] (Vector Int)]
                    [plain Int]]
                 value)"#,
        ));
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
        let ItemKind::Expr(expression) = &lowered.module.items[0].kind else {
            panic!("expected expression");
        };
        let ExprKind::Fn(function) = &expression.kind else {
            panic!("expected fn");
        };
        assert_eq!(function.params.len(), 4);
        assert!(matches!(
            function.params[0]
                .pattern
                .as_ref()
                .map(|pattern| &pattern.kind),
            Some(PatternKind::Map(_))
        ));
        assert!(matches!(
            function.params[1]
                .pattern
                .as_ref()
                .map(|pattern| &pattern.kind),
            Some(PatternKind::Vector(_))
        ));
        assert!(function.params[1].type_annotation.is_some());
        assert!(matches!(
            function.params[2]
                .pattern
                .as_ref()
                .map(|pattern| &pattern.kind),
            Some(PatternKind::Vector(_))
        ));
        assert!(function.params[2].type_annotation.is_some());
        assert!(function.params[3].pattern.is_none());
        assert!(function.params[3].type_annotation.is_some());
    }

    #[test]
    fn runtime_parameter_types_do_not_depend_on_case_or_script() {
        let lowered = lower_document(&read(
            "(fn [[left right] [参数 中文类型] [qualified pkg/lower]] left)",
        ));
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
        let ItemKind::Expr(expression) = &lowered.module.items[0].kind else {
            panic!("expected expression");
        };
        let ExprKind::Fn(function) = &expression.kind else {
            panic!("expected fn");
        };

        let expected = ["right", "中文类型", "pkg/lower"];
        for (parameter, expected_type) in function.params.iter().zip(expected) {
            assert!(parameter.pattern.is_none());
            assert!(matches!(
                parameter.type_annotation.as_ref().map(|ty| &ty.kind),
                Some(TypeExprKind::Name(name)) if name.canonical == expected_type
            ));
        }
    }

    #[test]
    fn lowers_type_and_tag_metadata_for_parameters_returns_and_locals() {
        let lowered = lower_document(&read(
            "(defn ^{:type (Vector Int)} increment-all
               [^{:type (Vector Int)} values]
               (let [^{:type Int} offset 1] values))
             (defn ^Vector tagged [^Int value] value)",
        ));
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);

        let ItemKind::Defn(function) = &lowered.module.items[0].kind else {
            panic!("expected defn");
        };
        assert!(matches!(
            function.params[0]
                .type_annotation
                .as_ref()
                .map(|ty| &ty.kind),
            Some(TypeExprKind::Apply { constructor, args })
                if matches!(&constructor.kind, TypeExprKind::Name(name) if name.canonical == "Vector")
                    && args.len() == 1
        ));
        assert!(matches!(
            function.return_type.as_ref().map(|ty| &ty.kind),
            Some(TypeExprKind::Apply { constructor, args })
                if matches!(&constructor.kind, TypeExprKind::Name(name) if name.canonical == "Vector")
                    && args.len() == 1
        ));
        assert_eq!(
            function
                .return_type
                .as_ref()
                .map_or(0, |ty| ty.metadata.len()),
            1
        );
        let ExprKind::Let { bindings, .. } = &function.body[0].kind else {
            panic!("expected let");
        };
        assert!(matches!(
            bindings[0].type_annotation.as_ref().map(|ty| &ty.kind),
            Some(TypeExprKind::Name(name)) if name.canonical == "Int"
        ));

        let ItemKind::Defn(tagged) = &lowered.module.items[1].kind else {
            panic!("expected tagged defn");
        };
        assert!(matches!(
            tagged.params[0]
                .type_annotation
                .as_ref()
                .map(|ty| &ty.kind),
            Some(TypeExprKind::Name(name)) if name.canonical == "Int"
        ));
        assert!(matches!(
            tagged.return_type.as_ref().map(|ty| &ty.kind),
            Some(TypeExprKind::Apply { constructor, args })
                if matches!(&constructor.kind, TypeExprKind::Name(name) if name.canonical == "Vector")
                    && matches!(args.as_slice(), [TypeExpr { kind: TypeExprKind::Name(name), .. }]
                        if name.canonical == "Any")
        ));
    }

    #[test]
    fn raw_core_container_metadata_defaults_parameters_to_any() {
        let lowered = lower_document(&read(
            "(defn raw [^Vector vector ^List list ^Set set ^Option option ^Map mapping] vector)
             (fn [[strict Vector]] strict)",
        ));
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
        let ItemKind::Defn(function) = &lowered.module.items[0].kind else {
            panic!("expected defn");
        };
        for (parameter, (constructor_name, arity)) in function.params.iter().zip([
            ("Vector", 1),
            ("List", 1),
            ("Set", 1),
            ("Option", 1),
            ("Map", 2),
        ]) {
            let Some(TypeExpr {
                kind: TypeExprKind::Apply { constructor, args },
                ..
            }) = &parameter.type_annotation
            else {
                panic!("expected raw container metadata to become an application");
            };
            assert!(matches!(
                &constructor.kind,
                TypeExprKind::Name(name) if name.canonical == constructor_name
            ));
            assert_eq!(args.len(), arity);
            assert!(args.iter().all(|argument| matches!(
                &argument.kind,
                TypeExprKind::Name(name) if name.canonical == "Any"
            )));
        }

        let ItemKind::Expr(expression) = &lowered.module.items[1].kind else {
            panic!("expected fn expression");
        };
        let ExprKind::Fn(function) = &expression.kind else {
            panic!("expected fn expression");
        };
        assert!(matches!(
            function.params[0]
                .type_annotation
                .as_ref()
                .map(|ty| &ty.kind),
            Some(TypeExprKind::Name(name)) if name.canonical == "Vector"
        ));
    }

    #[test]
    fn lowers_type_marker_parameter_prefix() {
        let lowered = lower_document(&read(
            "(defn increment-all [^:type (Vector Int) values] values)",
        ));
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
        let ItemKind::Defn(function) = &lowered.module.items[0].kind else {
            panic!("expected defn");
        };
        assert_eq!(function.params.len(), 1);
        assert_eq!(function.params[0].name.canonical, "values");
        assert!(matches!(
            function.params[0]
                .type_annotation
                .as_ref()
                .map(|ty| &ty.kind),
            Some(TypeExprKind::Apply { constructor, args })
                if matches!(&constructor.kind, TypeExprKind::Name(name) if name.canonical == "Vector")
                    && args.len() == 1
        ));
    }

    #[test]
    fn explicit_types_win_over_metadata_with_stable_diagnostics() {
        let lowered = lower_document(&read(
            "(defn ^{:type Int} choose [[^{:type Int} value Float]] -> Float value)",
        ));
        assert_eq!(
            lowered
                .diagnostics
                .iter()
                .filter(|diagnostic| { diagnostic.code == super::AST_CONFLICTING_TYPE_ANNOTATION })
                .count(),
            2,
            "{:?}",
            lowered.diagnostics
        );
        let ItemKind::Defn(function) = &lowered.module.items[0].kind else {
            panic!("expected defn");
        };
        assert!(matches!(
            function.params[0]
                .type_annotation
                .as_ref()
                .map(|ty| &ty.kind),
            Some(TypeExprKind::Name(name)) if name.canonical == "Float"
        ));
        assert!(matches!(
            function.return_type.as_ref().map(|ty| &ty.kind),
            Some(TypeExprKind::Name(name)) if name.canonical == "Float"
        ));
    }

    #[test]
    fn type_metadata_wins_over_tag_metadata_with_a_diagnostic() {
        let lowered = lower_document(&read("(defn ^{:type Int :tag Float} choose [value] value)"));
        assert_eq!(
            lowered
                .diagnostics
                .iter()
                .filter(|diagnostic| { diagnostic.code == super::AST_CONFLICTING_TYPE_ANNOTATION })
                .count(),
            1,
            "{:?}",
            lowered.diagnostics
        );
        let ItemKind::Defn(function) = &lowered.module.items[0].kind else {
            panic!("expected defn");
        };
        assert!(matches!(
            function.return_type.as_ref().map(|ty| &ty.kind),
            Some(TypeExprKind::Name(name)) if name.canonical == "Int"
        ));
    }

    #[test]
    fn malformed_type_metadata_is_recoverable() {
        let lowered = lower_document(&read(
            "(defn invalid [^{:type 1} value] value)\n\
             (defn incomplete [^:type Int] 1)\n\
             (defn okay [value] value)",
        ));
        assert!(
            lowered
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code == super::AST_INVALID_TYPE_METADATA)
                .count()
                >= 2,
            "{:?}",
            lowered.diagnostics
        );
        assert!(matches!(lowered.module.items[2].kind, ItemKind::Defn(_)));
    }

    #[test]
    fn phase_one_vector_parameters_are_patterns_regardless_of_spelling() {
        let lowered = lower_document(&read(
            "(defmacro choose [[left Type]] left)\n\
             (defn-for-syntax helper [[参数 中文类型]] 参数)",
        ));
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);

        let ItemKind::Defmacro(macro_) = &lowered.module.items[0].kind else {
            panic!("expected macro");
        };
        assert!(matches!(
            macro_.params[0]
                .pattern
                .as_ref()
                .map(|pattern| &pattern.kind),
            Some(PatternKind::Vector(_))
        ));

        let ItemKind::DefnForSyntax(helper) = &lowered.module.items[1].kind else {
            panic!("expected syntax helper");
        };
        assert!(matches!(
            helper.params[0]
                .pattern
                .as_ref()
                .map(|pattern| &pattern.kind),
            Some(PatternKind::Vector(_))
        ));
    }

    #[test]
    fn ambiguous_runtime_vector_pattern_reports_the_explicit_wrapper_rule() {
        let lowered = lower_document(&read("(fn [[left right extra]] left)"));
        assert!(lowered.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == AST_WRONG_SHAPE && diagnostic.message.contains("extra vector layer")
        }));
    }

    #[test]
    fn runtime_vector_pattern_requires_an_explicit_type() {
        let lowered = lower_document(&read("(fn [[[left right]]] left)"));
        assert!(lowered.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == AST_WRONG_SHAPE
                && diagnostic
                    .message
                    .contains("runtime vector destructuring requires an explicit type")
        }));
    }

    #[test]
    fn keeps_call_keyword_order_and_duplicates() {
        let document = read("(f first :周期 3 :周期 4 tail)");
        let lowered = lower_document(&document);
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
        let item = &lowered.module.items[0];
        let call = match &item.kind {
            ItemKind::Expr(expr) => match &expr.kind {
                ExprKind::Call(call) => call,
                other => panic!("expected call, got {other:?}"),
            },
            other => panic!("expected expression, got {other:?}"),
        };
        assert_eq!(call.args.len(), 4);
        assert_eq!(call.positional.len(), 2);
        assert_eq!(call.keywords.len(), 2);
        assert_eq!(call.keywords[0].key.canonical, ":周期");
        assert_eq!(call.keywords[1].key.canonical, ":周期");
    }

    #[test]
    fn malformed_keyword_and_bindings_recover_following_top_level_form() {
        let document = read("(f :missing) (let [x] x) (defn okay [value] value)");
        let lowered = lower_document(&document);
        assert!(
            lowered
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == super::AST_EXPECTED_PAIR)
        );
        assert_eq!(lowered.module.items.len(), 3);
        assert!(matches!(lowered.module.items[2].kind, ItemKind::Defn(_)));
    }

    #[test]
    fn lowers_special_forms_and_reader_quotes() {
        let document = read(
            "(fn [x] (if true (do x) (raise \"bad\")))
             '(quoted value)
             \x60(let [tmp ~x] ~@items)",
        );
        let lowered = lower_document(&document);
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
        let first = match &lowered.module.items[0].kind {
            ItemKind::Expr(expr) => expr,
            _ => panic!("expected expression"),
        };
        assert!(matches!(first.kind, ExprKind::Fn(_)));
        let quoted = match &lowered.module.items[1].kind {
            ItemKind::Expr(expr) => expr,
            _ => panic!("expected expression"),
        };
        assert!(matches!(quoted.kind, ExprKind::Quote(_)));
        let syntax_quoted = match &lowered.module.items[2].kind {
            ItemKind::Expr(expr) => expr,
            _ => panic!("expected expression"),
        };
        assert!(matches!(syntax_quoted.kind, ExprKind::SyntaxQuote(_)));
    }

    #[test]
    fn lowers_struct_generics_fields_and_checks() {
        let document = read(
            "(defstruct (Range T)
               \"closed range\"
               [min T]
               [max T = 1]
               (check (<= min max) \"ordered\"))",
        );
        let lowered = lower_document(&document);
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
        let structure = match &lowered.module.items[0].kind {
            ItemKind::Defstruct(structure) => structure,
            other => panic!("expected defstruct, got {other:?}"),
        };
        assert_eq!(structure.type_params.len(), 1);
        assert_eq!(structure.fields.len(), 2);
        assert_eq!(structure.checks.len(), 1);
        assert!(matches!(
            structure.checks[0].condition.kind,
            ExprKind::Call(_)
        ));
        assert!(matches!(
            structure.checks[0]
                .message
                .as_ref()
                .map(|message| &message.kind),
            Some(ExprKind::String(message)) if message == "ordered"
        ));
        assert_eq!(structure.doc.as_deref(), Some("closed range"));
    }

    #[test]
    fn type_expression_is_structured_but_keeps_extension_literals() {
        let document = read("(defn f [[value (Array Float [:time])]] -> (Union Int Float) value)");
        let lowered = lower_document(&document);
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
        let function = match &lowered.module.items[0].kind {
            ItemKind::Defn(function) => function,
            _ => panic!("expected defn"),
        };
        assert!(matches!(
            function.params[0]
                .type_annotation
                .as_ref()
                .map(|type_expr| &type_expr.kind),
            Some(super::TypeExprKind::Apply { .. })
        ));
        assert!(matches!(
            function
                .return_type
                .as_ref()
                .map(|type_expr| &type_expr.kind),
            Some(super::TypeExprKind::Union(_))
        ));
    }

    #[test]
    fn non_list_top_level_forms_are_surface_expressions() {
        let document = read("42");
        let lowered = lower_document(&document);
        assert!(lowered.diagnostics.is_empty());
        assert!(matches!(
            &lowered.module.items[0].kind,
            ItemKind::Expr(expr)
                if matches!(expr.kind, ExprKind::Integer(ref value) if value == "42")
        ));
    }

    #[test]
    fn metadata_and_spans_are_copied_to_ast_nodes() {
        let document = read("^{:doc \"x\"} (def ^:private value 1)");
        let lowered = lower_document(&document);
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
        assert_eq!(lowered.module.items[0].metadata.len(), 1);
        let definition = match &lowered.module.items[0].kind {
            ItemKind::Def(definition) => definition,
            _ => panic!("expected def"),
        };
        assert_eq!(definition.name.canonical, "value");
        assert_eq!(definition.metadata.len(), 2);
        assert!(definition.span.end > definition.span.start);
    }

    #[test]
    fn def_name_metadata_is_preserved_with_name_precedence() {
        let lowered = lower_document(&read(
            "^{:doc \"outer\" :dynamic false} (def ^{:doc \"name\" :dynamic true} *value* 1)",
        ));
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
        let ItemKind::Def(definition) = &lowered.module.items[0].kind else {
            panic!("expected def");
        };
        assert_eq!(definition.metadata.len(), 2);
        assert!(definition.metadata.iter().any(|entry| {
            metadata_key(&entry.key) == Some("doc")
                && matches!(&entry.value.kind, FormKind::String(value) if value == "name")
        }));
        assert!(definition.metadata.iter().any(|entry| {
            metadata_key(&entry.key) == Some("dynamic")
                && matches!(entry.value.kind, FormKind::Bool(true))
        }));
    }

    #[test]
    fn exposes_closed_operator_metadata_without_interpreting_it() {
        let lowered = lower_document(&read(
            "^{:osiris/operator :multiply} (defn scale [[value Float]] -> Float value)",
        ));
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
        let function = match &lowered.module.items[0].kind {
            ItemKind::Defn(function) => function,
            other => panic!("expected defn, got {other:?}"),
        };
        assert_eq!(
            operator_declaration(&function.metadata),
            Ok(Some("multiply".to_owned()))
        );

        let malformed = lower_document(&read(
            "^{:osiris/operator [:multiply]} (defn scale [[value Float]] -> Float value)",
        ));
        let malformed_function = match &malformed.module.items[0].kind {
            ItemKind::Defn(function) => function,
            other => panic!("expected defn, got {other:?}"),
        };
        assert_eq!(
            operator_declaration(&malformed_function.metadata),
            Err(OperatorMetadataError::ExpectedName)
        );

        let mut duplicate = function.metadata.clone();
        duplicate.push(function.metadata[0].clone());
        assert_eq!(
            operator_declaration(&duplicate),
            Err(OperatorMetadataError::Duplicate)
        );
    }

    #[test]
    fn no_module_header_yields_none_name() {
        let lowered = lower_document(&read("(def x 1)"));
        assert!(lowered.module.name.is_none());
    }

    #[test]
    fn scalar_form_kinds_are_not_lost() {
        let lowered = lower_document(&read("none true 1 1.0 \"s\" :k"));
        assert_eq!(lowered.module.items.len(), 6);
        assert!(matches!(
            lowered.module.items[0].kind,
            ItemKind::Expr(ref expr) if matches!(expr.kind, ExprKind::None)
        ));
        assert!(matches!(
            lowered.module.items[5].kind,
            ItemKind::Expr(ref expr) if matches!(expr.kind, ExprKind::Keyword(_))
        ));
    }

    #[test]
    fn def_distinguishes_call_values_from_type_only_declarations() {
        let lowered = lower_document(&read(
            "(def array (np.asarray [1 2]))\n\
             (def point (Point :x 1))\n\
             (def declared (Array Float [:time]))",
        ));
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);

        for index in 0..2 {
            let ItemKind::Def(definition) = &lowered.module.items[index].kind else {
                panic!("expected def");
            };
            assert!(definition.type_annotation.is_none());
            assert!(matches!(
                definition.value.as_ref().map(|value| &value.kind),
                Some(ExprKind::Call(_))
            ));
        }
        let ItemKind::Def(declared) = &lowered.module.items[2].kind else {
            panic!("expected def");
        };
        assert!(declared.type_annotation.is_some());
        assert!(declared.value.is_none());
    }

    #[test]
    fn malformed_declaration_still_has_a_recoverable_item() {
        let lowered = lower_document(&read("(defn 1) (def okay 2)"));
        assert!(
            lowered
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == super::AST_INVALID_NAME)
        );
        assert_eq!(lowered.module.items.len(), 2);
        assert!(matches!(lowered.module.items[1].kind, ItemKind::Def(_)));
    }

    #[test]
    fn map_and_set_expression_nodes_are_structured() {
        let lowered = lower_document(&read("{:a 1} #{:x :y}"));
        assert_eq!(lowered.module.items.len(), 2);
        assert!(matches!(
            lowered.module.items[0].kind,
            ItemKind::Expr(ref expr) if matches!(expr.kind, ExprKind::Map(_))
        ));
        assert!(matches!(
            lowered.module.items[1].kind,
            ItemKind::Expr(ref expr) if matches!(expr.kind, ExprKind::Set(_))
        ));
    }

    #[test]
    fn static_record_uses_a_single_map_with_keyword_fields() {
        let lowered = lower_document(&read(
            "(static-record component/ComponentDescriptor normalize
               {:component/id \"example.normalize\"
                :component/enabled true})",
        ));
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
        let record = match &lowered.module.items[0].kind {
            ItemKind::StaticRecord(record) => record,
            other => panic!("expected static-record, got {other:?}"),
        };
        assert_eq!(record.fields.len(), 2);
        assert_eq!(record.fields[0].0.canonical, ":component/id");
        assert!(matches!(record.fields[0].1.kind, ExprKind::String(_)));
        assert_eq!(record.fields[1].0.canonical, ":component/enabled");
        assert!(matches!(record.fields[1].1.kind, ExprKind::Bool(true)));
    }

    #[test]
    fn static_record_recovers_flat_fields_with_a_shape_diagnostic() {
        let lowered = lower_document(&read("(static-record Schema owner :field 1)"));
        assert!(
            lowered
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == super::AST_WRONG_SHAPE
                    && diagnostic.message.contains("single map"))
        );
        let record = match &lowered.module.items[0].kind {
            ItemKind::StaticRecord(record) => record,
            other => panic!("expected static-record, got {other:?}"),
        };
        assert_eq!(record.fields.len(), 1);
        assert_eq!(record.fields[0].0.canonical, ":field");
    }

    #[test]
    fn extern_contains_nested_declarations() {
        let lowered = lower_document(&read(
            "(extern python \"math\" (defn isfinite [[value Float]] -> Bool))",
        ));
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
        let external = match &lowered.module.items[0].kind {
            ItemKind::Extern(external) => external,
            _ => panic!("expected extern"),
        };
        assert_eq!(external.items.len(), 1);
        assert!(matches!(
            external.items[0].kind,
            ItemKind::Defn(ref function) if function.body.is_empty()
        ));
    }

    #[test]
    fn extern_contract_is_lowered_as_closed_static_data() {
        let lowered = lower_document(&read(
            r#"(extern python "host.data"
                 (defn moving-average [[values Series] [n Int]] -> Series
                   :contract
                   {:id "host.data/moving-average-v1"
                    :effects [:io :host/cache]
                    :temporal {:past "2*(n-1)"
                               :future 0
                               :availability :published}
                    :data {:schema "measurements"
                           :axes [:time]
                           :alignment :labelled
                           :ordered-by [:source :time]
                           :unique-by [:source :time]
                           :preserves-length true
                           :materializes false
                           :reshapes false
                           :nulls-possible true
                           :nan-possible true
                           :nonfinite-possible true
                           :nonfinite-policy :preserve-nonfinite}}))"#,
        ));
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
        let ItemKind::Extern(external) = &lowered.module.items[0].kind else {
            panic!("expected extern");
        };
        let ItemKind::Defn(function) = &external.items[0].kind else {
            panic!("expected extern function");
        };
        let contract = function.contract.as_ref().expect("contract");
        assert_eq!(contract.id, "host.data/moving-average-v1");
        assert!(contract.summaries.effects.effects.contains(&Effect::Io));
        assert!(
            contract
                .summaries
                .effects
                .effects
                .contains(&Effect::Custom("host/cache".to_owned()))
        );
        assert_eq!(
            contract.summaries.temporal.past,
            TemporalBound::Symbolic("2*(n-1)".to_owned())
        );
        assert_eq!(contract.summaries.temporal.future, TemporalBound::Finite(0));
        assert_eq!(
            contract.summaries.temporal.availability,
            Availability::Named("published".to_owned())
        );
        assert_eq!(contract.summaries.data.alignment, Alignment::Labelled);
        assert_eq!(
            contract.summaries.data.ordered_by,
            Some(vec!["source".to_owned(), "time".to_owned()])
        );
        assert_eq!(
            contract.summaries.data.unique_by,
            Some(vec!["source".to_owned(), "time".to_owned()])
        );
        assert_eq!(contract.summaries.data.preserves_length, Some(true));
        assert_eq!(contract.summaries.data.nan_possible, Some(true));
        assert_eq!(
            contract.summaries.data.nonfinite_policy.as_deref(),
            Some("preserve-nonfinite")
        );
    }

    #[test]
    fn malformed_extern_contract_fails_closed() {
        let lowered = lower_document(&read(
            r#"(extern python "host.series"
                 (defn lead [[values Series]] -> Series
                   :contract
                   {:id "host.series/lead-v1"
                    :id "duplicate"
                    :temporal {:future -1}
                    :executable-analyzer "host.analyze"}))"#,
        ));
        assert!(
            lowered
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == super::AST_INVALID_CONTRACT)
        );
        assert!(
            lowered
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == super::AST_UNKNOWN_CLAUSE)
        );
        let ItemKind::Extern(external) = &lowered.module.items[0].kind else {
            panic!("expected extern");
        };
        let ItemKind::Defn(function) = &external.items[0].kind else {
            panic!("expected extern function");
        };
        assert!(function.contract.is_none());
    }

    #[test]
    fn runtime_function_still_requires_a_body() {
        let lowered = lower_document(&read("(defn incomplete [[value Int]] -> Int)"));
        assert!(lowered.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == super::AST_WRONG_SHAPE
                && diagnostic.message == "function body cannot be empty"
        }));
    }

    #[test]
    fn error_nodes_are_serializable_without_panicking() {
        let lowered = lower_document(&read("(if true)"));
        let json = serde_json::to_string(&lowered).expect("AST should serialize");
        assert!(json.contains("diagnostics"));
        assert!(lowered
            .module
            .items
            .iter()
            .any(|item| matches!(&item.kind, ItemKind::Expr(expr) if matches!(expr.kind, ExprKind::If { .. }))));
    }
}
