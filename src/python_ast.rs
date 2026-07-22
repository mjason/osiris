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

/// Render a module as Python 3.9 source.
pub fn render(module: &Module) -> Result<String, PrintError> {
    let mut printer = Printer::default();
    printer.print_module(module)?;
    Ok(printer.output)
}

const PREC_LAMBDA: u8 = 1;
const PREC_IF_EXP: u8 = 2;
const PREC_OR: u8 = 3;
const PREC_AND: u8 = 4;
const PREC_NOT: u8 = 5;
const PREC_COMPARE: u8 = 6;
const PREC_BIT_OR: u8 = 7;
const PREC_BIT_XOR: u8 = 8;
const PREC_BIT_AND: u8 = 9;
const PREC_SHIFT: u8 = 10;
const PREC_ADD: u8 = 11;
const PREC_MULTIPLY: u8 = 12;
const PREC_UNARY: u8 = 13;
const PREC_POWER: u8 = 14;
const PREC_PRIMARY: u8 = 16;
const PREC_ATOM: u8 = 17;

#[derive(Default)]
struct Printer {
    output: String,
    indent: usize,
}

impl Printer {
    fn print_module(&mut self, module: &Module) -> Result<(), PrintError> {
        let mut previous_was_definition = false;
        for (index, statement) in module.body.iter().enumerate() {
            let is_definition = is_definition(statement);
            if index > 0 && (previous_was_definition || is_definition) {
                self.output.push_str("\n\n");
            }
            self.print_stmt(statement)?;
            previous_was_definition = is_definition;
        }
        Ok(())
    }

    fn print_stmt(&mut self, statement: &Stmt) -> Result<(), PrintError> {
        match statement {
            Stmt::Import(import) => self.print_import(import),
            Stmt::Assign(assign) => self.print_assign(assign),
            Stmt::AnnAssign(assign) => self.print_ann_assign(assign),
            Stmt::AugAssign(assign) => self.print_aug_assign(assign),
            Stmt::Expr(expression) => {
                self.start_line();
                self.print_expr(expression, 0)?;
                self.end_line();
                Ok(())
            }
            Stmt::FunctionDef(function) => self.print_function(function),
            Stmt::Return(value) => {
                self.start_line();
                self.output.push_str("return");
                if let Some(value) = value {
                    self.output.push(' ');
                    self.print_expr(value, 0)?;
                }
                self.end_line();
                Ok(())
            }
            Stmt::If(statement) => self.print_if(statement, false),
            Stmt::ClassDef(class) => self.print_class(class),
            Stmt::Try(statement) => self.print_try(statement),
            Stmt::Raise(statement) => self.print_raise(statement),
            Stmt::Assert { test, message } => {
                self.start_line();
                self.output.push_str("assert ");
                self.print_expr(test, 0)?;
                if let Some(message) = message {
                    self.output.push_str(", ");
                    self.print_expr(message, 0)?;
                }
                self.end_line();
                Ok(())
            }
            Stmt::Pass => {
                self.line("pass");
                Ok(())
            }
            Stmt::Break => {
                self.line("break");
                Ok(())
            }
            Stmt::Continue => {
                self.line("continue");
                Ok(())
            }
        }
    }

    fn print_import(&mut self, import: &Import) -> Result<(), PrintError> {
        self.start_line();
        match import {
            Import::Direct(names) => {
                if names.is_empty() {
                    return Err(PrintError::new("an import must contain at least one name"));
                }
                self.output.push_str("import ");
                self.print_import_aliases(names);
            }
            Import::From {
                module,
                names,
                level,
            } => {
                if names.is_empty() {
                    return Err(PrintError::new(
                        "a from-import must contain at least one name",
                    ));
                }
                if module.is_none() && *level == 0 {
                    return Err(PrintError::new(
                        "a from-import needs a module or a relative import level",
                    ));
                }
                self.output.push_str("from ");
                for _ in 0..*level {
                    self.output.push('.');
                }
                if let Some(module) = module {
                    self.output.push_str(module);
                }
                self.output.push_str(" import ");
                self.print_import_aliases(names);
            }
        }
        self.end_line();
        Ok(())
    }

    fn print_import_aliases(&mut self, aliases: &[ImportAlias]) {
        for (index, alias) in aliases.iter().enumerate() {
            if index > 0 {
                self.output.push_str(", ");
            }
            self.output.push_str(&alias.name);
            if let Some(as_name) = &alias.as_name {
                self.output.push_str(" as ");
                self.output.push_str(as_name);
            }
        }
    }

    fn print_assign(&mut self, assign: &Assign) -> Result<(), PrintError> {
        if assign.targets.is_empty() {
            return Err(PrintError::new(
                "an assignment must contain at least one target",
            ));
        }
        self.start_line();
        for target in &assign.targets {
            self.print_expr(target, 0)?;
            self.output.push_str(" = ");
        }
        self.print_expr(&assign.value, 0)?;
        self.end_line();
        Ok(())
    }

    fn print_ann_assign(&mut self, assign: &AnnAssign) -> Result<(), PrintError> {
        self.start_line();
        self.print_expr(&assign.target, 0)?;
        self.output.push_str(": ");
        self.print_expr(&assign.annotation, 0)?;
        if let Some(value) = &assign.value {
            self.output.push_str(" = ");
            self.print_expr(value, 0)?;
        }
        self.end_line();
        Ok(())
    }

    fn print_aug_assign(&mut self, assign: &AugAssign) -> Result<(), PrintError> {
        self.start_line();
        self.print_expr(&assign.target, 0)?;
        self.output.push(' ');
        self.output.push_str(assign.op.text());
        self.output.push_str("= ");
        self.print_expr(&assign.value, 0)?;
        self.end_line();
        Ok(())
    }

    fn print_function(&mut self, function: &FunctionDef) -> Result<(), PrintError> {
        for decorator in &function.decorators {
            self.start_line();
            self.output.push('@');
            self.print_expr(decorator, 0)?;
            self.end_line();
        }
        self.start_line();
        if function.is_async {
            self.output.push_str("async ");
        }
        self.output.push_str("def ");
        self.output.push_str(&function.name);
        self.output.push('(');
        self.print_parameters(&function.parameters, false)?;
        self.output.push(')');
        if let Some(returns) = &function.returns {
            self.output.push_str(" -> ");
            self.print_expr(returns, 0)?;
        }
        self.output.push_str(":\n");
        self.print_suite(&function.body)
    }

    fn print_parameters(
        &mut self,
        parameters: &Parameters,
        lambda: bool,
    ) -> Result<(), PrintError> {
        validate_parameters(parameters, lambda)?;
        let mut needs_separator = false;

        for parameter in &parameters.positional_only {
            self.parameter_separator(&mut needs_separator);
            self.print_parameter(parameter, lambda)?;
        }
        if !parameters.positional_only.is_empty() {
            self.parameter_separator(&mut needs_separator);
            self.output.push('/');
        }
        for parameter in &parameters.positional {
            self.parameter_separator(&mut needs_separator);
            self.print_parameter(parameter, lambda)?;
        }
        if let Some(vararg) = &parameters.vararg {
            self.parameter_separator(&mut needs_separator);
            self.output.push('*');
            self.print_parameter(vararg, lambda)?;
        } else if !parameters.keyword_only.is_empty() {
            self.parameter_separator(&mut needs_separator);
            self.output.push('*');
        }
        for parameter in &parameters.keyword_only {
            self.parameter_separator(&mut needs_separator);
            self.print_parameter(parameter, lambda)?;
        }
        if let Some(kwarg) = &parameters.kwarg {
            self.parameter_separator(&mut needs_separator);
            self.output.push_str("**");
            self.print_parameter(kwarg, lambda)?;
        }
        Ok(())
    }

    fn parameter_separator(&mut self, needs_separator: &mut bool) {
        if *needs_separator {
            self.output.push_str(", ");
        }
        *needs_separator = true;
    }

    fn print_parameter(&mut self, parameter: &Parameter, lambda: bool) -> Result<(), PrintError> {
        self.output.push_str(&parameter.name);
        if let Some(annotation) = &parameter.annotation {
            if lambda {
                return Err(PrintError::new(
                    "lambda parameters cannot contain annotations",
                ));
            }
            self.output.push_str(": ");
            self.print_expr(annotation, 0)?;
        }
        if let Some(default) = &parameter.default {
            self.output.push_str(" = ");
            self.print_expr(default, 0)?;
        }
        Ok(())
    }

    fn print_if(&mut self, statement: &IfStmt, elif: bool) -> Result<(), PrintError> {
        self.start_line();
        self.output.push_str(if elif { "elif " } else { "if " });
        self.print_expr(&statement.test, 0)?;
        self.output.push_str(":\n");
        self.print_suite(&statement.body)?;

        if let [Stmt::If(nested)] = statement.orelse.as_slice() {
            self.print_if(nested, true)?;
        } else if !statement.orelse.is_empty() {
            self.line("else:");
            self.print_suite(&statement.orelse)?;
        }
        Ok(())
    }

    fn print_class(&mut self, class: &ClassDef) -> Result<(), PrintError> {
        for decorator in &class.decorators {
            self.start_line();
            self.output.push('@');
            self.print_expr(decorator, 0)?;
            self.end_line();
        }
        self.start_line();
        self.output.push_str("class ");
        self.output.push_str(&class.name);
        if !class.bases.is_empty() || !class.keywords.is_empty() {
            self.output.push('(');
            let mut separator = false;
            for base in &class.bases {
                if separator {
                    self.output.push_str(", ");
                }
                self.print_expr(base, 0)?;
                separator = true;
            }
            for keyword in &class.keywords {
                if separator {
                    self.output.push_str(", ");
                }
                self.print_keyword(keyword)?;
                separator = true;
            }
            self.output.push(')');
        }
        self.output.push_str(":\n");
        self.print_suite(&class.body)
    }

    fn print_try(&mut self, statement: &Try) -> Result<(), PrintError> {
        if statement.handlers.is_empty() && statement.finalbody.is_empty() {
            return Err(PrintError::new(
                "a try statement needs an except or finally clause",
            ));
        }
        self.line("try:");
        self.print_suite(&statement.body)?;
        for handler in &statement.handlers {
            self.start_line();
            self.output.push_str("except");
            match (&handler.exception_type, &handler.name) {
                (Some(exception_type), name) => {
                    self.output.push(' ');
                    self.print_expr(exception_type, 0)?;
                    if let Some(name) = name {
                        self.output.push_str(" as ");
                        self.output.push_str(name);
                    }
                }
                (None, Some(_)) => {
                    return Err(PrintError::new("a bare except handler cannot bind a name"));
                }
                (None, None) => {}
            }
            self.output.push_str(":\n");
            self.print_suite(&handler.body)?;
        }
        if !statement.orelse.is_empty() {
            self.line("else:");
            self.print_suite(&statement.orelse)?;
        }
        if !statement.finalbody.is_empty() {
            self.line("finally:");
            self.print_suite(&statement.finalbody)?;
        }
        Ok(())
    }

    fn print_raise(&mut self, statement: &Raise) -> Result<(), PrintError> {
        if statement.exception.is_none() && statement.cause.is_some() {
            return Err(PrintError::new("a raise cause needs an exception"));
        }
        self.start_line();
        self.output.push_str("raise");
        if let Some(exception) = &statement.exception {
            self.output.push(' ');
            self.print_expr(exception, 0)?;
        }
        if let Some(cause) = &statement.cause {
            self.output.push_str(" from ");
            self.print_expr(cause, 0)?;
        }
        self.end_line();
        Ok(())
    }

    fn print_suite(&mut self, statements: &[Stmt]) -> Result<(), PrintError> {
        self.indent += 1;
        if statements.is_empty() {
            self.line("pass");
        } else {
            for statement in statements {
                self.print_stmt(statement)?;
            }
        }
        self.indent -= 1;
        Ok(())
    }

    fn print_expr(&mut self, expression: &Expr, parent_precedence: u8) -> Result<(), PrintError> {
        let precedence = expression_precedence(expression);
        let parenthesize = precedence < parent_precedence;
        if parenthesize {
            self.output.push('(');
        }

        match expression {
            Expr::Name(name) => self.output.push_str(name),
            Expr::Literal(literal) => self.print_literal(literal),
            Expr::Tuple(items) => self.print_tuple(items)?,
            Expr::List(items) => self.print_sequence('[', ']', items)?,
            Expr::Set(items) => {
                if items.is_empty() {
                    self.output.push_str("set()");
                } else {
                    self.print_sequence('{', '}', items)?;
                }
            }
            Expr::Dict(items) => self.print_dict(items)?,
            Expr::Attribute { value, attr } => {
                if needs_attribute_parentheses(value) {
                    self.output.push('(');
                    self.print_expr(value, 0)?;
                    self.output.push(')');
                } else {
                    self.print_expr(value, PREC_PRIMARY)?;
                }
                self.output.push('.');
                self.output.push_str(attr);
            }
            Expr::Subscript { value, slice } => {
                self.print_expr(value, PREC_PRIMARY)?;
                self.output.push('[');
                self.print_subscript_slice(slice)?;
                self.output.push(']');
            }
            Expr::Slice { .. } => {
                return Err(PrintError::new(
                    "a slice expression may only appear inside a subscript",
                ));
            }
            Expr::Call {
                function,
                arguments,
            } => {
                self.print_expr(function, PREC_PRIMARY)?;
                self.output.push('(');
                for (index, argument) in arguments.iter().enumerate() {
                    if index > 0 {
                        self.output.push_str(", ");
                    }
                    self.print_call_argument(argument)?;
                }
                self.output.push(')');
            }
            Expr::BoolOp { op, values } => {
                if values.len() < 2 {
                    return Err(PrintError::new(
                        "a boolean operation needs at least two operands",
                    ));
                }
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        self.output.push(' ');
                        self.output.push_str(op.text());
                        self.output.push(' ');
                    }
                    self.print_expr(value, precedence + 1)?;
                }
            }
            Expr::BinOp { left, op, right } => {
                if *op == BinaryOp::Power {
                    self.print_expr(left, precedence + 1)?;
                    self.output.push_str(" ** ");
                    self.print_expr(right, precedence)?;
                } else {
                    self.print_expr(left, precedence)?;
                    self.output.push(' ');
                    self.output.push_str(op.text());
                    self.output.push(' ');
                    self.print_expr(right, precedence + 1)?;
                }
            }
            Expr::UnaryOp { op, operand } => {
                self.output.push_str(op.text());
                self.print_expr(operand, precedence)?;
            }
            Expr::Compare { left, comparisons } => {
                if comparisons.is_empty() {
                    return Err(PrintError::new(
                        "a comparison needs at least one operator and right operand",
                    ));
                }
                self.print_expr(left, PREC_COMPARE + 1)?;
                for (op, right) in comparisons {
                    self.output.push(' ');
                    self.output.push_str(op.text());
                    self.output.push(' ');
                    self.print_expr(right, PREC_COMPARE + 1)?;
                }
            }
            Expr::IfExp { body, test, orelse } => {
                self.print_expr(body, PREC_IF_EXP + 1)?;
                self.output.push_str(" if ");
                self.print_expr(test, PREC_IF_EXP + 1)?;
                self.output.push_str(" else ");
                self.print_expr(orelse, PREC_IF_EXP)?;
            }
            Expr::Lambda { parameters, body } => {
                self.output.push_str("lambda");
                if !parameters_are_empty(parameters) {
                    self.output.push(' ');
                    self.print_parameters(parameters, true)?;
                }
                self.output.push_str(": ");
                self.print_expr(body, PREC_LAMBDA)?;
            }
            Expr::Starred(value) => {
                self.output.push('*');
                self.print_expr(value, PREC_UNARY)?;
            }
        }

        if parenthesize {
            self.output.push(')');
        }
        Ok(())
    }

    fn print_literal(&mut self, literal: &Literal) {
        match literal {
            Literal::None => self.output.push_str("None"),
            Literal::Bool(value) => self.output.push_str(if *value { "True" } else { "False" }),
            Literal::Integer(value) => self.output.push_str(&value.to_string()),
            Literal::IntegerText(value) => self.output.push_str(value),
            Literal::Float(value) => self.print_float(*value),
            Literal::String(value) => self.print_string(value),
            Literal::Bytes(value) => self.print_bytes(value),
            Literal::Ellipsis => self.output.push_str("..."),
        }
    }

    fn print_float(&mut self, value: f64) {
        if value.is_nan() {
            self.output.push_str("float(\"nan\")");
        } else if value == f64::INFINITY {
            self.output.push_str("float(\"inf\")");
        } else if value == f64::NEG_INFINITY {
            self.output.push_str("-float(\"inf\")");
        } else {
            let mut rendered = value.to_string();
            if !rendered.contains(['.', 'e', 'E']) {
                rendered.push_str(".0");
            }
            self.output.push_str(&rendered);
        }
    }

    fn print_string(&mut self, value: &str) {
        self.output.push('"');
        for character in value.chars() {
            match character {
                '\\' => self.output.push_str("\\\\"),
                '"' => self.output.push_str("\\\""),
                '\n' => self.output.push_str("\\n"),
                '\r' => self.output.push_str("\\r"),
                '\t' => self.output.push_str("\\t"),
                '\u{08}' => self.output.push_str("\\b"),
                '\u{0c}' => self.output.push_str("\\f"),
                character if character <= '\u{1f}' || character == '\u{7f}' => {
                    self.output.push_str("\\x");
                    push_hex_byte(&mut self.output, character as u8);
                }
                character => self.output.push(character),
            }
        }
        self.output.push('"');
    }

    fn print_bytes(&mut self, value: &[u8]) {
        self.output.push_str("b\"");
        for byte in value {
            match byte {
                b'\\' => self.output.push_str("\\\\"),
                b'"' => self.output.push_str("\\\""),
                b'\n' => self.output.push_str("\\n"),
                b'\r' => self.output.push_str("\\r"),
                b'\t' => self.output.push_str("\\t"),
                0x20..=0x7e => self.output.push(char::from(*byte)),
                _ => {
                    self.output.push_str("\\x");
                    push_hex_byte(&mut self.output, *byte);
                }
            }
        }
        self.output.push('"');
    }

    fn print_tuple(&mut self, items: &[Expr]) -> Result<(), PrintError> {
        self.output.push('(');
        for (index, item) in items.iter().enumerate() {
            if index > 0 {
                self.output.push_str(", ");
            }
            self.print_expr(item, 0)?;
        }
        if items.len() == 1 {
            self.output.push(',');
        }
        self.output.push(')');
        Ok(())
    }

    fn print_sequence(
        &mut self,
        open: char,
        close: char,
        items: &[Expr],
    ) -> Result<(), PrintError> {
        self.output.push(open);
        for (index, item) in items.iter().enumerate() {
            if index > 0 {
                self.output.push_str(", ");
            }
            self.print_expr(item, 0)?;
        }
        self.output.push(close);
        Ok(())
    }

    fn print_dict(&mut self, items: &[DictItem]) -> Result<(), PrintError> {
        self.output.push('{');
        for (index, item) in items.iter().enumerate() {
            if index > 0 {
                self.output.push_str(", ");
            }
            match item {
                DictItem::Pair { key, value } => {
                    self.print_expr(key, 0)?;
                    self.output.push_str(": ");
                    self.print_expr(value, 0)?;
                }
                DictItem::Unpack(value) => {
                    self.output.push_str("**");
                    self.print_expr(value, 0)?;
                }
            }
        }
        self.output.push('}');
        Ok(())
    }

    fn print_subscript_slice(&mut self, slice: &Expr) -> Result<(), PrintError> {
        if let Expr::Tuple(items) = slice {
            if items.is_empty() {
                self.output.push_str("()");
                return Ok(());
            }
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    self.output.push_str(", ");
                }
                self.print_slice_item(item)?;
            }
            if items.len() == 1 {
                self.output.push(',');
            }
            Ok(())
        } else {
            self.print_slice_item(slice)
        }
    }

    fn print_slice_item(&mut self, slice: &Expr) -> Result<(), PrintError> {
        if let Expr::Slice { lower, upper, step } = slice {
            if let Some(lower) = lower {
                self.print_expr(lower, 0)?;
            }
            self.output.push(':');
            if let Some(upper) = upper {
                self.print_expr(upper, 0)?;
            }
            if let Some(step) = step {
                self.output.push(':');
                self.print_expr(step, 0)?;
            }
            Ok(())
        } else {
            self.print_expr(slice, 0)
        }
    }

    fn print_call_argument(&mut self, argument: &CallArgument) -> Result<(), PrintError> {
        match argument {
            CallArgument::Positional(value) => self.print_expr(value, 0),
            CallArgument::Starred(value) => {
                self.output.push('*');
                self.print_expr(value, 0)
            }
            CallArgument::Keyword(keyword) => self.print_keyword(keyword),
        }
    }

    fn print_keyword(&mut self, keyword: &KeywordArgument) -> Result<(), PrintError> {
        match keyword {
            KeywordArgument::Named { name, value } => {
                self.output.push_str(name);
                self.output.push('=');
                self.print_expr(value, 0)
            }
            KeywordArgument::Unpack(value) => {
                self.output.push_str("**");
                self.print_expr(value, 0)
            }
        }
    }

    fn start_line(&mut self) {
        for _ in 0..self.indent {
            self.output.push_str("    ");
        }
    }

    fn end_line(&mut self) {
        self.output.push('\n');
    }

    fn line(&mut self, text: &str) {
        self.start_line();
        self.output.push_str(text);
        self.end_line();
    }
}

fn is_definition(statement: &Stmt) -> bool {
    matches!(statement, Stmt::FunctionDef(_) | Stmt::ClassDef(_))
}

fn validate_parameters(parameters: &Parameters, lambda: bool) -> Result<(), PrintError> {
    let mut saw_default = false;
    for parameter in parameters
        .positional_only
        .iter()
        .chain(&parameters.positional)
    {
        if parameter.default.is_some() {
            saw_default = true;
        } else if saw_default {
            return Err(PrintError::new(
                "a required positional parameter cannot follow one with a default",
            ));
        }
    }

    for special in [parameters.vararg.as_ref(), parameters.kwarg.as_ref()]
        .into_iter()
        .flatten()
    {
        if special.default.is_some() {
            return Err(PrintError::new(
                "variadic parameters cannot have default values",
            ));
        }
    }

    if lambda
        && parameters
            .positional_only
            .iter()
            .chain(&parameters.positional)
            .chain(parameters.vararg.iter())
            .chain(&parameters.keyword_only)
            .chain(parameters.kwarg.iter())
            .any(|parameter| parameter.annotation.is_some())
    {
        return Err(PrintError::new(
            "lambda parameters cannot contain annotations",
        ));
    }
    Ok(())
}

fn parameters_are_empty(parameters: &Parameters) -> bool {
    parameters.positional_only.is_empty()
        && parameters.positional.is_empty()
        && parameters.vararg.is_none()
        && parameters.keyword_only.is_empty()
        && parameters.kwarg.is_none()
}

fn expression_precedence(expression: &Expr) -> u8 {
    match expression {
        Expr::Lambda { .. } => PREC_LAMBDA,
        Expr::IfExp { .. } => PREC_IF_EXP,
        Expr::BoolOp {
            op: BooleanOp::Or, ..
        } => PREC_OR,
        Expr::BoolOp {
            op: BooleanOp::And, ..
        } => PREC_AND,
        Expr::UnaryOp {
            op: UnaryOp::Not, ..
        } => PREC_NOT,
        Expr::Compare { .. } => PREC_COMPARE,
        Expr::BinOp { op, .. } => op.precedence(),
        Expr::UnaryOp { .. } | Expr::Starred(_) => PREC_UNARY,
        Expr::Attribute { .. } | Expr::Subscript { .. } | Expr::Call { .. } => PREC_PRIMARY,
        Expr::Literal(Literal::Integer(value)) if *value < 0 => PREC_UNARY,
        Expr::Literal(Literal::IntegerText(value)) if value.starts_with('-') => PREC_UNARY,
        Expr::Literal(Literal::Float(value)) if value.is_sign_negative() => PREC_UNARY,
        Expr::Name(_)
        | Expr::Literal(_)
        | Expr::Tuple(_)
        | Expr::List(_)
        | Expr::Set(_)
        | Expr::Dict(_)
        | Expr::Slice { .. } => PREC_ATOM,
    }
}

fn needs_attribute_parentheses(expression: &Expr) -> bool {
    matches!(
        expression,
        Expr::Literal(Literal::Integer(_) | Literal::IntegerText(_))
    )
}

fn push_hex_byte(output: &mut String, byte: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    output.push(char::from(HEX[usize::from(byte >> 4)]));
    output.push(char::from(HEX[usize::from(byte & 0x0f)]));
}

impl BooleanOp {
    const fn text(self) -> &'static str {
        match self {
            Self::And => "and",
            Self::Or => "or",
        }
    }
}

impl BinaryOp {
    const fn text(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Subtract => "-",
            Self::Multiply => "*",
            Self::MatrixMultiply => "@",
            Self::Divide => "/",
            Self::FloorDivide => "//",
            Self::Modulo => "%",
            Self::Power => "**",
            Self::LeftShift => "<<",
            Self::RightShift => ">>",
            Self::BitAnd => "&",
            Self::BitXor => "^",
            Self::BitOr => "|",
        }
    }

    const fn precedence(self) -> u8 {
        match self {
            Self::BitOr => PREC_BIT_OR,
            Self::BitXor => PREC_BIT_XOR,
            Self::BitAnd => PREC_BIT_AND,
            Self::LeftShift | Self::RightShift => PREC_SHIFT,
            Self::Add | Self::Subtract => PREC_ADD,
            Self::Multiply
            | Self::MatrixMultiply
            | Self::Divide
            | Self::FloorDivide
            | Self::Modulo => PREC_MULTIPLY,
            Self::Power => PREC_POWER,
        }
    }
}

impl UnaryOp {
    const fn text(self) -> &'static str {
        match self {
            Self::Positive => "+",
            Self::Negative => "-",
            Self::Not => "not ",
            Self::Invert => "~",
        }
    }
}

impl CompareOp {
    const fn text(self) -> &'static str {
        match self {
            Self::Equal => "==",
            Self::NotEqual => "!=",
            Self::Less => "<",
            Self::LessEqual => "<=",
            Self::Greater => ">",
            Self::GreaterEqual => ">=",
            Self::In => "in",
            Self::NotIn => "not in",
            Self::Is => "is",
            Self::IsNot => "is not",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };

    use super::*;

    fn name(value: &str) -> Expr {
        Expr::name(value)
    }

    fn integer(value: i128) -> Expr {
        Expr::Literal(Literal::Integer(value))
    }

    fn parse_with_python(source: &str) {
        let Ok(mut child) = Command::new("python3")
            .args(["-c", "import ast, sys; ast.parse(sys.stdin.read())"])
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        else {
            return;
        };
        child
            .stdin
            .take()
            .expect("Python stdin should be piped")
            .write_all(source.as_bytes())
            .expect("source should be writable to Python");
        let output = child.wait_with_output().expect("Python should finish");
        assert!(
            output.status.success(),
            "Python rejected generated source:\n{source}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn renders_complete_module_and_python_accepts_it() {
        let module = Module::new(vec![
            Stmt::Import(Import::From {
                module: Some("dataclasses".into()),
                names: vec![ImportAlias::new("dataclass")],
                level: 0,
            }),
            Stmt::Import(Import::Direct(vec![ImportAlias::renamed("numpy", "np")])),
            Stmt::ClassDef(ClassDef {
                name: "Quote".into(),
                bases: vec![],
                keywords: vec![],
                decorators: vec![name("dataclass")],
                body: vec![
                    Stmt::AnnAssign(AnnAssign {
                        target: name("value"),
                        annotation: name("float"),
                        value: None,
                    }),
                    Stmt::AnnAssign(AnnAssign {
                        target: name("source"),
                        annotation: name("str"),
                        value: Some(Expr::string("市场\n")),
                    }),
                ],
            }),
            Stmt::FunctionDef(Box::new(FunctionDef {
                name: "normalize".into(),
                parameters: Parameters {
                    positional_only: vec![Parameter {
                        name: "values".into(),
                        annotation: Some(name("list")),
                        default: None,
                    }],
                    positional: vec![Parameter {
                        name: "scale".into(),
                        annotation: Some(name("float")),
                        default: Some(Expr::Literal(Literal::Float(1.0))),
                    }],
                    vararg: None,
                    keyword_only: vec![Parameter {
                        name: "strict".into(),
                        annotation: Some(name("bool")),
                        default: Some(Expr::Literal(Literal::Bool(true))),
                    }],
                    kwarg: None,
                },
                returns: Some(name("float")),
                decorators: vec![],
                is_async: false,
                body: vec![Stmt::Try(Try {
                    body: vec![Stmt::If(IfStmt {
                        test: name("strict"),
                        body: vec![Stmt::Return(Some(Expr::BinOp {
                            left: Box::new(Expr::Subscript {
                                value: Box::new(name("values")),
                                slice: Box::new(Expr::Slice {
                                    lower: None,
                                    upper: Some(Box::new(integer(1))),
                                    step: None,
                                }),
                            }),
                            op: BinaryOp::Multiply,
                            right: Box::new(name("scale")),
                        }))],
                        orelse: vec![Stmt::Return(Some(name("values")))],
                    })],
                    handlers: vec![ExceptHandler {
                        exception_type: Some(name("TypeError")),
                        name: Some("error".into()),
                        body: vec![Stmt::Raise(Raise {
                            exception: Some(Expr::call(
                                name("ValueError"),
                                vec![CallArgument::Positional(Expr::string("invalid input"))],
                            )),
                            cause: Some(name("error")),
                        })],
                    }],
                    orelse: vec![],
                    finalbody: vec![Stmt::Expr(Expr::call(name("cleanup"), vec![]))],
                })],
            })),
        ]);

        let source = module.to_source().expect("module should render");
        assert_eq!(
            source,
            concat!(
                "from dataclasses import dataclass\n",
                "import numpy as np\n",
                "\n\n",
                "@dataclass\n",
                "class Quote:\n",
                "    value: float\n",
                "    source: str = \"市场\\n\"\n",
                "\n\n",
                "def normalize(values: list, /, scale: float = 1.0, *, strict: bool = True) -> float:\n",
                "    try:\n",
                "        if strict:\n",
                "            return values[:1] * scale\n",
                "        else:\n",
                "            return values\n",
                "    except TypeError as error:\n",
                "        raise ValueError(\"invalid input\") from error\n",
                "    finally:\n",
                "        cleanup()\n",
            )
        );
        parse_with_python(&source);
    }

    #[test]
    fn preserves_operator_tree_with_parentheses() {
        let expression = Expr::BinOp {
            left: Box::new(Expr::UnaryOp {
                op: UnaryOp::Negative,
                operand: Box::new(name("a")),
            }),
            op: BinaryOp::Power,
            right: Box::new(Expr::BinOp {
                left: Box::new(name("b")),
                op: BinaryOp::Subtract,
                right: Box::new(Expr::BinOp {
                    left: Box::new(name("c")),
                    op: BinaryOp::Subtract,
                    right: Box::new(name("d")),
                }),
            }),
        };
        let source = Module::new(vec![Stmt::Expr(expression)])
            .to_source()
            .expect("expression should render");
        assert_eq!(source, "(-a) ** (b - (c - d))\n");
        parse_with_python(&source);
    }

    #[test]
    fn parenthesizes_negative_literals_on_the_left_of_power() {
        let expression = Expr::BinOp {
            left: Box::new(Expr::Literal(Literal::Float(-2.0))),
            op: BinaryOp::Power,
            right: Box::new(integer(2)),
        };
        let source = Module::new(vec![Stmt::Expr(expression)])
            .to_source()
            .expect("expression should render");
        assert_eq!(source, "(-2.0) ** 2\n");
        parse_with_python(&source);
    }

    #[test]
    fn renders_multidimensional_slices() {
        let expression = Expr::Subscript {
            value: Box::new(name("frame")),
            slice: Box::new(Expr::Tuple(vec![
                Expr::Slice {
                    lower: None,
                    upper: None,
                    step: None,
                },
                integer(0),
            ])),
        };
        let source = Module::new(vec![Stmt::Expr(expression)])
            .to_source()
            .expect("slice should render");
        assert_eq!(source, "frame[:, 0]\n");
        parse_with_python(&source);
    }

    #[test]
    fn escapes_text_bytes_and_integral_floats_stably() {
        let module = Module::new(vec![Stmt::Expr(Expr::Tuple(vec![
            Expr::string("quote=\" slash=\\ nul=\0 中文"),
            Expr::Literal(Literal::Bytes(vec![0, b'"', b'\\', 0xff])),
            Expr::Literal(Literal::Float(-0.0)),
            Expr::Literal(Literal::Float(f64::INFINITY)),
        ]))]);
        let first = module.to_source().expect("literals should render");
        let second = module.to_source().expect("literals should render again");
        assert_eq!(first, second);
        assert_eq!(
            first,
            "(\"quote=\\\" slash=\\\\ nul=\\x00 中文\", b\"\\x00\\\"\\\\\\xff\", -0.0, float(\"inf\"))\n"
        );
        parse_with_python(&first);
    }

    #[test]
    fn rejects_structurally_invalid_nodes() {
        let invalid_assignment = Module::new(vec![Stmt::Assign(Assign {
            targets: vec![],
            value: integer(1),
        })]);
        assert_eq!(
            invalid_assignment.to_source().unwrap_err().to_string(),
            "an assignment must contain at least one target"
        );

        let invalid_lambda = Module::new(vec![Stmt::Expr(Expr::Lambda {
            parameters: Box::new(Parameters {
                positional: vec![Parameter {
                    name: "value".into(),
                    annotation: Some(name("int")),
                    default: None,
                }],
                ..Parameters::default()
            }),
            body: Box::new(name("value")),
        })]);
        assert_eq!(
            invalid_lambda.to_source().unwrap_err().to_string(),
            "lambda parameters cannot contain annotations"
        );
    }
}
