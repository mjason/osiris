use std::collections::BTreeMap;

use serde::Serialize;

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
