//! Static schemas, records, and their deterministic runtime sidecar.
//!
//! This module is deliberately independent from the Python backend.  A static
//! record is data validated at compile time; it is never evaluated and it is
//! never represented by a Python object while compiling.  The public API is
//! also useful to interface/build consumers which only have an AST or an
//! already validated record set.

use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

use crate::{
    ast::{self, Expr, ExprKind, ItemKind, Module},
    diagnostic::Diagnostic,
    name::{BindingId, BindingKind},
    source::Span,
    syntax::Name,
};

pub const RECORD_INVALID_DATUM: &str = "OSR-S0001";
pub const RECORD_SCHEMA_SHAPE: &str = "OSR-S0002";
pub const RECORD_SCHEMA_FIELD: &str = "OSR-S0003";
pub const RECORD_SCHEMA_TYPE: &str = "OSR-S0004";
pub const RECORD_SCHEMA_INDEX: &str = "OSR-S0005";
pub const RECORD_RECORD_SHAPE: &str = "OSR-S0006";
pub const RECORD_RECORD_TYPE: &str = "OSR-S0007";
pub const RECORD_RECORD_INDEX: &str = "OSR-S0008";
pub const RECORD_INDEX_CONFLICT: &str = "OSR-S0009";
pub const RECORD_SIDECAR: &str = "OSR-S0010";

/// A failure from the canonical data/sidecar layer.  Source-facing APIs turn
/// these into [`Diagnostic`] values; sidecar readers use the richer string
/// because there is no source span to point at.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordError {
    pub code: &'static str,
    pub message: String,
    pub span: Option<Span>,
}

impl RecordError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            span: None,
        }
    }

    fn at(code: &'static str, message: impl Into<String>, span: Span) -> Self {
        Self {
            code,
            message: message.into(),
            span: Some(span),
        }
    }
}

impl fmt::Display for RecordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RecordError {}

/// The only values which may occur in a static record or metadata datum.
/// Collection order is normalized by [`StaticDatum::canonicalize`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StaticDatum {
    None,
    Bool(bool),
    /// Canonical base-10 spelling (no leading zeroes except `0`).
    Int(String),
    /// Exact IEEE-754 binary64 bits.  Non-finite values are rejected when the
    /// datum is canonicalized.
    Float(u64),
    Str(String),
    Keyword(String),
    Symbol {
        spelling: String,
        binding_id: Option<String>,
    },
    List(Vec<StaticDatum>),
    Vector(Vec<StaticDatum>),
    Map(Vec<(StaticDatum, StaticDatum)>),
    Set(Vec<StaticDatum>),
}

impl StaticDatum {
    /// Convert a lowered surface expression.  Names are symbols; calls,
    /// functions, control forms, and all other runtime expressions fail.
    pub fn from_expr(expression: &Expr) -> Result<Self, Diagnostic> {
        Self::from_expr_inner(expression, false).map_err(|error| {
            Diagnostic::error(
                error.code,
                error.message,
                error.span.unwrap_or(expression.span),
            )
        })
    }

    /// The quoted form is useful for authors who need a non-empty List or a
    /// symbol as data.  The quote itself is not retained in the datum.
    pub fn from_quoted_expr(expression: &Expr) -> Result<Self, Diagnostic> {
        Self::from_expr_inner(expression, true).map_err(|error| {
            Diagnostic::error(
                error.code,
                error.message,
                error.span.unwrap_or(expression.span),
            )
        })
    }

    fn from_expr_inner(expression: &Expr, quoted: bool) -> Result<Self, RecordError> {
        let value = match &expression.kind {
            ExprKind::None => Self::None,
            ExprKind::Bool(value) => Self::Bool(*value),
            ExprKind::Integer(value) => {
                let value = canonical_integer(value).ok_or_else(|| {
                    RecordError::at(
                        RECORD_INVALID_DATUM,
                        "invalid static integer",
                        expression.span,
                    )
                })?;
                Self::Int(value)
            }
            ExprKind::Float(value) => {
                let parsed = value.parse::<f64>().map_err(|_| {
                    RecordError::at(
                        RECORD_INVALID_DATUM,
                        "invalid static float",
                        expression.span,
                    )
                })?;
                if !parsed.is_finite() {
                    return Err(RecordError::at(
                        RECORD_INVALID_DATUM,
                        "static floats must be finite",
                        expression.span,
                    ));
                }
                Self::Float(parsed.to_bits())
            }
            ExprKind::String(value) => Self::Str(value.clone()),
            ExprKind::Keyword(name) => Self::Keyword(name.canonical.clone()),
            ExprKind::Name(name) => Self::Symbol {
                spelling: name.canonical.clone(),
                binding_id: None,
            },
            ExprKind::List(values) => {
                if !quoted && !values.is_empty() {
                    return Err(RecordError::at(
                        RECORD_INVALID_DATUM,
                        "non-empty list in a static record must be quoted",
                        expression.span,
                    ));
                }
                Self::List(
                    values
                        .iter()
                        .map(|value| Self::from_expr_inner(value, quoted))
                        .collect::<Result<_, _>>()?,
                )
            }
            ExprKind::Vector(values) => Self::Vector(
                values
                    .iter()
                    .map(|value| Self::from_expr_inner(value, quoted))
                    .collect::<Result<_, _>>()?,
            ),
            ExprKind::Map(entries) => {
                if entries.len() > 1_000_000 {
                    return Err(RecordError::at(
                        RECORD_INVALID_DATUM,
                        "static map exceeds the resource limit",
                        expression.span,
                    ));
                }
                Self::Map(
                    entries
                        .iter()
                        .map(|(key, value)| {
                            Ok((
                                Self::from_expr_inner(key, quoted)?,
                                Self::from_expr_inner(value, quoted)?,
                            ))
                        })
                        .collect::<Result<_, RecordError>>()?,
                )
            }
            ExprKind::Set(values) => Self::Set(
                values
                    .iter()
                    .map(|value| Self::from_expr_inner(value, quoted))
                    .collect::<Result<_, _>>()?,
            ),
            ExprKind::Quote(inner) if !quoted => return Self::from_expr_inner(inner, true),
            ExprKind::Quote(_) => {
                return Err(RecordError::at(
                    RECORD_INVALID_DATUM,
                    "nested quote is not a static datum",
                    expression.span,
                ));
            }
            ExprKind::Call(_) | ExprKind::Fn(_) => {
                if quoted {
                    // AST lowering turns a quoted non-empty list into a Call.
                    // Reconstruct its source datum while retaining keyword
                    // argument order.
                    if let ExprKind::Call(call) = &expression.kind {
                        let mut values = Vec::with_capacity(call.args.len() + 1);
                        values.push(Self::from_expr_inner(&call.callee, true)?);
                        for argument in &call.args {
                            match argument {
                                ast::CallArg::Positional(value) => {
                                    values.push(Self::from_expr_inner(value, true)?);
                                }
                                ast::CallArg::Keyword(argument) => {
                                    values.push(Self::Keyword(argument.key.canonical.clone()));
                                    values.push(Self::from_expr_inner(&argument.value, true)?);
                                }
                            }
                        }
                        return Self::List(values).canonicalize();
                    }
                }
                return Err(RecordError::at(
                    RECORD_INVALID_DATUM,
                    "calls and functions are not allowed in static data",
                    expression.span,
                ));
            }
            ExprKind::Let { .. }
            | ExprKind::If { .. }
            | ExprKind::Do(_)
            | ExprKind::Try(_)
            | ExprKind::Raise(_)
            | ExprKind::SyntaxQuote(_)
            | ExprKind::Unquote(_)
            | ExprKind::UnquoteSplicing(_)
            | ExprKind::Error(_) => {
                return Err(RecordError::at(
                    RECORD_INVALID_DATUM,
                    "runtime expressions are not allowed in static data",
                    expression.span,
                ));
            }
        };
        value.canonicalize()
    }

    /// Return the normalized value, rejecting duplicate map keys/set items.
    pub fn canonicalize(mut self) -> Result<Self, RecordError> {
        match &mut self {
            Self::Int(value) => {
                let Some(canonical) = canonical_integer(value) else {
                    return Err(RecordError::new(
                        RECORD_INVALID_DATUM,
                        "invalid static integer",
                    ));
                };
                *value = canonical;
            }
            Self::Float(bits) => {
                if !f64::from_bits(*bits).is_finite() {
                    return Err(RecordError::new(
                        RECORD_INVALID_DATUM,
                        "static floats must be finite",
                    ));
                }
            }
            Self::Symbol { spelling, .. } | Self::Keyword(spelling) => {
                *spelling = spelling.nfc().collect();
            }
            Self::List(items) | Self::Vector(items) | Self::Set(items) => {
                for item in items.iter_mut() {
                    *item = item.clone().canonicalize()?;
                }
            }
            Self::Map(entries) => {
                for (key, value) in entries.iter_mut() {
                    *key = key.clone().canonicalize()?;
                    *value = value.clone().canonicalize()?;
                }
            }
            Self::None | Self::Bool(_) | Self::Str(_) => {}
        }

        match &mut self {
            Self::Set(items) => {
                items.sort_by_key(canonical_bytes);
                if items
                    .windows(2)
                    .any(|pair| canonical_bytes(&pair[0]) == canonical_bytes(&pair[1]))
                {
                    return Err(RecordError::new(
                        RECORD_INVALID_DATUM,
                        "duplicate canonical item in static set",
                    ));
                }
            }
            Self::Map(entries) => {
                entries.sort_by(|left, right| {
                    canonical_bytes(&left.0).cmp(&canonical_bytes(&right.0))
                });
                if entries
                    .windows(2)
                    .any(|pair| canonical_bytes(&pair[0].0) == canonical_bytes(&pair[1].0))
                {
                    return Err(RecordError::new(
                        RECORD_INVALID_DATUM,
                        "duplicate canonical key in static map",
                    ));
                }
            }
            _ => {}
        }
        Ok(self)
    }

    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        canonical_bytes(self)
    }

    fn to_json(&self) -> Json {
        match self {
            Self::None => Json::Null,
            Self::Bool(value) => Json::Bool(*value),
            Self::Int(value) => tagged("int", vec![("value", Json::String(value.clone()))]),
            Self::Float(bits) => tagged(
                "float",
                vec![("value", Json::String(format!("{bits:016x}")))],
            ),
            Self::Str(value) => Json::String(value.clone()),
            Self::Keyword(value) => {
                tagged("keyword", vec![("spelling", Json::String(value.clone()))])
            }
            Self::Symbol {
                spelling,
                binding_id,
            } => {
                let mut fields = vec![("spelling", Json::String(spelling.clone()))];
                if let Some(binding_id) = binding_id {
                    fields.push(("binding-id", Json::String(binding_id.clone())));
                }
                tagged("symbol", fields)
            }
            Self::List(items) => tagged(
                "list",
                vec![(
                    "items",
                    Json::Array(items.iter().map(Self::to_json).collect()),
                )],
            ),
            Self::Vector(items) => tagged(
                "vector",
                vec![(
                    "items",
                    Json::Array(items.iter().map(Self::to_json).collect()),
                )],
            ),
            Self::Set(items) => tagged(
                "set",
                vec![(
                    "items",
                    Json::Array(items.iter().map(Self::to_json).collect()),
                )],
            ),
            Self::Map(entries) => tagged(
                "map",
                vec![(
                    "entries",
                    Json::Array(
                        entries
                            .iter()
                            .map(|(key, value)| Json::Array(vec![key.to_json(), value.to_json()]))
                            .collect(),
                    ),
                )],
            ),
        }
    }

    fn from_json(value: &Json) -> Result<Self, RecordError> {
        match value {
            Json::Null => Ok(Self::None),
            Json::Bool(value) => Ok(Self::Bool(*value)),
            Json::String(value) => Ok(Self::Str(value.clone())),
            Json::Number(_) => Err(RecordError::new(
                RECORD_SIDECAR,
                "untagged JSON numbers are not static datums",
            )),
            Json::Array(_) => Err(RecordError::new(
                RECORD_SIDECAR,
                "untagged JSON arrays are not static datums",
            )),
            Json::Object(fields) => {
                let tag = object_string(fields, "$osiris")?;
                match tag.as_str() {
                    "int" => {
                        let value = object_string(fields, "value")?;
                        Self::Int(value).canonicalize()
                    }
                    "float" => {
                        let value = object_string(fields, "value")?;
                        if value.len() != 16 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
                        {
                            return Err(RecordError::new(RECORD_SIDECAR, "invalid float bits"));
                        }
                        let bits = u64::from_str_radix(&value, 16)
                            .map_err(|_| RecordError::new(RECORD_SIDECAR, "invalid float bits"))?;
                        Self::Float(bits).canonicalize()
                    }
                    "keyword" => Self::Keyword(object_string(fields, "spelling")?).canonicalize(),
                    "symbol" => {
                        let spelling = object_string(fields, "spelling")?;
                        let binding_id = object_optional_string(fields, "binding-id")?;
                        Self::Symbol {
                            spelling,
                            binding_id,
                        }
                        .canonicalize()
                    }
                    "list" | "vector" | "set" => {
                        let items = object_array(fields, "items")?
                            .iter()
                            .map(Self::from_json)
                            .collect::<Result<Vec<_>, _>>()?;
                        match tag.as_str() {
                            "list" => Self::List(items).canonicalize(),
                            "vector" => Self::Vector(items).canonicalize(),
                            _ => Self::Set(items).canonicalize(),
                        }
                    }
                    "map" => {
                        let entries = object_array(fields, "entries")?;
                        let mut result = Vec::with_capacity(entries.len());
                        for entry in entries {
                            let Json::Array(pair) = entry else {
                                return Err(RecordError::new(
                                    RECORD_SIDECAR,
                                    "map entry must be a pair",
                                ));
                            };
                            if pair.len() != 2 {
                                return Err(RecordError::new(
                                    RECORD_SIDECAR,
                                    "map entry must have two values",
                                ));
                            }
                            result.push((Self::from_json(&pair[0])?, Self::from_json(&pair[1])?));
                        }
                        Self::Map(result).canonicalize()
                    }
                    _ => Err(RecordError::new(RECORD_SIDECAR, "unknown static datum tag")),
                }
            }
        }
    }
}

fn canonical_integer(value: &str) -> Option<String> {
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

fn canonical_bytes(value: &StaticDatum) -> Vec<u8> {
    value.to_json().bytes()
}

/// A small JSON tree used instead of `serde_json::Value`: it preserves object
/// member order while decoding, allowing us to reject duplicate names before
/// any lookup occurs.  The writer applies the RFC 8785 object-key ordering.
#[derive(Clone, Debug, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    fn bytes(&self) -> Vec<u8> {
        let mut output = String::new();
        self.write(&mut output);
        output.into_bytes()
    }

    fn write(&self, output: &mut String) {
        match self {
            Self::Null => output.push_str("null"),
            Self::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
            Self::Number(value) => output.push_str(value),
            Self::String(value) => output.push_str(&json_quote(value)),
            Self::Array(values) => {
                output.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        output.push(',');
                    }
                    value.write(output);
                }
                output.push(']');
            }
            Self::Object(fields) => {
                let mut fields = fields.iter().collect::<Vec<_>>();
                fields.sort_by(|left, right| utf16_cmp(&left.0, &right.0));
                output.push('{');
                for (index, (key, value)) in fields.iter().enumerate() {
                    if index != 0 {
                        output.push(',');
                    }
                    output.push_str(&json_quote(key));
                    output.push(':');
                    value.write(output);
                }
                output.push('}');
            }
        }
    }
}

impl Serialize for StaticDatum {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_json(&self.to_json(), serializer)
    }
}

fn serialize_json<S>(value: &Json, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match value {
        Json::Null => serializer.serialize_unit(),
        Json::Bool(value) => serializer.serialize_bool(*value),
        Json::Number(value) => {
            if let Ok(number) = value.parse::<u64>() {
                serializer.serialize_u64(number)
            } else if let Ok(number) = value.parse::<i64>() {
                serializer.serialize_i64(number)
            } else if let Ok(number) = value.parse::<f64>() {
                serializer.serialize_f64(number)
            } else {
                Err(ser::Error::custom("invalid canonical JSON number"))
            }
        }
        Json::String(value) => serializer.serialize_str(value),
        Json::Array(values) => {
            let mut sequence = serializer.serialize_seq(Some(values.len()))?;
            for value in values {
                sequence.serialize_element(&JsonSerializable(value))?;
            }
            sequence.end()
        }
        Json::Object(fields) => {
            let mut map = serializer.serialize_map(Some(fields.len()))?;
            for (key, value) in fields {
                map.serialize_entry(key, &JsonSerializable(value))?;
            }
            map.end()
        }
    }
}

struct JsonSerializable<'a>(&'a Json);

impl Serialize for JsonSerializable<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_json(self.0, serializer)
    }
}

fn utf16_cmp(left: &str, right: &str) -> Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}

fn json_quote(value: &str) -> String {
    // serde_json's string encoder follows JSON's required escaping rules and
    // leaves non-control Unicode scalars intact, which is also JCS's form.
    serde_json::to_string(value).expect("Rust strings are always JSON strings")
}

fn tagged(tag: &str, mut fields: Vec<(&str, Json)>) -> Json {
    let mut object = vec![("$osiris".to_owned(), Json::String(tag.to_owned()))];
    object.extend(fields.drain(..).map(|(key, value)| (key.to_owned(), value)));
    Json::Object(object)
}

fn object_field<'a>(fields: &'a [(String, Json)], key: &str) -> Result<&'a Json, RecordError> {
    fields
        .iter()
        .find_map(|(name, value)| (name == key).then_some(value))
        .ok_or_else(|| RecordError::new(RECORD_SIDECAR, format!("missing JSON member `{key}`")))
}

fn object_string(fields: &[(String, Json)], key: &str) -> Result<String, RecordError> {
    match object_field(fields, key)? {
        Json::String(value) => Ok(value.clone()),
        _ => Err(RecordError::new(
            RECORD_SIDECAR,
            format!("JSON member `{key}` must be a string"),
        )),
    }
}

fn object_optional_string(
    fields: &[(String, Json)],
    key: &str,
) -> Result<Option<String>, RecordError> {
    match fields
        .iter()
        .find_map(|(name, value)| (name == key).then_some(value))
    {
        None => Ok(None),
        Some(Json::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(RecordError::new(
            RECORD_SIDECAR,
            format!("JSON member `{key}` must be a string"),
        )),
    }
}

fn object_array<'a>(fields: &'a [(String, Json)], key: &str) -> Result<&'a [Json], RecordError> {
    match object_field(fields, key)? {
        Json::Array(values) => Ok(values),
        _ => Err(RecordError::new(
            RECORD_SIDECAR,
            format!("JSON member `{key}` must be an array"),
        )),
    }
}

struct JsonVisitor;

impl<'de> de::Visitor<'de> for JsonVisitor {
    type Value = Json;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Json::Null)
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Json::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Json::Number(value.to_string()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Json::Number(value.to_string()))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if !value.is_finite() {
            return Err(E::custom("non-finite JSON number"));
        }
        Ok(Json::Number(value.to_string()))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Json::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Json::String(value))
    }

    fn visit_seq<A>(self, mut access: A) -> Result<Self::Value, A::Error>
    where
        A: de::SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = access.next_element_seed(JsonSeed)? {
            values.push(value);
        }
        Ok(Json::Array(values))
    }

    fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
    where
        A: de::MapAccess<'de>,
    {
        let mut fields = Vec::new();
        while let Some(key) = access.next_key::<String>()? {
            if fields.iter().any(|(existing, _)| existing == &key) {
                return Err(de::Error::custom(format!("duplicate JSON member `{key}`")));
            }
            let value = access.next_value_seed(JsonSeed)?;
            fields.push((key, value));
        }
        Ok(Json::Object(fields))
    }
}

struct JsonSeed;

impl<'de> de::DeserializeSeed<'de> for JsonSeed {
    type Value = Json;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(JsonVisitor)
    }
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(JsonVisitor)
    }
}

/// The closed v0 schema type language.  `OneOf` contains static datums rather
/// than names so that a schema remains independent of a runtime registry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub enum StaticType {
    Any,
    None,
    Bool,
    Int,
    Float,
    Str,
    Keyword,
    Symbol,
    List(Box<StaticType>),
    Vector(Box<StaticType>),
    Map(Box<StaticType>, Box<StaticType>),
    Set(Box<StaticType>),
    Optional(Box<StaticType>),
    OneOf(Vec<StaticDatum>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SchemaField {
    pub name: String,
    #[serde(rename = "type")]
    pub datum_type: StaticType,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<StaticDatum>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub enum ProjectionKind {
    Field,
    Each,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IndexProjection {
    pub kind: ProjectionKind,
    pub field: String,
    pub role: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SchemaIndex {
    pub id: String,
    pub scope: String,
    pub projections: Vec<IndexProjection>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StaticSchema {
    pub name: String,
    pub schema_id: String,
    pub version: u64,
    pub fields: Vec<SchemaField>,
    pub indexes: Vec<SchemaIndex>,
    /// Hash of the canonical schema body, excluding source spans/metadata.
    pub body_hash: String,
}

impl StaticSchema {
    #[must_use]
    pub fn field(&self, name: &str) -> Option<&SchemaField> {
        self.fields.iter().find(|field| field.name == name)
    }

    #[must_use]
    pub fn identity(&self, binding_id: impl Into<String>) -> SchemaIdentity {
        SchemaIdentity {
            binding_id: binding_id.into(),
            schema_id: self.schema_id.clone(),
            version: self.version,
            body_hash: self.body_hash.clone(),
        }
    }

    fn body_json(&self) -> Json {
        Json::Object(vec![
            ("schema-id".to_owned(), Json::String(self.schema_id.clone())),
            ("version".to_owned(), Json::Number(self.version.to_string())),
            (
                "fields".to_owned(),
                Json::Array(self.fields.iter().map(schema_field_json).collect()),
            ),
            (
                "indexes".to_owned(),
                Json::Array(self.indexes.iter().map(schema_index_json).collect()),
            ),
        ])
    }

    #[must_use]
    pub fn canonical_body_bytes(&self) -> Vec<u8> {
        self.body_json().bytes()
    }

    /// Verify a schema reconstructed from a data-only compilation interface.
    /// This repeats the structural invariants normally established while
    /// lowering `defstatic-schema` and authenticates its embedded body hash.
    pub fn verify_integrity(&self) -> Result<(), RecordError> {
        if self.name.is_empty() {
            return Err(RecordError::new(RECORD_SCHEMA_SHAPE, "empty schema name"));
        }
        if !is_namespaced(&self.schema_id) {
            return Err(RecordError::new(
                RECORD_SCHEMA_SHAPE,
                "schema-id must contain a namespace separator `/`",
            ));
        }
        if self.version == 0 {
            return Err(RecordError::new(
                RECORD_SCHEMA_SHAPE,
                "schema version must be positive",
            ));
        }

        let mut field_names = BTreeSet::new();
        for field in &self.fields {
            if field.name.is_empty() || !field_names.insert(field.name.clone()) {
                return Err(RecordError::new(
                    RECORD_SCHEMA_FIELD,
                    format!("empty or duplicate schema field `{}`", field.name),
                ));
            }
            verify_static_type(&field.datum_type)?;
            if let Some(default) = &field.default {
                if default.clone().canonicalize()? != *default {
                    return Err(RecordError::new(
                        RECORD_SCHEMA_FIELD,
                        format!("default for field `{}` is not canonical", field.name),
                    ));
                }
                if !field.datum_type.accepts(default) {
                    return Err(RecordError::new(
                        RECORD_SCHEMA_TYPE,
                        format!("default for field `{}` does not match its type", field.name),
                    ));
                }
                if field.required {
                    return Err(RecordError::new(
                        RECORD_SCHEMA_FIELD,
                        format!("required field `{}` cannot also have a default", field.name),
                    ));
                }
            }
        }

        let field_types = self
            .fields
            .iter()
            .map(|field| (field.name.as_str(), &field.datum_type))
            .collect::<BTreeMap<_, _>>();
        let mut index_ids = BTreeSet::new();
        for index in &self.indexes {
            if !is_namespaced(&index.id) || !index_ids.insert(index.id.clone()) {
                return Err(RecordError::new(
                    RECORD_SCHEMA_INDEX,
                    format!("invalid or duplicate schema index `{}`", index.id),
                ));
            }
            if index.scope.is_empty() || index.projections.is_empty() {
                return Err(RecordError::new(
                    RECORD_SCHEMA_INDEX,
                    format!(
                        "schema index `{}` has an empty scope or projection list",
                        index.id
                    ),
                ));
            }
            for projection in &index.projections {
                let field_type = field_types.get(projection.field.as_str()).ok_or_else(|| {
                    RecordError::new(
                        RECORD_SCHEMA_INDEX,
                        format!(
                            "index `{}` references unknown field `{}`",
                            index.id, projection.field
                        ),
                    )
                })?;
                if projection.role.is_empty() {
                    return Err(RecordError::new(
                        RECORD_SCHEMA_INDEX,
                        format!("index `{}` has an empty projection role", index.id),
                    ));
                }
                if matches!(projection.kind, ProjectionKind::Each)
                    && !matches!(
                        field_type,
                        StaticType::List(_) | StaticType::Vector(_) | StaticType::Set(_)
                    )
                {
                    return Err(RecordError::new(
                        RECORD_SCHEMA_INDEX,
                        format!(
                            "`:each` projection `{}` must target a collection field",
                            projection.field
                        ),
                    ));
                }
            }
        }

        let expected = sha256_prefix(&self.canonical_body_bytes());
        if self.body_hash != expected {
            return Err(RecordError::new(
                RECORD_SCHEMA_SHAPE,
                "schema body hash does not match schema payload",
            ));
        }
        Ok(())
    }
}

fn verify_static_type(datum_type: &StaticType) -> Result<(), RecordError> {
    match datum_type {
        StaticType::List(inner)
        | StaticType::Vector(inner)
        | StaticType::Set(inner)
        | StaticType::Optional(inner) => verify_static_type(inner),
        StaticType::Map(key, value) => {
            verify_static_type(key)?;
            verify_static_type(value)
        }
        StaticType::OneOf(values) => {
            if values.is_empty() {
                return Err(RecordError::new(
                    RECORD_SCHEMA_TYPE,
                    "OneOf requires at least one static value",
                ));
            }
            let mut seen = BTreeSet::new();
            for value in values {
                if value.clone().canonicalize()? != *value || !seen.insert(value.canonical_bytes())
                {
                    return Err(RecordError::new(
                        RECORD_SCHEMA_TYPE,
                        "OneOf contains a non-canonical or duplicate value",
                    ));
                }
            }
            Ok(())
        }
        StaticType::Any
        | StaticType::None
        | StaticType::Bool
        | StaticType::Int
        | StaticType::Float
        | StaticType::Str
        | StaticType::Keyword
        | StaticType::Symbol => Ok(()),
    }
}

/// Parse a `defstatic-schema` AST declaration.  Parsing is intentionally
/// conservative: unknown clauses and unknown field/index keys are errors.
pub fn parse_schema(declaration: &ast::DefstaticSchema) -> Result<StaticSchema, Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    let mut clauses: BTreeMap<String, &Expr> = BTreeMap::new();
    let body = &declaration.body;
    if body.len() % 2 != 0 {
        diagnostics.push(Diagnostic::error(
            RECORD_SCHEMA_SHAPE,
            "defstatic-schema clauses require keyword/value pairs",
            declaration.span,
        ));
    }
    for pair in body.chunks(2) {
        let Some(key) = pair.first().and_then(expr_keyword) else {
            diagnostics.push(Diagnostic::error(
                RECORD_SCHEMA_SHAPE,
                "schema clause key must be a keyword",
                pair.first().map_or(declaration.span, |expr| expr.span),
            ));
            continue;
        };
        if clauses
            .insert(key.to_owned(), &pair[1.min(pair.len().saturating_sub(1))])
            .is_some()
        {
            diagnostics.push(Diagnostic::error(
                RECORD_SCHEMA_SHAPE,
                format!("duplicate schema clause `{key}`"),
                pair[0].span,
            ));
        }
    }

    let schema_id = clauses
        .get(":schema-id")
        .and_then(|expr| expr_string(expr))
        .unwrap_or_else(|| {
            diagnostics.push(Diagnostic::error(
                RECORD_SCHEMA_SHAPE,
                "schema requires string :schema-id",
                declaration.span,
            ));
            String::new()
        });
    if !schema_id.is_empty() && !is_namespaced(&schema_id) {
        diagnostics.push(Diagnostic::error(
            RECORD_SCHEMA_SHAPE,
            "schema-id must contain a namespace separator `/`",
            declaration.span,
        ));
    }

    let version = clauses
        .get(":version")
        .and_then(|expr| expr_integer(expr))
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or_else(|| {
            diagnostics.push(Diagnostic::error(
                RECORD_SCHEMA_SHAPE,
                "schema requires a positive integer :version",
                declaration.span,
            ));
            0
        });

    let fields = match clauses.get(":fields") {
        Some(expr) => parse_schema_fields(expr, &mut diagnostics),
        None => {
            diagnostics.push(Diagnostic::error(
                RECORD_SCHEMA_FIELD,
                "schema requires :fields",
                declaration.span,
            ));
            Vec::new()
        }
    };
    let indexes = match clauses.get(":indexes") {
        Some(expr) => parse_schema_indexes(expr, &fields, &mut diagnostics),
        None => Vec::new(),
    };
    for (key, expr) in &clauses {
        if !matches!(
            key.as_str(),
            ":schema-id" | ":version" | ":fields" | ":indexes"
        ) {
            diagnostics.push(Diagnostic::error(
                RECORD_SCHEMA_SHAPE,
                format!("unknown schema clause `{key}`"),
                expr.span,
            ));
        }
    }

    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    let mut schema = StaticSchema {
        name: declaration.name.canonical.clone(),
        schema_id,
        version,
        fields,
        indexes,
        body_hash: String::new(),
    };
    schema.body_hash = sha256_prefix(&schema.canonical_body_bytes());
    Ok(schema)
}

fn parse_schema_fields(expression: &Expr, diagnostics: &mut Vec<Diagnostic>) -> Vec<SchemaField> {
    let Some(entries) = expr_map(expression) else {
        diagnostics.push(Diagnostic::error(
            RECORD_SCHEMA_FIELD,
            ":fields must be a map",
            expression.span,
        ));
        return Vec::new();
    };
    let mut fields = Vec::new();
    let mut seen = BTreeSet::new();
    for (key, value) in entries {
        let Some(name) = expr_keyword(key).map(str::to_owned) else {
            diagnostics.push(Diagnostic::error(
                RECORD_SCHEMA_FIELD,
                "schema field name must be a keyword",
                key.span,
            ));
            continue;
        };
        if !seen.insert(name.clone()) {
            diagnostics.push(Diagnostic::error(
                RECORD_SCHEMA_FIELD,
                format!("duplicate schema field `{name}`"),
                key.span,
            ));
            continue;
        }
        let Some(field_options) = expr_map(value) else {
            diagnostics.push(Diagnostic::error(
                RECORD_SCHEMA_FIELD,
                format!("schema field `{name}` must be an option map"),
                value.span,
            ));
            continue;
        };
        let mut field_type = None;
        let mut required = false;
        let mut default = None;
        let mut option_keys = BTreeSet::new();
        for (option_key, option_value) in field_options {
            let Some(option_name) = expr_keyword(option_key) else {
                diagnostics.push(Diagnostic::error(
                    RECORD_SCHEMA_FIELD,
                    "schema field option must be a keyword",
                    option_key.span,
                ));
                continue;
            };
            if !option_keys.insert(option_name.to_owned()) {
                diagnostics.push(Diagnostic::error(
                    RECORD_SCHEMA_FIELD,
                    format!("duplicate option `{option_name}` for field `{name}`"),
                    option_key.span,
                ));
                continue;
            }
            match option_name {
                ":type" => match parse_static_type(option_value) {
                    Ok(value) => field_type = Some(value),
                    Err(error) => diagnostics.push(diagnostic_from_error(error, option_value.span)),
                },
                ":required" => match option_bool(option_value) {
                    Some(value) => required = value,
                    None => diagnostics.push(Diagnostic::error(
                        RECORD_SCHEMA_FIELD,
                        format!("field `{name}` :required must be boolean"),
                        option_value.span,
                    )),
                },
                ":default" => match StaticDatum::from_expr(option_value) {
                    Ok(value) => default = Some(value),
                    Err(error) => diagnostics.push(error),
                },
                _ => diagnostics.push(Diagnostic::error(
                    RECORD_SCHEMA_FIELD,
                    format!("unknown option `{option_name}` for field `{name}`"),
                    option_key.span,
                )),
            }
        }
        let datum_type = field_type.unwrap_or_else(|| {
            diagnostics.push(Diagnostic::error(
                RECORD_SCHEMA_TYPE,
                format!("field `{name}` requires :type"),
                value.span,
            ));
            StaticType::Any
        });
        if let Some(default_value) = &default {
            if !datum_type.accepts(default_value) {
                diagnostics.push(Diagnostic::error(
                    RECORD_SCHEMA_TYPE,
                    format!("default for field `{name}` does not match its type"),
                    value.span,
                ));
            }
        }
        if required && default.is_some() {
            // A default is still useful for tooling, but it makes the field
            // non-required in the effective record shape.
            diagnostics.push(Diagnostic::error(
                RECORD_SCHEMA_FIELD,
                format!("required field `{name}` cannot also have a default"),
                value.span,
            ));
        }
        fields.push(SchemaField {
            name,
            datum_type,
            required,
            default,
        });
    }
    fields
}

fn parse_schema_indexes(
    expression: &Expr,
    fields: &[SchemaField],
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<SchemaIndex> {
    let (ExprKind::Vector(index_values) | ExprKind::List(index_values)) = &expression.kind else {
        diagnostics.push(Diagnostic::error(
            RECORD_SCHEMA_INDEX,
            ":indexes must be a vector",
            expression.span,
        ));
        return Vec::new();
    };
    let field_types = fields
        .iter()
        .map(|field| (field.name.as_str(), &field.datum_type))
        .collect::<BTreeMap<_, _>>();
    let mut indexes = Vec::new();
    let mut seen_ids = BTreeSet::new();
    for index_value in index_values {
        let Some(entries) = expr_map(index_value) else {
            diagnostics.push(Diagnostic::error(
                RECORD_SCHEMA_INDEX,
                "index declaration must be a map",
                index_value.span,
            ));
            continue;
        };
        let mut id = None;
        let mut scope = "effective-dependency-graph".to_owned();
        let mut projections_expr = None;
        let mut keys = BTreeSet::new();
        for (key, value) in entries {
            let Some(key) = expr_keyword(key) else {
                diagnostics.push(Diagnostic::error(
                    RECORD_SCHEMA_INDEX,
                    "index option must be a keyword",
                    key.span,
                ));
                continue;
            };
            if !keys.insert(key.to_owned()) {
                diagnostics.push(Diagnostic::error(
                    RECORD_SCHEMA_INDEX,
                    format!("duplicate index option `{key}`"),
                    value.span,
                ));
                continue;
            }
            match key {
                ":id" => id = expr_string(value),
                ":scope" => {
                    if let Some(value) = expr_keyword(value) {
                        scope = value.trim_start_matches(':').to_owned();
                    } else {
                        diagnostics.push(Diagnostic::error(
                            RECORD_SCHEMA_INDEX,
                            "index :scope must be a keyword",
                            value.span,
                        ));
                    }
                }
                ":keys" => projections_expr = Some(value),
                _ => diagnostics.push(Diagnostic::error(
                    RECORD_SCHEMA_INDEX,
                    format!("unknown index option `{key}`"),
                    value.span,
                )),
            }
        }
        let index_id = id.unwrap_or_else(|| {
            diagnostics.push(Diagnostic::error(
                RECORD_SCHEMA_INDEX,
                "index requires string :id",
                index_value.span,
            ));
            String::new()
        });
        if !index_id.is_empty() && !is_namespaced(&index_id) {
            diagnostics.push(Diagnostic::error(
                RECORD_SCHEMA_INDEX,
                "index id must contain a namespace separator `/`",
                index_value.span,
            ));
        }
        if !seen_ids.insert(index_id.clone()) && !index_id.is_empty() {
            diagnostics.push(Diagnostic::error(
                RECORD_SCHEMA_INDEX,
                format!("duplicate index id `{index_id}`"),
                index_value.span,
            ));
        }
        let projection_values = projections_expr
            .and_then(|value| match &value.kind {
                ExprKind::Vector(values) | ExprKind::List(values) => Some(values.as_slice()),
                _ => {
                    diagnostics.push(Diagnostic::error(
                        RECORD_SCHEMA_INDEX,
                        "index :keys must be a vector",
                        value.span,
                    ));
                    None
                }
            })
            .unwrap_or(&[]);
        let mut projections = Vec::new();
        for projection_value in projection_values {
            let Some(entries) = expr_map(projection_value) else {
                diagnostics.push(Diagnostic::error(
                    RECORD_SCHEMA_INDEX,
                    "index projection must be a map",
                    projection_value.span,
                ));
                continue;
            };
            let mut field = None;
            let mut kind = None;
            let mut role = None;
            for (key, value) in entries {
                let Some(key) = expr_keyword(key) else {
                    diagnostics.push(Diagnostic::error(
                        RECORD_SCHEMA_INDEX,
                        "index projection option must be a keyword",
                        key.span,
                    ));
                    continue;
                };
                match key {
                    ":field" => {
                        field = expr_keyword(value).map(str::to_owned);
                        kind = Some(ProjectionKind::Field);
                    }
                    ":each" => {
                        field = expr_keyword(value).map(str::to_owned);
                        kind = Some(ProjectionKind::Each);
                    }
                    ":role" => role = expr_keyword(value).map(str::to_owned),
                    _ => diagnostics.push(Diagnostic::error(
                        RECORD_SCHEMA_INDEX,
                        format!("unknown projection option `{key}`"),
                        value.span,
                    )),
                }
            }
            let Some(field) = field else {
                diagnostics.push(Diagnostic::error(
                    RECORD_SCHEMA_INDEX,
                    "index projection requires :field or :each",
                    projection_value.span,
                ));
                continue;
            };
            let Some(kind) = kind else { continue };
            let role = role.unwrap_or_else(|| "value".to_owned());
            if let Some(field_type) = field_types.get(field.as_str()) {
                if matches!(kind, ProjectionKind::Each)
                    && !matches!(
                        field_type,
                        StaticType::List(_) | StaticType::Vector(_) | StaticType::Set(_)
                    )
                {
                    diagnostics.push(Diagnostic::error(
                        RECORD_SCHEMA_INDEX,
                        format!(":each projection `{field}` must target a collection field"),
                        projection_value.span,
                    ));
                }
            } else {
                diagnostics.push(Diagnostic::error(
                    RECORD_SCHEMA_INDEX,
                    format!("index references unknown field `{field}`"),
                    projection_value.span,
                ));
            }
            projections.push(IndexProjection { kind, field, role });
        }
        if projections.is_empty() {
            diagnostics.push(Diagnostic::error(
                RECORD_SCHEMA_INDEX,
                "index requires at least one projection",
                index_value.span,
            ));
        }
        indexes.push(SchemaIndex {
            id: index_id,
            scope,
            projections,
        });
    }
    indexes
}

fn parse_static_type(expression: &Expr) -> Result<StaticType, RecordError> {
    match &expression.kind {
        ExprKind::Name(name) | ExprKind::Keyword(name) => match name.canonical.as_str() {
            "Any" => Ok(StaticType::Any),
            "None" => Ok(StaticType::None),
            "Bool" => Ok(StaticType::Bool),
            "Int" => Ok(StaticType::Int),
            "Float" => Ok(StaticType::Float),
            "Str" => Ok(StaticType::Str),
            "Keyword" => Ok(StaticType::Keyword),
            "Symbol" => Ok(StaticType::Symbol),
            _ => Err(RecordError::at(
                RECORD_SCHEMA_TYPE,
                format!("unknown static type `{}`", name.spelling),
                expression.span,
            )),
        },
        ExprKind::Call(call) => {
            let Some(callee) = call.callee.name() else {
                return Err(RecordError::at(
                    RECORD_SCHEMA_TYPE,
                    "type constructor must have a name",
                    expression.span,
                ));
            };
            let name = callee.canonical.trim_start_matches(':');
            let mut positional: Vec<Expr> = Vec::new();
            for argument in &call.args {
                match argument {
                    ast::CallArg::Positional(value) => positional.push(value.clone()),
                    // AST call lowering treats keyword-shaped OneOf members as
                    // keyword arguments.  Preserve both sides as variants.
                    ast::CallArg::Keyword(argument) if name == "OneOf" => {
                        positional.push(Expr {
                            span: expression.span,
                            metadata: Vec::new(),
                            kind: ExprKind::Keyword(argument.key.clone()),
                        });
                        positional.push(argument.value.clone());
                    }
                    ast::CallArg::Keyword(_) => {
                        return Err(RecordError::at(
                            RECORD_SCHEMA_TYPE,
                            "type constructor arguments must be positional",
                            expression.span,
                        ));
                    }
                }
            }
            match name {
                "List" | "Vector" | "Set" | "Optional" => {
                    if positional.len() != 1 {
                        return Err(RecordError::at(
                            RECORD_SCHEMA_TYPE,
                            format!("{name} expects one type argument"),
                            expression.span,
                        ));
                    }
                    let inner = parse_static_type(&positional[0])?;
                    Ok(match name {
                        "List" => StaticType::List(Box::new(inner)),
                        "Vector" => StaticType::Vector(Box::new(inner)),
                        "Set" => StaticType::Set(Box::new(inner)),
                        _ => StaticType::Optional(Box::new(inner)),
                    })
                }
                "Map" => {
                    if positional.len() != 2 {
                        return Err(RecordError::at(
                            RECORD_SCHEMA_TYPE,
                            "Map expects key and value type arguments",
                            expression.span,
                        ));
                    }
                    Ok(StaticType::Map(
                        Box::new(parse_static_type(&positional[0])?),
                        Box::new(parse_static_type(&positional[1])?),
                    ))
                }
                "OneOf" => {
                    if positional.is_empty() {
                        return Err(RecordError::at(
                            RECORD_SCHEMA_TYPE,
                            "OneOf requires at least one static value",
                            expression.span,
                        ));
                    }
                    positional
                        .into_iter()
                        .map(|value| {
                            StaticDatum::from_expr(&value).map_err(|error| {
                                RecordError::at(RECORD_SCHEMA_TYPE, error.message, value.span)
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()
                        .map(StaticType::OneOf)
                }
                _ => Err(RecordError::at(
                    RECORD_SCHEMA_TYPE,
                    format!("unknown type constructor `{name}`"),
                    expression.span,
                )),
            }
        }
        _ => Err(RecordError::at(
            RECORD_SCHEMA_TYPE,
            "invalid static type expression",
            expression.span,
        )),
    }
}

impl StaticType {
    #[must_use]
    pub fn accepts(&self, value: &StaticDatum) -> bool {
        match self {
            Self::Any => true,
            Self::None => matches!(value, StaticDatum::None),
            Self::Bool => matches!(value, StaticDatum::Bool(_)),
            Self::Int => matches!(value, StaticDatum::Int(_)),
            Self::Float => matches!(value, StaticDatum::Float(_)),
            Self::Str => matches!(value, StaticDatum::Str(_)),
            Self::Keyword => matches!(value, StaticDatum::Keyword(_)),
            Self::Symbol => matches!(value, StaticDatum::Symbol { .. }),
            Self::List(inner) => {
                matches!(value, StaticDatum::List(values) if values.iter().all(|value| inner.accepts(value)))
            }
            Self::Vector(inner) => {
                matches!(value, StaticDatum::Vector(values) if values.iter().all(|value| inner.accepts(value)))
            }
            Self::Map(key, value_type) => {
                matches!(value, StaticDatum::Map(entries) if entries.iter().all(|(key_value, value)| key.accepts(key_value) && value_type.accepts(value)))
            }
            Self::Set(inner) => {
                matches!(value, StaticDatum::Set(values) if values.iter().all(|value| inner.accepts(value)))
            }
            Self::Optional(inner) => matches!(value, StaticDatum::None) || inner.accepts(value),
            Self::OneOf(values) => values.iter().any(|candidate| candidate == value),
        }
    }
}

fn schema_field_json(field: &SchemaField) -> Json {
    let mut fields = vec![
        ("name".to_owned(), Json::String(field.name.clone())),
        ("type".to_owned(), static_type_json(&field.datum_type)),
        ("required".to_owned(), Json::Bool(field.required)),
    ];
    if let Some(default) = &field.default {
        fields.push(("default".to_owned(), default.to_json()));
    }
    Json::Object(fields)
}

fn static_type_json(datum_type: &StaticType) -> Json {
    match datum_type {
        StaticType::Any => Json::String("Any".to_owned()),
        StaticType::None => Json::String("None".to_owned()),
        StaticType::Bool => Json::String("Bool".to_owned()),
        StaticType::Int => Json::String("Int".to_owned()),
        StaticType::Float => Json::String("Float".to_owned()),
        StaticType::Str => Json::String("Str".to_owned()),
        StaticType::Keyword => Json::String("Keyword".to_owned()),
        StaticType::Symbol => Json::String("Symbol".to_owned()),
        StaticType::List(inner) => tagged("list-type", vec![("item", static_type_json(inner))]),
        StaticType::Vector(inner) => tagged("vector-type", vec![("item", static_type_json(inner))]),
        StaticType::Set(inner) => tagged("set-type", vec![("item", static_type_json(inner))]),
        StaticType::Optional(inner) => {
            tagged("optional-type", vec![("item", static_type_json(inner))])
        }
        StaticType::Map(key, value) => tagged(
            "map-type",
            vec![
                ("key", static_type_json(key)),
                ("value", static_type_json(value)),
            ],
        ),
        StaticType::OneOf(values) => tagged(
            "one-of",
            vec![(
                "values",
                Json::Array(values.iter().map(StaticDatum::to_json).collect()),
            )],
        ),
    }
}

fn schema_index_json(index: &SchemaIndex) -> Json {
    Json::Object(vec![
        ("id".to_owned(), Json::String(index.id.clone())),
        ("scope".to_owned(), Json::String(index.scope.clone())),
        (
            "projections".to_owned(),
            Json::Array(
                index
                    .projections
                    .iter()
                    .map(|projection| {
                        Json::Object(vec![
                            (
                                "kind".to_owned(),
                                Json::String(
                                    match projection.kind {
                                        ProjectionKind::Field => "field",
                                        ProjectionKind::Each => "each",
                                    }
                                    .to_owned(),
                                ),
                            ),
                            ("field".to_owned(), Json::String(projection.field.clone())),
                            ("role".to_owned(), Json::String(projection.role.clone())),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

fn expr_keyword(expression: &Expr) -> Option<&str> {
    match &expression.kind {
        ExprKind::Keyword(name) | ExprKind::Name(name) => Some(name.canonical.as_str()),
        _ => None,
    }
}

fn expr_string(expression: &Expr) -> Option<String> {
    match &expression.kind {
        ExprKind::String(value) => Some(value.clone()),
        _ => None,
    }
}

fn expr_integer(expression: &Expr) -> Option<&str> {
    match &expression.kind {
        ExprKind::Integer(value) => Some(value.as_str()),
        _ => None,
    }
}

fn expr_map(expression: &Expr) -> Option<Vec<(&Expr, &Expr)>> {
    match &expression.kind {
        ExprKind::Map(entries) => Some(entries.iter().map(|(key, value)| (key, value)).collect()),
        _ => None,
    }
}

fn expr_bool(expression: &Expr) -> Option<bool> {
    match expression.kind {
        ExprKind::Bool(value) => Some(value),
        _ => None,
    }
}

fn option_bool(expression: &Expr) -> Option<bool> {
    expr_bool(expression)
}

fn diagnostic_from_error(error: RecordError, span: Span) -> Diagnostic {
    Diagnostic::error(error.code, error.message, error.span.unwrap_or(span))
}

fn is_namespaced(value: &str) -> bool {
    value.contains('/') && !value.starts_with('/') && !value.ends_with('/')
}

fn sha256_prefix(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("sha256:{digest:x}")
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct SchemaIdentity {
    pub binding_id: String,
    pub schema_id: String,
    pub version: u64,
    pub body_hash: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct RecordOccurrenceId {
    pub distribution: String,
    pub version: String,
    pub interface_member_id: String,
    pub semantic_interface_hash: String,
    pub stable_record_id: String,
    pub record_body_hash: String,
}

impl RecordOccurrenceId {
    fn json(&self) -> Json {
        Json::Object(vec![
            (
                "distribution".to_owned(),
                Json::String(self.distribution.clone()),
            ),
            ("version".to_owned(), Json::String(self.version.clone())),
            (
                "interface-member-id".to_owned(),
                Json::String(self.interface_member_id.clone()),
            ),
            (
                "semantic-interface-hash".to_owned(),
                Json::String(self.semantic_interface_hash.clone()),
            ),
            (
                "stable-record-id".to_owned(),
                Json::String(self.stable_record_id.clone()),
            ),
            (
                "record-body-hash".to_owned(),
                Json::String(self.record_body_hash.clone()),
            ),
        ])
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RecordOrigin {
    pub module: String,
    pub span: Span,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub macro_origin: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IndexClaim {
    pub index_id: String,
    pub projection_field: String,
    pub projection_role: String,
    pub key: StaticDatum,
    pub normalized_key: String,
    pub raw_spelling: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ValidatedRecord {
    pub schema: SchemaIdentity,
    pub owner_binding_id: String,
    pub owner_name: String,
    pub module: String,
    pub public: bool,
    pub stable_record_id: String,
    pub record_body_hash: String,
    pub fields: Vec<(String, StaticDatum)>,
    pub index_claims: Vec<IndexClaim>,
    pub origin: RecordOrigin,
}

impl ValidatedRecord {
    #[must_use]
    pub fn occurrence(
        &self,
        distribution: impl Into<String>,
        version: impl Into<String>,
        interface_member_id: impl Into<String>,
        semantic_interface_hash: impl Into<String>,
    ) -> RecordOccurrenceId {
        RecordOccurrenceId {
            distribution: distribution.into(),
            version: version.into(),
            interface_member_id: interface_member_id.into(),
            semantic_interface_hash: semantic_interface_hash.into(),
            stable_record_id: self.stable_record_id.clone(),
            record_body_hash: self.record_body_hash.clone(),
        }
    }

    #[must_use]
    pub fn as_public(&self) -> Option<&Self> {
        self.public.then_some(self)
    }

    #[must_use]
    pub fn canonical_body_bytes(&self) -> Vec<u8> {
        self.body_json().bytes()
    }

    /// Verify identities and canonical ordering for a record reconstructed
    /// from an interface.  Provider occurrence data is intentionally absent:
    /// it is assembled only after the semantic interface hash exists.
    pub fn verify_integrity(&self) -> Result<(), RecordError> {
        if self.owner_binding_id.is_empty()
            || self.owner_name.is_empty()
            || self.module.is_empty()
            || self.origin.module != self.module
        {
            return Err(RecordError::new(
                RECORD_RECORD_SHAPE,
                "record owner, module, and origin must be present and consistent",
            ));
        }
        if self.schema.binding_id.is_empty()
            || !is_namespaced(&self.schema.schema_id)
            || self.schema.version == 0
        {
            return Err(RecordError::new(
                RECORD_RECORD_SHAPE,
                "record contains an invalid schema identity",
            ));
        }

        let expected_stable_id = sha256_prefix(
            &Json::Array(vec![
                Json::String(self.schema.binding_id.clone()),
                Json::String(self.schema.schema_id.clone()),
                Json::Number(self.schema.version.to_string()),
                Json::String(self.schema.body_hash.clone()),
                Json::String(self.owner_binding_id.clone()),
            ])
            .bytes(),
        );
        if self.stable_record_id != expected_stable_id {
            return Err(RecordError::new(
                RECORD_RECORD_SHAPE,
                "stable record id does not match schema and owner identities",
            ));
        }

        let mut previous_field = None;
        for (name, value) in &self.fields {
            if name.is_empty() || previous_field.is_some_and(|previous| previous >= name.as_str()) {
                return Err(RecordError::new(
                    RECORD_RECORD_SHAPE,
                    "record fields must have unique names in canonical order",
                ));
            }
            if value.clone().canonicalize()? != *value {
                return Err(RecordError::new(
                    RECORD_RECORD_SHAPE,
                    format!("record field `{name}` is not canonical"),
                ));
            }
            previous_field = Some(name.as_str());
        }

        let mut sorted_claims = self.index_claims.clone();
        sorted_claims.sort_by(index_claim_cmp);
        if sorted_claims != self.index_claims {
            return Err(RecordError::new(
                RECORD_RECORD_INDEX,
                "record index claims are not in canonical order",
            ));
        }
        let mut claim_keys = BTreeSet::new();
        for claim in &self.index_claims {
            if claim.index_id.is_empty()
                || claim.projection_field.is_empty()
                || claim.projection_role.is_empty()
                || !claim_keys.insert((claim.index_id.clone(), claim.normalized_key.clone()))
            {
                return Err(RecordError::new(
                    RECORD_RECORD_INDEX,
                    "record contains an empty or duplicate index claim",
                ));
            }
            if claim.key.clone().canonicalize()? != claim.key {
                return Err(RecordError::new(
                    RECORD_RECORD_INDEX,
                    "record index key is not canonical",
                ));
            }
            let (normalized_key, raw_spelling) = normalize_index_key(&claim.key)?;
            if normalized_key != claim.normalized_key || raw_spelling != claim.raw_spelling {
                return Err(RecordError::new(
                    RECORD_RECORD_INDEX,
                    "record index normalization does not match its key",
                ));
            }
        }

        let expected_body_hash = sha256_prefix(&self.canonical_body_bytes());
        if self.record_body_hash != expected_body_hash {
            return Err(RecordError::new(
                RECORD_RECORD_SHAPE,
                "record body hash does not match record payload",
            ));
        }
        Ok(())
    }

    fn body_json(&self) -> Json {
        Json::Object(vec![
            (
                "stable-record-id".to_owned(),
                Json::String(self.stable_record_id.clone()),
            ),
            ("schema".to_owned(), schema_identity_json(&self.schema)),
            (
                "owner-binding-id".to_owned(),
                Json::String(self.owner_binding_id.clone()),
            ),
            (
                "owner-name".to_owned(),
                Json::String(self.owner_name.clone()),
            ),
            ("module".to_owned(), Json::String(self.module.clone())),
            (
                "fields".to_owned(),
                Json::Array(
                    self.fields
                        .iter()
                        .map(|(name, value)| {
                            Json::Array(vec![Json::String(name.clone()), value.to_json()])
                        })
                        .collect(),
                ),
            ),
            (
                "index-claims".to_owned(),
                Json::Array(self.index_claims.iter().map(index_claim_json).collect()),
            ),
        ])
    }
}

/// Verify that an interface record is precisely the canonical projection of
/// the schema it names.  This catches a producer which recomputes hashes after
/// changing field types or index claims.
pub fn verify_record_against_schema(
    record: &ValidatedRecord,
    schema: &StaticSchema,
    schema_binding_id: &str,
) -> Result<(), RecordError> {
    schema.verify_integrity()?;
    record.verify_integrity()?;
    if record.schema != schema.identity(schema_binding_id) {
        return Err(RecordError::new(
            RECORD_RECORD_SHAPE,
            "record schema identity does not match the exported schema",
        ));
    }

    let fields = record
        .fields
        .iter()
        .map(|(name, value)| (name.as_str(), value))
        .collect::<BTreeMap<_, _>>();
    for field in &schema.fields {
        match fields.get(field.name.as_str()) {
            Some(value) if !field.datum_type.accepts(value) => {
                return Err(RecordError::new(
                    RECORD_RECORD_TYPE,
                    format!("field `{}` does not match its schema type", field.name),
                ));
            }
            Some(_) => {}
            None if field.required || field.default.is_some() => {
                return Err(RecordError::new(
                    RECORD_RECORD_SHAPE,
                    format!("record is missing materialized field `{}`", field.name),
                ));
            }
            None => {}
        }
    }
    if let Some(name) = fields
        .keys()
        .find(|name| schema.field(name).is_none())
        .copied()
    {
        return Err(RecordError::new(
            RECORD_RECORD_SHAPE,
            format!("record contains unknown field `{name}`"),
        ));
    }

    let mut expected_claims = Vec::new();
    for index in &schema.indexes {
        for projection in &index.projections {
            let value = fields.get(projection.field.as_str()).ok_or_else(|| {
                RecordError::new(
                    RECORD_RECORD_INDEX,
                    format!("index projection field `{}` is missing", projection.field),
                )
            })?;
            let projected = match projection.kind {
                ProjectionKind::Field => vec![(*value).clone()],
                ProjectionKind::Each => match value {
                    StaticDatum::List(values)
                    | StaticDatum::Vector(values)
                    | StaticDatum::Set(values) => values.clone(),
                    _ => {
                        return Err(RecordError::new(
                            RECORD_RECORD_INDEX,
                            format!("`:each` field `{}` is not a collection", projection.field),
                        ));
                    }
                },
            };
            for key in projected {
                let (normalized_key, raw_spelling) = normalize_index_key(&key)?;
                expected_claims.push(IndexClaim {
                    index_id: index.id.clone(),
                    projection_field: projection.field.clone(),
                    projection_role: projection.role.clone(),
                    key,
                    normalized_key,
                    raw_spelling,
                });
            }
        }
    }
    expected_claims.sort_by(index_claim_cmp);
    if expected_claims != record.index_claims {
        return Err(RecordError::new(
            RECORD_RECORD_INDEX,
            "record index claims do not match schema projections",
        ));
    }
    Ok(())
}

/// Validate one static record against a parsed schema.  `owner_binding_id`
/// and `public` come from name/export resolution, not from record data.
pub fn validate_record(
    schema: &StaticSchema,
    declaration: &ast::StaticRecord,
    owner_binding_id: impl Into<String>,
    public: bool,
    module: impl Into<String>,
) -> Result<ValidatedRecord, Vec<Diagnostic>> {
    validate_record_with_schema_binding(
        schema,
        declaration,
        schema.name.clone(),
        owner_binding_id,
        public,
        module,
    )
}

/// Variant used by interface/name resolution when the schema was imported
/// through a qualified name.  The binding id, rather than the source spelling,
/// is what makes a record identity stable across aliases.
pub fn validate_record_with_schema_binding(
    schema: &StaticSchema,
    declaration: &ast::StaticRecord,
    schema_binding_id: impl Into<String>,
    owner_binding_id: impl Into<String>,
    public: bool,
    module: impl Into<String>,
) -> Result<ValidatedRecord, Vec<Diagnostic>> {
    let module = module.into();
    let schema_binding_id = schema_binding_id.into();
    let owner_binding_id = owner_binding_id.into();
    let mut diagnostics = Vec::new();
    let mut supplied = BTreeMap::<String, StaticDatum>::new();
    for (name, expression) in &declaration.fields {
        let field_name = name.canonical.clone();
        if supplied.contains_key(&field_name) {
            diagnostics.push(Diagnostic::error(
                RECORD_RECORD_SHAPE,
                format!("duplicate static-record field `{field_name}`"),
                name_span(name, declaration.span),
            ));
            continue;
        }
        match StaticDatum::from_expr(expression) {
            Ok(value) => {
                supplied.insert(field_name, value);
            }
            Err(error) => diagnostics.push(error),
        }
    }
    let mut fields = Vec::new();
    for field in &schema.fields {
        let value = match supplied.remove(&field.name) {
            Some(value) => value,
            None if field.required => {
                diagnostics.push(Diagnostic::error(
                    RECORD_RECORD_SHAPE,
                    format!("missing required static-record field `{}`", field.name),
                    declaration.span,
                ));
                continue;
            }
            None => match &field.default {
                Some(value) => value.clone(),
                None => continue,
            },
        };
        if !field.datum_type.accepts(&value) {
            diagnostics.push(Diagnostic::error(
                RECORD_RECORD_TYPE,
                format!(
                    "static-record field `{}` does not match its schema type",
                    field.name
                ),
                declaration.span,
            ));
        }
        fields.push((field.name.clone(), value));
    }
    for (unknown, _) in supplied {
        diagnostics.push(Diagnostic::error(
            RECORD_RECORD_SHAPE,
            format!("unknown static-record field `{unknown}`"),
            declaration.span,
        ));
    }
    let mut index_claims = Vec::new();
    let field_values = fields
        .iter()
        .map(|(name, value)| (name.as_str(), value))
        .collect::<BTreeMap<_, _>>();
    let mut claim_keys = BTreeSet::new();
    for index in &schema.indexes {
        for projection in &index.projections {
            let Some(value) = field_values.get(projection.field.as_str()) else {
                // The schema parser already checks this.  Keep this guard for
                // callers constructing a schema programmatically.
                diagnostics.push(Diagnostic::error(
                    RECORD_RECORD_INDEX,
                    format!("index references absent field `{}`", projection.field),
                    declaration.span,
                ));
                continue;
            };
            let projected = match projection.kind {
                ProjectionKind::Field => vec![(*value).clone()],
                ProjectionKind::Each => match value {
                    StaticDatum::List(values)
                    | StaticDatum::Vector(values)
                    | StaticDatum::Set(values) => values.clone(),
                    _ => {
                        diagnostics.push(Diagnostic::error(
                            RECORD_RECORD_INDEX,
                            format!("`:each` field `{}` is not a collection", projection.field),
                            declaration.span,
                        ));
                        Vec::new()
                    }
                },
            };
            for key in projected {
                let (normalized_key, raw_spelling) = match normalize_index_key(&key) {
                    Ok(value) => value,
                    Err(error) => {
                        diagnostics.push(diagnostic_from_error(error, declaration.span));
                        continue;
                    }
                };
                let claim_key = (index.id.clone(), normalized_key.clone());
                if !claim_keys.insert(claim_key) {
                    diagnostics.push(Diagnostic::error(
                        RECORD_RECORD_INDEX,
                        format!("duplicate key `{normalized_key}` in index `{}`", index.id),
                        declaration.span,
                    ));
                    continue;
                }
                index_claims.push(IndexClaim {
                    index_id: index.id.clone(),
                    projection_field: projection.field.clone(),
                    projection_role: projection.role.clone(),
                    key,
                    normalized_key,
                    raw_spelling,
                });
            }
        }
    }
    index_claims.sort_by(index_claim_cmp);
    fields.sort_by(|left, right| left.0.cmp(&right.0));
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    let owner_name = declaration.owner.canonical.clone();
    let schema_identity = schema.identity(schema_binding_id);
    let stable_record_id = sha256_prefix(
        &Json::Array(vec![
            Json::String(schema_identity.binding_id.clone()),
            Json::String(schema_identity.schema_id.clone()),
            Json::Number(schema_identity.version.to_string()),
            Json::String(schema_identity.body_hash.clone()),
            Json::String(owner_binding_id.clone()),
        ])
        .bytes(),
    );
    let provisional = ValidatedRecord {
        schema: schema_identity,
        owner_binding_id,
        owner_name,
        module: module.clone(),
        public,
        stable_record_id,
        record_body_hash: String::new(),
        fields,
        index_claims,
        origin: RecordOrigin {
            module,
            span: declaration.span,
            macro_origin: None,
        },
    };
    let record_body_hash = sha256_prefix(&provisional.body_json().bytes());
    Ok(ValidatedRecord {
        record_body_hash,
        ..provisional
    })
}

fn normalize_index_key(key: &StaticDatum) -> Result<(String, Option<String>), RecordError> {
    match key {
        StaticDatum::Str(value) => {
            let normalized = value.nfc().collect::<String>();
            if normalized.is_empty()
                || normalized.trim() != normalized
                || normalized.chars().any(char::is_control)
                || normalized.chars().count() > 4096
            {
                return Err(RecordError::new(
                    RECORD_RECORD_INDEX,
                    "string index key is empty, padded, contains control characters, or is too long",
                ));
            }
            Ok((normalized.clone(), Some(value.clone())))
        }
        _ => Ok((hex_bytes(&key.canonical_bytes()), None)),
    }
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

fn index_claim_cmp(left: &IndexClaim, right: &IndexClaim) -> Ordering {
    left.index_id
        .cmp(&right.index_id)
        .then_with(|| left.normalized_key.cmp(&right.normalized_key))
        .then_with(|| left.projection_field.cmp(&right.projection_field))
        .then_with(|| left.projection_role.cmp(&right.projection_role))
}

fn name_span(_name: &Name, fallback: Span) -> Span {
    fallback
}

fn schema_identity_json(identity: &SchemaIdentity) -> Json {
    Json::Object(vec![
        (
            "binding-id".to_owned(),
            Json::String(identity.binding_id.clone()),
        ),
        (
            "schema-id".to_owned(),
            Json::String(identity.schema_id.clone()),
        ),
        (
            "version".to_owned(),
            Json::Number(identity.version.to_string()),
        ),
        (
            "body-hash".to_owned(),
            Json::String(identity.body_hash.clone()),
        ),
    ])
}

fn index_claim_json(claim: &IndexClaim) -> Json {
    let mut fields = vec![
        ("index-id".to_owned(), Json::String(claim.index_id.clone())),
        (
            "projection-field".to_owned(),
            Json::String(claim.projection_field.clone()),
        ),
        (
            "projection-role".to_owned(),
            Json::String(claim.projection_role.clone()),
        ),
        ("key".to_owned(), claim.key.to_json()),
        (
            "normalized-key".to_owned(),
            Json::String(claim.normalized_key.clone()),
        ),
    ];
    if let Some(raw) = &claim.raw_spelling {
        fields.push(("raw-spelling".to_owned(), Json::String(raw.clone())));
    }
    Json::Object(fields)
}

#[derive(Clone, Debug, Default)]
pub struct StaticModuleData {
    pub schemas: Vec<StaticSchema>,
    pub records: Vec<ValidatedRecord>,
    pub diagnostics: Vec<Diagnostic>,
}

impl StaticModuleData {
    #[must_use]
    pub fn public_records(&self) -> Vec<&ValidatedRecord> {
        self.records.iter().filter(|record| record.public).collect()
    }
}

#[derive(Clone, Debug)]
struct ImportedSchema<'a> {
    module: String,
    schema: &'a StaticSchema,
    binding_id: String,
}

#[derive(Clone, Debug)]
enum ImportedName<'a> {
    Schema(ImportedSchema<'a>),
    /// A public binding which is not a static schema.  Keeping this state in
    /// the lookup table lets a later `static-record` report a wrong-kind
    /// reference instead of silently treating it as unresolved/local.
    NonSchema,
    /// A malformed/private schema or a missing alias target.  Interfaces read
    /// from `.osri` should already reject these, but direct API callers still
    /// need fail-closed behavior.
    Invalid,
    /// Two imports assign different identities to the same local spelling.
    Conflict,
}

#[derive(Clone, Debug, Default)]
struct ImportedSchemaScope<'a> {
    qualified: BTreeMap<String, ImportedName<'a>>,
    referred: BTreeMap<String, ImportedName<'a>>,
    conflicting_qualifiers: BTreeSet<String>,
}

#[derive(Clone, Debug)]
enum ResolvedSchema {
    Local(StaticSchema),
    Imported {
        schema: StaticSchema,
        binding_id: String,
    },
    Missing,
    NonSchema,
    Invalid,
    Conflict,
}

fn insert_imported_name<'a>(
    names: &mut BTreeMap<String, ImportedName<'a>>,
    key: impl Into<String>,
    value: ImportedName<'a>,
) {
    let key = key.into();
    let Some(existing) = names.get_mut(&key) else {
        names.insert(key, value);
        return;
    };
    if matches!(existing, ImportedName::Conflict) {
        return;
    }
    let same_schema = matches!(
        (&*existing, &value),
        (ImportedName::Schema(left), ImportedName::Schema(right))
            if left.module == right.module && left.binding_id == right.binding_id
    );
    if !same_schema {
        *existing = ImportedName::Conflict;
    }
}

fn public_schema_names<'a>(
    module_name: &str,
    interface: &'a crate::interface::Interface,
) -> BTreeMap<String, ImportedName<'a>> {
    let mut by_binding = BTreeMap::<String, ImportedName<'a>>::new();
    let mut schema_names = BTreeMap::<String, ImportedName<'a>>::new();

    for schema in &interface.static_schemas {
        let binding_id = BindingId::new(module_name, &schema.name, BindingKind::Type)
            .as_str()
            .to_owned();
        let valid_binding = interface.bindings.iter().any(|binding| {
            binding.id == binding_id
                && binding.kind == BindingKind::Type
                && binding.canonical == schema.name
        });
        let valid_schema = valid_binding && schema.verify_integrity().is_ok();
        let entry = if valid_schema {
            ImportedName::Schema(ImportedSchema {
                module: module_name.to_owned(),
                schema,
                binding_id: binding_id.clone(),
            })
        } else {
            ImportedName::Invalid
        };
        insert_imported_name(&mut by_binding, binding_id.clone(), entry);
        insert_imported_name(
            &mut schema_names,
            schema.name.clone(),
            by_binding[&binding_id].clone(),
        );
    }

    // A public type binding without a matching static schema is deliberately
    // marked invalid: accepting it as a record schema would make the result
    // depend on an interface producer's omitted payload.
    for binding in &interface.bindings {
        if binding.kind != BindingKind::Type {
            insert_imported_name(
                &mut schema_names,
                binding.canonical.clone(),
                ImportedName::NonSchema,
            );
            continue;
        }
        let entry = by_binding
            .get(&binding.id)
            .cloned()
            .unwrap_or(ImportedName::Invalid);
        insert_imported_name(&mut schema_names, binding.canonical.clone(), entry);
    }

    // Public aliases are resolved by their stable target binding id.  An alias
    // to a function/value remains a non-schema entry so `static-record` can
    // issue a precise wrong-kind diagnostic.
    for alias in &interface.aliases {
        let entry = by_binding
            .get(&alias.target)
            .cloned()
            .unwrap_or(ImportedName::Invalid);
        insert_imported_name(&mut schema_names, alias.canonical.clone(), entry.clone());
        insert_imported_name(&mut schema_names, alias.spelling.clone(), entry);
    }

    schema_names
}

fn imported_schema_scope<'a>(
    module: &Module,
    interfaces: &'a BTreeMap<String, crate::interface::Interface>,
) -> ImportedSchemaScope<'a> {
    let mut scope = ImportedSchemaScope::default();
    let mut qualifier_modules = BTreeMap::<String, String>::new();
    for item in &module.items {
        let ItemKind::Import(import) = &item.kind else {
            continue;
        };
        let module_name = import.module.canonical.as_str();
        let Some(interface) = interfaces.get(module_name) else {
            // Missing interfaces are reported when a static-record actually
            // names one of their members; ordinary runtime imports remain the
            // responsibility of the module/HIR dependency checker.
            continue;
        };
        if interface.module != module_name {
            continue;
        }
        let names = public_schema_names(module_name, interface);
        let base = import
            .alias
            .as_ref()
            .map_or(module_name, |alias| alias.canonical.as_str());
        for qualifier in [base, module_name] {
            if let Some(previous) =
                qualifier_modules.insert(qualifier.to_owned(), module_name.to_owned())
                && previous != module_name
            {
                scope.conflicting_qualifiers.insert(qualifier.to_owned());
            }
            for (name, entry) in &names {
                insert_imported_name(
                    &mut scope.qualified,
                    format!("{qualifier}/{name}"),
                    entry.clone(),
                );
                insert_imported_name(
                    &mut scope.qualified,
                    format!("{qualifier}.{name}"),
                    entry.clone(),
                );
            }
        }
        for member in &import.members {
            let entry = names
                .get(&member.canonical)
                .or_else(|| names.get(&member.spelling))
                .cloned()
                .unwrap_or(ImportedName::Invalid);
            insert_imported_name(&mut scope.referred, member.canonical.clone(), entry.clone());
            insert_imported_name(&mut scope.referred, member.spelling.clone(), entry);
        }
    }
    scope
}

fn resolve_imported_name<'scope, 'interface>(
    scope: &'scope ImportedSchemaScope<'interface>,
    name: &str,
) -> Option<&'scope ImportedName<'interface>> {
    if name.contains('/') || name.contains('.') {
        scope.qualified.get(name)
    } else {
        scope.referred.get(name)
    }
}

fn resolve_schema(
    name: &str,
    local: &BTreeMap<String, StaticSchema>,
    scope: &ImportedSchemaScope<'_>,
) -> ResolvedSchema {
    if (name.contains('/') || name.contains('.'))
        && name
            .split_once('/')
            .or_else(|| name.split_once('.'))
            .is_some_and(|(base, _)| scope.conflicting_qualifiers.contains(base))
    {
        return ResolvedSchema::Conflict;
    }
    let imported = resolve_imported_name(scope, name);
    if let Some(schema) = local.get(name) {
        // A local declaration and an imported `:refer`/qualified entry with
        // the same spelling are ambiguous, even though the local map could
        // technically win by insertion order.
        if imported.is_some() {
            return ResolvedSchema::Conflict;
        }
        return ResolvedSchema::Local(schema.clone());
    }
    let Some(imported) = imported else {
        return ResolvedSchema::Missing;
    };
    match imported {
        ImportedName::Schema(schema) => ResolvedSchema::Imported {
            schema: schema.schema.clone(),
            binding_id: schema.binding_id.clone(),
        },
        ImportedName::NonSchema => ResolvedSchema::NonSchema,
        ImportedName::Invalid => ResolvedSchema::Invalid,
        ImportedName::Conflict => ResolvedSchema::Conflict,
    }
}

/// Parse and validate the static declarations that can be resolved inside one
/// module.  Imported schemas can be supplied later through
/// [`validate_record_with_schema_binding`]; unresolved qualified names are
/// reported rather than guessed.
pub fn analyze_module(module: &Module) -> StaticModuleData {
    let interfaces: BTreeMap<String, crate::interface::Interface> = BTreeMap::new();
    analyze_module_with_interfaces(module, &interfaces)
}

/// Parse and validate static declarations with an explicit, read-only map of
/// imported compilation interfaces.  Only public type bindings which have a
/// matching public static schema are made available to a record; this keeps
/// static validation independent from the Python runtime and fails closed on
/// missing, private, malformed, or ambiguous imports.
pub fn analyze_module_with_interfaces(
    module: &Module,
    interfaces: &BTreeMap<String, crate::interface::Interface>,
) -> StaticModuleData {
    let module_name = module
        .name
        .as_ref()
        .map_or_else(|| "<anonymous>".to_owned(), |name| name.canonical.clone());
    let mut result = StaticModuleData::default();
    let mut schemas = BTreeMap::<String, StaticSchema>::new();
    for item in &module.items {
        if let ItemKind::DefstaticSchema(declaration) = &item.kind {
            match parse_schema(declaration) {
                Ok(schema) => {
                    if schemas
                        .insert(declaration.name.canonical.clone(), schema.clone())
                        .is_some()
                    {
                        result.diagnostics.push(Diagnostic::error(
                            RECORD_SCHEMA_SHAPE,
                            format!("duplicate schema `{}`", declaration.name.canonical),
                            declaration.span,
                        ));
                    } else {
                        result.schemas.push(schema);
                    }
                }
                Err(diagnostics) => result.diagnostics.extend(diagnostics),
            }
        }
    }

    let mut declarations = BTreeMap::<String, BindingKind>::new();
    let mut aliases = BTreeMap::<String, String>::new();
    let mut exports = BTreeSet::new();
    for item in &module.items {
        match &item.kind {
            ItemKind::Def(declaration) => {
                declarations.insert(declaration.name.canonical.clone(), BindingKind::Value);
            }
            ItemKind::Defn(function) => {
                if let Some(name) = &function.name {
                    declarations.insert(name.canonical.clone(), BindingKind::Function);
                }
            }
            ItemKind::Defstruct(declaration) => {
                declarations.insert(declaration.name.canonical.clone(), BindingKind::Type);
            }
            ItemKind::Extern(external) => {
                for nested in &external.items {
                    match &nested.kind {
                        ItemKind::Def(declaration) => {
                            declarations
                                .insert(declaration.name.canonical.clone(), BindingKind::Value);
                        }
                        ItemKind::Defn(function) => {
                            if let Some(name) = &function.name {
                                declarations.insert(name.canonical.clone(), BindingKind::Function);
                            }
                        }
                        _ => {}
                    }
                }
            }
            ItemKind::Alias(alias) => {
                aliases.insert(
                    alias.local.canonical.clone(),
                    alias.target.canonical.clone(),
                );
            }
            ItemKind::Export(export) => {
                exports.extend(export.names.iter().map(|name| name.canonical.clone()));
            }
            _ => {}
        }
    }
    for schema in &result.schemas {
        declarations.insert(schema.name.clone(), BindingKind::Type);
    }

    let resolve_alias = |name: &str, aliases: &BTreeMap<String, String>| {
        let mut current = name.to_owned();
        let mut visited = BTreeSet::new();
        while let Some(target) = aliases.get(&current) {
            if !visited.insert(current.clone()) {
                break;
            }
            current = target.clone();
        }
        current
    };
    let is_exported = |name: &str| {
        let canonical = resolve_alias(name, &aliases);
        exports.contains(name) || exports.contains(&canonical)
    };
    let imported_scope = imported_schema_scope(module, interfaces);

    let mut seen_owner_schema = BTreeSet::new();
    for item in &module.items {
        let ItemKind::StaticRecord(record) = &item.kind else {
            continue;
        };
        let (schema, schema_binding, schema_is_public) = match resolve_schema(
            &record.schema.canonical,
            &schemas,
            &imported_scope,
        ) {
            ResolvedSchema::Local(schema) => {
                let binding = BindingId::new(&module_name, &schema.name, BindingKind::Type)
                    .as_str()
                    .to_owned();
                let public = is_exported(&schema.name);
                (schema, binding, public)
            }
            ResolvedSchema::Imported { schema, binding_id } => (schema, binding_id, true),
            ResolvedSchema::Missing => {
                result.diagnostics.push(Diagnostic::error(
                    RECORD_RECORD_SHAPE,
                    format!(
                        "static-record references unresolved schema `{}`; schema must be local or imported from a public interface",
                        record.schema.canonical
                    ),
                    record.span,
                ));
                continue;
            }
            ResolvedSchema::NonSchema => {
                result.diagnostics.push(Diagnostic::error(
                    RECORD_RECORD_SHAPE,
                    format!(
                        "static-record schema `{}` resolves to a non-schema export",
                        record.schema.canonical
                    ),
                    record.span,
                ));
                continue;
            }
            ResolvedSchema::Invalid => {
                result.diagnostics.push(Diagnostic::error(
                    RECORD_RECORD_SHAPE,
                    format!(
                        "static-record schema `{}` references a missing, private, or invalid imported schema",
                        record.schema.canonical
                    ),
                    record.span,
                ));
                continue;
            }
            ResolvedSchema::Conflict => {
                result.diagnostics.push(Diagnostic::error(
                    RECORD_RECORD_SHAPE,
                    format!(
                        "static-record schema `{}` has conflicting local or imported bindings",
                        record.schema.canonical
                    ),
                    record.span,
                ));
                continue;
            }
        };
        let owner_name = resolve_alias(&record.owner.canonical, &aliases);
        let Some(owner_kind) = declarations.get(&owner_name).copied() else {
            result.diagnostics.push(Diagnostic::error(
                RECORD_RECORD_SHAPE,
                format!(
                    "static-record owner `{}` is not a top-level declaration",
                    record.owner.canonical
                ),
                record.span,
            ));
            continue;
        };
        let owner_binding = BindingId::new(&module_name, &owner_name, owner_kind)
            .as_str()
            .to_owned();
        let public = is_exported(&owner_name);
        if public && !schema_is_public {
            result.diagnostics.push(Diagnostic::error(
                RECORD_RECORD_SHAPE,
                format!(
                    "public record owner requires exported schema `{}`",
                    schema.name
                ),
                record.span,
            ));
        }
        let pair = (
            schema.schema_id.clone(),
            schema.version,
            owner_binding.clone(),
        );
        if !seen_owner_schema.insert(pair) {
            result.diagnostics.push(Diagnostic::error(
                RECORD_RECORD_SHAPE,
                format!(
                    "owner `{}` has more than one record for schema `{}`",
                    owner_name, schema.schema_id
                ),
                record.span,
            ));
            continue;
        }
        match validate_record_with_schema_binding(
            &schema,
            record,
            schema_binding,
            owner_binding,
            public,
            module_name.clone(),
        ) {
            Ok(record) => result.records.push(record),
            Err(diagnostics) => result.diagnostics.extend(diagnostics),
        }
    }
    result
        .schemas
        .sort_by(|left, right| left.name.cmp(&right.name));
    result
        .records
        .sort_by(|left, right| left.stable_record_id.cmp(&right.stable_record_id));
    result
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IndexedRecord {
    pub occurrence: RecordOccurrenceId,
    pub record: ValidatedRecord,
    #[serde(default)]
    pub dependency_path: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MergedIndexClaim {
    pub index_id: String,
    pub normalized_key: String,
    pub raw_spelling: Option<String>,
    pub projection_field: String,
    pub projection_role: String,
    pub occurrence: RecordOccurrenceId,
    pub dependency_path: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MergedIndexes {
    pub claims: Vec<MergedIndexClaim>,
    pub effective_record_index_hash: String,
}

/// Merge all public index claims.  Exact occurrence+claim repeats are the only
/// idempotent case, which is what makes diamond dependency traversal harmless.
pub fn merge_unique_indexes<I>(records: I) -> Result<MergedIndexes, Vec<Diagnostic>>
where
    I: IntoIterator<Item = IndexedRecord>,
{
    let mut records = records
        .into_iter()
        .filter(|record| record.record.public)
        .collect::<Vec<_>>();
    records.sort_by(|left, right| {
        left.occurrence
            .cmp(&right.occurrence)
            .then_with(|| left.dependency_path.cmp(&right.dependency_path))
    });
    let mut by_key = BTreeMap::<(String, String), Vec<MergedIndexClaim>>::new();
    for indexed in records {
        for claim in &indexed.record.index_claims {
            let merged = MergedIndexClaim {
                index_id: claim.index_id.clone(),
                normalized_key: claim.normalized_key.clone(),
                raw_spelling: claim.raw_spelling.clone(),
                projection_field: claim.projection_field.clone(),
                projection_role: claim.projection_role.clone(),
                occurrence: indexed.occurrence.clone(),
                dependency_path: indexed.dependency_path.clone(),
            };
            by_key
                .entry((merged.index_id.clone(), merged.normalized_key.clone()))
                .or_default()
                .push(merged);
        }
    }
    let mut claims = Vec::new();
    let mut diagnostics = Vec::new();
    for ((index_id, normalized_key), mut candidates) in by_key {
        candidates.sort_by(merged_claim_cmp);
        let mut unique = Vec::new();
        for candidate in candidates {
            if unique.iter().any(|existing: &MergedIndexClaim| {
                existing.occurrence == candidate.occurrence
                    && existing.projection_field == candidate.projection_field
                    && existing.projection_role == candidate.projection_role
                    && existing.normalized_key == candidate.normalized_key
            }) {
                continue;
            }
            if let Some(existing) = unique.first() {
                diagnostics.push(Diagnostic::error(
                    RECORD_INDEX_CONFLICT,
                    format!(
                        "unique index `{index_id}` key `{normalized_key}` is claimed by `{}` ({}) and `{}` ({})",
                        occurrence_display(&existing.occurrence),
                        existing.projection_role,
                        occurrence_display(&candidate.occurrence),
                        candidate.projection_role,
                    ),
                    candidate_span(&candidate),
                ));
            } else {
                unique.push(candidate);
            }
        }
        if diagnostics.is_empty() {
            claims.extend(unique);
        }
    }
    if !diagnostics.is_empty() {
        diagnostics.sort_by(|left, right| {
            left.message
                .cmp(&right.message)
                .then_with(|| left.span.start.cmp(&right.span.start))
        });
        return Err(diagnostics);
    }
    claims.sort_by(merged_claim_cmp);
    let payload = Json::Array(claims.iter().map(merged_claim_json).collect());
    Ok(MergedIndexes {
        effective_record_index_hash: sha256_prefix(&payload.bytes()),
        claims,
    })
}

fn merged_claim_cmp(left: &MergedIndexClaim, right: &MergedIndexClaim) -> Ordering {
    left.index_id
        .cmp(&right.index_id)
        .then_with(|| left.normalized_key.cmp(&right.normalized_key))
        .then_with(|| left.occurrence.cmp(&right.occurrence))
        .then_with(|| left.projection_field.cmp(&right.projection_field))
        .then_with(|| left.projection_role.cmp(&right.projection_role))
}

fn occurrence_display(occurrence: &RecordOccurrenceId) -> String {
    format!(
        "{}:{}:{}:{}",
        occurrence.distribution,
        occurrence.version,
        occurrence.interface_member_id,
        occurrence.stable_record_id
    )
}

fn candidate_span(candidate: &MergedIndexClaim) -> Span {
    // Dependency records carry their source span in `ValidatedRecord`; this
    // compact merged claim intentionally remains transport-friendly.
    let _ = candidate;
    Span::default()
}

fn merged_claim_json(claim: &MergedIndexClaim) -> Json {
    Json::Object(vec![
        ("index-id".to_owned(), Json::String(claim.index_id.clone())),
        (
            "normalized-key".to_owned(),
            Json::String(claim.normalized_key.clone()),
        ),
        (
            "raw-spelling".to_owned(),
            claim
                .raw_spelling
                .as_ref()
                .map_or(Json::Null, |value| Json::String(value.clone())),
        ),
        (
            "projection-field".to_owned(),
            Json::String(claim.projection_field.clone()),
        ),
        (
            "projection-role".to_owned(),
            Json::String(claim.projection_role.clone()),
        ),
        ("occurrence".to_owned(), claim.occurrence.json()),
        (
            "dependency-path".to_owned(),
            Json::Array(
                claim
                    .dependency_path
                    .iter()
                    .map(|value| Json::String(value.clone()))
                    .collect(),
            ),
        ),
    ])
}

pub const RECORD_SIDECAR_FORMAT_VERSION: u64 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SidecarRecord {
    pub occurrence: RecordOccurrenceId,
    pub record: ValidatedRecord,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RecordSidecar {
    pub format_version: u64,
    pub interface_semantic_hashes: Vec<String>,
    pub records: Vec<SidecarRecord>,
    pub record_set_hash: String,
}

impl RecordSidecar {
    #[must_use]
    pub fn records_payload_bytes(&self) -> Vec<u8> {
        Json::Array(self.records.iter().map(sidecar_record_json).collect()).bytes()
    }

    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        sidecar_json(self).bytes()
    }

    #[must_use]
    pub fn records_hash(&self) -> String {
        sha256_prefix(&self.canonical_bytes())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncodedSidecar {
    pub sidecar: RecordSidecar,
    pub bytes: Vec<u8>,
    pub record_set_hash: String,
    pub records_hash: String,
}

/// Build the distribution-owned sidecar from public records only.  The caller
/// should pass records reconstructed from `.osri`; this function never imports
/// or executes an extension module.
pub fn encode_sidecar(
    interface_semantic_hashes: impl IntoIterator<Item = String>,
    records: impl IntoIterator<Item = IndexedRecord>,
) -> Result<EncodedSidecar, RecordError> {
    let mut records = records.into_iter().collect::<Vec<_>>();
    if records.iter().any(|record| !record.record.public) {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            "private static records cannot be emitted in a runtime sidecar",
        ));
    }
    records.sort_by(|left, right| left.occurrence.cmp(&right.occurrence));
    let mut sidecar_records = Vec::new();
    let mut seen = BTreeSet::new();
    for indexed in records {
        if indexed.occurrence.stable_record_id != indexed.record.stable_record_id
            || indexed.occurrence.record_body_hash != indexed.record.record_body_hash
        {
            return Err(RecordError::new(
                RECORD_SIDECAR,
                "record occurrence does not match its validated record",
            ));
        }
        if !seen.insert(indexed.occurrence.clone()) {
            return Err(RecordError::new(
                RECORD_SIDECAR,
                "duplicate record occurrence in sidecar",
            ));
        }
        sidecar_records.push(SidecarRecord {
            occurrence: indexed.occurrence,
            record: indexed.record,
        });
    }
    let mut interface_semantic_hashes = interface_semantic_hashes.into_iter().collect::<Vec<_>>();
    interface_semantic_hashes.sort();
    interface_semantic_hashes.dedup();
    let records_payload = Json::Array(sidecar_records.iter().map(sidecar_record_json).collect());
    let record_set_hash = sha256_prefix(&records_payload.bytes());
    let sidecar = RecordSidecar {
        format_version: RECORD_SIDECAR_FORMAT_VERSION,
        interface_semantic_hashes,
        records: sidecar_records,
        record_set_hash,
    };
    let bytes = sidecar.canonical_bytes();
    let records_hash = sha256_prefix(&bytes);
    Ok(EncodedSidecar {
        record_set_hash: sidecar.record_set_hash.clone(),
        records_hash,
        bytes,
        sidecar,
    })
}

/// Decode and fully verify a sidecar.  `expected_records_hash` is the marker's
/// hash of the complete sidecar bytes; passing it is strongly recommended and
/// makes tampering fail before the decoded data is exposed.
pub fn decode_sidecar(
    bytes: &[u8],
    expected_records_hash: Option<&str>,
) -> Result<RecordSidecar, RecordError> {
    if let Some(expected) = expected_records_hash {
        let actual = sha256_prefix(bytes);
        if actual != expected {
            return Err(RecordError::new(
                RECORD_SIDECAR,
                format!("sidecar byte hash mismatch: expected `{expected}`, got `{actual}`"),
            ));
        }
    }
    let root = serde_json::from_slice::<Json>(bytes).map_err(|error| {
        RecordError::new(RECORD_SIDECAR, format!("invalid sidecar JSON: {error}"))
    })?;
    let sidecar = sidecar_from_json(&root)?;
    if sidecar.canonical_bytes() != bytes {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            "sidecar bytes are not canonical RFC 8785 JSON",
        ));
    }
    let actual_set_hash = sha256_prefix(&sidecar.records_payload_bytes());
    if actual_set_hash != sidecar.record_set_hash {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            format!(
                "record-set-hash mismatch: expected `{}`, got `{actual_set_hash}`",
                sidecar.record_set_hash
            ),
        ));
    }
    Ok(sidecar)
}

/// Compare a sidecar generated from a fresh interface with an existing byte
/// artifact.  This is the fail-closed check used by build backends.
pub fn verify_sidecar_against_records(
    bytes: &[u8],
    expected_records_hash: Option<&str>,
    expected_interface_hashes: &[String],
    expected_records: &[IndexedRecord],
) -> Result<(), RecordError> {
    let decoded = decode_sidecar(bytes, expected_records_hash)?;
    let encoded = encode_sidecar(
        expected_interface_hashes.iter().cloned(),
        expected_records.iter().cloned(),
    )?;
    if decoded != encoded.sidecar || bytes != encoded.bytes {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            "sidecar records differ from the canonical interface records",
        ));
    }
    Ok(())
}

fn sidecar_json(sidecar: &RecordSidecar) -> Json {
    Json::Object(vec![
        (
            "format-version".to_owned(),
            Json::Number(sidecar.format_version.to_string()),
        ),
        (
            "interface-semantic-hashes".to_owned(),
            Json::Array(
                sidecar
                    .interface_semantic_hashes
                    .iter()
                    .map(|value| Json::String(value.clone()))
                    .collect(),
            ),
        ),
        (
            "record-identities".to_owned(),
            Json::Array(
                sidecar
                    .records
                    .iter()
                    .map(|record| record.occurrence.json())
                    .collect(),
            ),
        ),
        (
            "record-set-hash".to_owned(),
            Json::String(sidecar.record_set_hash.clone()),
        ),
        (
            "records".to_owned(),
            Json::Array(sidecar.records.iter().map(sidecar_record_json).collect()),
        ),
    ])
}

fn sidecar_record_json(record: &SidecarRecord) -> Json {
    Json::Object(vec![
        ("occurrence".to_owned(), record.occurrence.json()),
        ("record".to_owned(), record_json(&record.record)),
    ])
}

fn record_json(record: &ValidatedRecord) -> Json {
    let mut fields = vec![
        ("schema".to_owned(), schema_identity_json(&record.schema)),
        (
            "owner-binding-id".to_owned(),
            Json::String(record.owner_binding_id.clone()),
        ),
        (
            "owner-name".to_owned(),
            Json::String(record.owner_name.clone()),
        ),
        ("module".to_owned(), Json::String(record.module.clone())),
        ("public".to_owned(), Json::Bool(record.public)),
        (
            "stable-record-id".to_owned(),
            Json::String(record.stable_record_id.clone()),
        ),
        (
            "record-body-hash".to_owned(),
            Json::String(record.record_body_hash.clone()),
        ),
        (
            "fields".to_owned(),
            Json::Array(
                record
                    .fields
                    .iter()
                    .map(|(name, value)| {
                        Json::Array(vec![Json::String(name.clone()), value.to_json()])
                    })
                    .collect(),
            ),
        ),
        (
            "index-claims".to_owned(),
            Json::Array(record.index_claims.iter().map(index_claim_json).collect()),
        ),
        ("origin".to_owned(), origin_json(&record.origin)),
    ];
    // `record_json` is a sidecar payload, so all members are mandatory.  The
    // vector is kept explicit to make additions visible in code review.
    fields.shrink_to_fit();
    Json::Object(fields)
}

fn origin_json(origin: &RecordOrigin) -> Json {
    let mut fields = vec![
        ("module".to_owned(), Json::String(origin.module.clone())),
        (
            "span".to_owned(),
            Json::Array(vec![
                Json::Number(origin.span.start.to_string()),
                Json::Number(origin.span.end.to_string()),
            ]),
        ),
    ];
    if let Some(value) = &origin.macro_origin {
        fields.push(("macro-origin".to_owned(), Json::String(value.clone())));
    }
    Json::Object(fields)
}

fn sidecar_from_json(root: &Json) -> Result<RecordSidecar, RecordError> {
    let Json::Object(fields) = root else {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            "sidecar root must be an object",
        ));
    };
    let format_version = object_u64(fields, "format-version")?;
    if format_version != RECORD_SIDECAR_FORMAT_VERSION {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            format!("unsupported sidecar format version {format_version}"),
        ));
    }
    let interface_semantic_hashes = object_string_array(fields, "interface-semantic-hashes")?;
    let record_identities = object_array(fields, "record-identities")?;
    let record_set_hash = object_string(fields, "record-set-hash")?;
    let records_json = object_array(fields, "records")?;
    if record_identities.len() != records_json.len() {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            "record-identities and records lengths differ",
        ));
    }
    let mut records = Vec::with_capacity(records_json.len());
    for (identity_json, record_json) in record_identities.iter().zip(records_json) {
        let occurrence = occurrence_from_json(identity_json)?;
        let Json::Object(record_fields) = record_json else {
            return Err(RecordError::new(
                RECORD_SIDECAR,
                "sidecar record must be an object",
            ));
        };
        let occurrence_in_record =
            occurrence_from_json(object_field(record_fields, "occurrence")?)?;
        if occurrence != occurrence_in_record {
            return Err(RecordError::new(
                RECORD_SIDECAR,
                "record identity differs between header and record payload",
            ));
        }
        let record = validated_record_from_json(object_field(record_fields, "record")?)?;
        if occurrence.stable_record_id != record.stable_record_id
            || occurrence.record_body_hash != record.record_body_hash
        {
            return Err(RecordError::new(
                RECORD_SIDECAR,
                "record occurrence does not match record payload identity",
            ));
        }
        records.push(SidecarRecord { occurrence, record });
    }
    let sidecar = RecordSidecar {
        format_version,
        interface_semantic_hashes,
        records,
        record_set_hash,
    };
    if sidecar.records.iter().any(|record| !record.record.public) {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            "sidecar contains a private record",
        ));
    }
    Ok(sidecar)
}

fn object_u64(fields: &[(String, Json)], key: &str) -> Result<u64, RecordError> {
    match object_field(fields, key)? {
        Json::Number(value) => value.parse::<u64>().map_err(|_| {
            RecordError::new(
                RECORD_SIDECAR,
                format!("JSON member `{key}` must be a positive integer"),
            )
        }),
        _ => Err(RecordError::new(
            RECORD_SIDECAR,
            format!("JSON member `{key}` must be an integer"),
        )),
    }
}

fn object_string_array(fields: &[(String, Json)], key: &str) -> Result<Vec<String>, RecordError> {
    object_array(fields, key)?
        .iter()
        .map(|value| match value {
            Json::String(value) => Ok(value.clone()),
            _ => Err(RecordError::new(
                RECORD_SIDECAR,
                format!("JSON member `{key}` must contain strings"),
            )),
        })
        .collect()
}

fn occurrence_from_json(value: &Json) -> Result<RecordOccurrenceId, RecordError> {
    let Json::Object(fields) = value else {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            "record occurrence must be an object",
        ));
    };
    require_keys(
        fields,
        &[
            "distribution",
            "version",
            "interface-member-id",
            "semantic-interface-hash",
            "stable-record-id",
            "record-body-hash",
        ],
    )?;
    Ok(RecordOccurrenceId {
        distribution: object_string(fields, "distribution")?,
        version: object_string(fields, "version")?,
        interface_member_id: object_string(fields, "interface-member-id")?,
        semantic_interface_hash: object_string(fields, "semantic-interface-hash")?,
        stable_record_id: object_string(fields, "stable-record-id")?,
        record_body_hash: object_string(fields, "record-body-hash")?,
    })
}

fn schema_identity_from_json(value: &Json) -> Result<SchemaIdentity, RecordError> {
    let Json::Object(fields) = value else {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            "schema identity must be an object",
        ));
    };
    require_keys(fields, &["binding-id", "schema-id", "version", "body-hash"])?;
    Ok(SchemaIdentity {
        binding_id: object_string(fields, "binding-id")?,
        schema_id: object_string(fields, "schema-id")?,
        version: object_u64(fields, "version")?,
        body_hash: object_string(fields, "body-hash")?,
    })
}

fn validated_record_from_json(value: &Json) -> Result<ValidatedRecord, RecordError> {
    let Json::Object(fields) = value else {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            "record payload must be an object",
        ));
    };
    require_keys(
        fields,
        &[
            "schema",
            "owner-binding-id",
            "owner-name",
            "module",
            "public",
            "stable-record-id",
            "record-body-hash",
            "fields",
            "index-claims",
            "origin",
        ],
    )?;
    let schema = schema_identity_from_json(object_field(fields, "schema")?)?;
    let owner_binding_id = object_string(fields, "owner-binding-id")?;
    let owner_name = object_string(fields, "owner-name")?;
    let module = object_string(fields, "module")?;
    let public = object_bool(fields, "public")?;
    let stable_record_id = object_string(fields, "stable-record-id")?;
    let record_body_hash = object_string(fields, "record-body-hash")?;
    let mut record_fields = Vec::new();
    let mut field_names = BTreeSet::new();
    for field_json in object_array(fields, "fields")? {
        let Json::Array(pair) = field_json else {
            return Err(RecordError::new(
                RECORD_SIDECAR,
                "record field must be a pair",
            ));
        };
        if pair.len() != 2 {
            return Err(RecordError::new(
                RECORD_SIDECAR,
                "record field pair must have two values",
            ));
        }
        let Json::String(name) = &pair[0] else {
            return Err(RecordError::new(
                RECORD_SIDECAR,
                "record field name must be a string",
            ));
        };
        if !field_names.insert(name.clone()) {
            return Err(RecordError::new(RECORD_SIDECAR, "duplicate record field"));
        }
        record_fields.push((name.clone(), StaticDatum::from_json(&pair[1])?));
    }
    let mut index_claims = object_array(fields, "index-claims")?
        .iter()
        .map(index_claim_from_json)
        .collect::<Result<Vec<_>, _>>()?;
    index_claims.sort_by(index_claim_cmp);
    let origin = origin_from_json(object_field(fields, "origin")?)?;
    let record = ValidatedRecord {
        schema,
        owner_binding_id,
        owner_name,
        module,
        public,
        stable_record_id,
        record_body_hash,
        fields: record_fields,
        index_claims,
        origin,
    };
    // The body hash is producer data, but it is cheap to recompute and catches
    // field/index tampering even when a caller omitted marker verification.
    let expected_body_hash = sha256_prefix(&record.body_json().bytes());
    if expected_body_hash != record.record_body_hash {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            "record-body-hash does not match record payload",
        ));
    }
    Ok(record)
}

fn index_claim_from_json(value: &Json) -> Result<IndexClaim, RecordError> {
    let Json::Object(fields) = value else {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            "index claim must be an object",
        ));
    };
    let allowed = [
        "index-id",
        "projection-field",
        "projection-role",
        "key",
        "normalized-key",
        "raw-spelling",
    ];
    for (name, _) in fields {
        if !allowed.contains(&name.as_str()) {
            return Err(RecordError::new(
                RECORD_SIDECAR,
                format!("unknown index claim member `{name}`"),
            ));
        }
    }
    Ok(IndexClaim {
        index_id: object_string(fields, "index-id")?,
        projection_field: object_string(fields, "projection-field")?,
        projection_role: object_string(fields, "projection-role")?,
        key: StaticDatum::from_json(object_field(fields, "key")?)?,
        normalized_key: object_string(fields, "normalized-key")?,
        raw_spelling: object_optional_string(fields, "raw-spelling")?,
    })
}

fn origin_from_json(value: &Json) -> Result<RecordOrigin, RecordError> {
    let Json::Object(fields) = value else {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            "record origin must be an object",
        ));
    };
    let module = object_string(fields, "module")?;
    let span = match object_field(fields, "span")? {
        Json::Array(values) if values.len() == 2 => {
            Span::new(json_usize(&values[0])?, json_usize(&values[1])?)
        }
        _ => {
            return Err(RecordError::new(
                RECORD_SIDECAR,
                "record origin span must be a pair",
            ));
        }
    };
    Ok(RecordOrigin {
        module,
        span,
        macro_origin: object_optional_string(fields, "macro-origin")?,
    })
}

fn object_bool(fields: &[(String, Json)], key: &str) -> Result<bool, RecordError> {
    match object_field(fields, key)? {
        Json::Bool(value) => Ok(*value),
        _ => Err(RecordError::new(
            RECORD_SIDECAR,
            format!("JSON member `{key}` must be boolean"),
        )),
    }
}

fn json_usize(value: &Json) -> Result<usize, RecordError> {
    match value {
        Json::Number(value) => value.parse::<usize>().map_err(|_| {
            RecordError::new(RECORD_SIDECAR, "span offset must be a non-negative integer")
        }),
        _ => Err(RecordError::new(
            RECORD_SIDECAR,
            "span offset must be an integer",
        )),
    }
}

fn require_keys(fields: &[(String, Json)], required: &[&str]) -> Result<(), RecordError> {
    for key in required {
        if !fields.iter().any(|(name, _)| name == key) {
            return Err(RecordError::new(
                RECORD_SIDECAR,
                format!("missing JSON member `{key}`"),
            ));
        }
    }
    for (name, _) in fields {
        if !required.contains(&name.as_str()) {
            return Err(RecordError::new(
                RECORD_SIDECAR,
                format!("unknown JSON member `{name}`"),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::{ast::lower_document, hir, interface, reader::read};

    fn lower(source: &str) -> ast::Module {
        let document = read(source);
        let result = lower_document(&document);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        result.module
    }

    fn sample_module() -> ast::Module {
        lower(
            r#"(module example)
               (export [owner S])
               (defstatic-schema S
                 :schema-id "example/schema"
                 :version 1
                 :fields {:id {:type Str :required true}
                          :tags {:type (Vector Str) :default []}}
                 :indexes [{:id "example/id"
                            :keys [{:field :id :role :canonical}]}])
               (def owner none)
               (static-record S owner {:id "alpha"})"#,
        )
    }

    fn dependency_interface_named(module_name: &str) -> interface::Interface {
        let source = format!(
            r#"(module {module_name})
               (defstatic-schema Descriptor
                 :schema-id "dep/descriptor"
                 :version 1
                 :fields {{:id {{:type Str :required true}}}})
               (alias SchemaAlias Descriptor)
               (def owner none)
               (export [Descriptor SchemaAlias])"#
        );
        let surface = lower(&source);
        let typed = hir::lower_module(&surface, module_name);
        assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
        let encoded = interface::emit(&typed.module, &surface).expect("dependency .osri");
        interface::read(&encoded).expect("dependency .osri should validate")
    }

    fn dependency_interface() -> interface::Interface {
        dependency_interface_named("dep.schemas")
    }

    fn analyze_imported(source: &str) -> StaticModuleData {
        let module = lower(source);
        let dependency = dependency_interface();
        let interfaces = BTreeMap::from([(dependency.module.clone(), dependency)]);
        analyze_module_with_interfaces(&module, &interfaces)
    }

    #[test]
    fn datum_encoding_is_tagged_and_preserves_float_bits() {
        let datum = StaticDatum::Float((-0.0f64).to_bits());
        assert_eq!(
            String::from_utf8(datum.canonical_bytes()).expect("JSON"),
            r#"{"$osiris":"float","value":"8000000000000000"}"#
        );
        let integer = StaticDatum::Int("0007".to_owned()).canonicalize().unwrap();
        assert_eq!(integer, StaticDatum::Int("7".to_owned()));
    }

    #[test]
    fn duplicate_map_keys_and_set_items_are_rejected() {
        let map = StaticDatum::Map(vec![
            (
                StaticDatum::Keyword(":a".to_owned()),
                StaticDatum::Int("1".to_owned()),
            ),
            (
                StaticDatum::Keyword(":a".to_owned()),
                StaticDatum::Int("2".to_owned()),
            ),
        ]);
        assert!(map.canonicalize().is_err());
        let set = StaticDatum::Set(vec![StaticDatum::Bool(true), StaticDatum::Bool(true)]);
        assert!(set.canonicalize().is_err());
    }

    #[test]
    fn runtime_call_is_not_static_data() {
        let module = lower("(foo 1)");
        let ast::ItemKind::Expr(expression) = &module.items[0].kind else {
            panic!("expected expression");
        };
        let error = StaticDatum::from_expr(expression).expect_err("call must fail");
        assert_eq!(error.code, RECORD_INVALID_DATUM);
    }

    #[test]
    fn schema_defaults_types_and_index_claims_validate() {
        let module = sample_module();
        let data = analyze_module(&module);
        assert!(data.diagnostics.is_empty(), "{:?}", data.diagnostics);
        assert_eq!(data.schemas.len(), 1);
        assert_eq!(data.records.len(), 1);
        let record = &data.records[0];
        assert!(record.public);
        assert_eq!(record.fields.len(), 2, "default should be materialized");
        assert_eq!(record.index_claims[0].normalized_key, "alpha");
    }

    #[test]
    fn imported_qualified_schema_uses_provider_binding_identity() {
        let data = analyze_imported(
            r#"(module app.records)
               (import dep.schemas :as dep)
               (def owner none)
               (export [owner])
               (static-record dep/Descriptor owner {:id "alpha"})"#,
        );
        assert!(data.diagnostics.is_empty(), "{:?}", data.diagnostics);
        assert_eq!(data.records.len(), 1);
        assert_eq!(
            data.records[0].schema.binding_id,
            "dep.schemas::type::Descriptor"
        );
        assert!(data.records[0].public);
    }

    #[test]
    fn imported_schema_alias_and_refer_resolve_to_the_same_identity() {
        let data = analyze_imported(
            r#"(module app.records)
               (import dep.schemas :refer [SchemaAlias])
               (def owner none)
               (static-record SchemaAlias owner {:id "alpha"})"#,
        );
        assert!(data.diagnostics.is_empty(), "{:?}", data.diagnostics);
        assert_eq!(data.records.len(), 1);
        assert_eq!(
            data.records[0].schema.binding_id,
            "dep.schemas::type::Descriptor"
        );

        let qualified = analyze_imported(
            r#"(module app.records)
               (import dep.schemas :as dep)
               (def owner none)
               (static-record dep/SchemaAlias owner {:id "alpha"})"#,
        );
        assert!(
            qualified.diagnostics.is_empty(),
            "{:?}",
            qualified.diagnostics
        );
        assert_eq!(
            qualified.records[0].schema.binding_id,
            data.records[0].schema.binding_id
        );
    }

    #[test]
    fn imported_missing_or_private_schema_fails_closed() {
        let missing = analyze_imported(
            r#"(module app.records)
               (import dep.schemas :as dep)
               (def owner none)
               (static-record dep/Missing owner {:id "alpha"})"#,
        );
        assert!(missing.records.is_empty());
        assert!(missing.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == RECORD_RECORD_SHAPE
                && diagnostic.message.contains("unresolved schema")
        }));

        let private = analyze_imported(
            r#"(module app.records)
               (import dep.schemas :as dep)
               (def owner none)
               (static-record dep/owner owner {:id "alpha"})"#,
        );
        assert!(private.records.is_empty());
        assert!(!private.diagnostics.is_empty());
    }

    #[test]
    fn conflicting_referred_schemas_are_rejected() {
        let first = dependency_interface();
        let second = dependency_interface_named("other.schemas");

        let module = lower(
            r#"(module app.records)
               (import dep.schemas :refer [Descriptor])
               (import other.schemas :refer [Descriptor])
               (def owner none)
               (static-record Descriptor owner {:id "alpha"})"#,
        );
        let interfaces = BTreeMap::from([
            (first.module.clone(), first),
            (second.module.clone(), second),
        ]);
        let data = analyze_module_with_interfaces(&module, &interfaces);
        assert!(data.records.is_empty());
        assert!(data.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == RECORD_RECORD_SHAPE && diagnostic.message.contains("conflicting")
        }));
    }

    #[test]
    fn conflicting_module_aliases_are_rejected_even_for_disjoint_schema_names() {
        let first = dependency_interface();
        let second = dependency_interface_named("other.schemas");
        let module = lower(
            r#"(module app.records)
               (import dep.schemas :as dep)
               (import other.schemas :as dep)
               (def owner none)
               (static-record dep/Descriptor owner {:id "alpha"})"#,
        );
        let interfaces = BTreeMap::from([
            (first.module.clone(), first),
            (second.module.clone(), second),
        ]);
        let data = analyze_module_with_interfaces(&module, &interfaces);
        assert!(data.records.is_empty());
        assert!(data.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == RECORD_RECORD_SHAPE && diagnostic.message.contains("conflicting")
        }));
    }

    #[test]
    fn imported_schema_cannot_use_an_imported_owner() {
        let data = analyze_imported(
            r#"(module app.records)
               (import dep.schemas :as dep)
               (def owner none)
               (static-record dep/Descriptor dep/owner {:id "alpha"})"#,
        );
        assert!(data.records.is_empty());
        assert!(data.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("owner `dep/owner` is not a top-level declaration")
        }));
    }

    #[test]
    fn private_owner_is_filtered_from_public_records() {
        let module = lower(
            r#"(module example)
               (defstatic-schema S :schema-id "example/schema" :version 1
                 :fields {:id {:type Str :required true}})
               (def owner none)
               (static-record S owner {:id "alpha"})"#,
        );
        let data = analyze_module(&module);
        assert!(data.diagnostics.is_empty(), "{:?}", data.diagnostics);
        assert_eq!(data.public_records().len(), 0);
    }

    fn fake_indexed(distribution: &str, key: &str, path: &[&str]) -> IndexedRecord {
        let schema = StaticSchema {
            name: "S".to_owned(),
            schema_id: "example/schema".to_owned(),
            version: 1,
            fields: Vec::new(),
            indexes: Vec::new(),
            body_hash: "sha256:schema".to_owned(),
        };
        let claim = IndexClaim {
            index_id: "example/index".to_owned(),
            projection_field: ":id".to_owned(),
            projection_role: "canonical".to_owned(),
            key: StaticDatum::Str(key.to_owned()),
            normalized_key: key.to_owned(),
            raw_spelling: Some(key.to_owned()),
        };
        let record = ValidatedRecord {
            schema: schema.identity("example::type::S"),
            owner_binding_id: format!("example::value::{distribution}"),
            owner_name: distribution.to_owned(),
            module: "example".to_owned(),
            public: true,
            stable_record_id: format!("sha256:{distribution}"),
            record_body_hash: format!("sha256:body-{distribution}"),
            fields: Vec::new(),
            index_claims: vec![claim],
            origin: RecordOrigin {
                module: "example".to_owned(),
                span: Span::default(),
                macro_origin: None,
            },
        };
        IndexedRecord {
            occurrence: record.occurrence("pkg", "1", distribution, "sha256:iface"),
            record,
            dependency_path: path.iter().map(|value| (*value).to_owned()).collect(),
        }
    }

    #[test]
    fn index_merge_deduplicates_exact_diamond_occurrence() {
        let first = fake_indexed("owner", "alpha", &["root", "a"]);
        let mut second = first.clone();
        second.dependency_path = vec!["root".to_owned(), "b".to_owned()];
        let merged = merge_unique_indexes(vec![second, first]).expect("same occurrence dedupes");
        assert_eq!(merged.claims.len(), 1);
        assert!(merged.effective_record_index_hash.starts_with("sha256:"));
    }

    #[test]
    fn index_merge_reports_conflicts_independent_of_traversal_order() {
        let first = fake_indexed("owner-a", "alpha", &["z"]);
        let second = fake_indexed("owner-b", "alpha", &["a"]);
        let left = merge_unique_indexes(vec![first.clone(), second.clone()]).expect_err("conflict");
        let right = merge_unique_indexes(vec![second, first]).expect_err("conflict");
        assert_eq!(left[0].message, right[0].message);
        assert_eq!(left[0].code, RECORD_INDEX_CONFLICT);
    }

    #[test]
    fn sidecar_round_trip_and_tamper_check() {
        let module = sample_module();
        let data = analyze_module(&module);
        let record = data.records[0].clone();
        let indexed = IndexedRecord {
            occurrence: record.occurrence("example", "0.1", "example::owner", "sha256:iface"),
            record,
            dependency_path: Vec::new(),
        };
        let encoded =
            encode_sidecar(vec!["sha256:iface".to_owned()], vec![indexed.clone()]).unwrap();
        let decoded = decode_sidecar(&encoded.bytes, Some(&encoded.records_hash)).unwrap();
        assert_eq!(decoded, encoded.sidecar);
        let mut tampered = encoded.bytes.clone();
        let index = tampered.iter().position(|byte| *byte == b'a').unwrap();
        tampered[index] = b'b';
        assert!(decode_sidecar(&tampered, Some(&encoded.records_hash)).is_err());
        verify_sidecar_against_records(
            &encoded.bytes,
            Some(&encoded.records_hash),
            &["sha256:iface".to_owned()],
            &[indexed],
        )
        .unwrap();
    }

    #[test]
    fn duplicate_json_member_is_rejected_before_hash_validation() {
        let json = br#"{"format-version":1,"format-version":1,"interface-semantic-hashes":[],"record-identities":[],"record-set-hash":"sha256:00","records":[]}"#;
        let error = decode_sidecar(json, None).expect_err("duplicate member");
        assert_eq!(error.code, RECORD_SIDECAR);
        assert!(error.message.contains("duplicate"));
    }
}
