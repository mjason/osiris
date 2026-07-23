pub(in crate::records) fn sidecar_from_json(root: &Json) -> Result<RecordSidecar, RecordError> {
    let Json::Object(fields) = root else {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            "sidecar root must be an object",
        ));
    };
    let format_version = object_u64(fields, "format-version")?;
    if format_version != RECORD_SIDECAR_FORMAT_VERSION {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            format!("unsupported sidecar format version {format_version}"),
        ));
    }
    let interface_semantic_hashes = object_string_array(fields, "interface-semantic-hashes")?;
    let record_identities = object_array(fields, "record-identities")?;
    let record_set_hash = object_string(fields, "record-set-hash")?;
    let records_json = object_array(fields, "records")?;
    if record_identities.len() != records_json.len() {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            "record-identities and records lengths differ",
        ));
    }
    let mut records = Vec::with_capacity(records_json.len());
    for (identity_json, record_json) in record_identities.iter().zip(records_json) {
        let occurrence = occurrence_from_json(identity_json)?;
        let Json::Object(record_fields) = record_json else {
            return Err(RecordError::new(
                RECORD_SIDECAR,
                "sidecar record must be an object",
            ));
        };
        let occurrence_in_record =
            occurrence_from_json(object_field(record_fields, "occurrence")?)?;
        if occurrence != occurrence_in_record {
            return Err(RecordError::new(
                RECORD_SIDECAR,
                "record identity differs between header and record payload",
            ));
        }
        let record = validated_record_from_json(object_field(record_fields, "record")?)?;
        if occurrence.stable_record_id != record.stable_record_id
            || occurrence.record_body_hash != record.record_body_hash
        {
            return Err(RecordError::new(
                RECORD_SIDECAR,
                "record occurrence does not match record payload identity",
            ));
        }
        records.push(SidecarRecord { occurrence, record });
    }
    let sidecar = RecordSidecar {
        format_version,
        interface_semantic_hashes,
        records,
        record_set_hash,
    };
    if sidecar.records.iter().any(|record| !record.record.public) {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            "sidecar contains a private record",
        ));
    }
    Ok(sidecar)
}

pub(in crate::records) fn object_u64(
    fields: &[(String, Json)],
    key: &str,
) -> Result<u64, RecordError> {
    match object_field(fields, key)? {
        Json::Number(value) => value.parse::<u64>().map_err(|_| {
            RecordError::new(
                RECORD_SIDECAR,
                format!("JSON member `{key}` must be a positive integer"),
            )
        }),
        _ => Err(RecordError::new(
            RECORD_SIDECAR,
            format!("JSON member `{key}` must be an integer"),
        )),
    }
}

pub(in crate::records) fn object_string_array(
    fields: &[(String, Json)],
    key: &str,
) -> Result<Vec<String>, RecordError> {
    object_array(fields, key)?
        .iter()
        .map(|value| match value {
            Json::String(value) => Ok(value.clone()),
            _ => Err(RecordError::new(
                RECORD_SIDECAR,
                format!("JSON member `{key}` must contain strings"),
            )),
        })
        .collect()
}

pub(in crate::records) fn occurrence_from_json(
    value: &Json,
) -> Result<RecordOccurrenceId, RecordError> {
    let Json::Object(fields) = value else {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            "record occurrence must be an object",
        ));
    };
    require_keys(
        fields,
        &[
            "distribution",
            "version",
            "interface-member-id",
            "semantic-interface-hash",
            "stable-record-id",
            "record-body-hash",
        ],
    )?;
    Ok(RecordOccurrenceId {
        distribution: object_string(fields, "distribution")?,
        version: object_string(fields, "version")?,
        interface_member_id: object_string(fields, "interface-member-id")?,
        semantic_interface_hash: object_string(fields, "semantic-interface-hash")?,
        stable_record_id: object_string(fields, "stable-record-id")?,
        record_body_hash: object_string(fields, "record-body-hash")?,
    })
}

pub(in crate::records) fn schema_identity_from_json(
    value: &Json,
) -> Result<SchemaIdentity, RecordError> {
    let Json::Object(fields) = value else {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            "schema identity must be an object",
        ));
    };
    require_keys(fields, &["binding-id", "schema-id", "version", "body-hash"])?;
    Ok(SchemaIdentity {
        binding_id: object_string(fields, "binding-id")?,
        schema_id: object_string(fields, "schema-id")?,
        version: object_u64(fields, "version")?,
        body_hash: object_string(fields, "body-hash")?,
    })
}

pub(in crate::records) fn validated_record_from_json(
    value: &Json,
) -> Result<ValidatedRecord, RecordError> {
    let Json::Object(fields) = value else {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            "record payload must be an object",
        ));
    };
    require_keys(
        fields,
        &[
            "schema",
            "owner-binding-id",
            "owner-name",
            "module",
            "public",
            "stable-record-id",
            "record-body-hash",
            "fields",
            "index-claims",
            "origin",
        ],
    )?;
    let schema = schema_identity_from_json(object_field(fields, "schema")?)?;
    let owner_binding_id = object_string(fields, "owner-binding-id")?;
    let owner_name = object_string(fields, "owner-name")?;
    let module = object_string(fields, "module")?;
    let public = object_bool(fields, "public")?;
    let stable_record_id = object_string(fields, "stable-record-id")?;
    let record_body_hash = object_string(fields, "record-body-hash")?;
    let mut record_fields = Vec::new();
    let mut field_names = BTreeSet::new();
    for field_json in object_array(fields, "fields")? {
        let Json::Array(pair) = field_json else {
            return Err(RecordError::new(
                RECORD_SIDECAR,
                "record field must be a pair",
            ));
        };
        if pair.len() != 2 {
            return Err(RecordError::new(
                RECORD_SIDECAR,
                "record field pair must have two values",
            ));
        }
        let Json::String(name) = &pair[0] else {
            return Err(RecordError::new(
                RECORD_SIDECAR,
                "record field name must be a string",
            ));
        };
        if !field_names.insert(name.clone()) {
            return Err(RecordError::new(RECORD_SIDECAR, "duplicate record field"));
        }
        record_fields.push((name.clone(), StaticDatum::from_json(&pair[1])?));
    }
    let mut index_claims = object_array(fields, "index-claims")?
        .iter()
        .map(index_claim_from_json)
        .collect::<Result<Vec<_>, _>>()?;
    index_claims.sort_by(index_claim_cmp);
    let origin = origin_from_json(object_field(fields, "origin")?)?;
    let record = ValidatedRecord {
        schema,
        owner_binding_id,
        owner_name,
        module,
        public,
        stable_record_id,
        record_body_hash,
        fields: record_fields,
        index_claims,
        origin,
    };
    // The body hash is producer data, but it is cheap to recompute and catches
    // field/index tampering even when a caller omitted marker verification.
    let expected_body_hash = sha256(&record.body_json().bytes());
    if expected_body_hash != record.record_body_hash {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            "record-body-hash does not match record payload",
        ));
    }
    Ok(record)
}

pub(in crate::records) fn index_claim_from_json(value: &Json) -> Result<IndexClaim, RecordError> {
    let Json::Object(fields) = value else {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            "index claim must be an object",
        ));
    };
    let allowed = [
        "index-id",
        "projection-field",
        "projection-role",
        "key",
        "normalized-key",
        "raw-spelling",
    ];
    for (name, _) in fields {
        if !allowed.contains(&name.as_str()) {
            return Err(RecordError::new(
                RECORD_SIDECAR,
                format!("unknown index claim member `{name}`"),
            ));
        }
    }
    Ok(IndexClaim {
        index_id: object_string(fields, "index-id")?,
        projection_field: object_string(fields, "projection-field")?,
        projection_role: object_string(fields, "projection-role")?,
        key: StaticDatum::from_json(object_field(fields, "key")?)?,
        normalized_key: object_string(fields, "normalized-key")?,
        raw_spelling: object_optional_string(fields, "raw-spelling")?,
    })
}

pub(in crate::records) fn origin_from_json(value: &Json) -> Result<RecordOrigin, RecordError> {
    let Json::Object(fields) = value else {
        return Err(RecordError::new(
            RECORD_SIDECAR,
            "record origin must be an object",
        ));
    };
    let module = object_string(fields, "module")?;
    let span = match object_field(fields, "span")? {
        Json::Array(values) if values.len() == 2 => {
            Span::new(json_usize(&values[0])?, json_usize(&values[1])?)
        }
        _ => {
            return Err(RecordError::new(
                RECORD_SIDECAR,
                "record origin span must be a pair",
            ));
        }
    };
    Ok(RecordOrigin {
        module,
        span,
        macro_origin: object_optional_string(fields, "macro-origin")?,
    })
}

pub(in crate::records) fn object_bool(
    fields: &[(String, Json)],
    key: &str,
) -> Result<bool, RecordError> {
    match object_field(fields, key)? {
        Json::Bool(value) => Ok(*value),
        _ => Err(RecordError::new(
            RECORD_SIDECAR,
            format!("JSON member `{key}` must be boolean"),
        )),
    }
}

pub(in crate::records) fn json_usize(value: &Json) -> Result<usize, RecordError> {
    match value {
        Json::Number(value) => value.parse::<usize>().map_err(|_| {
            RecordError::new(RECORD_SIDECAR, "span offset must be a non-negative integer")
        }),
        _ => Err(RecordError::new(
            RECORD_SIDECAR,
            "span offset must be an integer",
        )),
    }
}

pub(in crate::records) fn require_keys(
    fields: &[(String, Json)],
    required: &[&str],
) -> Result<(), RecordError> {
    for key in required {
        if !fields.iter().any(|(name, _)| name == key) {
            return Err(RecordError::new(
                RECORD_SIDECAR,
                format!("missing JSON member `{key}`"),
            ));
        }
    }
    for (name, _) in fields {
        if !required.contains(&name.as_str()) {
            return Err(RecordError::new(
                RECORD_SIDECAR,
                format!("unknown JSON member `{name}`"),
            ));
        }
    }
    Ok(())
}
