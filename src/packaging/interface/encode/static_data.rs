use super::super::*;

pub(super) fn static_schema_form(module: &str, schema: &StaticSchema) -> Form {
    map(vec![
        (
            "binding",
            string(BindingId::new(module, &schema.name, BindingKind::Type).as_str()),
        ),
        ("name", string(&schema.name)),
        ("schema-id", string(&schema.schema_id)),
        ("version", integer_u64(schema.version)),
        (
            "fields",
            vector(schema.fields.iter().map(static_schema_field_form).collect()),
        ),
        (
            "indexes",
            vector(
                schema
                    .indexes
                    .iter()
                    .map(static_schema_index_form)
                    .collect(),
            ),
        ),
        ("body-hash", string(&schema.body_hash)),
        ("visibility", keyword("public")),
    ])
}

pub(super) fn static_schema_field_form(field: &records::SchemaField) -> Form {
    map(vec![
        ("name", string(&field.name)),
        ("type", static_type_form(&field.datum_type)),
        ("required", boolean(field.required)),
        (
            "default",
            vector(field.default.iter().map(static_datum_form).collect()),
        ),
    ])
}

pub(super) fn static_schema_index_form(index: &records::SchemaIndex) -> Form {
    map(vec![
        ("id", string(&index.id)),
        ("scope", string(&index.scope)),
        (
            "projections",
            vector(
                index
                    .projections
                    .iter()
                    .map(|projection| {
                        map(vec![
                            (
                                "kind",
                                keyword(match projection.kind {
                                    ProjectionKind::Field => "field",
                                    ProjectionKind::Each => "each",
                                }),
                            ),
                            ("field", string(&projection.field)),
                            ("role", string(&projection.role)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

pub(super) fn static_type_form(datum_type: &StaticType) -> Form {
    match datum_type {
        StaticType::Any => keyword("any"),
        StaticType::None => keyword("none"),
        StaticType::Bool => keyword("bool"),
        StaticType::Int => keyword("int"),
        StaticType::Float => keyword("float"),
        StaticType::Str => keyword("str"),
        StaticType::Keyword => keyword("keyword"),
        StaticType::Symbol => keyword("symbol"),
        StaticType::List(inner) => vector(vec![keyword("list"), static_type_form(inner)]),
        StaticType::Vector(inner) => vector(vec![keyword("vector"), static_type_form(inner)]),
        StaticType::Map(key, value) => vector(vec![
            keyword("map"),
            static_type_form(key),
            static_type_form(value),
        ]),
        StaticType::Set(inner) => vector(vec![keyword("set"), static_type_form(inner)]),
        StaticType::Optional(inner) => vector(vec![keyword("optional"), static_type_form(inner)]),
        StaticType::OneOf(values) => vector(vec![
            keyword("one-of"),
            vector(values.iter().map(static_datum_form).collect()),
        ]),
    }
}

pub(super) fn static_record_form(record: &ValidatedRecord, projection: MetadataProjection) -> Form {
    map(vec![
        ("schema", schema_identity_form(&record.schema)),
        ("owner-binding-id", string(&record.owner_binding_id)),
        ("owner-name", string(&record.owner_name)),
        ("module", string(&record.module)),
        ("visibility", keyword("public")),
        ("stable-record-id", string(&record.stable_record_id)),
        ("record-body-hash", string(&record.record_body_hash)),
        (
            "fields",
            vector(
                record
                    .fields
                    .iter()
                    .map(|(name, value)| vector(vec![string(name), static_datum_form(value)]))
                    .collect(),
            ),
        ),
        (
            "index-claims",
            vector(record.index_claims.iter().map(index_claim_form).collect()),
        ),
        (
            "origin",
            match projection {
                MetadataProjection::Full => record_origin_form(&record.origin),
                MetadataProjection::Semantic => none(),
            },
        ),
    ])
}

pub(super) fn schema_identity_form(identity: &records::SchemaIdentity) -> Form {
    map(vec![
        ("binding-id", string(&identity.binding_id)),
        ("schema-id", string(&identity.schema_id)),
        ("version", integer_u64(identity.version)),
        ("body-hash", string(&identity.body_hash)),
    ])
}

pub(super) fn index_claim_form(claim: &records::IndexClaim) -> Form {
    map(vec![
        ("index-id", string(&claim.index_id)),
        ("projection-field", string(&claim.projection_field)),
        ("projection-role", string(&claim.projection_role)),
        ("key", static_datum_form(&claim.key)),
        ("normalized-key", string(&claim.normalized_key)),
        (
            "raw-spelling",
            optional_string(claim.raw_spelling.as_deref()),
        ),
    ])
}

pub(super) fn record_origin_form(origin: &records::RecordOrigin) -> Form {
    map(vec![
        ("module", string(&origin.module)),
        (
            "span",
            vector(vec![
                integer_usize(origin.span.start),
                integer_usize(origin.span.end),
            ]),
        ),
        (
            "macro-origin",
            optional_string(origin.macro_origin.as_deref()),
        ),
    ])
}

pub(super) fn static_datum_form(value: &StaticDatum) -> Form {
    match value {
        StaticDatum::None => vector(vec![keyword("none")]),
        StaticDatum::Bool(value) => vector(vec![keyword("bool"), boolean(*value)]),
        StaticDatum::Int(value) => vector(vec![keyword("int"), string(value)]),
        StaticDatum::Float(bits) => vector(vec![keyword("float"), string(&format!("{bits:016x}"))]),
        StaticDatum::Str(value) => vector(vec![keyword("string"), string(value)]),
        StaticDatum::Keyword(value) => vector(vec![keyword("keyword"), string(value)]),
        StaticDatum::Symbol {
            spelling,
            binding_id,
        } => vector(vec![
            keyword("symbol"),
            string(spelling),
            optional_string(binding_id.as_deref()),
        ]),
        StaticDatum::List(values) => vector(vec![
            keyword("list"),
            vector(values.iter().map(static_datum_form).collect()),
        ]),
        StaticDatum::Vector(values) => vector(vec![
            keyword("vector"),
            vector(values.iter().map(static_datum_form).collect()),
        ]),
        StaticDatum::Map(entries) => vector(vec![
            keyword("map"),
            vector(
                entries
                    .iter()
                    .map(|(key, value)| {
                        vector(vec![static_datum_form(key), static_datum_form(value)])
                    })
                    .collect(),
            ),
        ]),
        StaticDatum::Set(values) => vector(vec![
            keyword("set"),
            vector(values.iter().map(static_datum_form).collect()),
        ]),
    }
}
