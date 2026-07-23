pub(in crate::records) fn parse_static_type(expression: &Expr) -> Result<StaticType, RecordError> {
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
