//! Core types and local type inference primitives.
//!
//! This module deliberately has no dependency on name resolution or HIR.  It
//! can therefore be reused by the compiler, interface reader, and LSP.
//! Nominal types carry a stable type-binding identity separately from their
//! short display name, so equal spellings exported by different modules never
//! become the same semantic type.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt::{self, Write},
};

use serde::Serialize;
use unicode_normalization::UnicodeNormalization;

use crate::{
    source::Span,
    syntax::{Form, FormKind},
};

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

/// A runtime effect produced when a function value is called.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Effect {
    Io,
    Throw,
    Mutation,
    HiddenState,
    PythonDynamic,
    Custom(String),
}

/// A closed row enumerates every effect. An open row conservatively permits
/// effects which are not yet known (for example at a dynamic Python boundary).
#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct EffectRow {
    pub effects: BTreeSet<Effect>,
    pub open: bool,
}

impl EffectRow {
    #[must_use]
    pub const fn pure() -> Self {
        Self {
            effects: BTreeSet::new(),
            open: false,
        }
    }

    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            effects: BTreeSet::new(),
            open: true,
        }
    }

    #[must_use]
    pub fn singleton(effect: Effect) -> Self {
        Self {
            effects: BTreeSet::from([effect]),
            open: false,
        }
    }

    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        Self {
            effects: self.effects.union(&other.effects).cloned().collect(),
            open: self.open || other.open,
        }
    }

    /// Whether a function with this effect row may be used where `allowed` is
    /// expected. An open expected row accepts additional effects; an open
    /// actual row cannot satisfy a closed expectation.
    #[must_use]
    pub fn is_within(&self, allowed: &Self) -> bool {
        allowed.open || (!self.open && self.effects.is_subset(&allowed.effects))
    }
}

/// One side of an event-time dependency interval.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum TemporalBound {
    Finite(u64),
    Symbolic(String),
    Unbounded,
    Unknown,
}

impl TemporalBound {
    #[must_use]
    pub fn join(&self, other: &Self) -> Self {
        match (self, other) {
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            (Self::Unbounded, _) | (_, Self::Unbounded) => Self::Unbounded,
            (Self::Finite(left), Self::Finite(right)) => Self::Finite((*left).max(*right)),
            (Self::Finite(0), Self::Symbolic(value)) | (Self::Symbolic(value), Self::Finite(0)) => {
                Self::Symbolic(value.clone())
            }
            (Self::Symbolic(left), Self::Symbolic(right)) => {
                match (
                    SymbolicMultiple::parse(left),
                    SymbolicMultiple::parse(right),
                ) {
                    (Some(left), Some(right)) if left.base == right.base => {
                        Self::Symbolic(left.with_multiplier(left.multiplier.max(right.multiplier)))
                    }
                    _ if left == right => Self::Symbolic(left.clone()),
                    _ => Self::Unknown,
                }
            }
            _ => Self::Unknown,
        }
    }

    /// Composes two relative dependency bounds across a function call.
    ///
    /// For example, applying a `n-1` rolling window to a value which already
    /// depends on `n-1` past rows yields `2*(n-1)`. Symbolic arithmetic stays
    /// deliberately small: unsupported expressions become `unknown` rather
    /// than claiming a dependency bound the compiler cannot prove.
    #[must_use]
    pub fn compose(&self, relative: &Self) -> Self {
        match (self, relative) {
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            (Self::Unbounded, _) | (_, Self::Unbounded) => Self::Unbounded,
            (Self::Finite(left), Self::Finite(right)) => left
                .checked_add(*right)
                .map_or(Self::Unbounded, Self::Finite),
            (Self::Finite(0), value) | (value, Self::Finite(0)) => value.clone(),
            (Self::Symbolic(left), Self::Symbolic(right)) => {
                match (
                    SymbolicMultiple::parse(left),
                    SymbolicMultiple::parse(right),
                ) {
                    (Some(left), Some(right)) if left.base == right.base => left
                        .multiplier
                        .checked_add(right.multiplier)
                        .map_or(Self::Unbounded, |multiplier| {
                            Self::Symbolic(left.with_multiplier(multiplier))
                        }),
                    _ => Self::Unknown,
                }
            }
            _ => Self::Unknown,
        }
    }

    /// Replaces callable parameter names in a symbolic contract with the
    /// corresponding static argument expressions at one call site.
    #[must_use]
    pub fn substitute(&self, values: &BTreeMap<String, String>) -> Self {
        let Self::Symbolic(expression) = self else {
            return self.clone();
        };
        let substituted = substitute_symbolic_names(expression, values);
        if let Some(value) = evaluate_nonnegative_temporal_constant(&substituted) {
            return Self::Finite(value);
        }
        SymbolicMultiple::parse(&substituted).map_or(Self::Unknown, |normalized| {
            Self::Symbolic(normalized.with_multiplier(normalized.multiplier))
        })
    }

    #[must_use]
    fn is_within(&self, allowed: &Self) -> bool {
        match (self, allowed) {
            (_, Self::Unknown | Self::Unbounded) => true,
            (Self::Finite(actual), Self::Finite(limit)) => actual <= limit,
            (Self::Symbolic(actual), Self::Symbolic(limit)) => actual == limit,
            (Self::Unbounded | Self::Unknown, _) => false,
            _ => false,
        }
    }
}

fn substitute_symbolic_names(expression: &str, values: &BTreeMap<String, String>) -> String {
    let mut names = values.keys().collect::<Vec<_>>();
    names.sort_by_key(|name| std::cmp::Reverse(name.len()));
    let mut output = String::with_capacity(expression.len());
    let mut offset = 0;
    while offset < expression.len() {
        let remaining = &expression[offset..];
        let previous = expression[..offset].chars().next_back();
        let replacement = names.iter().find_map(|name| {
            let suffix = remaining.strip_prefix(name.as_str())?;
            let next = suffix.chars().next();
            if previous.is_some_and(symbolic_identifier_character)
                || next.is_some_and(symbolic_identifier_character)
            {
                return None;
            }
            Some((
                name.len(),
                values.get(name.as_str()).expect("key came from map"),
            ))
        });
        if let Some((length, value)) = replacement {
            output.push_str(value);
            offset += length;
        } else {
            let character = remaining.chars().next().expect("offset is in bounds");
            output.push(character);
            offset += character.len_utf8();
        }
    }
    output
}

fn symbolic_identifier_character(character: char) -> bool {
    character == '_' || character.is_alphanumeric()
}

fn evaluate_nonnegative_temporal_constant(expression: &str) -> Option<u64> {
    let mut parser = ConstantExpressionParser {
        input: expression.as_bytes(),
        offset: 0,
    };
    let value = parser.expression()?;
    parser.whitespace();
    (parser.offset == parser.input.len() && value >= 0)
        .then(|| u64::try_from(value).ok())
        .flatten()
}

struct ConstantExpressionParser<'input> {
    input: &'input [u8],
    offset: usize,
}

impl ConstantExpressionParser<'_> {
    fn expression(&mut self) -> Option<i128> {
        let mut value = self.term()?;
        loop {
            self.whitespace();
            match self.peek() {
                Some(b'+') => {
                    self.offset += 1;
                    value = value.checked_add(self.term()?)?;
                }
                Some(b'-') => {
                    self.offset += 1;
                    value = value.checked_sub(self.term()?)?;
                }
                _ => return Some(value),
            }
        }
    }

    fn term(&mut self) -> Option<i128> {
        let mut value = self.factor()?;
        loop {
            self.whitespace();
            if self.peek() != Some(b'*') {
                return Some(value);
            }
            self.offset += 1;
            value = value.checked_mul(self.factor()?)?;
        }
    }

    fn factor(&mut self) -> Option<i128> {
        self.whitespace();
        if self.peek() == Some(b'(') {
            self.offset += 1;
            let value = self.expression()?;
            self.whitespace();
            if self.peek() != Some(b')') {
                return None;
            }
            self.offset += 1;
            return Some(value);
        }
        let start = self.offset;
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.offset += 1;
        }
        (self.offset > start)
            .then(|| std::str::from_utf8(&self.input[start..self.offset]).ok())
            .flatten()?
            .parse()
            .ok()
    }

    fn whitespace(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.offset += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.offset).copied()
    }
}

/// A narrow, deterministic symbolic normal form used by temporal composition.
/// `base` is opaque; recognizing a positive integer multiplier is enough to
/// preserve common rolling-window expressions without embedding a CAS.
#[derive(Clone, Debug, Eq, PartialEq)]
struct SymbolicMultiple {
    multiplier: u64,
    base: String,
}

impl SymbolicMultiple {
    fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        if let Some((multiplier, base)) = value.split_once("*(")
            && let Some(base) = base.strip_suffix(')')
            && let Ok(multiplier) = multiplier.parse::<u64>()
            && multiplier > 0
            && !base.trim().is_empty()
        {
            return Some(Self {
                multiplier,
                base: base.trim().to_owned(),
            });
        }
        Some(Self {
            multiplier: 1,
            base: value.to_owned(),
        })
    }

    fn with_multiplier(&self, multiplier: u64) -> String {
        if multiplier == 1 {
            self.base.clone()
        } else {
            format!("{multiplier}*({})", self.base)
        }
    }
}

/// Availability of a value relative to its decision time.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum Availability {
    Immediate,
    Named(String),
    Unknown,
}

impl Availability {
    #[must_use]
    fn join(&self, other: &Self) -> Self {
        match (self, other) {
            (Self::Immediate, value) | (value, Self::Immediate) => value.clone(),
            (left, right) if left == right => left.clone(),
            _ => Self::Unknown,
        }
    }

    #[must_use]
    fn is_within(&self, allowed: &Self) -> bool {
        allowed == &Self::Unknown || self == allowed
    }
}

/// Minimal temporal summary carried by every callable type.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct TemporalSummary {
    pub past: TemporalBound,
    pub future: TemporalBound,
    pub availability: Availability,
}

impl TemporalSummary {
    #[must_use]
    pub const fn pointwise() -> Self {
        Self {
            past: TemporalBound::Finite(0),
            future: TemporalBound::Finite(0),
            availability: Availability::Immediate,
        }
    }

    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            past: TemporalBound::Unknown,
            future: TemporalBound::Unknown,
            availability: Availability::Unknown,
        }
    }

    #[must_use]
    pub fn join(&self, other: &Self) -> Self {
        Self {
            past: self.past.join(&other.past),
            future: self.future.join(&other.future),
            availability: self.availability.join(&other.availability),
        }
    }

    /// Applies a callable's relative temporal dependency to an argument value.
    #[must_use]
    pub fn compose(&self, relative: &Self) -> Self {
        Self {
            past: self.past.compose(&relative.past),
            future: self.future.compose(&relative.future),
            availability: self.availability.join(&relative.availability),
        }
    }

    #[must_use]
    pub fn substitute(&self, values: &BTreeMap<String, String>) -> Self {
        Self {
            past: self.past.substitute(values),
            future: self.future.substitute(values),
            availability: self.availability.clone(),
        }
    }

    #[must_use]
    pub fn is_within(&self, allowed: &Self) -> bool {
        self.past.is_within(&allowed.past)
            && self.future.is_within(&allowed.future)
            && self.availability.is_within(&allowed.availability)
    }
}

impl Default for TemporalSummary {
    fn default() -> Self {
        Self::pointwise()
    }
}

/// Statically known alignment of data values.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Alignment {
    Positional,
    Labelled,
    AsOf,
    Unknown,
}

/// Conservative data-shape facts. `None` means that the fact is unknown, not
/// false. Domain extensions can later replace this with transfer expressions
/// while preserving the callable type layout.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct DataProperties {
    pub schema: Option<String>,
    pub axes: Option<Vec<String>>,
    pub alignment: Alignment,
    /// Keys which are statically known to be in ascending lexicographic order.
    pub ordered_by: Option<Vec<String>>,
    /// Keys which are statically known to identify rows uniquely.
    pub unique_by: Option<Vec<String>>,
    pub preserves_length: Option<bool>,
    pub materializes: Option<bool>,
    pub reshapes: Option<bool>,
    pub nulls_possible: Option<bool>,
    pub nan_possible: Option<bool>,
    pub nonfinite_possible: Option<bool>,
    pub nonfinite_policy: Option<String>,
}

impl DataProperties {
    #[must_use]
    pub const fn scalar() -> Self {
        Self {
            schema: None,
            axes: None,
            alignment: Alignment::Positional,
            ordered_by: None,
            unique_by: None,
            preserves_length: Some(true),
            materializes: Some(false),
            reshapes: Some(false),
            nulls_possible: Some(false),
            nan_possible: Some(false),
            nonfinite_possible: Some(false),
            nonfinite_policy: None,
        }
    }

    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            schema: None,
            axes: None,
            alignment: Alignment::Unknown,
            ordered_by: None,
            unique_by: None,
            preserves_length: None,
            materializes: None,
            reshapes: None,
            nulls_possible: None,
            nan_possible: None,
            nonfinite_possible: None,
            nonfinite_policy: None,
        }
    }

    #[must_use]
    pub fn join(&self, other: &Self) -> Self {
        Self {
            schema: equal_fact(&self.schema, &other.schema).flatten(),
            axes: equal_fact(&self.axes, &other.axes).flatten(),
            alignment: if self.alignment == other.alignment {
                self.alignment.clone()
            } else {
                Alignment::Unknown
            },
            ordered_by: equal_fact(&self.ordered_by, &other.ordered_by).flatten(),
            unique_by: equal_fact(&self.unique_by, &other.unique_by).flatten(),
            preserves_length: equal_fact(&self.preserves_length, &other.preserves_length).flatten(),
            materializes: equal_fact(&self.materializes, &other.materializes).flatten(),
            reshapes: equal_fact(&self.reshapes, &other.reshapes).flatten(),
            nulls_possible: equal_fact(&self.nulls_possible, &other.nulls_possible).flatten(),
            nan_possible: equal_fact(&self.nan_possible, &other.nan_possible).flatten(),
            nonfinite_possible: equal_fact(&self.nonfinite_possible, &other.nonfinite_possible)
                .flatten(),
            nonfinite_policy: equal_fact(&self.nonfinite_policy, &other.nonfinite_policy).flatten(),
        }
    }

    #[must_use]
    fn is_within(&self, allowed: &Self) -> bool {
        fact_is_within(&self.schema, &allowed.schema)
            && fact_is_within(&self.axes, &allowed.axes)
            && (allowed.alignment == Alignment::Unknown || self.alignment == allowed.alignment)
            && fact_is_within(&self.ordered_by, &allowed.ordered_by)
            && fact_is_within(&self.unique_by, &allowed.unique_by)
            && fact_is_within(&self.preserves_length, &allowed.preserves_length)
            && fact_is_within(&self.materializes, &allowed.materializes)
            && fact_is_within(&self.reshapes, &allowed.reshapes)
            && fact_is_within(&self.nulls_possible, &allowed.nulls_possible)
            && fact_is_within(&self.nan_possible, &allowed.nan_possible)
            && fact_is_within(&self.nonfinite_possible, &allowed.nonfinite_possible)
            && fact_is_within(&self.nonfinite_policy, &allowed.nonfinite_policy)
    }
}

impl Default for DataProperties {
    fn default() -> Self {
        Self::scalar()
    }
}

fn equal_fact<T: Clone + PartialEq>(left: &T, right: &T) -> Option<T> {
    (left == right).then(|| left.clone())
}

fn fact_is_within<T: PartialEq>(actual: &Option<T>, allowed: &Option<T>) -> bool {
    allowed
        .as_ref()
        .is_none_or(|allowed| actual.as_ref() == Some(allowed))
}

/// The summaries produced by calling a function. They are latent: merely
/// evaluating a function value does not produce them.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct CallSummaries {
    pub effects: EffectRow,
    pub temporal: TemporalSummary,
    pub data: DataProperties,
}

impl CallSummaries {
    #[must_use]
    pub const fn pure_scalar() -> Self {
        Self {
            effects: EffectRow::pure(),
            temporal: TemporalSummary::pointwise(),
            data: DataProperties::scalar(),
        }
    }

    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            effects: EffectRow::unknown(),
            temporal: TemporalSummary::unknown(),
            data: DataProperties::unknown(),
        }
    }

    #[must_use]
    pub fn join(&self, other: &Self) -> Self {
        Self {
            effects: self.effects.union(&other.effects),
            temporal: self.temporal.join(&other.temporal),
            data: self.data.join(&other.data),
        }
    }

    #[must_use]
    fn is_within(&self, allowed: &Self) -> bool {
        self.effects.is_within(&allowed.effects)
            && self.temporal.is_within(&allowed.temporal)
            && self.data.is_within(&allowed.data)
    }
}

impl Default for CallSummaries {
    fn default() -> Self {
        Self::pure_scalar()
    }
}

/// A callable's type and the summaries produced when it is invoked.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct FunctionType {
    pub parameters: Vec<Type>,
    pub return_type: Box<Type>,
    pub summaries: CallSummaries,
}

impl FunctionType {
    #[must_use]
    pub fn new(parameters: Vec<Type>, return_type: Type) -> Self {
        Self {
            parameters,
            return_type: Box::new(return_type),
            summaries: CallSummaries::default(),
        }
    }

    #[must_use]
    pub fn with_summaries(mut self, summaries: CallSummaries) -> Self {
        self.summaries = summaries;
        self
    }
}

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

/// A Python interpreter target used for compatibility-sensitive typing names.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct PythonVersion {
    pub major: u8,
    pub minor: u8,
}

impl PythonVersion {
    pub const PYTHON_3_9: Self = Self::new(3, 9);

    #[must_use]
    pub const fn new(major: u8, minor: u8) -> Self {
        Self { major, minor }
    }

    #[must_use]
    pub const fn at_least(self, major: u8, minor: u8) -> bool {
        self.major > major || (self.major == major && self.minor >= minor)
    }
}

/// One `from module import name` required by a generated annotation.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct PythonTypingImport {
    pub module: &'static str,
    pub name: &'static str,
}

impl PythonTypingImport {
    #[must_use]
    pub const fn new(module: &'static str, name: &'static str) -> Self {
        Self { module, name }
    }

    #[must_use]
    pub const fn typing(name: &'static str) -> Self {
        Self::new("typing", name)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PythonTypeError {
    Unresolved(Box<Type>),
}

impl fmt::Display for PythonTypeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unresolved(ty) => write!(formatter, "cannot emit unresolved type `{ty}`"),
        }
    }
}

impl Error for PythonTypeError {}

/// Stateful local unification context. Failed unifications are transactional:
/// they do not leave partially bound variables behind.
#[derive(Clone, Debug, Default)]
pub struct TypeContext {
    next_variable: u32,
    substitutions: BTreeMap<TypeVarId, Type>,
}

impl TypeContext {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            next_variable: 0,
            substitutions: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn fresh_var(&mut self) -> Type {
        let variable = TypeVarId(self.next_variable);
        self.next_variable = self
            .next_variable
            .checked_add(1)
            .expect("too many type variables");
        Type::TypeVar(variable)
    }

    #[must_use]
    pub fn substitution(&self, variable: TypeVarId) -> Option<Type> {
        self.substitutions.get(&variable).map(|ty| self.resolve(ty))
    }

    /// Recursively applies all known substitutions to a type.
    #[must_use]
    pub fn resolve(&self, ty: &Type) -> Type {
        match ty {
            Type::TypeVar(variable) => self
                .substitutions
                .get(variable)
                .map_or_else(|| ty.clone(), |bound| self.resolve(bound)),
            Type::Option(inner) => Type::option(self.resolve(inner)),
            Type::Union(members) => Type::union(members.iter().map(|member| self.resolve(member))),
            Type::Tuple(members) => {
                Type::Tuple(members.iter().map(|member| self.resolve(member)).collect())
            }
            Type::List(item) => Type::List(Box::new(self.resolve(item))),
            Type::Vector(item) => Type::Vector(Box::new(self.resolve(item))),
            Type::Map(key, value) => {
                Type::Map(Box::new(self.resolve(key)), Box::new(self.resolve(value)))
            }
            Type::Set(item) => Type::Set(Box::new(self.resolve(item))),
            Type::Fn(function) => Type::Fn(FunctionType {
                parameters: function
                    .parameters
                    .iter()
                    .map(|parameter| self.resolve(parameter))
                    .collect(),
                return_type: Box::new(self.resolve(&function.return_type)),
                summaries: function.summaries.clone(),
            }),
            Type::Nominal { binding, args } => Type::Nominal {
                binding: binding.clone(),
                args: args.iter().map(|argument| self.resolve(argument)).collect(),
            },
            _ => ty.clone(),
        }
    }

    /// Unifies two inference types and returns their resolved common type.
    pub fn unify(&mut self, left: &Type, right: &Type) -> Result<Type, TypeError> {
        let checkpoint = self.substitutions.clone();
        match self.unify_inner(left, right) {
            Ok(ty) => Ok(self.resolve(&ty)),
            Err(error) => {
                self.substitutions = checkpoint;
                Err(error)
            }
        }
    }

    fn unify_inner(&mut self, left: &Type, right: &Type) -> Result<Type, TypeError> {
        let left = self.resolve(left);
        let right = self.resolve(right);
        if left == right {
            return Ok(left);
        }

        match (&left, &right) {
            (Type::Error, _) | (_, Type::Error) => Ok(Type::Error),
            (Type::TypeVar(variable), _) => self.bind(*variable, &right),
            (_, Type::TypeVar(variable)) => self.bind(*variable, &left),
            (Type::Any, _) | (_, Type::Any) => Err(TypeError::new(
                TypeErrorKind::AnyRequiresExplicitCast,
                left,
                right,
            )),
            (Type::Unknown, _) | (_, Type::Unknown) => Ok(Type::Unknown),
            (Type::Never, _) => Ok(right),
            (_, Type::Never) => Ok(left),
            (Type::Int, Type::Float) | (Type::Float, Type::Int) => Ok(Type::Float),
            (Type::Option(left), Type::Option(right)) => {
                Ok(Type::option(self.unify_inner(left, right)?))
            }
            (Type::Tuple(left), Type::Tuple(right)) => {
                Ok(Type::Tuple(self.unify_sequences(left, right)?))
            }
            (Type::List(left), Type::List(right)) => {
                Ok(Type::List(Box::new(self.unify_inner(left, right)?)))
            }
            (Type::Vector(left), Type::Vector(right)) => {
                Ok(Type::Vector(Box::new(self.unify_inner(left, right)?)))
            }
            (Type::Map(left_key, left_value), Type::Map(right_key, right_value)) => Ok(Type::Map(
                Box::new(self.unify_inner(left_key, right_key)?),
                Box::new(self.unify_inner(left_value, right_value)?),
            )),
            (Type::Set(left), Type::Set(right)) => {
                Ok(Type::Set(Box::new(self.unify_inner(left, right)?)))
            }
            (Type::Union(left), Type::Union(right)) => self.unify_unions(left, right),
            (Type::Fn(left), Type::Fn(right)) => self.unify_functions(left, right),
            (
                Type::Nominal {
                    binding: left_binding,
                    args: left_args,
                },
                Type::Nominal {
                    binding: right_binding,
                    args: right_args,
                },
            ) if left_binding == right_binding => Ok(Type::Nominal {
                binding: left_binding.clone(),
                args: self.unify_sequences(left_args, right_args)?,
            }),
            _ => Err(TypeError::mismatch(left, right)),
        }
    }

    fn bind(&mut self, variable: TypeVarId, ty: &Type) -> Result<Type, TypeError> {
        let ty = self.resolve(ty);
        if ty == Type::TypeVar(variable) {
            return Ok(ty);
        }
        if self.occurs(variable, &ty) {
            return Err(TypeError {
                kind: TypeErrorKind::OccursCheck { variable },
                expected: Some(Box::new(Type::TypeVar(variable))),
                found: Some(Box::new(ty)),
            });
        }
        self.substitutions.insert(variable, ty.clone());
        Ok(ty)
    }

    fn occurs(&self, variable: TypeVarId, ty: &Type) -> bool {
        match self.resolve(ty) {
            Type::TypeVar(candidate) => variable == candidate,
            Type::Option(inner) | Type::List(inner) | Type::Vector(inner) | Type::Set(inner) => {
                self.occurs(variable, &inner)
            }
            Type::Union(members) | Type::Tuple(members) => {
                members.iter().any(|member| self.occurs(variable, member))
            }
            Type::Map(key, value) => self.occurs(variable, &key) || self.occurs(variable, &value),
            Type::Fn(function) => {
                function
                    .parameters
                    .iter()
                    .any(|parameter| self.occurs(variable, parameter))
                    || self.occurs(variable, &function.return_type)
            }
            Type::Nominal { args, .. } => {
                args.iter().any(|argument| self.occurs(variable, argument))
            }
            _ => false,
        }
    }

    fn unify_sequences(&mut self, left: &[Type], right: &[Type]) -> Result<Vec<Type>, TypeError> {
        if left.len() != right.len() {
            return Err(TypeError {
                kind: TypeErrorKind::ArityMismatch {
                    expected: left.len(),
                    found: right.len(),
                },
                expected: Some(Box::new(Type::Tuple(left.to_vec()))),
                found: Some(Box::new(Type::Tuple(right.to_vec()))),
            });
        }
        left.iter()
            .zip(right)
            .map(|(left, right)| self.unify_inner(left, right))
            .collect()
    }

    fn unify_unions(&mut self, left: &[Type], right: &[Type]) -> Result<Type, TypeError> {
        if left.len() != right.len() {
            return Err(TypeError::mismatch(
                Type::Union(left.to_vec()),
                Type::Union(right.to_vec()),
            ));
        }

        let checkpoint = self.substitutions.clone();
        let mut matched = vec![false; right.len()];
        if self.match_union_members(left, right, 0, &mut matched) {
            Ok(Type::union(left.iter().map(|member| self.resolve(member))))
        } else {
            self.substitutions = checkpoint;
            Err(TypeError::mismatch(
                Type::Union(left.to_vec()),
                Type::Union(right.to_vec()),
            ))
        }
    }

    fn match_union_members(
        &mut self,
        left: &[Type],
        right: &[Type],
        index: usize,
        matched: &mut [bool],
    ) -> bool {
        if index == left.len() {
            return true;
        }

        for candidate in 0..right.len() {
            if matched[candidate] {
                continue;
            }
            let checkpoint = self.substitutions.clone();
            if self.unify_inner(&left[index], &right[candidate]).is_ok() {
                matched[candidate] = true;
                if self.match_union_members(left, right, index + 1, matched) {
                    return true;
                }
                matched[candidate] = false;
            }
            self.substitutions = checkpoint;
        }
        false
    }

    fn unify_functions(
        &mut self,
        left: &FunctionType,
        right: &FunctionType,
    ) -> Result<Type, TypeError> {
        if left.summaries != right.summaries {
            return Err(TypeError {
                kind: TypeErrorKind::SummaryMismatch,
                expected: Some(Box::new(Type::Fn(left.clone()))),
                found: Some(Box::new(Type::Fn(right.clone()))),
            });
        }
        let parameters = self.unify_sequences(&left.parameters, &right.parameters)?;
        let return_type = self.unify_inner(&left.return_type, &right.return_type)?;
        Ok(Type::Fn(
            FunctionType::new(parameters, return_type).with_summaries(left.summaries.clone()),
        ))
    }

    /// Directional compatibility (`source` can flow into `target`). This is
    /// intentionally stricter than Python at an `Any -> T` boundary.
    #[must_use]
    pub fn is_assignable(&self, source: &Type, target: &Type) -> bool {
        self.check_assignable(source, target).is_ok()
    }

    /// Structured counterpart to [`Self::is_assignable`].
    pub fn check_assignable(&self, source: &Type, target: &Type) -> Result<(), TypeError> {
        let source = self.resolve(source);
        let target = self.resolve(target);
        if is_assignable_resolved(&source, &target) {
            Ok(())
        } else {
            let kind = if source == Type::Any && target != Type::Any {
                TypeErrorKind::AnyRequiresExplicitCast
            } else if matches!((&source, &target), (Type::Fn(_), Type::Fn(_))) {
                TypeErrorKind::SummaryOrFunctionMismatch
            } else {
                TypeErrorKind::Mismatch
            };
            Err(TypeError::new(kind, target, source))
        }
    }

    /// Least conservative type containing both inputs, used for branches and
    /// collection literals. Unlike unification this never binds variables.
    #[must_use]
    pub fn join(&self, left: &Type, right: &Type) -> Type {
        join_resolved(&self.resolve(left), &self.resolve(right))
    }

    #[must_use]
    pub fn join_all<'a>(&self, types: impl IntoIterator<Item = &'a Type>) -> Type {
        types
            .into_iter()
            .fold(Type::Never, |joined, ty| self.join(&joined, ty))
    }
}

fn is_assignable_resolved(source: &Type, target: &Type) -> bool {
    if source == target || source == &Type::Error || target == &Type::Error {
        return true;
    }
    if source == &Type::Never || target == &Type::Any {
        return true;
    }
    if source == &Type::Any {
        return false;
    }
    if source == &Type::Unknown || target == &Type::Unknown {
        return source == target;
    }
    if source == &Type::Int && target == &Type::Float {
        return true;
    }

    match (source, target) {
        (Type::None, Type::Option(_)) => true,
        (Type::Option(source), Type::Option(target)) => is_assignable_resolved(source, target),
        (source, Type::Option(target)) => {
            source != &Type::None && is_assignable_resolved(source, target)
        }
        (Type::Option(source), Type::Union(targets)) => {
            is_assignable_resolved(source, &Type::Union(targets.clone()))
                && targets.iter().any(|target| target == &Type::None)
        }
        (Type::Union(sources), target) => sources
            .iter()
            .all(|source| is_assignable_resolved(source, target)),
        (source, Type::Union(targets)) => targets
            .iter()
            .any(|target| is_assignable_resolved(source, target)),
        (Type::Tuple(sources), Type::Tuple(targets)) => {
            sources.len() == targets.len()
                && sources
                    .iter()
                    .zip(targets)
                    .all(|(source, target)| is_assignable_resolved(source, target))
        }
        (Type::List(source), Type::List(target))
        | (Type::Vector(source), Type::Vector(target))
        | (Type::Set(source), Type::Set(target)) => source == target,
        (Type::Map(source_key, source_value), Type::Map(target_key, target_value)) => {
            source_key == target_key && source_value == target_value
        }
        (Type::Fn(source), Type::Fn(target)) => function_is_assignable(source, target),
        (
            Type::Nominal {
                binding: source_binding,
                args: source_args,
            },
            Type::Nominal {
                binding: target_binding,
                args: target_args,
            },
        ) => source_binding == target_binding && source_args == target_args,
        (Type::TypeVar(source), Type::TypeVar(target)) => source == target,
        _ => false,
    }
}

fn function_is_assignable(source: &FunctionType, target: &FunctionType) -> bool {
    source.parameters.len() == target.parameters.len()
        && source
            .parameters
            .iter()
            .zip(&target.parameters)
            .all(|(source, target)| is_assignable_resolved(target, source))
        && is_assignable_resolved(&source.return_type, &target.return_type)
        && source.summaries.is_within(&target.summaries)
}

fn join_resolved(left: &Type, right: &Type) -> Type {
    if left == right {
        return left.clone();
    }
    match (left, right) {
        (Type::Error, _) | (_, Type::Error) => Type::Error,
        (Type::Unknown, _) | (_, Type::Unknown) => Type::Unknown,
        (Type::Any, _) | (_, Type::Any) => Type::Any,
        (Type::Never, other) | (other, Type::Never) => other.clone(),
        (Type::Int, Type::Float) | (Type::Float, Type::Int) => Type::Float,
        (Type::Option(left), Type::Option(right)) => Type::option(join_resolved(left, right)),
        (Type::List(left), Type::List(right)) => Type::List(Box::new(join_resolved(left, right))),
        (Type::Vector(left), Type::Vector(right)) => {
            Type::Vector(Box::new(join_resolved(left, right)))
        }
        (Type::Set(left), Type::Set(right)) => Type::Set(Box::new(join_resolved(left, right))),
        (Type::Map(left_key, left_value), Type::Map(right_key, right_value)) => Type::Map(
            Box::new(join_resolved(left_key, right_key)),
            Box::new(join_resolved(left_value, right_value)),
        ),
        (Type::Tuple(left), Type::Tuple(right)) if left.len() == right.len() => Type::Tuple(
            left.iter()
                .zip(right)
                .map(|(left, right)| join_resolved(left, right))
                .collect(),
        ),
        (Type::Fn(left), Type::Fn(right)) if left.parameters == right.parameters => Type::Fn(
            FunctionType::new(
                left.parameters.clone(),
                join_resolved(&left.return_type, &right.return_type),
            )
            .with_summaries(left.summaries.join(&right.summaries)),
        ),
        _ if is_assignable_resolved(left, right) => right.clone(),
        _ if is_assignable_resolved(right, left) => left.clone(),
        _ => Type::union([left.clone(), right.clone()]),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeError {
    pub kind: TypeErrorKind,
    pub expected: Option<Box<Type>>,
    pub found: Option<Box<Type>>,
}

impl TypeError {
    #[must_use]
    pub fn new(kind: TypeErrorKind, expected: Type, found: Type) -> Self {
        Self {
            kind,
            expected: Some(Box::new(expected)),
            found: Some(Box::new(found)),
        }
    }

    #[must_use]
    pub fn mismatch(expected: Type, found: Type) -> Self {
        Self::new(TypeErrorKind::Mismatch, expected, found)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypeErrorKind {
    Mismatch,
    ArityMismatch { expected: usize, found: usize },
    OccursCheck { variable: TypeVarId },
    AnyRequiresExplicitCast,
    SummaryMismatch,
    SummaryOrFunctionMismatch,
}

impl fmt::Display for TypeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.kind, &self.expected, &self.found) {
            (TypeErrorKind::OccursCheck { variable }, _, Some(found)) => {
                write!(
                    formatter,
                    "type variable ?{} occurs in `{found}`",
                    variable.0
                )
            }
            (TypeErrorKind::ArityMismatch { expected, found }, _, _) => {
                write!(
                    formatter,
                    "expected {expected} type arguments, found {found}"
                )
            }
            (TypeErrorKind::AnyRequiresExplicitCast, Some(expected), Some(found)) => write!(
                formatter,
                "cannot unify `{expected}` with `{found}` across an Any boundary without an explicit cast"
            ),
            (TypeErrorKind::SummaryMismatch, _, _) => {
                formatter.write_str("function latent summaries do not match")
            }
            (TypeErrorKind::SummaryOrFunctionMismatch, _, _) => {
                formatter.write_str("function signature or latent summaries are not assignable")
            }
            (_, Some(expected), Some(found)) => {
                write!(formatter, "expected `{expected}`, found `{found}`")
            }
            _ => formatter.write_str("type error"),
        }
    }
}

impl Error for TypeError {}

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

/// Closed scalar operators supplied by the core prelude. Extension-owned
/// nominal instances live in interfaces and are not added to this table.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScalarOperator {
    Add,
    Subtract,
    Multiply,
    TrueDivide,
    FloorDivide,
    Remainder,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
    Negate,
    Positive,
    Abs,
}

impl ScalarOperator {
    /// Stable wire spelling used by `.osri` and the `:osiris/operator`
    /// declaration metadata.  This is intentionally independent of Rust's
    /// enum debug representation.
    #[must_use]
    pub const fn stable_name(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Subtract => "subtract",
            Self::Multiply => "multiply",
            Self::TrueDivide => "true-divide",
            Self::FloorDivide => "floor-divide",
            Self::Remainder => "remainder",
            Self::Less => "less",
            Self::LessEqual => "less-equal",
            Self::Greater => "greater",
            Self::GreaterEqual => "greater-equal",
            Self::Equal => "equal",
            Self::NotEqual => "not-equal",
            Self::Negate => "negate",
            Self::Positive => "positive",
            Self::Abs => "abs",
        }
    }

    /// Parse the closed metadata vocabulary.  `divide`/`/` are accepted as a
    /// convenience spelling but normalize to the canonical `true-divide` id.
    #[must_use]
    pub fn from_stable_name(name: &str) -> Option<Self> {
        Some(match name.trim_start_matches(':') {
            "add" | "+" => Self::Add,
            "subtract" | "sub" | "-" => Self::Subtract,
            "multiply" | "mul" | "*" => Self::Multiply,
            "true-divide" | "divide" | "div" | "/" => Self::TrueDivide,
            "floor-divide" | "floor" | "//" => Self::FloorDivide,
            "remainder" | "mod" | "%" => Self::Remainder,
            "less" | "<" => Self::Less,
            "less-equal" | "<=" => Self::LessEqual,
            "greater" | ">" => Self::Greater,
            "greater-equal" | ">=" => Self::GreaterEqual,
            "equal" | "=" | "==" => Self::Equal,
            "not-equal" | "not=" | "!=" => Self::NotEqual,
            "negate" => Self::Negate,
            "positive" => Self::Positive,
            "abs" => Self::Abs,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OperatorSignature {
    pub operator: ScalarOperator,
    pub operands: Vec<Type>,
    pub result: Type,
    pub summaries: CallSummaries,
}

/// A closed, statically selected operator implementation published by a
/// module.  `binding` identifies the implementation callable; `owner_binding`
/// identifies a public nominal type owned by the declaring module and enforces
/// the orphan rule.  No runtime dispatch is implied by this data structure.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OperatorInstance {
    pub id: String,
    pub binding: String,
    pub owner_binding: String,
    pub operator: ScalarOperator,
    pub operands: Vec<Type>,
    pub result: Type,
    pub summaries: CallSummaries,
}

impl OperatorInstance {
    #[must_use]
    pub fn new(
        binding: impl Into<String>,
        owner_binding: impl Into<String>,
        operator: ScalarOperator,
        operands: Vec<Type>,
        result: Type,
        summaries: CallSummaries,
    ) -> Self {
        let binding = binding.into();
        let owner_binding = owner_binding.into();
        let id = format!("{}::operator::{}", binding, operator.stable_name());
        Self {
            id,
            binding,
            owner_binding,
            operator,
            operands,
            result,
            summaries,
        }
    }

    #[must_use]
    pub fn function_type(&self) -> FunctionType {
        FunctionType::new(self.operands.clone(), self.result.clone())
            .with_summaries(self.summaries.clone())
    }
}

impl OperatorSignature {
    #[must_use]
    fn pure(operator: ScalarOperator, operands: Vec<Type>, result: Type) -> Self {
        Self {
            operator,
            operands,
            result,
            summaries: CallSummaries::pure_scalar(),
        }
    }

    #[must_use]
    pub fn function_type(&self) -> FunctionType {
        FunctionType::new(self.operands.clone(), self.result.clone())
            .with_summaries(self.summaries.clone())
    }
}

/// Returns the deterministic core scalar overload table for one operator.
#[must_use]
pub fn scalar_operator_signatures(operator: ScalarOperator) -> Vec<OperatorSignature> {
    use ScalarOperator::{
        Abs, Add, Equal, FloorDivide, Greater, GreaterEqual, Less, LessEqual, Multiply, Negate,
        NotEqual, Positive, Remainder, Subtract, TrueDivide,
    };

    match operator {
        Add | Subtract | Multiply | FloorDivide | Remainder => {
            let mut signatures = numeric_binary_signatures(operator, false);
            if operator == Add {
                signatures.push(OperatorSignature::pure(
                    operator,
                    vec![Type::Str, Type::Str],
                    Type::Str,
                ));
                signatures.push(OperatorSignature::pure(
                    operator,
                    vec![Type::Bytes, Type::Bytes],
                    Type::Bytes,
                ));
            }
            signatures
        }
        TrueDivide => numeric_binary_signatures(operator, true),
        Less | LessEqual | Greater | GreaterEqual => {
            let mut signatures = numeric_comparison_signatures(operator);
            for ty in [Type::Str, Type::Bytes] {
                signatures.push(OperatorSignature::pure(
                    operator,
                    vec![ty.clone(), ty],
                    Type::Bool,
                ));
            }
            signatures
        }
        Equal | NotEqual => {
            let mut signatures = numeric_comparison_signatures(operator);
            for ty in [Type::Bool, Type::Str, Type::Bytes, Type::None] {
                signatures.push(OperatorSignature::pure(
                    operator,
                    vec![ty.clone(), ty],
                    Type::Bool,
                ));
            }
            signatures
        }
        Negate | Positive | Abs => vec![
            OperatorSignature::pure(operator, vec![Type::Int], Type::Int),
            OperatorSignature::pure(operator, vec![Type::Float], Type::Float),
        ],
    }
}

fn numeric_binary_signatures(
    operator: ScalarOperator,
    always_float: bool,
) -> Vec<OperatorSignature> {
    [
        (Type::Int, Type::Int),
        (Type::Int, Type::Float),
        (Type::Float, Type::Int),
        (Type::Float, Type::Float),
    ]
    .into_iter()
    .map(|(left, right)| {
        let result = if always_float || left == Type::Float || right == Type::Float {
            Type::Float
        } else {
            Type::Int
        };
        OperatorSignature::pure(operator, vec![left, right], result)
    })
    .collect()
}

fn numeric_comparison_signatures(operator: ScalarOperator) -> Vec<OperatorSignature> {
    numeric_binary_signatures(operator, false)
        .into_iter()
        .map(|mut signature| {
            signature.result = Type::Bool;
            signature
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::{
        Alignment, Availability, CallSummaries, DataProperties, Effect, EffectRow, FunctionType,
        PythonTypingImport, PythonVersion, ScalarOperator, TemporalBound, TemporalSummary, Type,
        TypeContext, TypeErrorKind, TypeLiteral, TypeVarId, parse_type,
        python_builtin_exception_binding, python_builtin_exception_from_binding,
        python_builtin_exception_name, scalar_operator_signatures,
    };
    use crate::{
        reader::read,
        syntax::{Form, FormKind},
    };

    fn read_one(source: &str) -> Form {
        let document = read(source);
        assert!(document.diagnostics.is_empty(), "reader diagnostics");
        assert_eq!(document.forms.len(), 1);
        document.forms.into_iter().next().expect("one form")
    }

    #[test]
    fn infers_a_generic_collection_element() {
        let mut context = TypeContext::new();
        let variable = context.fresh_var();
        let expected = Type::List(Box::new(variable.clone()));

        let unified = context
            .unify(&expected, &Type::List(Box::new(Type::Int)))
            .expect("types unify");

        assert_eq!(unified, Type::List(Box::new(Type::Int)));
        let Type::TypeVar(variable) = variable else {
            panic!("fresh type variable")
        };
        assert_eq!(context.substitution(variable), Some(Type::Int));
    }

    #[test]
    fn occurs_check_rejects_an_infinite_type_without_leaking_a_binding() {
        let mut context = TypeContext::new();
        let variable = context.fresh_var();
        let recursive = Type::List(Box::new(variable.clone()));

        let error = context
            .unify(&variable, &recursive)
            .expect_err("infinite type is rejected");

        assert!(matches!(error.kind, TypeErrorKind::OccursCheck { .. }));
        let Type::TypeVar(variable) = variable else {
            panic!("fresh type variable")
        };
        assert_eq!(context.substitution(variable), None);
    }

    #[test]
    fn any_is_a_one_way_explicit_boundary() {
        let context = TypeContext::new();

        assert!(context.is_assignable(&Type::Int, &Type::Any));
        assert!(!context.is_assignable(&Type::Any, &Type::Int));
        let error = TypeContext::new()
            .unify(&Type::Any, &Type::Int)
            .expect_err("Any cannot silently become Int");
        assert_eq!(error.kind, TypeErrorKind::AnyRequiresExplicitCast);
    }

    #[test]
    fn nominal_identity_is_the_defining_type_binding_not_the_short_name() {
        let left = Type::Nominal {
            binding: "dep.alpha::type::X".to_owned(),
            args: Vec::new(),
        };
        let right = Type::Nominal {
            binding: "dep.beta::type::X".to_owned(),
            args: Vec::new(),
        };
        let context = TypeContext::new();

        assert!(!context.is_assignable(&left, &right));
        assert!(TypeContext::new().unify(&left, &right).is_err());
        assert_eq!(
            Type::union([left, right]),
            Type::Union(vec![
                Type::Nominal {
                    binding: "dep.alpha::type::X".to_owned(),
                    args: Vec::new(),
                },
                Type::Nominal {
                    binding: "dep.beta::type::X".to_owned(),
                    args: Vec::new(),
                },
            ])
        );
    }

    #[test]
    fn options_and_unions_are_canonical_and_assignable() {
        let context = TypeContext::new();
        let option = Type::union([Type::None, Type::Int, Type::Int]);

        assert_eq!(option, Type::Option(Box::new(Type::Int)));
        assert!(context.is_assignable(&Type::None, &option));
        assert!(context.is_assignable(&Type::Int, &option));
        assert!(!context.is_assignable(&Type::Str, &option));
        assert_eq!(context.join(&Type::None, &Type::Int), option);
        assert_eq!(
            Type::union([Type::Str, Type::Int]),
            Type::Union(vec![Type::Int, Type::Str])
        );
    }

    #[test]
    fn joins_collection_branches_elementwise() {
        let context = TypeContext::new();
        assert_eq!(
            context.join(
                &Type::Vector(Box::new(Type::Int)),
                &Type::Vector(Box::new(Type::Never)),
            ),
            Type::Vector(Box::new(Type::Int))
        );
        assert_eq!(
            context.join(
                &Type::Map(Box::new(Type::Str), Box::new(Type::Int)),
                &Type::Map(Box::new(Type::Str), Box::new(Type::Float)),
            ),
            Type::Map(Box::new(Type::Str), Box::new(Type::Float))
        );
    }

    #[test]
    fn function_assignment_is_contravariant_and_summary_aware() {
        let context = TypeContext::new();
        let broad_parameter = Type::Fn(FunctionType::new(vec![Type::Float], Type::Int));
        let narrow_parameter = Type::Fn(FunctionType::new(vec![Type::Int], Type::Float));

        assert!(context.is_assignable(&broad_parameter, &narrow_parameter));
        assert!(!context.is_assignable(&narrow_parameter, &broad_parameter));

        let throwing = Type::Fn(FunctionType::new(vec![], Type::Int).with_summaries(
            CallSummaries {
                effects: EffectRow::singleton(Effect::Throw),
                ..CallSummaries::pure_scalar()
            },
        ));
        let pure = Type::Fn(FunctionType::new(vec![], Type::Int));
        assert!(!context.is_assignable(&throwing, &pure));
        assert!(context.is_assignable(&pure, &throwing));

        let source_unspecified = parse_type(&read_one("(Fn [] -> Int)"), &BTreeMap::new())
            .expect("source function type parses");
        let rich_callback = Type::Fn(FunctionType::new(vec![], Type::Int).with_summaries(
            CallSummaries {
                effects: EffectRow::singleton(Effect::Mutation),
                temporal: TemporalSummary {
                    past: TemporalBound::Finite(2),
                    future: TemporalBound::Finite(1),
                    availability: Availability::Named("published".to_owned()),
                },
                data: DataProperties {
                    axes: Some(vec!["time".to_owned()]),
                    alignment: Alignment::Labelled,
                    preserves_length: Some(true),
                    ..DataProperties::unknown()
                },
            },
        ));
        assert!(context.is_assignable(&rich_callback, &source_unspecified));
        assert!(!context.is_assignable(&rich_callback, &pure));
    }

    #[test]
    fn pointwise_temporal_facts_are_join_identities() {
        let declared = TemporalSummary {
            past: TemporalBound::Symbolic("window".to_owned()),
            future: TemporalBound::Finite(0),
            availability: Availability::Named("published".to_owned()),
        };

        assert_eq!(declared.join(&TemporalSummary::pointwise()), declared);
    }

    #[test]
    fn rolling_temporal_bounds_compose_and_join_symbolically() {
        let rolling = TemporalSummary {
            past: TemporalBound::Symbolic("n-1".to_owned()),
            future: TemporalBound::Finite(0),
            availability: Availability::Named("published".to_owned()),
        };

        let twice = rolling.compose(&rolling);
        assert_eq!(twice.past, TemporalBound::Symbolic("2*(n-1)".to_owned()));
        assert_eq!(rolling.join(&twice), twice);

        let specialized =
            rolling.substitute(&BTreeMap::from([("n".to_owned(), "window".to_owned())]));
        assert_eq!(
            specialized.past,
            TemporalBound::Symbolic("window-1".to_owned())
        );
        let literal = rolling.substitute(&BTreeMap::from([("n".to_owned(), "96".to_owned())]));
        assert_eq!(literal.past, TemporalBound::Finite(95));
    }

    #[test]
    fn parses_explicit_generic_and_function_types() {
        let generic = parse_type(
            &read_one("(Map Str (Option T))"),
            &BTreeMap::from([("T".to_owned(), TypeVarId(7))]),
        )
        .expect("generic type parses");
        assert_eq!(
            generic,
            Type::Map(
                Box::new(Type::Str),
                Box::new(Type::Option(Box::new(Type::TypeVar(TypeVarId(7)))))
            )
        );

        let function = parse_type(
            &read_one("(Fn [Int (List T)] -> (Option T))"),
            &BTreeMap::from([("T".to_owned(), TypeVarId(7))]),
        )
        .expect("function type parses");
        assert_eq!(
            function,
            Type::Fn(
                FunctionType::new(
                    vec![Type::Int, Type::List(Box::new(Type::TypeVar(TypeVarId(7))))],
                    Type::Option(Box::new(Type::TypeVar(TypeVarId(7))))
                )
                .with_summaries(CallSummaries::unknown())
            )
        );
    }

    #[test]
    fn parses_canonical_literal_type_arguments_for_axes_and_frame_schema() {
        let array = parse_type(
            &read_one("(Array Float [:time :feature])"),
            &BTreeMap::new(),
        )
        .expect("array type parses");
        assert_eq!(
            array,
            Type::Nominal {
                binding: "Array".to_owned(),
                args: vec![
                    Type::Float,
                    Type::Literal(TypeLiteral::Vector(vec![
                        TypeLiteral::Keyword(":time".to_owned()),
                        TypeLiteral::Keyword(":feature".to_owned()),
                    ])),
                ],
            }
        );
        assert_eq!(
            array.to_python_annotation(PythonVersion::PYTHON_3_9),
            Ok("Array[float, Literal[\"[:time :feature]\"]]".to_owned())
        );
        assert!(
            array
                .python_typing_imports(PythonVersion::PYTHON_3_9)
                .contains(&PythonTypingImport::typing("Literal"))
        );
        let annotated_axes = parse_type(
            &read_one("(Array Float ^{:doc \"display only\"} [:time :feature])"),
            &BTreeMap::new(),
        )
        .expect("metadata-bearing axes parse");
        assert_eq!(array, annotated_axes, "metadata is not type identity");
        let other_axes = parse_type(
            &read_one("(Array Float [:time :channel])"),
            &BTreeMap::new(),
        )
        .expect("second array type parses");
        let context = TypeContext::new();
        assert!(context.is_assignable(&array, &array));
        assert!(!context.is_assignable(&array, &other_axes));

        let frame = parse_type(
            &read_one(
                "(Frame {:value Float :time Datetime :category Str} \
                         :key [:time :category] :order [:time])",
            ),
            &BTreeMap::new(),
        )
        .expect("frame type parses");
        let Type::Nominal { args, .. } = frame else {
            panic!("frame is nominal")
        };
        assert_eq!(args.len(), 5);
        let Type::Literal(schema) = &args[0] else {
            panic!("frame schema is literal")
        };
        assert_eq!(
            schema.canonical_text(),
            "{:category Str :time Datetime :value Float}"
        );
        assert_eq!(
            args[1],
            Type::Literal(TypeLiteral::Keyword(":key".to_owned()))
        );
        assert_eq!(
            args[3],
            Type::Literal(TypeLiteral::Keyword(":order".to_owned()))
        );
    }

    #[test]
    fn numeric_unification_and_scalar_signatures_promote_to_float() {
        assert_eq!(
            TypeContext::new().unify(&Type::Int, &Type::Float),
            Ok(Type::Float)
        );
        let division = scalar_operator_signatures(ScalarOperator::TrueDivide);
        assert_eq!(division.len(), 4);
        assert!(
            division
                .iter()
                .all(|signature| signature.result == Type::Float)
        );
        let addition = scalar_operator_signatures(ScalarOperator::Add);
        assert!(addition.iter().any(|signature| {
            signature.operands == vec![Type::Int, Type::Float] && signature.result == Type::Float
        }));
    }

    #[test]
    fn reports_typing_imports_and_readable_annotations() {
        let ty = Type::Fn(FunctionType::new(
            vec![
                Type::Nominal {
                    binding: "data.series::type::Series".to_owned(),
                    args: vec![Type::Float],
                },
                Type::Never,
            ],
            Type::option(Type::Int),
        ));
        assert_eq!(
            ty.python_typing_imports(PythonVersion::PYTHON_3_9),
            BTreeSet::from([
                PythonTypingImport::typing("Callable"),
                PythonTypingImport::typing("NoReturn"),
                PythonTypingImport::typing("Optional"),
            ])
        );
        assert_eq!(
            ty.to_python_annotation(PythonVersion::PYTHON_3_9),
            Ok("Callable[[Series[float], NoReturn], Optional[int]]".to_owned())
        );
    }

    #[test]
    fn malformed_function_annotation_has_a_source_span() {
        let form = read_one("(Fn Int Str)");
        let error = parse_type(&form, &BTreeMap::new()).expect_err("parameter vector required");
        assert!(matches!(
            error.kind,
            super::TypeParseErrorKind::FunctionParameters
        ));
        let FormKind::List(items) = form.kind else {
            panic!("list")
        };
        assert_eq!(error.span, items[1].span);
    }

    #[test]
    fn builtin_exception_type_whitelist_is_closed_and_roundtrips() {
        assert_eq!(
            python_builtin_exception_name("Exception"),
            Some("Exception")
        );
        assert_eq!(
            python_builtin_exception_name("builtins/ValueError"),
            Some("ValueError")
        );
        assert_eq!(python_builtin_exception_name("custom/Exception"), None);
        let binding = python_builtin_exception_binding("TypeError").expect("known exception");
        assert_eq!(
            python_builtin_exception_from_binding(&binding),
            Some("TypeError")
        );
        assert_eq!(python_builtin_exception_from_binding("Exception"), None);
    }
}
