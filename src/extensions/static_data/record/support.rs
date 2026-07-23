pub(in crate::records) fn normalize_index_key(
    key: &StaticDatum,
) -> Result<(String, Option<String>), RecordError> {
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

pub(in crate::records) fn hex_bytes(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

pub(in crate::records) fn index_claim_cmp(left: &IndexClaim, right: &IndexClaim) -> Ordering {
    left.index_id
        .cmp(&right.index_id)
        .then_with(|| left.normalized_key.cmp(&right.normalized_key))
        .then_with(|| left.projection_field.cmp(&right.projection_field))
        .then_with(|| left.projection_role.cmp(&right.projection_role))
}

pub(in crate::records) fn name_span(_name: &Name, fallback: Span) -> Span {
    fallback
}

pub(in crate::records) fn schema_identity_json(identity: &SchemaIdentity) -> Json {
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

pub(in crate::records) fn index_claim_json(claim: &IndexClaim) -> Json {
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
