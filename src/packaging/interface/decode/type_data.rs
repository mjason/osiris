use super::super::*;
use super::support::*;

pub(super) fn decode_type(form: &Form) -> InterfaceResult<Type> {
    if let FormKind::Keyword(name) = &form.kind {
        return match name.canonical.trim_start_matches(':') {
            "bool" => Ok(Type::Bool),
            "int" => Ok(Type::Int),
            "float" => Ok(Type::Float),
            "str" => Ok(Type::Str),
            "bytes" => Ok(Type::Bytes),
            "none" => Ok(Type::None),
            "any" => Ok(Type::Any),
            "never" => Ok(Type::Never),
            "unknown" => Ok(Type::Unknown),
            "error" => Ok(Type::Error),
            tag => Err(InterfaceError::new(
                "OSR-I0031",
                format!("unknown type tag `{tag}`"),
            )),
        };
    }
    let parts = expect_vector(form, "type")?;
    let tag = parts
        .first()
        .ok_or_else(|| InterfaceError::new("OSR-I0031", "empty type"))?;
    match expect_keyword(tag, "type tag")? {
        "option" if parts.len() == 2 => Ok(Type::option(decode_type(&parts[1])?)),
        "union" => Ok(Type::union(decode_types(&parts[1..])?)),
        "tuple" => Ok(Type::Tuple(decode_types(&parts[1..])?)),
        "list" if parts.len() == 2 => Ok(Type::List(Box::new(decode_type(&parts[1])?))),
        "vector" if parts.len() == 2 => Ok(Type::Vector(Box::new(decode_type(&parts[1])?))),
        "set" if parts.len() == 2 => Ok(Type::Set(Box::new(decode_type(&parts[1])?))),
        "map" if parts.len() == 3 => Ok(Type::Map(
            Box::new(decode_type(&parts[1])?),
            Box::new(decode_type(&parts[2])?),
        )),
        "fn" if parts.len() == 4 => Ok(Type::Fn(FunctionType {
            parameters: expect_vector(&parts[1], "function parameters")?
                .iter()
                .map(decode_type)
                .collect::<InterfaceResult<_>>()?,
            return_type: Box::new(decode_type(&parts[2])?),
            summaries: decode_summaries(&parts[3])?,
        })),
        "nominal" if parts.len() >= 2 => Ok(Type::Nominal {
            binding: expect_string(&parts[1], "nominal binding")?,
            args: decode_types(&parts[2..])?,
        }),
        "literal" if parts.len() == 2 => Ok(Type::Literal(decode_type_literal(&parts[1])?)),
        "type-var" if parts.len() == 2 => Ok(Type::TypeVar(TypeVarId(expect_u32(
            &parts[1],
            "type variable",
        )?))),
        tag => Err(InterfaceError::new(
            "OSR-I0031",
            format!("invalid type encoding `{tag}`"),
        )),
    }
}

pub(super) fn decode_type_literal(form: &Form) -> InterfaceResult<TypeLiteral> {
    let values = expect_vector(form, "type literal")?;
    let Some(tag) = values.first() else {
        return Err(InterfaceError::new("OSR-I0031", "empty type literal"));
    };
    let tag = expect_keyword(tag, "type literal tag")?;
    let literal = match (tag, values.len()) {
        ("none", 1) => TypeLiteral::None,
        ("bool", 2) => TypeLiteral::Bool(expect_bool(&values[1], "literal bool")?),
        ("integer", 2) => TypeLiteral::Integer(expect_string(&values[1], "literal integer")?),
        ("float", 2) => {
            let bits = expect_string(&values[1], "literal float bits")?;
            if bits.len() != 16
                || !bits
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            {
                return Err(InterfaceError::new(
                    "OSR-I0031",
                    "literal float requires 16 lowercase hexadecimal bits",
                ));
            }
            TypeLiteral::Float(
                u64::from_str_radix(&bits, 16)
                    .map_err(|_| InterfaceError::new("OSR-I0031", "invalid literal float bits"))?,
            )
        }
        ("string", 2) => TypeLiteral::String(expect_string(&values[1], "literal string")?),
        ("keyword", 2) => TypeLiteral::Keyword(expect_string(&values[1], "literal keyword")?),
        ("symbol", 2) => TypeLiteral::Symbol(expect_string(&values[1], "literal symbol")?),
        ("list" | "vector" | "set", 2) => {
            let items = expect_vector(&values[1], "type literal items")?
                .iter()
                .map(decode_type_literal)
                .collect::<InterfaceResult<Vec<_>>>()?;
            match tag {
                "list" => TypeLiteral::List(items),
                "vector" => TypeLiteral::Vector(items),
                _ => TypeLiteral::Set(items),
            }
        }
        ("map", 2) => {
            let entries = expect_vector(&values[1], "type literal map entries")?
                .iter()
                .map(|form| {
                    let pair = expect_vector(form, "type literal map entry")?;
                    if pair.len() != 2 {
                        return Err(InterfaceError::new(
                            "OSR-I0031",
                            "type literal map entry must be a pair",
                        ));
                    }
                    Ok((
                        decode_type_literal(&pair[0])?,
                        decode_type_literal(&pair[1])?,
                    ))
                })
                .collect::<InterfaceResult<Vec<_>>>()?;
            TypeLiteral::Map(entries)
        }
        _ => {
            return Err(InterfaceError::new(
                "OSR-I0031",
                "invalid type literal encoding",
            ));
        }
    };
    literal.canonicalize().map_err(|error| {
        InterfaceError::new(
            "OSR-I0031",
            format!("invalid type literal: {}", error.message()),
        )
    })
}

pub(super) fn decode_types(forms: &[Form]) -> InterfaceResult<Vec<Type>> {
    forms.iter().map(decode_type).collect()
}

pub(super) fn decode_summaries(form: &Form) -> InterfaceResult<CallSummaries> {
    let values = strict_map(form, &["effects", "temporal", "data"])?;
    Ok(CallSummaries {
        effects: decode_effects(get(&values, "effects")?)?,
        temporal: decode_temporal(get(&values, "temporal")?)?,
        data: decode_data(get(&values, "data")?)?,
    })
}

pub(super) fn decode_effects(form: &Form) -> InterfaceResult<EffectRow> {
    let values = strict_map(form, &["open", "items"])?;
    let mut effects = BTreeSet::new();
    for value in expect_vector(get(&values, "items")?, "effects")? {
        let effect = match &value.kind {
            FormKind::Keyword(name) => match name.canonical.trim_start_matches(':') {
                "io" => Effect::Io,
                "throw" => Effect::Throw,
                "mutation" => Effect::Mutation,
                "hidden-state" => Effect::HiddenState,
                "python-dynamic" => Effect::PythonDynamic,
                tag => {
                    return Err(InterfaceError::new(
                        "OSR-I0032",
                        format!("unknown effect `{tag}`"),
                    ));
                }
            },
            FormKind::Vector(parts)
                if parts.len() == 2 && expect_keyword(&parts[0], "effect")? == "custom" =>
            {
                Effect::Custom(expect_string(&parts[1], "custom effect")?)
            }
            _ => return Err(InterfaceError::new("OSR-I0032", "invalid effect")),
        };
        if !effects.insert(effect) {
            return Err(InterfaceError::new("OSR-I0033", "duplicate effect"));
        }
    }
    Ok(EffectRow {
        effects,
        open: expect_bool(get(&values, "open")?, "effect open")?,
    })
}

pub(super) fn decode_temporal(form: &Form) -> InterfaceResult<TemporalSummary> {
    let values = strict_map(form, &["past", "future", "availability"])?;
    Ok(TemporalSummary {
        past: decode_bound(get(&values, "past")?)?,
        future: decode_bound(get(&values, "future")?)?,
        availability: decode_availability(get(&values, "availability")?)?,
    })
}

pub(super) fn decode_bound(form: &Form) -> InterfaceResult<TemporalBound> {
    if let FormKind::Keyword(name) = &form.kind {
        return match name.canonical.trim_start_matches(':') {
            "unknown" => Ok(TemporalBound::Unknown),
            "unbounded" => Ok(TemporalBound::Unbounded),
            _ => Err(InterfaceError::new("OSR-I0034", "invalid temporal bound")),
        };
    }
    let parts = expect_vector(form, "temporal bound")?;
    if parts.len() != 2 {
        return Err(InterfaceError::new("OSR-I0034", "invalid temporal bound"));
    }
    match expect_keyword(&parts[0], "temporal bound")? {
        "finite" => Ok(TemporalBound::Finite(expect_u64(
            &parts[1],
            "finite bound",
        )?)),
        "symbolic" => Ok(TemporalBound::Symbolic(expect_string(
            &parts[1],
            "symbolic bound",
        )?)),
        _ => Err(InterfaceError::new("OSR-I0034", "invalid temporal bound")),
    }
}

pub(super) fn decode_availability(form: &Form) -> InterfaceResult<Availability> {
    if let FormKind::Keyword(name) = &form.kind {
        return match name.canonical.trim_start_matches(':') {
            "immediate" => Ok(Availability::Immediate),
            "unknown" => Ok(Availability::Unknown),
            _ => Err(InterfaceError::new("OSR-I0035", "invalid availability")),
        };
    }
    let parts = expect_vector(form, "availability")?;
    if parts.len() == 2 && expect_keyword(&parts[0], "availability")? == "named" {
        Ok(Availability::Named(expect_string(
            &parts[1],
            "availability",
        )?))
    } else {
        Err(InterfaceError::new("OSR-I0035", "invalid availability"))
    }
}

pub(super) fn decode_data(form: &Form) -> InterfaceResult<DataProperties> {
    let values = strict_map(
        form,
        &[
            "schema",
            "axes",
            "alignment",
            "ordered-by",
            "unique-by",
            "preserves-length",
            "materializes",
            "reshapes",
            "nulls-possible",
            "nan-possible",
            "nonfinite-possible",
            "nonfinite-policy",
        ],
    )?;
    let alignment = match expect_keyword(get(&values, "alignment")?, "alignment")? {
        "positional" => Alignment::Positional,
        "labelled" => Alignment::Labelled,
        "as-of" => Alignment::AsOf,
        "unknown" => Alignment::Unknown,
        _ => return Err(InterfaceError::new("OSR-I0036", "invalid alignment")),
    };
    Ok(DataProperties {
        schema: decode_optional_string(get(&values, "schema")?, "schema")?,
        axes: if is_none(get(&values, "axes")?) {
            None
        } else {
            Some(decode_strings(get(&values, "axes")?, "axes")?)
        },
        alignment,
        ordered_by: if is_none(get(&values, "ordered-by")?) {
            None
        } else {
            Some(decode_strings(get(&values, "ordered-by")?, "ordered-by")?)
        },
        unique_by: if is_none(get(&values, "unique-by")?) {
            None
        } else {
            Some(decode_strings(get(&values, "unique-by")?, "unique-by")?)
        },
        preserves_length: decode_optional_bool(get(&values, "preserves-length")?)?,
        materializes: decode_optional_bool(get(&values, "materializes")?)?,
        reshapes: decode_optional_bool(get(&values, "reshapes")?)?,
        nulls_possible: decode_optional_bool(get(&values, "nulls-possible")?)?,
        nan_possible: decode_optional_bool(get(&values, "nan-possible")?)?,
        nonfinite_possible: decode_optional_bool(get(&values, "nonfinite-possible")?)?,
        nonfinite_policy: decode_optional_string(
            get(&values, "nonfinite-policy")?,
            "nonfinite-policy",
        )?,
    })
}

pub(in crate::interface) fn normalize_model(interface: &mut Interface) -> InterfaceResult<()> {
    // Validate before recursive normalization so direct API callers and
    // forged interfaces cannot bypass metadata limits.
    validate_interface_metadata_resources(interface)?;
    interface.metadata = normalize_metadata(&interface.metadata)?;
    for binding in &mut interface.bindings {
        binding.metadata = normalize_metadata(&binding.metadata)?;
    }
    for function in &mut interface.functions {
        for parameter in &mut function.parameters {
            parameter.metadata = normalize_metadata(&parameter.metadata)?;
        }
    }
    for structure in &mut interface.structs {
        for field in &mut structure.fields {
            field.metadata = normalize_metadata(&field.metadata)?;
        }
    }
    for macro_ in &mut interface.macros {
        macro_.parameters = normalize_form(&macro_.parameters);
        macro_.phase_ir = normalize_form(&macro_.phase_ir);
    }
    for helper in &mut interface.phase_helpers {
        helper.phase_ir = normalize_form(&helper.phase_ir);
    }
    Ok(())
}
