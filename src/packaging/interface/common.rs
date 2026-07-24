use super::*;

mod metadata;

pub(in crate::interface) use metadata::validate_declaration_metadata;
use metadata::{normalize_documentation, normalize_localized_names};

pub(in crate::interface) fn project_metadata(
    metadata: &[MetadataEntry],
    projection: MetadataProjection,
) -> Vec<MetadataEntry> {
    match projection {
        MetadataProjection::Full => metadata.to_vec(),
        MetadataProjection::Semantic => metadata
            .iter()
            .filter(|entry| {
                let key = form_name(&entry.key)
                    .unwrap_or_default()
                    .trim_start_matches(':');
                !(key == "doc"
                    || key == "since"
                    || key == "deprecated"
                    || key == "replacement"
                    || key.starts_with("agent/")
                    || key.starts_with("render/"))
            })
            .cloned()
            .collect(),
    }
}

pub(crate) fn normalize_metadata(
    metadata: &[MetadataEntry],
) -> InterfaceResult<Vec<MetadataEntry>> {
    validate_metadata_target(metadata, "metadata target")?;
    let mut values = metadata
        .iter()
        .map(|entry| {
            let key = normalize_form(&entry.key);
            let value = match form_name(&key).map(|name| name.trim_start_matches(':')) {
                Some("doc") => normalize_documentation(&entry.value)?,
                Some("osiris/names") => normalize_localized_names(&entry.value)?,
                _ => normalize_form(&entry.value),
            };
            Ok(MetadataEntry { key, value })
        })
        .collect::<InterfaceResult<Vec<_>>>()?;
    values.sort_by_cached_key(|entry| form_text(&entry.key));
    for pair in values.windows(2) {
        if form_text(&pair[0].key) == form_text(&pair[1].key) {
            return Err(InterfaceError::new("OSR-I0040", "duplicate metadata key"));
        }
    }
    Ok(values)
}

pub(crate) fn normalize_form(form: &Form) -> Form {
    let kind = match &form.kind {
        FormKind::None => FormKind::None,
        FormKind::Bool(value) => FormKind::Bool(*value),
        FormKind::Integer(value) => FormKind::Integer(value.clone()),
        FormKind::Float(value) => FormKind::Float(value.clone()),
        FormKind::String(value) => FormKind::String(value.clone()),
        FormKind::Keyword(name) => FormKind::Keyword(normalize_name(name)),
        FormKind::Symbol(name) => FormKind::Symbol(normalize_name(name)),
        FormKind::List(values) => FormKind::List(values.iter().map(normalize_form).collect()),
        FormKind::Vector(values) => FormKind::Vector(values.iter().map(normalize_form).collect()),
        FormKind::Map(values) => {
            let mut pairs = values
                .chunks_exact(2)
                .map(|pair| (normalize_form(&pair[0]), normalize_form(&pair[1])))
                .collect::<Vec<_>>();
            pairs.sort_by_cached_key(|(key, _)| form_text(key));
            FormKind::Map(
                pairs
                    .into_iter()
                    .flat_map(|(key, value)| [key, value])
                    .collect(),
            )
        }
        FormKind::Set(values) => {
            let mut values = values.iter().map(normalize_form).collect::<Vec<_>>();
            values.sort_by_cached_key(form_text);
            FormKind::Set(values)
        }
        FormKind::ReaderMacro { macro_kind, form } => FormKind::ReaderMacro {
            macro_kind: *macro_kind,
            form: Box::new(normalize_form(form)),
        },
        FormKind::Error(message) => FormKind::Error(message.clone()),
    };
    let mut result = form_node(kind);
    result.metadata = normalize_metadata(&form.metadata).unwrap_or_else(|_| form.metadata.clone());
    result
}

pub(in crate::interface) fn normalize_name(name: &Name) -> Name {
    Name {
        spelling: name.canonical.clone(),
        canonical: name.canonical.clone(),
    }
}

pub(in crate::interface) fn binding_kind_name(kind: BindingKind) -> &'static str {
    match kind {
        BindingKind::Module => "module",
        BindingKind::Value => "value",
        BindingKind::Function => "function",
        BindingKind::Type => "type",
        BindingKind::Field => "field",
        BindingKind::Parameter => "parameter",
        BindingKind::Macro => "macro",
        BindingKind::PythonModule => "python-module",
    }
}

pub(in crate::interface) fn metadata_form(metadata: &[MetadataEntry]) -> Form {
    form_node(FormKind::Map(
        metadata
            .iter()
            .flat_map(|entry| [entry.key.clone(), entry.value.clone()])
            .collect(),
    ))
}

pub(in crate::interface) fn strings_form(values: &[String]) -> Form {
    vector(values.iter().map(|value| string(value)).collect())
}

pub(in crate::interface) fn optional_string(value: Option<&str>) -> Form {
    value.map_or_else(none, string)
}

pub(in crate::interface) fn optional_bool(value: Option<bool>) -> Form {
    value.map_or_else(none, boolean)
}

pub(in crate::interface) fn wrap(head: &str, value: Form) -> Form {
    form_node(FormKind::List(vec![symbol(head), value]))
}

pub(in crate::interface) fn map(entries: Vec<(&str, Form)>) -> Form {
    form_node(FormKind::Map(
        entries
            .into_iter()
            .flat_map(|(key, value)| [keyword(key), value])
            .collect(),
    ))
}

pub(in crate::interface) fn vector(values: Vec<Form>) -> Form {
    form_node(FormKind::Vector(values))
}

pub(in crate::interface) fn none() -> Form {
    form_node(FormKind::None)
}

pub(in crate::interface) fn boolean(value: bool) -> Form {
    form_node(FormKind::Bool(value))
}

pub(in crate::interface) fn integer(value: u32) -> Form {
    form_node(FormKind::Integer(value.to_string()))
}

pub(in crate::interface) fn integer_u64(value: u64) -> Form {
    form_node(FormKind::Integer(value.to_string()))
}

pub(in crate::interface) fn integer_usize(value: usize) -> Form {
    form_node(FormKind::Integer(value.to_string()))
}

pub(in crate::interface) fn string(value: &str) -> Form {
    form_node(FormKind::String(value.to_owned()))
}

pub(in crate::interface) fn keyword(value: &str) -> Form {
    let value = format!(":{}", value.trim_start_matches(':'));
    form_node(FormKind::Keyword(Name {
        spelling: value.clone(),
        canonical: value,
    }))
}

pub(in crate::interface) fn symbol(value: &str) -> Form {
    form_node(FormKind::Symbol(Name {
        spelling: value.to_owned(),
        canonical: value.to_owned(),
    }))
}

pub(in crate::interface) fn form_node(kind: FormKind) -> Form {
    Form::new(kind, Span::default())
}

pub(in crate::interface) fn is_none(form: &Form) -> bool {
    matches!(form.kind, FormKind::None)
}

pub(in crate::interface) fn form_name(form: &Form) -> Option<&str> {
    match &form.kind {
        FormKind::Keyword(name) | FormKind::Symbol(name) => Some(&name.canonical),
        _ => None,
    }
}

pub(in crate::interface) fn render_forms(forms: &[Form]) -> String {
    render_document_text(&Document {
        format_version: 1,
        source_len: 0,
        tokens: Vec::new(),
        forms: forms.to_vec(),
        nodes: Vec::new(),
        diagnostics: Vec::new(),
    })
}

pub(in crate::interface) fn form_text(form: &Form) -> String {
    render_forms(std::slice::from_ref(form))
        .trim_end_matches('\n')
        .to_owned()
}

pub(in crate::interface) fn hash_form(form: &Form) -> String {
    hash_text(&form_text(form))
}

pub(in crate::interface) fn hash_text(value: &str) -> String {
    crate::hash::sha256(value.as_bytes())
}
