/// Function/lambda parameter. `variadic` marks the parameter following `&`.
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
    pub(in crate::ast) fn from_form(form: &Form, kind: ExprKind) -> Self {
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
    pub(in crate::ast) fn from_form(form: &Form, kind: TypeExprKind) -> Self {
        let info = NodeInfo::from_form(form);
        Self {
            span: info.span,
            metadata: info.metadata,
            kind,
        }
    }
}
