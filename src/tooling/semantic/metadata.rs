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
                labels: labels_for_name(target_canonical, Some(alias.spelling.clone())),
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

pub(super) fn labels_for_name(canonical: &str, preferred: Option<String>) -> LocalizedLabel {
    let chinese = preferred.filter(|value| contains_cjk(value));
    LocalizedLabel::new(canonical.to_owned(), chinese)
}

pub(super) fn preferred_alias(
    aliases: &[SemanticAlias],
    metadata: &[MetadataEntry],
) -> Option<String> {
    aliases
        .iter()
        .find(|alias| alias.public && contains_cjk(&alias.spelling))
        .or_else(|| aliases.iter().find(|alias| contains_cjk(&alias.spelling)))
        .map(|alias| alias.spelling.clone())
        .or_else(|| metadata_preferred_name(metadata))
}

pub(super) fn metadata_preferred_name(metadata: &[MetadataEntry]) -> Option<String> {
    for entry in metadata {
        let key = form_name(&entry.key).unwrap_or_default();
        let key = key.trim_start_matches(':').to_ascii_lowercase();
        if matches!(key.as_str(), "preferred" | "name" | "zh-cn" | "zh_cn") {
            if let Some(value) = form_name(&entry.value) {
                return Some(value);
            }
        }
        if key == "osiris/names" || key == "names" {
            if let Some(value) = metadata_preferred_name_from_form(&entry.value) {
                return Some(value);
            }
        }
    }
    None
}

pub(super) fn metadata_preferred_name_from_form(form: &Form) -> Option<String> {
    let FormKind::Map(entries) = &form.kind else {
        return form_name(form);
    };
    for pair in entries.chunks_exact(2) {
        let key = form_name(&pair[0])?
            .trim_start_matches(':')
            .to_ascii_lowercase();
        if matches!(key.as_str(), "preferred" | "zh-cn" | "zh_cn") {
            if let Some(name) = form_name(&pair[1]) {
                return Some(name);
            }
        }
        if key == "aliases" {
            if let FormKind::Vector(values) = &pair[1].kind {
                if let Some(name) = values
                    .iter()
                    .filter_map(form_name)
                    .find(|name| contains_cjk(name))
                {
                    return Some(name);
                }
            }
        }
    }
    None
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
