
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
    pub(in crate::records) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            span: None,
        }
    }

    pub(in crate::records) fn at(
        code: &'static str,
        message: impl Into<String>,
        span: Span,
    ) -> Self {
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

    pub(in crate::records) fn to_json(&self) -> Json {
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

    pub(in crate::records) fn from_json(value: &Json) -> Result<Self, RecordError> {
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

pub(in crate::records) fn canonical_integer(value: &str) -> Option<String> {
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

pub(in crate::records) fn canonical_bytes(value: &StaticDatum) -> Vec<u8> {
    value.to_json().bytes()
}
