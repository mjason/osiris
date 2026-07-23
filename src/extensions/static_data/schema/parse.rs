/// Parse a `defstatic-schema` declaration and reject unknown clauses.
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
    schema.body_hash = sha256(&schema.canonical_body_bytes());
    Ok(schema)
}

pub(in crate::records) fn parse_schema_fields(
    expression: &Expr,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<SchemaField> {
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

pub(in crate::records) fn parse_schema_indexes(
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
