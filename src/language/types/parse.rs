use super::*;

/// Pure parsing entry point for an explicit source type. `type_variables` maps
/// canonical source spellings to the inference ids allocated by the enclosing
/// generic declaration.
pub fn parse_type(
    form: &Form,
    type_variables: &BTreeMap<String, TypeVarId>,
) -> Result<Type, TypeParseError> {
    match &form.kind {
        FormKind::Symbol(name) => Ok(parse_type_name(&name.canonical, type_variables)),
        FormKind::List(items) if items.is_empty() => Err(TypeParseError::new(
            TypeParseErrorKind::EmptyApplication,
            form.span,
        )),
        FormKind::List(items) => parse_type_application(form.span, items, type_variables),
        _ => Err(TypeParseError::new(
            TypeParseErrorKind::ExpectedType,
            form.span,
        )),
    }
}

fn parse_type_name(name: &str, type_variables: &BTreeMap<String, TypeVarId>) -> Type {
    if let Some(variable) = type_variables.get(name) {
        return Type::TypeVar(*variable);
    }
    match name {
        "Bool" => Type::Bool,
        "Int" => Type::Int,
        "Float" => Type::Float,
        "Str" => Type::Str,
        "Bytes" => Type::Bytes,
        "None" => Type::None,
        "Any" => Type::Any,
        "Never" => Type::Never,
        "Unknown" => Type::Unknown,
        "Error" => Type::Error,
        _ => Type::Nominal {
            binding: name.to_owned(),
            args: Vec::new(),
        },
    }
}

fn parse_type_application(
    span: Span,
    items: &[Form],
    type_variables: &BTreeMap<String, TypeVarId>,
) -> Result<Type, TypeParseError> {
    let FormKind::Symbol(head) = &items[0].kind else {
        return Err(TypeParseError::new(
            TypeParseErrorKind::ExpectedConstructor,
            items[0].span,
        ));
    };
    let arguments = &items[1..];
    match head.canonical.as_str() {
        name @ ("Bool" | "Int" | "Float" | "Str" | "Bytes" | "None" | "Any" | "Never"
        | "Unknown" | "Error") => {
            require_arity(name, arguments, 0, span)?;
            Ok(parse_type_name(name, type_variables))
        }
        "Option" => {
            require_arity("Option", arguments, 1, span)?;
            Ok(Type::option(parse_type(&arguments[0], type_variables)?))
        }
        "Union" => {
            if arguments.len() < 2 {
                return Err(TypeParseError::new(
                    TypeParseErrorKind::MinimumArity {
                        constructor: "Union".to_owned(),
                        minimum: 2,
                        found: arguments.len(),
                    },
                    span,
                ));
            }
            Ok(Type::union(
                arguments
                    .iter()
                    .map(|argument| parse_type(argument, type_variables))
                    .collect::<Result<Vec<_>, _>>()?,
            ))
        }
        "Tuple" => Ok(Type::Tuple(
            arguments
                .iter()
                .map(|argument| parse_type(argument, type_variables))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        "List" => parse_unary_constructor("List", arguments, span, type_variables, |item| {
            Type::List(Box::new(item))
        }),
        "Vector" => parse_unary_constructor("Vector", arguments, span, type_variables, |item| {
            Type::Vector(Box::new(item))
        }),
        "Map" => {
            require_arity("Map", arguments, 2, span)?;
            Ok(Type::Map(
                Box::new(parse_type(&arguments[0], type_variables)?),
                Box::new(parse_type(&arguments[1], type_variables)?),
            ))
        }
        "Set" => parse_unary_constructor("Set", arguments, span, type_variables, |item| {
            Type::Set(Box::new(item))
        }),
        "Fn" => parse_function_type(arguments, span, type_variables),
        name if type_variables.contains_key(name) => Err(TypeParseError::new(
            TypeParseErrorKind::AppliedTypeVariable(name.to_owned()),
            span,
        )),
        name => Ok(Type::Nominal {
            binding: name.to_owned(),
            args: arguments
                .iter()
                .map(|argument| parse_type_argument(argument, type_variables))
                .collect::<Result<Vec<_>, _>>()?,
        }),
    }
}

#[must_use]
pub fn nominal_short_name(binding: &str) -> &str {
    binding
        .rsplit_once("::type::")
        .map_or(binding, |(_, name)| name)
}

/// Stable marker used for the small, explicitly supported set of Python
/// built-in exception classes.  These types are available to `catch` without
/// requiring a user declaration or a runtime import.  Keeping a distinct
/// binding namespace means an arbitrary nominal spelling can never silently
/// become an exception type.
pub const PYTHON_BUILTIN_EXCEPTION_PREFIX: &str = "__osiris_builtin_exception__::type::";

const PYTHON_BUILTIN_EXCEPTION_NAMES: &[&str] = &[
    "BaseException",
    "Exception",
    "ArithmeticError",
    "FloatingPointError",
    "LookupError",
    "AssertionError",
    "AttributeError",
    "BufferError",
    "EOFError",
    "ImportError",
    "IndexError",
    "KeyError",
    "MemoryError",
    "NameError",
    "NotImplementedError",
    "OSError",
    "EnvironmentError",
    "IOError",
    "OverflowError",
    "ReferenceError",
    "RuntimeError",
    "RecursionError",
    "StopAsyncIteration",
    "StopIteration",
    "SyntaxError",
    "SystemError",
    "SystemExit",
    "TypeError",
    "UnboundLocalError",
    "UnicodeError",
    "UnicodeDecodeError",
    "UnicodeEncodeError",
    "UnicodeTranslateError",
    "ValueError",
    "ZeroDivisionError",
    "GeneratorExit",
    "KeyboardInterrupt",
    "Warning",
    "UserWarning",
    "DeprecationWarning",
    "PendingDeprecationWarning",
    "SyntaxWarning",
    "RuntimeWarning",
    "FutureWarning",
    "ImportWarning",
    "UnicodeWarning",
    "BytesWarning",
    "ResourceWarning",
    "IndentationError",
    "TabError",
    "ModuleNotFoundError",
    "FileNotFoundError",
    "PermissionError",
    "TimeoutError",
    "ConnectionError",
    "BrokenPipeError",
    "ChildProcessError",
    "ConnectionAbortedError",
    "ConnectionRefusedError",
    "ConnectionResetError",
    "IsADirectoryError",
    "NotADirectoryError",
    "ProcessLookupError",
    "InterruptedError",
    "BlockingIOError",
    "FileExistsError",
];

/// All exception names accepted by the built-in exception type whitelist.
#[must_use]
pub const fn python_builtin_exception_names() -> &'static [&'static str] {
    PYTHON_BUILTIN_EXCEPTION_NAMES
}

/// Resolve a source spelling to a Python built-in exception name.
///
/// Only unqualified names and the explicit `builtins/`/`builtins.` qualified
/// spellings are accepted.  Extension/module aliases are intentionally not
/// inferred here: callers that need a custom exception should declare an
/// actual nominal type or use an extension contract.
#[must_use]
pub fn python_builtin_exception_name(name: &str) -> Option<&'static str> {
    let short = name
        .strip_prefix("builtins/")
        .or_else(|| name.strip_prefix("builtins."))
        .unwrap_or(name);
    PYTHON_BUILTIN_EXCEPTION_NAMES
        .iter()
        .find_map(|candidate| (*candidate == short).then_some(*candidate))
}

/// Return the stable nominal binding for a supported Python exception.
#[must_use]
pub fn python_builtin_exception_binding(name: &str) -> Option<String> {
    python_builtin_exception_name(name)
        .map(|name| format!("{PYTHON_BUILTIN_EXCEPTION_PREFIX}{name}"))
}

/// Decode a stable built-in exception binding back to its Python class name.
#[must_use]
pub fn python_builtin_exception_from_binding(binding: &str) -> Option<&'static str> {
    let name = binding.strip_prefix(PYTHON_BUILTIN_EXCEPTION_PREFIX)?;
    python_builtin_exception_name(name)
}

fn parse_type_argument(
    form: &Form,
    type_variables: &BTreeMap<String, TypeVarId>,
) -> Result<Type, TypeParseError> {
    let is_literal = match &form.kind {
        FormKind::None
        | FormKind::Bool(_)
        | FormKind::Integer(_)
        | FormKind::Float(_)
        | FormKind::String(_)
        | FormKind::Keyword(_)
        | FormKind::Vector(_)
        | FormKind::Map(_)
        | FormKind::Set(_) => true,
        FormKind::List(values) => values.is_empty(),
        FormKind::Symbol(_) | FormKind::ReaderMacro { .. } | FormKind::Error(_) => false,
    };
    if !is_literal {
        return parse_type(form, type_variables);
    }
    TypeLiteral::from_form(form)
        .map(Type::Literal)
        .map_err(|error| {
            TypeParseError::new(
                TypeParseErrorKind::InvalidLiteral(error.message().to_owned()),
                form.span,
            )
        })
}

fn parse_unary_constructor(
    name: &str,
    arguments: &[Form],
    span: Span,
    type_variables: &BTreeMap<String, TypeVarId>,
    build: impl FnOnce(Type) -> Type,
) -> Result<Type, TypeParseError> {
    require_arity(name, arguments, 1, span)?;
    Ok(build(parse_type(&arguments[0], type_variables)?))
}

fn parse_function_type(
    arguments: &[Form],
    span: Span,
    type_variables: &BTreeMap<String, TypeVarId>,
) -> Result<Type, TypeParseError> {
    let (parameters, return_form) = match arguments {
        [parameters, returns] => (parameters, returns),
        [parameters, arrow, returns] if is_symbol(arrow, "->") => (parameters, returns),
        _ => {
            return Err(TypeParseError::new(TypeParseErrorKind::FunctionShape, span));
        }
    };
    let FormKind::Vector(parameters) = &parameters.kind else {
        return Err(TypeParseError::new(
            TypeParseErrorKind::FunctionParameters,
            parameters.span,
        ));
    };
    let parameters = parameters
        .iter()
        .map(|parameter| parse_type(parameter, type_variables))
        .collect::<Result<Vec<_>, _>>()?;
    let return_type = parse_type(return_form, type_variables)?;
    Ok(Type::Fn(
        FunctionType::new(parameters, return_type).with_summaries(CallSummaries::unknown()),
    ))
}

fn require_arity(
    constructor: &str,
    arguments: &[Form],
    expected: usize,
    span: Span,
) -> Result<(), TypeParseError> {
    if arguments.len() == expected {
        Ok(())
    } else {
        Err(TypeParseError::new(
            TypeParseErrorKind::Arity {
                constructor: constructor.to_owned(),
                expected,
                found: arguments.len(),
            },
            span,
        ))
    }
}

fn is_symbol(form: &Form, expected: &str) -> bool {
    matches!(&form.kind, FormKind::Symbol(name) if name.canonical == expected)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeParseError {
    pub kind: TypeParseErrorKind,
    pub span: Span,
}

impl TypeParseError {
    #[must_use]
    pub const fn new(kind: TypeParseErrorKind, span: Span) -> Self {
        Self { kind, span }
    }

    #[must_use]
    pub const fn code(&self) -> &'static str {
        "OSR-TYPE-001"
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypeParseErrorKind {
    ExpectedType,
    EmptyApplication,
    ExpectedConstructor,
    Arity {
        constructor: String,
        expected: usize,
        found: usize,
    },
    MinimumArity {
        constructor: String,
        minimum: usize,
        found: usize,
    },
    FunctionShape,
    FunctionParameters,
    AppliedTypeVariable(String),
    InvalidLiteral(String),
}

impl fmt::Display for TypeParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            TypeParseErrorKind::ExpectedType => formatter.write_str("expected a type form"),
            TypeParseErrorKind::EmptyApplication => {
                formatter.write_str("a type application cannot be empty")
            }
            TypeParseErrorKind::ExpectedConstructor => {
                formatter.write_str("expected a type constructor name")
            }
            TypeParseErrorKind::Arity {
                constructor,
                expected,
                found,
            } => write!(
                formatter,
                "type constructor `{constructor}` expects {expected} arguments, found {found}"
            ),
            TypeParseErrorKind::MinimumArity {
                constructor,
                minimum,
                found,
            } => write!(
                formatter,
                "type constructor `{constructor}` expects at least {minimum} arguments, found {found}"
            ),
            TypeParseErrorKind::FunctionShape => formatter.write_str(
                "function type must have the shape `(Fn [Parameter ...] Return)` or `(Fn [Parameter ...] -> Return)`",
            ),
            TypeParseErrorKind::FunctionParameters => {
                formatter.write_str("function type parameters must be a vector")
            }
            TypeParseErrorKind::AppliedTypeVariable(name) => {
                write!(formatter, "type variable `{name}` cannot be used as a constructor")
            }
            TypeParseErrorKind::InvalidLiteral(message) => {
                write!(formatter, "invalid type literal: {message}")
            }
        }
    }
}

impl Error for TypeParseError {}
