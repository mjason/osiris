use super::super::*;

pub(super) fn strict_map(
    form: &Form,
    expected: &[&str],
) -> InterfaceResult<BTreeMap<String, Form>> {
    let FormKind::Map(entries) = &form.kind else {
        return Err(InterfaceError::new("OSR-I0041", "expected interface map"));
    };
    if entries.len() % 2 != 0 {
        return Err(InterfaceError::new("OSR-I0041", "unmatched map key"));
    }
    let allowed = expected.iter().copied().collect::<BTreeSet<_>>();
    let mut values = BTreeMap::new();
    for pair in entries.chunks_exact(2) {
        let key = expect_keyword(&pair[0], "map key")?.to_owned();
        if !allowed.contains(key.as_str()) {
            return Err(InterfaceError::new(
                "OSR-I0042",
                format!("unknown interface field `:{key}`"),
            ));
        }
        if values.insert(key.clone(), pair[1].clone()).is_some() {
            return Err(InterfaceError::new(
                "OSR-I0043",
                format!("duplicate interface field `:{key}`"),
            ));
        }
    }
    for key in expected {
        if !values.contains_key(*key) {
            return Err(InterfaceError::new(
                "OSR-I0044",
                format!("missing interface field `:{key}`"),
            ));
        }
    }
    Ok(values)
}

pub(in crate::interface) fn reject_duplicate_maps(form: &Form) -> InterfaceResult<()> {
    match &form.kind {
        FormKind::Map(entries) => {
            if entries.len() % 2 != 0 {
                return Err(InterfaceError::new("OSR-I0043", "unmatched map key"));
            }
            let mut keys = BTreeSet::new();
            for pair in entries.chunks_exact(2) {
                let key = form_text(&normalize_form(&pair[0]));
                if !keys.insert(key.clone()) {
                    return Err(InterfaceError::new(
                        "OSR-I0043",
                        format!("duplicate map key `{key}`"),
                    ));
                }
                reject_duplicate_maps(&pair[0])?;
                reject_duplicate_maps(&pair[1])?;
            }
        }
        FormKind::List(values) | FormKind::Vector(values) | FormKind::Set(values) => {
            for value in values {
                reject_duplicate_maps(value)?;
            }
        }
        FormKind::ReaderMacro { form, .. } => reject_duplicate_maps(form)?,
        _ => {}
    }
    Ok(())
}

pub(in crate::interface) fn unwrap<'a>(
    form: &'a Form,
    expected: &str,
) -> InterfaceResult<&'a Form> {
    let FormKind::List(values) = &form.kind else {
        return Err(InterfaceError::new(
            "OSR-I0045",
            "expected interface section",
        ));
    };
    if values.len() != 2 || form_name(&values[0]) != Some(expected) {
        return Err(InterfaceError::new(
            "OSR-I0045",
            format!("expected `({expected} ...)`"),
        ));
    }
    Ok(&values[1])
}

pub(super) fn get<'a>(map: &'a BTreeMap<String, Form>, key: &str) -> InterfaceResult<&'a Form> {
    map.get(key)
        .ok_or_else(|| InterfaceError::new("OSR-I0044", format!("missing `:{key}`")))
}

pub(super) fn decode_vector<T>(
    form: &Form,
    decode: impl Fn(&Form) -> InterfaceResult<T>,
) -> InterfaceResult<Vec<T>> {
    expect_vector(form, "section")?.iter().map(decode).collect()
}

pub(super) fn decode_strings(form: &Form, context: &str) -> InterfaceResult<Vec<String>> {
    expect_vector(form, context)?
        .iter()
        .map(|value| expect_string(value, context))
        .collect()
}

pub(super) fn decode_metadata(form: &Form) -> InterfaceResult<Vec<MetadataEntry>> {
    let FormKind::Map(entries) = &form.kind else {
        return Err(InterfaceError::new("OSR-I0046", "metadata must be a map"));
    };
    let entry_count = entries.len() / 2;
    if entry_count > METADATA_TARGET_LIMITS.max_entries {
        return Err(metadata_resource_error(
            "metadata target",
            "syntax target",
            MetadataLimitExceeded {
                resource: "entry count",
                actual: entry_count,
                limit: METADATA_TARGET_LIMITS.max_entries,
            },
        ));
    }
    normalize_metadata(
        &entries
            .chunks_exact(2)
            .map(|pair| MetadataEntry {
                key: pair[0].clone(),
                value: pair[1].clone(),
            })
            .collect::<Vec<_>>(),
    )
}

pub(super) fn expect_vector<'a>(form: &'a Form, context: &str) -> InterfaceResult<&'a [Form]> {
    match &form.kind {
        FormKind::Vector(values) => Ok(values),
        _ => Err(InterfaceError::new(
            "OSR-I0047",
            format!("{context} must be a vector"),
        )),
    }
}

pub(super) fn expect_string(form: &Form, context: &str) -> InterfaceResult<String> {
    match &form.kind {
        FormKind::String(value) => Ok(value.clone()),
        _ => Err(InterfaceError::new(
            "OSR-I0048",
            format!("{context} must be a string"),
        )),
    }
}

pub(super) fn expect_keyword<'a>(form: &'a Form, context: &str) -> InterfaceResult<&'a str> {
    match &form.kind {
        FormKind::Keyword(name) => Ok(name.canonical.trim_start_matches(':')),
        _ => Err(InterfaceError::new(
            "OSR-I0049",
            format!("{context} must be a keyword"),
        )),
    }
}

pub(super) fn expect_bool(form: &Form, context: &str) -> InterfaceResult<bool> {
    match form.kind {
        FormKind::Bool(value) => Ok(value),
        _ => Err(InterfaceError::new(
            "OSR-I0050",
            format!("{context} must be a boolean"),
        )),
    }
}

pub(super) fn expect_u32(form: &Form, context: &str) -> InterfaceResult<u32> {
    expect_integer(form, context)?
        .parse()
        .map_err(|_| InterfaceError::new("OSR-I0051", format!("{context} must fit u32")))
}

pub(super) fn expect_u64(form: &Form, context: &str) -> InterfaceResult<u64> {
    expect_integer(form, context)?
        .parse()
        .map_err(|_| InterfaceError::new("OSR-I0051", format!("{context} must fit u64")))
}

pub(super) fn expect_usize(form: &Form, context: &str) -> InterfaceResult<usize> {
    expect_integer(form, context)?
        .parse()
        .map_err(|_| InterfaceError::new("OSR-I0051", format!("{context} must fit usize")))
}

pub(super) fn expect_integer<'a>(form: &'a Form, context: &str) -> InterfaceResult<&'a str> {
    match &form.kind {
        FormKind::Integer(value) => Ok(value),
        _ => Err(InterfaceError::new(
            "OSR-I0051",
            format!("{context} must be an integer"),
        )),
    }
}

pub(super) fn expect_hash(form: &Form) -> InterfaceResult<String> {
    let value = expect_string(form, "hash")?;
    let Some(digest) = value.strip_prefix("sha256:") else {
        return Err(InterfaceError::new("OSR-I0052", "hash must use SHA-256"));
    };
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(InterfaceError::new("OSR-I0052", "invalid SHA-256 hash"));
    }
    Ok(value)
}

pub(super) fn decode_optional_string(
    form: &Form,
    context: &str,
) -> InterfaceResult<Option<String>> {
    if is_none(form) {
        Ok(None)
    } else {
        expect_string(form, context).map(Some)
    }
}

pub(super) fn decode_optional_bool(form: &Form) -> InterfaceResult<Option<bool>> {
    if is_none(form) {
        Ok(None)
    } else {
        expect_bool(form, "optional bool").map(Some)
    }
}

pub(super) fn require_public(form: &Form) -> InterfaceResult<()> {
    if expect_keyword(form, "visibility")? == "public" {
        Ok(())
    } else {
        Err(InterfaceError::new(
            "OSR-I0053",
            "private declaration leaked into interface",
        ))
    }
}

pub(super) fn require_private(form: &Form) -> InterfaceResult<()> {
    if expect_keyword(form, "visibility")? == "private" {
        Ok(())
    } else {
        Err(InterfaceError::new(
            "OSR-I0060",
            "phase-1 helper closure member must be private",
        ))
    }
}

pub(super) fn decode_binding_kind(form: &Form) -> InterfaceResult<BindingKind> {
    match expect_keyword(form, "binding kind")? {
        "module" => Ok(BindingKind::Module),
        "value" => Ok(BindingKind::Value),
        "function" => Ok(BindingKind::Function),
        "type" => Ok(BindingKind::Type),
        "field" => Ok(BindingKind::Field),
        "parameter" => Ok(BindingKind::Parameter),
        "macro" => Ok(BindingKind::Macro),
        "python-module" => Ok(BindingKind::PythonModule),
        _ => Err(InterfaceError::new("OSR-I0054", "unknown binding kind")),
    }
}
