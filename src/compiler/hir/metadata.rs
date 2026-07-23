use super::*;

pub(in crate::hir) fn metadata_flag(metadata: &[MetadataEntry], expected: &str) -> bool {
    metadata.iter().any(|entry| {
        matches!(
            &entry.key.kind,
            FormKind::Keyword(key) | FormKind::Symbol(key)
                if key.canonical.trim_start_matches(':') == expected
        ) && matches!(entry.value.kind, FormKind::Bool(true))
    })
}

pub(in crate::hir) fn dynamic_state_summaries() -> CallSummaries {
    CallSummaries {
        effects: EffectRow::singleton(Effect::HiddenState),
        ..CallSummaries::pure_scalar()
    }
}

pub(in crate::hir) fn causal_requirement(
    metadata: &[MetadataEntry],
) -> Result<Option<CausalRequirement>, String> {
    let mut value = None;
    for entry in metadata {
        let key = match &entry.key.kind {
            FormKind::Keyword(name) | FormKind::Symbol(name) => {
                name.canonical.trim_start_matches(':')
            }
            _ => continue,
        };
        if key != "osiris/causal" {
            continue;
        }
        if value.is_some() {
            return Err("`:osiris/causal` metadata is duplicated".to_owned());
        }
        value = Some(&entry.value);
    }
    let Some(value) = value else {
        return Ok(None);
    };
    match &value.kind {
        FormKind::Bool(false) => Ok(None),
        FormKind::Bool(true) => Ok(Some(CausalRequirement {
            decision_point: None,
        })),
        FormKind::Map(entries) if entries.len() == 2 => {
            let key = form_keyword_or_symbol(&entries[0]).map(|key| key.trim_start_matches(':'));
            if key != Some("decision-point") {
                return Err("`:osiris/causal` map requires exactly `:decision-point`".to_owned());
            }
            let decision_point = match &entries[1].kind {
                FormKind::Keyword(name) => name.canonical.trim_start_matches(':').to_owned(),
                FormKind::Symbol(name) => name.canonical.clone(),
                FormKind::String(value) => value.clone(),
                _ => {
                    return Err("causal `:decision-point` must be a static name".to_owned());
                }
            };
            if decision_point.is_empty() {
                return Err("causal `:decision-point` must not be empty".to_owned());
            }
            Ok(Some(CausalRequirement {
                decision_point: Some(decision_point),
            }))
        }
        _ => Err("`:osiris/causal` must be Bool or `{:decision-point <static-name>}`".to_owned()),
    }
}

pub(in crate::hir) fn parameter_names(name: &Name, metadata: &[MetadataEntry]) -> BTreeSet<String> {
    let mut names = BTreeSet::from([name.canonical.clone()]);
    for entry in metadata {
        let is_names_metadata = match &entry.key.kind {
            FormKind::Keyword(key) | FormKind::Symbol(key) => {
                key.canonical.trim_start_matches(':') == "osiris/names"
            }
            _ => false,
        };
        if is_names_metadata {
            collect_parameter_names(&entry.value, &mut names);
        }
    }
    names
}

pub(in crate::hir) fn find_imported_binding<'a>(
    interface: &'a Interface,
    name: &str,
) -> Option<&'a PublicBinding> {
    if let Some(binding) = interface
        .bindings
        .iter()
        .find(|binding| binding.canonical == name || binding.id == name)
    {
        return Some(binding);
    }
    let alias = interface
        .aliases
        .iter()
        .find(|alias| alias.canonical == name || alias.spelling == name)?;
    interface
        .bindings
        .iter()
        .find(|binding| binding.id == alias.target)
}

pub(in crate::hir) fn alias_target_canonical(
    interface: &Interface,
    alias: &crate::interface::PublicAlias,
) -> String {
    interface
        .bindings
        .iter()
        .find(|binding| binding.id == alias.target)
        .map_or_else(|| alias.target.clone(), |binding| binding.canonical.clone())
}

pub(in crate::hir) fn requested_alias_key(
    requested: &BTreeSet<String>,
    alias: &crate::interface::PublicAlias,
) -> String {
    if requested.contains(&alias.canonical) {
        alias.canonical.clone()
    } else {
        alias.spelling.clone()
    }
}

pub(in crate::hir) fn member_span(_member: &Name, fallback: Span) -> Span {
    // Surface `Name` intentionally carries no independent span; the import
    // declaration span is the closest stable diagnostic location.
    fallback
}

pub(in crate::hir) fn interface_parameter_names(
    parameter: &crate::interface::ParameterInterface,
) -> BTreeSet<String> {
    std::iter::once(parameter.canonical.clone())
        .chain(parameter.aliases.iter().cloned())
        .collect()
}

pub(in crate::hir) fn interface_field_names(
    field: &crate::interface::FieldInterface,
) -> BTreeSet<String> {
    std::iter::once(field.canonical.clone())
        .chain(field.aliases.iter().cloned())
        .collect()
}
