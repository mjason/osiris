use super::*;

#[derive(Clone)]
struct Resource {
    language: String,
    body: String,
    span: Span,
    body_span: Span,
}

pub(super) fn prepare_forms(
    forms: &[Form],
) -> (
    Vec<Form>,
    BTreeSet<String>,
    Vec<EmbeddedContentReference>,
    Vec<Diagnostic>,
) {
    let mut resources = BTreeMap::<String, Resource>::new();
    let mut diagnostics = Vec::new();
    for form in forms {
        let FormKind::EmbeddedLanguage {
            language,
            label,
            body,
            body_span,
            ..
        } = &form.kind
        else {
            continue;
        };
        if resources
            .insert(
                label.canonical.clone(),
                Resource {
                    language: language.clone(),
                    body: body.clone(),
                    span: form.span,
                    body_span: *body_span,
                },
            )
            .is_some()
        {
            diagnostics.push(Diagnostic::error(
                "OSR-A0012",
                format!("duplicate embedded label `{}`", label.spelling),
                form.span,
            ));
        }
    }

    let mut prepared = forms.to_vec();
    let mut content_references = Vec::new();
    for form in &mut prepared {
        resolve_form_metadata(form, &resources, &mut content_references, &mut diagnostics);
    }

    let mut runtime_references = BTreeSet::new();
    for form in &prepared {
        collect_runtime_references(form, &mut runtime_references);
    }
    (
        prepared,
        runtime_references,
        content_references,
        diagnostics,
    )
}

fn resolve_form_metadata(
    form: &mut Form,
    resources: &BTreeMap<String, Resource>,
    references: &mut Vec<EmbeddedContentReference>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let reference_start = references.len();
    for entry in &mut form.metadata {
        let key = metadata_key(&entry.key);
        match key {
            Some("doc") => resolve_doc(&mut entry.value, resources, references, diagnostics),
            Some("examples") => {
                resolve_examples(&mut entry.value, resources, references, diagnostics)
            }
            _ => {}
        }
    }
    if references.len() > reference_start {
        let value = Form::new(
            FormKind::Vector(
                references[reference_start..]
                    .iter()
                    .map(content_reference_form)
                    .collect(),
            ),
            form.span,
        );
        form.metadata.push(MetadataEntry {
            key: Form::new(
                FormKind::Keyword(Name {
                    spelling: ":osiris/content-references".to_owned(),
                    canonical: ":osiris/content-references".to_owned(),
                }),
                form.span,
            ),
            value,
        });
    }
    for entry in &mut form.metadata {
        resolve_form_metadata(&mut entry.key, resources, references, diagnostics);
        resolve_form_metadata(&mut entry.value, resources, references, diagnostics);
    }
    match &mut form.kind {
        FormKind::List(items)
        | FormKind::Vector(items)
        | FormKind::Map(items)
        | FormKind::Set(items) => {
            for item in items {
                resolve_form_metadata(item, resources, references, diagnostics);
            }
        }
        FormKind::ReaderMacro { form, .. } => {
            resolve_form_metadata(form, resources, references, diagnostics);
        }
        _ => {}
    }
}

fn content_reference_form(reference: &EmbeddedContentReference) -> Form {
    let span = reference.reference_span;
    let entry = |name: &str, value: FormKind| {
        [
            Form::new(
                FormKind::Keyword(Name {
                    spelling: format!(":{name}"),
                    canonical: format!(":{name}"),
                }),
                span,
            ),
            Form::new(value, span),
        ]
    };
    let mut items = Vec::new();
    items.extend(entry("field", FormKind::String(reference.field.clone())));
    items.extend(entry(
        "language",
        FormKind::String(reference.language.clone()),
    ));
    items.extend(entry("label", FormKind::String(reference.label.clone())));
    items.extend(entry(
        "content",
        FormKind::String(reference.content.clone()),
    ));
    items.extend(entry(
        "content-hash",
        FormKind::String(reference.content_hash.clone()),
    ));
    items.extend(entry(
        "source-start",
        FormKind::Integer(reference.source_span.start.to_string()),
    ));
    items.extend(entry(
        "source-end",
        FormKind::Integer(reference.source_span.end.to_string()),
    ));
    items.extend(entry(
        "body-start",
        FormKind::Integer(reference.body_span.start.to_string()),
    ));
    items.extend(entry(
        "body-end",
        FormKind::Integer(reference.body_span.end.to_string()),
    ));
    Form::new(FormKind::Map(items), span)
}

fn resolve_doc(
    value: &mut Form,
    resources: &BTreeMap<String, Resource>,
    references: &mut Vec<EmbeddedContentReference>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match &mut value.kind {
        FormKind::Symbol(_) => resolve_reference(
            value,
            "doc/default",
            "markdown",
            resources,
            references,
            diagnostics,
        ),
        FormKind::Map(items) => {
            for pair in items.chunks_exact_mut(2) {
                let field = match &pair[0].kind {
                    FormKind::Keyword(name)
                        if name.canonical.trim_start_matches(':') == "default" =>
                    {
                        "doc/default".to_owned()
                    }
                    FormKind::String(locale) => format!("doc/{locale}"),
                    _ => "doc/unknown".to_owned(),
                };
                let referenced = &mut pair[1];
                if matches!(referenced.kind, FormKind::Symbol(_)) {
                    resolve_reference(
                        referenced,
                        &field,
                        "markdown",
                        resources,
                        references,
                        diagnostics,
                    );
                }
            }
        }
        _ => {}
    }
}

fn resolve_examples(
    value: &mut Form,
    resources: &BTreeMap<String, Resource>,
    references: &mut Vec<EmbeddedContentReference>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let FormKind::Vector(examples) = &mut value.kind else {
        diagnostics.push(Diagnostic::error(
            "OSR-A0012",
            "`:examples` must be a vector of `~osiris` binding names",
            value.span,
        ));
        return;
    };
    if examples.is_empty() {
        diagnostics.push(Diagnostic::error(
            "OSR-A0012",
            "`:examples` must reference at least one `~osiris` block",
            value.span,
        ));
    }
    for (index, example) in examples.iter_mut().enumerate() {
        let FormKind::Symbol(name) = &example.kind else {
            diagnostics.push(Diagnostic::error(
                "OSR-A0012",
                "each `:examples` member must be an unquoted `~osiris` binding name",
                example.span,
            ));
            continue;
        };
        let Some(resource) = resources.get(&name.canonical) else {
            diagnostics.push(Diagnostic::error(
                "OSR-A0012",
                format!("unknown embedded example `{}`", name.spelling),
                example.span,
            ));
            continue;
        };
        if resource.language != "osiris" {
            diagnostics.push(Diagnostic::error(
                "OSR-A0012",
                format!(
                    "example `{}` must reference `~osiris`, found `~{}`",
                    name.spelling, resource.language
                ),
                example.span,
            ));
            continue;
        }
        if resource.body.trim().is_empty() {
            diagnostics.push(Diagnostic::error(
                "OSR-A0012",
                format!("example `{}` must not be empty", name.spelling),
                resource.span,
            ));
            continue;
        }
        match crate::formatter::format_source(&resource.body) {
            Ok(formatted)
                if formatted.trim_end_matches(['\r', '\n'])
                    == resource.body.trim_end_matches(['\r', '\n']) => {}
            Ok(_) => diagnostics.push(Diagnostic::error(
                "OSR-A0012",
                format!(
                    "example `{}` must use canonical Osiris formatting",
                    name.spelling
                ),
                resource.span,
            )),
            Err(error) => diagnostics.extend(error.diagnostics),
        }
        references.push(content_reference(
            format!("examples/{index}"),
            example.span,
            name,
            resource,
        ));
        example.kind = FormKind::Vector(
            resource
                .body
                .split('\n')
                .map(|line| Form::new(FormKind::String(line.to_owned()), resource.span))
                .collect(),
        );
    }
}

fn resolve_reference(
    value: &mut Form,
    field: &str,
    expected_language: &str,
    resources: &BTreeMap<String, Resource>,
    references: &mut Vec<EmbeddedContentReference>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let FormKind::Symbol(name) = &value.kind else {
        return;
    };
    let Some(resource) = resources.get(&name.canonical) else {
        diagnostics.push(Diagnostic::error(
            "OSR-A0012",
            format!("unknown embedded documentation `{}`", name.spelling),
            value.span,
        ));
        return;
    };
    if resource.language != expected_language {
        diagnostics.push(Diagnostic::error(
            "OSR-A0012",
            format!(
                "documentation `{}` must reference `~{expected_language}`, found `~{}`",
                name.spelling, resource.language
            ),
            value.span,
        ));
        return;
    }
    if resource.body.trim().is_empty() {
        diagnostics.push(Diagnostic::error(
            "OSR-A0012",
            format!("documentation `{}` must not be empty", name.spelling),
            resource.span,
        ));
        return;
    }
    references.push(content_reference(
        field.to_owned(),
        value.span,
        name,
        resource,
    ));
    value.kind = FormKind::String(resource.body.clone());
}

fn content_reference(
    field: String,
    reference_span: Span,
    name: &Name,
    resource: &Resource,
) -> EmbeddedContentReference {
    EmbeddedContentReference {
        field,
        reference_span,
        language: resource.language.clone(),
        label: name.spelling.clone(),
        content: resource.body.clone(),
        source_span: resource.span,
        body_span: resource.body_span,
        content_hash: crate::hash::sha256(resource.body.as_bytes()),
    }
}

fn collect_runtime_references(form: &Form, references: &mut BTreeSet<String>) {
    match &form.kind {
        FormKind::Symbol(name) => {
            references.insert(name.canonical.clone());
        }
        FormKind::List(items)
        | FormKind::Vector(items)
        | FormKind::Map(items)
        | FormKind::Set(items) => {
            for item in items {
                collect_runtime_references(item, references);
            }
        }
        FormKind::ReaderMacro { form, .. } => collect_runtime_references(form, references),
        FormKind::EmbeddedLanguage { .. } => {}
        _ => {}
    }
}

impl Lowerer {
    pub(super) fn lower_embedded_item(&mut self, form: &Form) -> Item {
        let FormKind::EmbeddedLanguage {
            language,
            label,
            raw_body,
            body,
            body_span,
            ..
        } = &form.kind
        else {
            unreachable!("embedded lowering requires an embedded form");
        };
        let kind = if language == "python" {
            ItemKind::EmbeddedPython(EmbeddedPython {
                span: form.span,
                body_span: *body_span,
                handle: label.clone(),
                raw_body: raw_body.clone(),
                body: body.clone(),
                logical_module: None,
                source_path: None,
            })
        } else {
            ItemKind::EmbeddedText(EmbeddedText {
                span: form.span,
                body_span: *body_span,
                language: language.clone(),
                label: label.clone(),
                body: body.clone(),
                runtime_reachable: self.embedded_runtime_references.contains(&label.canonical),
            })
        };
        Item::new(form, kind)
    }
}
