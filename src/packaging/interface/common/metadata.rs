use super::*;
use unicode_normalization::UnicodeNormalization;

pub(in crate::interface) fn validate_declaration_metadata(
    metadata: &[MetadataEntry],
    context: &str,
    canonical: &str,
    require_doc: bool,
) -> InterfaceResult<()> {
    let normalized = normalize_metadata(metadata).map_err(|error| {
        InterfaceError::new(error.code, format!("{context}: {}", error.message))
    })?;
    if require_doc
        && !normalized
            .iter()
            .any(|entry| metadata_key(&entry.key) == Some("doc"))
    {
        return Err(InterfaceError::new(
            "OSR-I0087",
            format!("{context} must provide non-empty `:doc` metadata"),
        ));
    }
    let Some(names) = normalized
        .iter()
        .find(|entry| metadata_key(&entry.key) == Some("osiris/names"))
    else {
        return Ok(());
    };
    let FormKind::Map(entries) = &names.value.kind else {
        unreachable!("localized names were normalized above")
    };
    let canonical = canonical.nfc().collect::<String>();
    for name in localized_name_values(entries) {
        if name == canonical {
            return Err(InterfaceError::new(
                "OSR-I0086",
                format!("{context} repeats canonical name `{canonical}` in `:osiris/names`"),
            ));
        }
    }
    Ok(())
}

pub(super) fn normalize_documentation(form: &Form) -> InterfaceResult<Form> {
    if let FormKind::String(value) = &form.kind {
        require_non_empty_doc(value)?;
        return Ok(normalize_form(form));
    }
    let FormKind::Map(entries) = &form.kind else {
        return Err(InterfaceError::new(
            "OSR-I0085",
            "`:doc` must be a non-empty string or locale map",
        ));
    };
    if entries.len() % 2 != 0 {
        return Err(InterfaceError::new(
            "OSR-I0085",
            "`:doc` map has an odd entry count",
        ));
    }
    let mut pairs = Vec::new();
    let mut locales = BTreeSet::new();
    let mut has_default = false;
    for pair in entries.chunks_exact(2) {
        let key = match &pair[0].kind {
            FormKind::Keyword(name) if name.canonical.trim_start_matches(':') == "default" => {
                if has_default {
                    return Err(InterfaceError::new(
                        "OSR-I0085",
                        "`:doc` repeats the required `:default` entry",
                    ));
                }
                has_default = true;
                keyword("default")
            }
            FormKind::String(locale) => {
                let canonical = canonical_locale(locale, "`:doc`")?;
                if !locales.insert(canonical.clone()) {
                    return Err(InterfaceError::new(
                        "OSR-I0085",
                        format!("`:doc` repeats locale `{canonical}` after BCP 47 normalization"),
                    ));
                }
                string(&canonical)
            }
            _ => {
                return Err(InterfaceError::new(
                    "OSR-I0085",
                    "`:doc` map keys must be `:default` or BCP 47 locale strings",
                ));
            }
        };
        let FormKind::String(text) = &pair[1].kind else {
            return Err(InterfaceError::new(
                "OSR-I0085",
                "`:doc` map values must be non-empty strings",
            ));
        };
        require_non_empty_doc(text)?;
        pairs.push((key, string(text)));
    }
    if !has_default {
        return Err(InterfaceError::new(
            "OSR-I0085",
            "`:doc` locale map requires a `:default` entry",
        ));
    }
    pairs.sort_by_cached_key(|(key, _)| form_text(key));
    Ok(form_node(FormKind::Map(
        pairs
            .into_iter()
            .flat_map(|pair| [pair.0, pair.1])
            .collect(),
    )))
}

pub(super) fn normalize_localized_names(form: &Form) -> InterfaceResult<Form> {
    let FormKind::Map(entries) = &form.kind else {
        return Err(InterfaceError::new(
            "OSR-I0086",
            "`:osiris/names` must be a locale map",
        ));
    };
    if entries.len() % 2 != 0 {
        return Err(InterfaceError::new(
            "OSR-I0086",
            "`:osiris/names` map has an odd entry count",
        ));
    }
    let mut pairs = Vec::new();
    let mut locales = BTreeSet::new();
    let mut names = BTreeSet::new();
    for pair in entries.chunks_exact(2) {
        let FormKind::String(locale) = &pair[0].kind else {
            return Err(InterfaceError::new(
                "OSR-I0086",
                "`:osiris/names` keys must be BCP 47 locale strings",
            ));
        };
        let locale = canonical_locale(locale, "`:osiris/names`")?;
        if !locales.insert(locale.clone()) {
            return Err(InterfaceError::new(
                "OSR-I0086",
                format!("`:osiris/names` repeats locale `{locale}` after BCP 47 normalization"),
            ));
        }
        let value = normalize_localized_name_entry(&pair[1], &mut names)?;
        pairs.push((string(&locale), value));
    }
    pairs.sort_by_cached_key(|(key, _)| form_text(key));
    Ok(form_node(FormKind::Map(
        pairs
            .into_iter()
            .flat_map(|pair| [pair.0, pair.1])
            .collect(),
    )))
}

fn normalize_localized_name_entry(
    form: &Form,
    names: &mut BTreeSet<String>,
) -> InterfaceResult<Form> {
    let FormKind::Map(entries) = &form.kind else {
        return Err(InterfaceError::new(
            "OSR-I0086",
            "each `:osiris/names` locale value must be a map",
        ));
    };
    if entries.len() % 2 != 0 {
        return Err(InterfaceError::new(
            "OSR-I0086",
            "localized name entry has an odd entry count",
        ));
    }
    let mut preferred = None;
    let mut aliases = None;
    for pair in entries.chunks_exact(2) {
        match metadata_key(&pair[0]) {
            Some("preferred") if preferred.is_none() => {
                preferred = Some(normalize_localized_symbol(&pair[1], names, "`:preferred`")?);
            }
            Some("aliases") if aliases.is_none() => {
                let FormKind::Vector(values) = &pair[1].kind else {
                    return Err(InterfaceError::new(
                        "OSR-I0086",
                        "`:aliases` must be a vector of symbols",
                    ));
                };
                aliases = Some(form_node(FormKind::Vector(
                    values
                        .iter()
                        .map(|value| normalize_localized_symbol(value, names, "`:aliases`"))
                        .collect::<InterfaceResult<Vec<_>>>()?,
                )));
            }
            Some("preferred" | "aliases") => {
                return Err(InterfaceError::new(
                    "OSR-I0086",
                    "localized name entry repeats a key",
                ));
            }
            _ => {
                return Err(InterfaceError::new(
                    "OSR-I0086",
                    "localized name entry permits only `:preferred` and `:aliases`",
                ));
            }
        }
    }
    let Some(preferred) = preferred else {
        return Err(InterfaceError::new(
            "OSR-I0086",
            "localized name entry requires one symbol in `:preferred`",
        ));
    };
    let mut values = vec![keyword("preferred"), preferred];
    if let Some(aliases) = aliases {
        values.extend([keyword("aliases"), aliases]);
    }
    Ok(form_node(FormKind::Map(values)))
}

fn normalize_localized_symbol(
    form: &Form,
    names: &mut BTreeSet<String>,
    field: &str,
) -> InterfaceResult<Form> {
    let FormKind::Symbol(name) = &form.kind else {
        return Err(InterfaceError::new(
            "OSR-I0086",
            format!("{field} values must be symbols"),
        ));
    };
    let canonical = name.canonical.nfc().collect::<String>();
    if !names.insert(canonical.clone()) {
        return Err(InterfaceError::new(
            "OSR-I0086",
            format!("localized name `{canonical}` is duplicated after NFC normalization"),
        ));
    }
    Ok(symbol(&canonical))
}

fn canonical_locale(locale: &str, field: &str) -> InterfaceResult<String> {
    oxilangtag::LanguageTag::parse_and_normalize(locale)
        .map(|tag| tag.to_string())
        .map_err(|_| {
            InterfaceError::new(
                if field == "`:doc`" {
                    "OSR-I0085"
                } else {
                    "OSR-I0086"
                },
                format!("{field} locale `{locale}` is not a well-formed BCP 47 tag"),
            )
        })
}

fn require_non_empty_doc(value: &str) -> InterfaceResult<()> {
    if value.trim().is_empty() {
        return Err(InterfaceError::new(
            "OSR-I0085",
            "`:doc` values must not be empty",
        ));
    }
    Ok(())
}

fn metadata_key(form: &Form) -> Option<&str> {
    form_name(form).map(|name| name.trim_start_matches(':'))
}

fn localized_name_values(entries: &[Form]) -> impl Iterator<Item = String> + '_ {
    entries.chunks_exact(2).flat_map(|locale| {
        let FormKind::Map(values) = &locale[1].kind else {
            return Vec::new().into_iter();
        };
        values
            .chunks_exact(2)
            .flat_map(|entry| match metadata_key(&entry[0]) {
                Some("preferred") => vec![&entry[1]],
                Some("aliases") => match &entry[1].kind {
                    FormKind::Vector(items) => items.iter().collect(),
                    _ => Vec::new(),
                },
                _ => Vec::new(),
            })
            .filter_map(|form| match &form.kind {
                FormKind::Symbol(name) => Some(name.canonical.nfc().collect()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .into_iter()
    })
}
