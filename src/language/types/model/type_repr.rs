use std::{
    collections::BTreeSet,
    fmt::{self, Write},
};

use serde::Serialize;

use super::super::{PythonTypeError, PythonTypingImport, PythonVersion, nominal_short_name};
use super::{
    literal::{TypeLiteral, TypeVarId},
    summaries::FunctionType,
};

/// The closed core type representation. Data libraries use `Nominal` rather
/// than adding compiler-known variants for Array, Series, or Frame.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
// Boxing only `Fn` would churn the public HIR/interface shape. Types are
// compiler-owned semantic nodes, not a dense runtime collection.
#[allow(clippy::large_enum_variant)]
pub enum Type {
    Bool,
    Int,
    Float,
    Str,
    Bytes,
    None,
    Any,
    Never,
    Unknown,
    Error,
    Option(Box<Type>),
    Union(Vec<Type>),
    Tuple(Vec<Type>),
    List(Box<Type>),
    Vector(Box<Type>),
    Map(Box<Type>, Box<Type>),
    Set(Box<Type>),
    Fn(FunctionType),
    Nominal {
        /// Stable `BindingId` of the defining type in typed HIR and interfaces.
        /// Parser-level helpers temporarily retain the unresolved source name
        /// here until HIR name resolution replaces it.
        binding: String,
        args: Vec<Type>,
    },
    Literal(TypeLiteral),
    TypeVar(TypeVarId),
}

impl Type {
    /// Builds the canonical representation of a nullable type.
    #[must_use]
    pub fn option(inner: Self) -> Self {
        Self::union([inner, Self::None])
    }

    /// Builds a deterministic, flattened union. `Option[T]` is the canonical
    /// spelling for a union containing `None`.
    #[must_use]
    pub fn union(types: impl IntoIterator<Item = Self>) -> Self {
        let mut members = Vec::new();
        for ty in types {
            flatten_union(ty, &mut members);
        }

        if members.contains(&Self::Error) {
            return Self::Error;
        }
        if members.contains(&Self::Any) {
            return Self::Any;
        }

        members.retain(|member| member != &Self::Never);
        members.sort();
        members.dedup();

        let has_none = members
            .binary_search(&Self::None)
            .ok()
            .map(|index| members.remove(index))
            .is_some();
        let body = match members.len() {
            0 => Self::Never,
            1 => members.pop().expect("one union member"),
            _ => Self::Union(members),
        };

        if has_none {
            if body == Self::Never {
                Self::None
            } else {
                Self::Option(Box::new(body))
            }
        } else {
            body
        }
    }

    /// Returns all imports from `typing` or `typing_extensions` required to
    /// render this type for the selected Python target.
    #[must_use]
    pub fn python_typing_imports(&self, target: PythonVersion) -> BTreeSet<PythonTypingImport> {
        let mut imports = BTreeSet::new();
        self.collect_python_typing_imports(target, &mut imports);
        imports
    }

    fn collect_python_typing_imports(
        &self,
        target: PythonVersion,
        imports: &mut BTreeSet<PythonTypingImport>,
    ) {
        match self {
            Self::Any => {
                imports.insert(PythonTypingImport::typing("Any"));
            }
            Self::Never => {
                let name = if target.at_least(3, 11) {
                    "Never"
                } else {
                    "NoReturn"
                };
                imports.insert(PythonTypingImport::typing(name));
            }
            Self::Option(inner) => {
                imports.insert(PythonTypingImport::typing("Optional"));
                inner.collect_python_typing_imports(target, imports);
            }
            Self::Union(members) => {
                imports.insert(PythonTypingImport::typing("Union"));
                for member in members {
                    member.collect_python_typing_imports(target, imports);
                }
            }
            Self::Tuple(members) => {
                for member in members {
                    member.collect_python_typing_imports(target, imports);
                }
            }
            Self::List(item) | Self::Vector(item) | Self::Set(item) => {
                item.collect_python_typing_imports(target, imports);
            }
            Self::Map(key, value) => {
                key.collect_python_typing_imports(target, imports);
                value.collect_python_typing_imports(target, imports);
            }
            Self::Fn(function) => {
                imports.insert(PythonTypingImport::typing("Callable"));
                for parameter in &function.parameters {
                    parameter.collect_python_typing_imports(target, imports);
                }
                function
                    .return_type
                    .collect_python_typing_imports(target, imports);
            }
            Self::Nominal { args, .. } => {
                for argument in args {
                    argument.collect_python_typing_imports(target, imports);
                }
            }
            Self::Literal(_) => {
                imports.insert(PythonTypingImport::typing("Literal"));
            }
            Self::TypeVar(_) => {
                imports.insert(PythonTypingImport::typing("TypeVar"));
            }
            Self::Bool
            | Self::Int
            | Self::Float
            | Self::Str
            | Self::Bytes
            | Self::None
            | Self::Unknown
            | Self::Error => {}
        }
    }

    /// Renders a standard Python annotation. Nominal names use `module/name`
    /// internally and are emitted as `module.name`.
    pub fn to_python_annotation(&self, target: PythonVersion) -> Result<String, PythonTypeError> {
        let annotation = match self {
            Self::Bool => "bool".to_owned(),
            Self::Int => "int".to_owned(),
            Self::Float => "float".to_owned(),
            Self::Str => "str".to_owned(),
            Self::Bytes => "bytes".to_owned(),
            Self::None => "None".to_owned(),
            Self::Any => "Any".to_owned(),
            Self::Never if target.at_least(3, 11) => "Never".to_owned(),
            Self::Never => "NoReturn".to_owned(),
            Self::Unknown => {
                return Err(PythonTypeError::Unresolved(Box::new(Type::Unknown)));
            }
            Self::Error => return Err(PythonTypeError::Unresolved(Box::new(Type::Error))),
            Self::Option(inner) => {
                format!("Optional[{}]", inner.to_python_annotation(target)?)
            }
            Self::Union(members) => {
                format!("Union[{}]", render_python_types(members, target, ", ")?)
            }
            Self::Tuple(members) => {
                format!("tuple[{}]", render_python_types(members, target, ", ")?)
            }
            Self::List(item) => format!("list[{}]", item.to_python_annotation(target)?),
            Self::Vector(item) => format!("tuple[{}, ...]", item.to_python_annotation(target)?),
            Self::Map(key, value) => format!(
                "dict[{}, {}]",
                key.to_python_annotation(target)?,
                value.to_python_annotation(target)?
            ),
            Self::Set(item) => format!("set[{}]", item.to_python_annotation(target)?),
            Self::Fn(function) => format!(
                "Callable[[{}], {}]",
                render_python_types(&function.parameters, target, ", ")?,
                function.return_type.to_python_annotation(target)?
            ),
            Self::Nominal { binding, args } => {
                let python_name = nominal_short_name(binding).replace('/', ".");
                if args.is_empty() {
                    python_name
                } else {
                    format!(
                        "{python_name}[{}]",
                        render_python_types(args, target, ", ")?
                    )
                }
            }
            Self::Literal(value) => format!(
                "Literal[{}]",
                python_string_literal(&value.canonical_text())
            ),
            Self::TypeVar(variable) => format!("_T{}", variable.0),
        };
        Ok(annotation)
    }
}

fn flatten_union(ty: Type, members: &mut Vec<Type>) {
    match ty {
        Type::Union(nested) => {
            for member in nested {
                flatten_union(member, members);
            }
        }
        Type::Option(inner) => {
            flatten_union(*inner, members);
            members.push(Type::None);
        }
        other => members.push(other),
    }
}

fn render_python_types(
    types: &[Type],
    target: PythonVersion,
    separator: &str,
) -> Result<String, PythonTypeError> {
    types
        .iter()
        .map(|ty| ty.to_python_annotation(target))
        .collect::<Result<Vec<_>, _>>()
        .map(|parts| parts.join(separator))
}

impl fmt::Display for Type {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool => formatter.write_str("Bool"),
            Self::Int => formatter.write_str("Int"),
            Self::Float => formatter.write_str("Float"),
            Self::Str => formatter.write_str("Str"),
            Self::Bytes => formatter.write_str("Bytes"),
            Self::None => formatter.write_str("None"),
            Self::Any => formatter.write_str("Any"),
            Self::Never => formatter.write_str("Never"),
            Self::Unknown => formatter.write_str("Unknown"),
            Self::Error => formatter.write_str("Error"),
            Self::Option(inner) => write!(formatter, "Option[{inner}]"),
            Self::Union(members) => write_type_list(formatter, "Union", members),
            Self::Tuple(members) => write_type_list(formatter, "Tuple", members),
            Self::List(item) => write!(formatter, "List[{item}]"),
            Self::Vector(item) => write!(formatter, "Vector[{item}]"),
            Self::Map(key, value) => write!(formatter, "Map[{key}, {value}]"),
            Self::Set(item) => write!(formatter, "Set[{item}]"),
            Self::Fn(function) => {
                formatter.write_str("Fn[[")?;
                write_joined_types(formatter, &function.parameters)?;
                write!(formatter, "], {}]", function.return_type)
            }
            Self::Nominal { binding, args } if args.is_empty() => {
                formatter.write_str(nominal_short_name(binding))
            }
            Self::Nominal { binding, args } => {
                write_type_list(formatter, nominal_short_name(binding), args)
            }
            Self::Literal(value) => write!(formatter, "Literal[{value}]"),
            Self::TypeVar(variable) => write!(formatter, "?{}", variable.0),
        }
    }
}

fn python_string_literal(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('"');
    for character in value.chars() {
        match character {
            '\\' => output.push_str("\\\\"),
            '"' => output.push_str("\\\""),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                let _ = write!(output, "\\u{:04x}", character as u32);
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}

fn write_type_list(formatter: &mut fmt::Formatter<'_>, name: &str, types: &[Type]) -> fmt::Result {
    write!(formatter, "{name}[")?;
    write_joined_types(formatter, types)?;
    formatter.write_str("]")
}

fn write_joined_types(formatter: &mut fmt::Formatter<'_>, types: &[Type]) -> fmt::Result {
    for (index, ty) in types.iter().enumerate() {
        if index > 0 {
            formatter.write_str(", ")?;
        }
        write!(formatter, "{ty}")?;
    }
    Ok(())
}
