use super::super::*;
use super::support::*;

pub(super) fn decode_static_schema(form: &Form, module: &str) -> InterfaceResult<StaticSchema> {
    let values = strict_map(
        form,
        &[
            "binding",
            "name",
            "schema-id",
            "version",
            "fields",
            "indexes",
            "body-hash",
            "visibility",
        ],
    )?;
    require_public(get(&values, "visibility")?)?;
    let name = expect_string(get(&values, "name")?, "static schema name")?;
    let binding = expect_string(get(&values, "binding")?, "static schema binding")?;
    let expected_binding = BindingId::new(module, &name, BindingKind::Type);
    if binding != expected_binding.as_str() {
        return Err(InterfaceError::new(
            "OSR-I0056",
            format!("static schema `{name}` has an invalid binding id"),
        ));
    }
    Ok(StaticSchema {
        name,
        schema_id: expect_string(get(&values, "schema-id")?, "schema id")?,
        version: expect_u64(get(&values, "version")?, "schema version")?,
        fields: decode_vector(get(&values, "fields")?, decode_static_schema_field)?,
        indexes: decode_vector(get(&values, "indexes")?, decode_static_schema_index)?,
        body_hash: expect_hash(get(&values, "body-hash")?)?,
    })
}

pub(super) fn decode_static_schema_field(form: &Form) -> InterfaceResult<records::SchemaField> {
    let values = strict_map(form, &["name", "type", "required", "default"])?;
    let defaults = expect_vector(get(&values, "default")?, "static field default")?;
    if defaults.len() > 1 {
        return Err(InterfaceError::new(
            "OSR-I0056",
            "static field default must contain zero or one datum",
        ));
    }
    Ok(records::SchemaField {
        name: expect_string(get(&values, "name")?, "static field name")?,
        datum_type: decode_static_type(get(&values, "type")?)?,
        required: expect_bool(get(&values, "required")?, "static field required")?,
        default: defaults.first().map(decode_static_datum).transpose()?,
    })
}

pub(super) fn decode_static_schema_index(form: &Form) -> InterfaceResult<records::SchemaIndex> {
    let values = strict_map(form, &["id", "scope", "projections"])?;
    Ok(records::SchemaIndex {
        id: expect_string(get(&values, "id")?, "static index id")?,
        scope: expect_string(get(&values, "scope")?, "static index scope")?,
        projections: decode_vector(get(&values, "projections")?, |form| {
            let projection = strict_map(form, &["kind", "field", "role"])?;
            let kind = match expect_keyword(get(&projection, "kind")?, "projection kind")? {
                "field" => ProjectionKind::Field,
                "each" => ProjectionKind::Each,
                _ => {
                    return Err(InterfaceError::new(
                        "OSR-I0056",
                        "unknown static index projection kind",
                    ));
                }
            };
            Ok(records::IndexProjection {
                kind,
                field: expect_string(get(&projection, "field")?, "projection field")?,
                role: expect_string(get(&projection, "role")?, "projection role")?,
            })
        })?,
    })
}

pub(super) fn decode_static_type(form: &Form) -> InterfaceResult<StaticType> {
    if let FormKind::Keyword(_) = &form.kind {
        return match expect_keyword(form, "static type")? {
            "any" => Ok(StaticType::Any),
            "none" => Ok(StaticType::None),
            "bool" => Ok(StaticType::Bool),
            "int" => Ok(StaticType::Int),
            "float" => Ok(StaticType::Float),
            "str" => Ok(StaticType::Str),
            "keyword" => Ok(StaticType::Keyword),
            "symbol" => Ok(StaticType::Symbol),
            _ => Err(InterfaceError::new("OSR-I0056", "unknown static type")),
        };
    }
    let values = expect_vector(form, "static type")?;
    let Some(tag) = values.first() else {
        return Err(InterfaceError::new("OSR-I0056", "empty static type"));
    };
    match expect_keyword(tag, "static type tag")? {
        "list" | "vector" | "set" | "optional" if values.len() == 2 => {
            let inner = Box::new(decode_static_type(&values[1])?);
            Ok(match expect_keyword(tag, "static type tag")? {
                "list" => StaticType::List(inner),
                "vector" => StaticType::Vector(inner),
                "set" => StaticType::Set(inner),
                _ => StaticType::Optional(inner),
            })
        }
        "map" if values.len() == 3 => Ok(StaticType::Map(
            Box::new(decode_static_type(&values[1])?),
            Box::new(decode_static_type(&values[2])?),
        )),
        "one-of" if values.len() == 2 => Ok(StaticType::OneOf(
            expect_vector(&values[1], "OneOf values")?
                .iter()
                .map(decode_static_datum)
                .collect::<InterfaceResult<_>>()?,
        )),
        _ => Err(InterfaceError::new(
            "OSR-I0056",
            "invalid static type constructor",
        )),
    }
}

pub(super) fn decode_static_record(form: &Form) -> InterfaceResult<ValidatedRecord> {
    let values = strict_map(
        form,
        &[
            "schema",
            "owner-binding-id",
            "owner-name",
            "module",
            "visibility",
            "stable-record-id",
            "record-body-hash",
            "fields",
            "index-claims",
            "origin",
        ],
    )?;
    require_public(get(&values, "visibility")?)?;
    let fields = decode_vector(get(&values, "fields")?, |form| {
        let pair = expect_vector(form, "static record field")?;
        if pair.len() != 2 {
            return Err(InterfaceError::new(
                "OSR-I0057",
                "static record field must be a pair",
            ));
        }
        Ok((
            expect_string(&pair[0], "static record field name")?,
            decode_static_datum(&pair[1])?,
        ))
    })?;
    Ok(ValidatedRecord {
        schema: decode_schema_identity(get(&values, "schema")?)?,
        owner_binding_id: expect_string(
            get(&values, "owner-binding-id")?,
            "record owner binding id",
        )?,
        owner_name: expect_string(get(&values, "owner-name")?, "record owner name")?,
        module: expect_string(get(&values, "module")?, "record module")?,
        public: true,
        stable_record_id: expect_hash(get(&values, "stable-record-id")?)?,
        record_body_hash: expect_hash(get(&values, "record-body-hash")?)?,
        fields,
        index_claims: decode_vector(get(&values, "index-claims")?, decode_index_claim)?,
        origin: decode_record_origin(get(&values, "origin")?)?,
    })
}

pub(super) fn decode_schema_identity(form: &Form) -> InterfaceResult<records::SchemaIdentity> {
    let values = strict_map(form, &["binding-id", "schema-id", "version", "body-hash"])?;
    Ok(records::SchemaIdentity {
        binding_id: expect_string(get(&values, "binding-id")?, "schema binding id")?,
        schema_id: expect_string(get(&values, "schema-id")?, "schema id")?,
        version: expect_u64(get(&values, "version")?, "schema version")?,
        body_hash: expect_hash(get(&values, "body-hash")?)?,
    })
}

pub(super) fn decode_index_claim(form: &Form) -> InterfaceResult<records::IndexClaim> {
    let values = strict_map(
        form,
        &[
            "index-id",
            "projection-field",
            "projection-role",
            "key",
            "normalized-key",
            "raw-spelling",
        ],
    )?;
    Ok(records::IndexClaim {
        index_id: expect_string(get(&values, "index-id")?, "index claim id")?,
        projection_field: expect_string(
            get(&values, "projection-field")?,
            "index projection field",
        )?,
        projection_role: expect_string(get(&values, "projection-role")?, "index projection role")?,
        key: decode_static_datum(get(&values, "key")?)?,
        normalized_key: expect_string(get(&values, "normalized-key")?, "normalized index key")?,
        raw_spelling: decode_optional_string(get(&values, "raw-spelling")?, "raw spelling")?,
    })
}

pub(super) fn decode_record_origin(form: &Form) -> InterfaceResult<records::RecordOrigin> {
    let values = strict_map(form, &["module", "span", "macro-origin"])?;
    let span = expect_vector(get(&values, "span")?, "record origin span")?;
    if span.len() != 2 {
        return Err(InterfaceError::new(
            "OSR-I0057",
            "record origin span must have two offsets",
        ));
    }
    let start = expect_usize(&span[0], "record origin start")?;
    let end = expect_usize(&span[1], "record origin end")?;
    if start > end {
        return Err(InterfaceError::new(
            "OSR-I0057",
            "record origin span is reversed",
        ));
    }
    Ok(records::RecordOrigin {
        module: expect_string(get(&values, "module")?, "record origin module")?,
        span: Span::new(start, end),
        macro_origin: decode_optional_string(get(&values, "macro-origin")?, "record macro origin")?,
    })
}

pub(super) fn decode_static_datum(form: &Form) -> InterfaceResult<StaticDatum> {
    let values = expect_vector(form, "static datum")?;
    let Some(tag) = values.first() else {
        return Err(InterfaceError::new("OSR-I0058", "empty static datum"));
    };
    let tag = expect_keyword(tag, "static datum tag")?;
    let datum = match (tag, values.len()) {
        ("none", 1) => StaticDatum::None,
        ("bool", 2) => StaticDatum::Bool(expect_bool(&values[1], "static bool")?),
        ("int", 2) => StaticDatum::Int(expect_string(&values[1], "static integer")?),
        ("float", 2) => {
            let bits = expect_string(&values[1], "static float bits")?;
            if bits.len() != 16
                || !bits
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            {
                return Err(InterfaceError::new(
                    "OSR-I0058",
                    "static float requires 16 lowercase hexadecimal bits",
                ));
            }
            StaticDatum::Float(
                u64::from_str_radix(&bits, 16)
                    .map_err(|_| InterfaceError::new("OSR-I0058", "invalid static float bits"))?,
            )
        }
        ("string", 2) => StaticDatum::Str(expect_string(&values[1], "static string")?),
        ("keyword", 2) => StaticDatum::Keyword(expect_string(&values[1], "static keyword")?),
        ("symbol", 3) => StaticDatum::Symbol {
            spelling: expect_string(&values[1], "static symbol")?,
            binding_id: decode_optional_string(&values[2], "static symbol binding")?,
        },
        ("list" | "vector" | "set", 2) => {
            let items = expect_vector(&values[1], "static datum items")?
                .iter()
                .map(decode_static_datum)
                .collect::<InterfaceResult<Vec<_>>>()?;
            match tag {
                "list" => StaticDatum::List(items),
                "vector" => StaticDatum::Vector(items),
                _ => StaticDatum::Set(items),
            }
        }
        ("map", 2) => {
            let entries = expect_vector(&values[1], "static map entries")?
                .iter()
                .map(|form| {
                    let pair = expect_vector(form, "static map entry")?;
                    if pair.len() != 2 {
                        return Err(InterfaceError::new(
                            "OSR-I0058",
                            "static map entry must be a pair",
                        ));
                    }
                    Ok((
                        decode_static_datum(&pair[0])?,
                        decode_static_datum(&pair[1])?,
                    ))
                })
                .collect::<InterfaceResult<Vec<_>>>()?;
            StaticDatum::Map(entries)
        }
        _ => {
            return Err(InterfaceError::new(
                "OSR-I0058",
                "invalid static datum encoding",
            ));
        }
    };
    datum.canonicalize().map_err(|error| {
        InterfaceError::new(
            "OSR-I0058",
            format!("invalid static datum: {}", error.message),
        )
    })
}
