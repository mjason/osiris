/// Verify that a record is precisely the canonical projection of its schema.
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
    let stable_record_id = sha256(
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
    let record_body_hash = sha256(&provisional.body_json().bytes());
    Ok(ValidatedRecord {
        record_body_hash,
        ..provisional
    })
}
