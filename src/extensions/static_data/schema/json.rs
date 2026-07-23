pub(in crate::records) fn schema_field_json(field: &SchemaField) -> Json {
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

pub(in crate::records) fn static_type_json(datum_type: &StaticType) -> Json {
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

pub(in crate::records) fn schema_index_json(index: &SchemaIndex) -> Json {
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

pub(in crate::records) fn expr_keyword(expression: &Expr) -> Option<&str> {
    match &expression.kind {
        ExprKind::Keyword(name) | ExprKind::Name(name) => Some(name.canonical.as_str()),
        _ => None,
    }
}

pub(in crate::records) fn expr_string(expression: &Expr) -> Option<String> {
    match &expression.kind {
        ExprKind::String(value) => Some(value.clone()),
        _ => None,
    }
}

pub(in crate::records) fn expr_integer(expression: &Expr) -> Option<&str> {
    match &expression.kind {
        ExprKind::Integer(value) => Some(value.as_str()),
        _ => None,
    }
}

pub(in crate::records) fn expr_map(expression: &Expr) -> Option<Vec<(&Expr, &Expr)>> {
    match &expression.kind {
        ExprKind::Map(entries) => Some(entries.iter().map(|(key, value)| (key, value)).collect()),
        _ => None,
    }
}

pub(in crate::records) fn expr_bool(expression: &Expr) -> Option<bool> {
    match expression.kind {
        ExprKind::Bool(value) => Some(value),
        _ => None,
    }
}

pub(in crate::records) fn option_bool(expression: &Expr) -> Option<bool> {
    expr_bool(expression)
}

pub(in crate::records) fn diagnostic_from_error(error: RecordError, span: Span) -> Diagnostic {
    Diagnostic::error(error.code, error.message, error.span.unwrap_or(span))
}

pub(in crate::records) fn is_namespaced(value: &str) -> bool {
    value.contains('/') && !value.starts_with('/') && !value.ends_with('/')
}
