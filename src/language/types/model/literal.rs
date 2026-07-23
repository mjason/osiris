use std::{
    error::Error,
    fmt::{self, Write},
};

use serde::Serialize;
use unicode_normalization::UnicodeNormalization;

use crate::syntax::{Form, FormKind};

/// An inference variable local to one [`TypeContext`].
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct TypeVarId(pub u32);

/// A span-free, metadata-free datum used as a nominal type argument.
///
/// Literal parameters describe facts such as array axes or a frame schema.
/// They are part of type identity, so maps and sets are canonicalized and
/// duplicate canonical keys/items are rejected. Source spelling, trivia,
/// metadata, and source locations never enter this representation.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum TypeLiteral {
    None,
    Bool(bool),
    Integer(String),
    /// Exact IEEE-754 binary64 bits. Non-finite values are rejected.
    Float(u64),
    String(String),
    Keyword(String),
    Symbol(String),
    List(Vec<TypeLiteral>),
    Vector(Vec<TypeLiteral>),
    Map(Vec<(TypeLiteral, TypeLiteral)>),
    Set(Vec<TypeLiteral>),
}

impl TypeLiteral {
    /// Convert reader data into canonical type identity, deliberately
    /// discarding metadata and all source-local identity.
    pub fn from_form(form: &Form) -> Result<Self, TypeLiteralError> {
        let literal = match &form.kind {
            FormKind::None => Self::None,
            FormKind::Bool(value) => Self::Bool(*value),
            FormKind::Integer(value) => Self::Integer(value.clone()),
            FormKind::Float(value) => {
                let value = value.parse::<f64>().map_err(|_| {
                    TypeLiteralError::new(format!("invalid literal float `{value}`"))
                })?;
                Self::Float(value.to_bits())
            }
            FormKind::String(value) => Self::String(value.clone()),
            FormKind::Keyword(name) => Self::Keyword(name.canonical.clone()),
            FormKind::Symbol(name) => Self::Symbol(name.canonical.clone()),
            FormKind::List(values) => Self::List(
                values
                    .iter()
                    .map(Self::from_form)
                    .collect::<Result<_, _>>()?,
            ),
            FormKind::Vector(values) => Self::Vector(
                values
                    .iter()
                    .map(Self::from_form)
                    .collect::<Result<_, _>>()?,
            ),
            FormKind::Map(values) => {
                if values.len() % 2 != 0 {
                    return Err(TypeLiteralError::new(
                        "literal map requires key/value pairs",
                    ));
                }
                Self::Map(
                    values
                        .chunks_exact(2)
                        .map(|pair| Ok((Self::from_form(&pair[0])?, Self::from_form(&pair[1])?)))
                        .collect::<Result<_, TypeLiteralError>>()?,
                )
            }
            FormKind::Set(values) => Self::Set(
                values
                    .iter()
                    .map(Self::from_form)
                    .collect::<Result<_, _>>()?,
            ),
            FormKind::ReaderMacro { .. } | FormKind::Error(_) => {
                return Err(TypeLiteralError::new(
                    "reader macros and error forms are not valid type literals",
                ));
            }
        };
        literal.canonicalize()
    }

    /// Normalize a programmatically constructed literal using the same rules
    /// as source lowering and interface decoding.
    pub fn canonicalize(mut self) -> Result<Self, TypeLiteralError> {
        match &mut self {
            Self::Integer(value) => {
                *value = canonical_type_literal_integer(value).ok_or_else(|| {
                    TypeLiteralError::new(format!("invalid literal integer `{value}`"))
                })?;
            }
            Self::Float(bits) => {
                if !f64::from_bits(*bits).is_finite() {
                    return Err(TypeLiteralError::new("literal floats must be finite"));
                }
            }
            Self::Keyword(value) => {
                *value = value.nfc().collect();
                if !value.starts_with(':') {
                    value.insert(0, ':');
                }
            }
            Self::Symbol(value) => *value = value.nfc().collect(),
            Self::List(values) | Self::Vector(values) | Self::Set(values) => {
                for value in values {
                    *value = value.clone().canonicalize()?;
                }
            }
            Self::Map(entries) => {
                for (key, value) in entries {
                    *key = key.clone().canonicalize()?;
                    *value = value.clone().canonicalize()?;
                }
            }
            Self::None | Self::Bool(_) | Self::String(_) => {}
        }

        match &mut self {
            Self::Set(values) => {
                values.sort();
                if values.windows(2).any(|pair| pair[0] == pair[1]) {
                    return Err(TypeLiteralError::new(
                        "duplicate canonical item in literal set",
                    ));
                }
            }
            Self::Map(entries) => {
                entries.sort_by(|left, right| left.0.cmp(&right.0));
                if entries.windows(2).any(|pair| pair[0].0 == pair[1].0) {
                    return Err(TypeLiteralError::new(
                        "duplicate canonical key in literal map",
                    ));
                }
            }
            _ => {}
        }
        Ok(self)
    }

    #[must_use]
    pub fn canonical_text(&self) -> String {
        self.to_string()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeLiteralError {
    message: String,
}

impl TypeLiteralError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for TypeLiteralError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for TypeLiteralError {}

impl fmt::Display for TypeLiteral {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => formatter.write_str("none"),
            Self::Bool(value) => formatter.write_str(if *value { "true" } else { "false" }),
            Self::Integer(value) => formatter.write_str(value),
            Self::Float(bits) => write!(formatter, "{:?}", f64::from_bits(*bits)),
            Self::String(value) => write_osiris_string(formatter, value),
            Self::Keyword(value) | Self::Symbol(value) => formatter.write_str(value),
            Self::List(values) => write_literal_collection(formatter, "(", ")", values),
            Self::Vector(values) => write_literal_collection(formatter, "[", "]", values),
            Self::Map(entries) => {
                formatter.write_str("{")?;
                for (index, (key, value)) in entries.iter().enumerate() {
                    if index > 0 {
                        formatter.write_str(" ")?;
                    }
                    write!(formatter, "{key} {value}")?;
                }
                formatter.write_str("}")
            }
            Self::Set(values) => write_literal_collection(formatter, "#{", "}", values),
        }
    }
}

fn canonical_type_literal_integer(value: &str) -> Option<String> {
    if value.is_empty() || value.contains('_') {
        return None;
    }
    let (negative, digits) = match value.as_bytes()[0] {
        b'+' => (false, &value[1..]),
        b'-' => (true, &value[1..]),
        _ => (false, value),
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        Some("0".to_owned())
    } else if negative {
        Some(format!("-{digits}"))
    } else {
        Some(digits.to_owned())
    }
}

fn write_literal_collection(
    formatter: &mut fmt::Formatter<'_>,
    open: &str,
    close: &str,
    values: &[TypeLiteral],
) -> fmt::Result {
    formatter.write_str(open)?;
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            formatter.write_str(" ")?;
        }
        write!(formatter, "{value}")?;
    }
    formatter.write_str(close)
}

fn write_osiris_string(formatter: &mut fmt::Formatter<'_>, value: &str) -> fmt::Result {
    formatter.write_str("\"")?;
    for character in value.chars() {
        match character {
            '\\' => formatter.write_str("\\\\")?,
            '"' => formatter.write_str("\\\"")?,
            '\n' => formatter.write_str("\\n")?,
            '\r' => formatter.write_str("\\r")?,
            '\t' => formatter.write_str("\\t")?,
            character if character.is_control() => {
                write!(formatter, "\\u{:04x}", character as u32)?;
            }
            character => formatter.write_char(character)?,
        }
    }
    formatter.write_str("\"")
}
