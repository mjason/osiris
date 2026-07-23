use super::super::*;

pub(super) fn type_form(ty: &Type) -> Form {
    match ty {
        Type::Bool => keyword("bool"),
        Type::Int => keyword("int"),
        Type::Float => keyword("float"),
        Type::Str => keyword("str"),
        Type::Bytes => keyword("bytes"),
        Type::None => keyword("none"),
        Type::Any => keyword("any"),
        Type::Never => keyword("never"),
        Type::Unknown => keyword("unknown"),
        Type::Error => keyword("error"),
        Type::Option(value) => vector(vec![keyword("option"), type_form(value)]),
        Type::Union(values) => type_sequence("union", values),
        Type::Tuple(values) => type_sequence("tuple", values),
        Type::List(value) => vector(vec![keyword("list"), type_form(value)]),
        Type::Vector(value) => vector(vec![keyword("vector"), type_form(value)]),
        Type::Map(key, value) => vector(vec![keyword("map"), type_form(key), type_form(value)]),
        Type::Set(value) => vector(vec![keyword("set"), type_form(value)]),
        Type::Fn(function) => vector(vec![
            keyword("fn"),
            vector(function.parameters.iter().map(type_form).collect()),
            type_form(&function.return_type),
            summaries_form(&function.summaries),
        ]),
        Type::Nominal { binding, args } => {
            let mut values = vec![keyword("nominal"), string(binding)];
            values.extend(args.iter().map(type_form));
            vector(values)
        }
        Type::Literal(value) => vector(vec![keyword("literal"), type_literal_form(value)]),
        Type::TypeVar(variable) => vector(vec![keyword("type-var"), integer(variable.0)]),
    }
}

pub(super) fn type_literal_form(value: &TypeLiteral) -> Form {
    match value {
        TypeLiteral::None => vector(vec![keyword("none")]),
        TypeLiteral::Bool(value) => vector(vec![keyword("bool"), boolean(*value)]),
        TypeLiteral::Integer(value) => vector(vec![keyword("integer"), string(value)]),
        TypeLiteral::Float(bits) => vector(vec![keyword("float"), string(&format!("{bits:016x}"))]),
        TypeLiteral::String(value) => vector(vec![keyword("string"), string(value)]),
        TypeLiteral::Keyword(value) => vector(vec![keyword("keyword"), string(value)]),
        TypeLiteral::Symbol(value) => vector(vec![keyword("symbol"), string(value)]),
        TypeLiteral::List(values) => vector(vec![
            keyword("list"),
            vector(values.iter().map(type_literal_form).collect()),
        ]),
        TypeLiteral::Vector(values) => vector(vec![
            keyword("vector"),
            vector(values.iter().map(type_literal_form).collect()),
        ]),
        TypeLiteral::Map(entries) => vector(vec![
            keyword("map"),
            vector(
                entries
                    .iter()
                    .map(|(key, value)| {
                        vector(vec![type_literal_form(key), type_literal_form(value)])
                    })
                    .collect(),
            ),
        ]),
        TypeLiteral::Set(values) => vector(vec![
            keyword("set"),
            vector(values.iter().map(type_literal_form).collect()),
        ]),
    }
}

pub(super) fn type_sequence(tag: &str, types: &[Type]) -> Form {
    let mut values = vec![keyword(tag)];
    values.extend(types.iter().map(type_form));
    vector(values)
}

pub(super) fn summaries_form(summaries: &CallSummaries) -> Form {
    map(vec![
        ("effects", effects_form(&summaries.effects)),
        ("temporal", temporal_form(&summaries.temporal)),
        ("data", data_form(&summaries.data)),
    ])
}

pub(super) fn effects_form(row: &EffectRow) -> Form {
    map(vec![
        ("open", boolean(row.open)),
        (
            "items",
            vector(
                row.effects
                    .iter()
                    .map(|effect| match effect {
                        Effect::Io => keyword("io"),
                        Effect::Throw => keyword("throw"),
                        Effect::Mutation => keyword("mutation"),
                        Effect::HiddenState => keyword("hidden-state"),
                        Effect::PythonDynamic => keyword("python-dynamic"),
                        Effect::Custom(name) => vector(vec![keyword("custom"), string(name)]),
                    })
                    .collect(),
            ),
        ),
    ])
}

pub(super) fn temporal_form(summary: &TemporalSummary) -> Form {
    map(vec![
        ("past", bound_form(&summary.past)),
        ("future", bound_form(&summary.future)),
        (
            "availability",
            match &summary.availability {
                Availability::Immediate => keyword("immediate"),
                Availability::Named(name) => vector(vec![keyword("named"), string(name)]),
                Availability::Unknown => keyword("unknown"),
            },
        ),
    ])
}

pub(super) fn bound_form(bound: &TemporalBound) -> Form {
    match bound {
        TemporalBound::Finite(value) => vector(vec![keyword("finite"), integer_u64(*value)]),
        TemporalBound::Symbolic(value) => vector(vec![keyword("symbolic"), string(value)]),
        TemporalBound::Unbounded => keyword("unbounded"),
        TemporalBound::Unknown => keyword("unknown"),
    }
}

pub(super) fn data_form(data: &DataProperties) -> Form {
    map(vec![
        ("schema", optional_string(data.schema.as_deref())),
        (
            "axes",
            data.axes
                .as_ref()
                .map_or_else(none, |axes| strings_form(axes)),
        ),
        (
            "alignment",
            keyword(match data.alignment {
                Alignment::Positional => "positional",
                Alignment::Labelled => "labelled",
                Alignment::AsOf => "as-of",
                Alignment::Unknown => "unknown",
            }),
        ),
        (
            "ordered-by",
            data.ordered_by
                .as_ref()
                .map_or_else(none, |keys| strings_form(keys)),
        ),
        (
            "unique-by",
            data.unique_by
                .as_ref()
                .map_or_else(none, |keys| strings_form(keys)),
        ),
        ("preserves-length", optional_bool(data.preserves_length)),
        ("materializes", optional_bool(data.materializes)),
        ("reshapes", optional_bool(data.reshapes)),
        ("nulls-possible", optional_bool(data.nulls_possible)),
        ("nan-possible", optional_bool(data.nan_possible)),
        ("nonfinite-possible", optional_bool(data.nonfinite_possible)),
        (
            "nonfinite-policy",
            optional_string(data.nonfinite_policy.as_deref()),
        ),
    ])
}
