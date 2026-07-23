//! A small, structured Python 3.9 AST and deterministic source printer.
//!
//! Backend passes should build these nodes instead of assembling Python source
//! fragments.  The printer deliberately owns all syntax decisions, including
//! operator precedence, indentation, blank lines, and literal escaping.

use std::{error::Error, fmt};

/// A Python source file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Module {
    pub body: Vec<Stmt>,
}

impl Module {
    #[must_use]
    pub const fn new(body: Vec<Stmt>) -> Self {
        Self { body }
    }

    /// Render this module as deterministic Python 3.9 source.
    pub fn to_source(&self) -> Result<String, PrintError> {
        render(self)
    }
}

/// A Python statement.
#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Import(Import),
    Assign(Assign),
    AnnAssign(AnnAssign),
    AugAssign(AugAssign),
    Expr(Expr),
    FunctionDef(Box<FunctionDef>),
    Return(Option<Expr>),
    If(IfStmt),
    ClassDef(ClassDef),
    Try(Try),
    Raise(Raise),
    Assert { test: Expr, message: Option<Expr> },
    Pass,
    Break,
    Continue,
}

/// Either form of Python import statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Import {
    Direct(Vec<ImportAlias>),
    From {
        /// `None` is useful for relative imports such as `from . import x`.
        module: Option<String>,
        names: Vec<ImportAlias>,
        level: usize,
    },
}

/// A name, and its optional local name, in an import statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportAlias {
    pub name: String,
    pub as_name: Option<String>,
}

impl ImportAlias {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            as_name: None,
        }
    }

    #[must_use]
    pub fn renamed(name: impl Into<String>, as_name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            as_name: Some(as_name.into()),
        }
    }
}

/// A chained assignment such as `left = alias = value`.
#[derive(Clone, Debug, PartialEq)]
pub struct Assign {
    pub targets: Vec<Expr>,
    pub value: Expr,
}

/// An annotated assignment such as `price: float = 0.0`.
#[derive(Clone, Debug, PartialEq)]
pub struct AnnAssign {
    pub target: Expr,
    pub annotation: Expr,
    pub value: Option<Expr>,
}

/// An augmented assignment such as `total += value`.
#[derive(Clone, Debug, PartialEq)]
pub struct AugAssign {
    pub target: Expr,
    pub op: BinaryOp,
    pub value: Expr,
}

/// A function definition.
#[derive(Clone, Debug, PartialEq)]
pub struct FunctionDef {
    pub name: String,
    pub parameters: Parameters,
    pub returns: Option<Expr>,
    pub decorators: Vec<Expr>,
    pub body: Vec<Stmt>,
    pub is_async: bool,
}

/// A single typed or untyped function parameter.
#[derive(Clone, Debug, PartialEq)]
pub struct Parameter {
    pub name: String,
    pub annotation: Option<Expr>,
    pub default: Option<Expr>,
}

impl Parameter {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            annotation: None,
            default: None,
        }
    }
}

/// Python's five parameter regions, modeled so `/`, `*`, and `**` cannot be
/// confused with ordinary expressions.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parameters {
    pub positional_only: Vec<Parameter>,
    pub positional: Vec<Parameter>,
    pub vararg: Option<Parameter>,
    pub keyword_only: Vec<Parameter>,
    pub kwarg: Option<Parameter>,
}

/// An `if` statement.  A sole nested `If` in `orelse` is printed as `elif`.
#[derive(Clone, Debug, PartialEq)]
pub struct IfStmt {
    pub test: Expr,
    pub body: Vec<Stmt>,
    pub orelse: Vec<Stmt>,
}

/// A class definition.  Dataclasses are represented structurally by putting
/// `Expr::Name("dataclass")` (or a call to it) in `decorators`.
#[derive(Clone, Debug, PartialEq)]
pub struct ClassDef {
    pub name: String,
    pub bases: Vec<Expr>,
    pub keywords: Vec<KeywordArgument>,
    pub decorators: Vec<Expr>,
    pub body: Vec<Stmt>,
}

/// A `try` statement with any combination accepted by Python 3.9.
#[derive(Clone, Debug, PartialEq)]
pub struct Try {
    pub body: Vec<Stmt>,
    pub handlers: Vec<ExceptHandler>,
    pub orelse: Vec<Stmt>,
    pub finalbody: Vec<Stmt>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExceptHandler {
    pub exception_type: Option<Expr>,
    pub name: Option<String>,
    pub body: Vec<Stmt>,
}

/// A bare raise, an exception raise, or an exception chain.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Raise {
    pub exception: Option<Expr>,
    pub cause: Option<Expr>,
}

/// A Python expression.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Name(String),
    Literal(Literal),
    Tuple(Vec<Expr>),
    List(Vec<Expr>),
    Set(Vec<Expr>),
    Dict(Vec<DictItem>),
    Attribute {
        value: Box<Expr>,
        attr: String,
    },
    Subscript {
        value: Box<Expr>,
        slice: Box<Expr>,
    },
    /// A slice expression.  It is valid in a subscript (including a tuple
    /// used as a multidimensional subscript), not as a standalone expression.
    Slice {
        lower: Option<Box<Expr>>,
        upper: Option<Box<Expr>>,
        step: Option<Box<Expr>>,
    },
    Call {
        function: Box<Expr>,
        arguments: Vec<CallArgument>,
    },
    BoolOp {
        op: BooleanOp,
        values: Vec<Expr>,
    },
    BinOp {
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
    },
    UnaryOp {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Compare {
        left: Box<Expr>,
        comparisons: Vec<(CompareOp, Expr)>,
    },
    IfExp {
        body: Box<Expr>,
        test: Box<Expr>,
        orelse: Box<Expr>,
    },
    Lambda {
        parameters: Box<Parameters>,
        body: Box<Expr>,
    },
    /// `*value` in a call or container display.
    Starred(Box<Expr>),
}

impl Expr {
    #[must_use]
    pub fn name(name: impl Into<String>) -> Self {
        Self::Name(name.into())
    }

    #[must_use]
    pub fn string(value: impl Into<String>) -> Self {
        Self::Literal(Literal::String(value.into()))
    }

    #[must_use]
    pub fn call(function: Self, arguments: Vec<CallArgument>) -> Self {
        Self::Call {
            function: Box::new(function),
            arguments,
        }
    }
}

/// Python literal values.  Container displays are expression nodes because
/// their children need not themselves be literals.
#[derive(Clone, Debug, PartialEq)]
pub enum Literal {
    None,
    Bool(bool),
    Integer(i128),
    /// An arbitrary-precision decimal integer spelling.  Python integers are
    /// unbounded, so the backend must not force source literals through Rust's
    /// `i128` range.
    IntegerText(String),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
    Ellipsis,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DictItem {
    Pair { key: Expr, value: Expr },
    Unpack(Expr),
}

/// Call arguments remain ordered, which preserves legal forms such as
/// `f(named=1, *more)`.
#[derive(Clone, Debug, PartialEq)]
pub enum CallArgument {
    Positional(Expr),
    Starred(Expr),
    Keyword(KeywordArgument),
}

#[derive(Clone, Debug, PartialEq)]
pub enum KeywordArgument {
    Named { name: String, value: Expr },
    Unpack(Expr),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BooleanOp {
    And,
    Or,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    MatrixMultiply,
    Divide,
    FloorDivide,
    Modulo,
    Power,
    LeftShift,
    RightShift,
    BitAnd,
    BitXor,
    BitOr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Positive,
    Negative,
    Not,
    Invert,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareOp {
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    In,
    NotIn,
    Is,
    IsNot,
}

/// A structurally invalid AST cannot be rendered as valid Python.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrintError {
    message: String,
}

impl PrintError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for PrintError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for PrintError {}

mod printer;

pub use printer::render;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
