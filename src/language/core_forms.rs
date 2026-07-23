//! The closed surface-form kernel accepted directly by the compiler.
//!
//! Forms not listed here are ordinary calls or macros. Extension packages add
//! syntax by exporting hygienic macros; they do not add parser or AST cases.

/// Top-level forms that establish module, ABI, or phase boundaries.
pub const AUTHORED_BOUNDARY_FORMS: &[&str] = &[
    "module",
    "import",
    "import-for-syntax",
    "py/import",
    "export",
    "alias",
    "defmacro",
    "defn-for-syntax",
    "defstatic-schema",
];

/// Runtime declarations that a hygienic declaration macro may produce.
pub const MACRO_DECLARATION_FORMS: &[&str] = &[
    "def",
    "defn",
    "defstruct",
    "extern",
    "static-record",
    "py/decorate",
];

/// Core expression heads. Every other list head is a call or macro invocation.
pub const EXPRESSION_FORMS: &[&str] = &["fn", "let", "if", "do", "try", "raise"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeclarationForm {
    Import,
    ImportForSyntax,
    PythonImport,
    PythonDecorate,
    Export,
    Alias,
    Def,
    Defn,
    Defstruct,
    DefstaticSchema,
    StaticRecord,
    Extern,
    Defmacro,
    DefnForSyntax,
}

impl DeclarationForm {
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "import" => Self::Import,
            "import-for-syntax" => Self::ImportForSyntax,
            "py/import" => Self::PythonImport,
            "py/decorate" => Self::PythonDecorate,
            "export" => Self::Export,
            "alias" => Self::Alias,
            "def" => Self::Def,
            "defn" => Self::Defn,
            "defstruct" => Self::Defstruct,
            "defstatic-schema" => Self::DefstaticSchema,
            "static-record" => Self::StaticRecord,
            "extern" => Self::Extern,
            "defmacro" => Self::Defmacro,
            "defn-for-syntax" => Self::DefnForSyntax,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExpressionForm {
    Fn,
    Let,
    If,
    Do,
    Try,
    Raise,
}

impl ExpressionForm {
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "fn" => Self::Fn,
            "let" => Self::Let,
            "if" => Self::If,
            "do" => Self::Do,
            "try" => Self::Try,
            "raise" => Self::Raise,
            _ => return None,
        })
    }
}

#[must_use]
pub fn is_phase_declaration(name: &str) -> bool {
    matches!(name, "defmacro" | "defn-for-syntax")
}

#[must_use]
pub fn is_authored_boundary(name: &str) -> bool {
    AUTHORED_BOUNDARY_FORMS.contains(&name)
}

#[must_use]
pub fn is_macro_declaration(name: &str) -> bool {
    MACRO_DECLARATION_FORMS.contains(&name)
}
