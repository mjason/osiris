use super::*;

pub(super) fn contains(span: Span, offset: usize) -> bool {
    (span.start..=span.end).contains(&offset)
}

pub(super) fn aliases_by_target(module: &hir::Module) -> BTreeMap<String, Vec<SemanticAlias>> {
    let canonical_by_id = module
        .bindings
        .iter()
        .map(|binding| {
            (
                binding.name.id.as_str().to_owned(),
                binding.name.canonical.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut aliases = BTreeMap::<String, Vec<SemanticAlias>>::new();
    for alias in &module.aliases {
        let Some(target_canonical) = canonical_by_id.get(alias.target.as_str()) else {
            continue;
        };
        aliases
            .entry(alias.target.as_str().to_owned())
            .or_default()
            .push(SemanticAlias {
                spelling: alias.spelling.clone(),
                canonical: alias.canonical.clone(),
                public: alias.public,
                preferred: false,
                span: alias.span,
                labels: labels_for_name(target_canonical, &BTreeMap::new()),
            });
    }
    for values in aliases.values_mut() {
        values.sort_by(|left, right| {
            (!left.public, !left.preferred, &left.spelling).cmp(&(
                !right.public,
                !right.preferred,
                &right.spelling,
            ))
        });
        if let Some(first) = values.first_mut() {
            first.preferred = true;
        }
    }
    aliases
}

pub(super) fn labels_for_name(
    canonical: &str,
    localized: &BTreeMap<String, LocalizedNameEntry>,
) -> LocalizedLabel {
    LocalizedLabel::new(
        canonical.to_owned(),
        localized
            .iter()
            .map(|(locale, entry)| (locale.clone(), entry.preferred.clone()))
            .collect(),
    )
}

pub fn localized_names(metadata: &[MetadataEntry]) -> BTreeMap<String, LocalizedNameEntry> {
    let Some(names) = metadata.iter().find_map(|entry| {
        (form_name(&entry.key)
            .as_deref()
            .map(|value| value.trim_start_matches(':'))
            == Some("osiris/names"))
        .then_some(&entry.value)
    }) else {
        return BTreeMap::new();
    };
    let FormKind::Map(entries) = &names.kind else {
        return BTreeMap::new();
    };
    let mut result = BTreeMap::new();
    for pair in entries.chunks_exact(2) {
        let FormKind::String(raw_locale) = &pair[0].kind else {
            continue;
        };
        let Ok(locale) = oxilangtag::LanguageTag::parse_and_normalize(raw_locale) else {
            continue;
        };
        let FormKind::Map(values) = &pair[1].kind else {
            continue;
        };
        let mut preferred = None;
        let mut aliases = Vec::new();
        for entry in values.chunks_exact(2) {
            match form_name(&entry[0])
                .as_deref()
                .map(|value| value.trim_start_matches(':'))
            {
                Some("preferred") => preferred = form_name(&entry[1]),
                Some("aliases") => {
                    if let FormKind::Vector(items) = &entry[1].kind {
                        aliases.extend(items.iter().filter_map(form_name));
                    }
                }
                _ => {}
            }
        }
        if let Some(preferred) = preferred {
            aliases.sort();
            aliases.dedup();
            result.insert(
                locale.to_string(),
                LocalizedNameEntry { preferred, aliases },
            );
        }
    }
    result
}

pub fn documentation(metadata: &[MetadataEntry]) -> SemanticDocumentation {
    let Some(value) = metadata.iter().find_map(|entry| {
        (form_name(&entry.key)
            .as_deref()
            .map(|value| value.trim_start_matches(':'))
            == Some("doc"))
        .then_some(&entry.value)
    }) else {
        return SemanticDocumentation::default();
    };
    if let FormKind::String(default) = &value.kind {
        return SemanticDocumentation {
            default: Some(default.clone()),
            translations: BTreeMap::new(),
        };
    }
    let FormKind::Map(entries) = &value.kind else {
        return SemanticDocumentation::default();
    };
    let mut result = SemanticDocumentation::default();
    for pair in entries.chunks_exact(2) {
        let FormKind::String(text) = &pair[1].kind else {
            continue;
        };
        match &pair[0].kind {
            FormKind::Keyword(name) if name.canonical.trim_start_matches(':') == "default" => {
                result.default = Some(text.clone());
            }
            FormKind::String(raw_locale) => {
                if let Ok(locale) = oxilangtag::LanguageTag::parse_and_normalize(raw_locale) {
                    result.translations.insert(locale.to_string(), text.clone());
                }
            }
            _ => {}
        }
    }
    result
}

pub fn localized_name_for<'a>(
    localized: &'a BTreeMap<String, LocalizedNameEntry>,
    locale: Option<&str>,
) -> Option<&'a LocalizedNameEntry> {
    let tag = oxilangtag::LanguageTag::parse_and_normalize(locale?).ok()?;
    let mut candidate = tag.to_string();
    loop {
        if let Some(value) = localized.get(&candidate) {
            return Some(value);
        }
        let (parent, _) = candidate.rsplit_once('-')?;
        candidate.truncate(parent.len());
    }
}

pub(super) fn form_name(form: &Form) -> Option<String> {
    match &form.kind {
        FormKind::Symbol(name) | FormKind::Keyword(name) => Some(name.canonical.clone()),
        FormKind::String(value) => Some(value.clone()),
        _ => None,
    }
}

pub(super) fn metadata_entries(metadata: &[MetadataEntry]) -> Vec<AuthoredMetadata> {
    metadata
        .iter()
        .map(|entry| AuthoredMetadata {
            key: form_json(&entry.key),
            value: form_json(&entry.value),
            key_text: form_text(&entry.key),
            value_text: form_text(&entry.value),
            span: entry.key.span.cover(entry.value.span),
            raw: json!({ "key": form_json(&entry.key), "value": form_json(&entry.value) }),
        })
        .collect()
}

pub(super) fn collect_authored(analysis: &Analysis) -> Vec<AuthoredMetadata> {
    let mut authored = Vec::new();
    for form in &analysis.document.forms {
        collect_form_authored(form, &mut authored);
    }

    let module = &analysis.hir;
    authored.extend(metadata_entries(&module.metadata));
    for item in &module.items {
        authored.extend(metadata_entries(&item.metadata));
    }
    for binding in &module.bindings {
        authored.extend(metadata_entries(&binding.metadata));
    }
    authored.sort_by(|left, right| {
        (
            left.span.start,
            left.span.end,
            &left.key_text,
            &left.value_text,
        )
            .cmp(&(
                right.span.start,
                right.span.end,
                &right.key_text,
                &right.value_text,
            ))
    });
    authored.dedup_by(|left, right| {
        left.span == right.span
            && left.key_text == right.key_text
            && left.value_text == right.value_text
    });
    authored
}

pub(super) fn collect_form_authored(form: &Form, authored: &mut Vec<AuthoredMetadata>) {
    authored.extend(metadata_entries(&form.metadata));
    for entry in &form.metadata {
        collect_form_authored(&entry.key, authored);
        collect_form_authored(&entry.value, authored);
    }
    match &form.kind {
        FormKind::List(items)
        | FormKind::Vector(items)
        | FormKind::Map(items)
        | FormKind::Set(items) => {
            for item in items {
                collect_form_authored(item, authored);
            }
        }
        FormKind::ReaderMacro { form, .. } => collect_form_authored(form, authored),
        FormKind::None
        | FormKind::Bool(_)
        | FormKind::Integer(_)
        | FormKind::Float(_)
        | FormKind::String(_)
        | FormKind::Keyword(_)
        | FormKind::Symbol(_)
        | FormKind::Error(_) => {}
    }
}

pub(super) fn layers_for_metadata(
    metadata: &[MetadataEntry],
    span: Span,
    summary: &SemanticSummary,
) -> SemanticLayers {
    let authored = metadata_entries(metadata);
    let declared = declared_facts(metadata, span);
    let verified = vec![SemanticFact {
        kind: "inferred-summary".to_owned(),
        value: json!({
            "effects": summary.effects,
            "temporal": summary.temporal,
            "data": summary.data,
        }),
        provenance: vec![FactOrigin {
            kind: "typed-hir".to_owned(),
            span,
            detail: Some("derived from local typed HIR".to_owned()),
        }],
        trust: "compiler-verified".to_owned(),
        span,
    }];
    SemanticLayers {
        authored,
        records: Vec::new(),
        declared,
        verified,
    }
}

pub(super) fn declared_facts(metadata: &[MetadataEntry], span: Span) -> Vec<SemanticFact> {
    metadata
        .iter()
        .filter_map(|entry| {
            let key = form_name(&entry.key)?;
            let normalized = key.trim_start_matches(':').to_ascii_lowercase();
            let semantic = normalized.starts_with("osiris/")
                || matches!(
                    normalized.as_str(),
                    "pure"
                        | "effect"
                        | "effects"
                        | "future"
                        | "lookahead"
                        | "availability"
                        | "schema"
                        | "axis"
                        | "contract"
                        | "temporal"
                        | "data"
                );
            semantic.then(|| SemanticFact {
                kind: key,
                value: form_json(&entry.value),
                provenance: vec![FactOrigin {
                    kind: "authored-metadata".to_owned(),
                    span: entry.key.span.cover(entry.value.span),
                    detail: Some("declared by source metadata; not a proof".to_owned()),
                }],
                trust: "declared".to_owned(),
                span,
            })
        })
        .collect()
}

pub(super) fn verified_module_fact(
    module: &hir::Module,
    summary: &SemanticSummary,
) -> SemanticFact {
    SemanticFact {
        kind: "module-summary".to_owned(),
        value: json!({
            "effects": summary.effects,
            "temporal": summary.temporal,
            "data": summary.data,
        }),
        provenance: vec![FactOrigin {
            kind: "typed-hir".to_owned(),
            span: module.span,
            detail: Some("joined summaries of module operations".to_owned()),
        }],
        trust: "compiler-verified".to_owned(),
        span: module.span,
    }
}
