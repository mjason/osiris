use super::*;

pub(super) fn display_form(form: &Form) -> String {
    match &form.kind {
        FormKind::None => String::new(),
        FormKind::Bool(value) => value.to_string(),
        FormKind::Integer(value) | FormKind::Float(value) | FormKind::String(value) => {
            value.clone()
        }
        FormKind::Symbol(name) | FormKind::Keyword(name) => name.spelling.clone(),
        FormKind::List(items) => display_collection("(", ")", items),
        FormKind::Vector(items) => display_collection("[", "]", items),
        FormKind::Map(items) => display_collection("{", "}", items),
        FormKind::Set(items) => display_collection("#{", "}", items),
        FormKind::ReaderMacro { form, .. } => display_form(form),
        FormKind::Error(message) => format!("#<error:{message}>"),
    }
}

pub(super) fn display_collection(open: &str, close: &str, items: &[Form]) -> String {
    format!(
        "{open}{}{close}",
        items.iter().map(display_form).collect::<Vec<_>>().join(" ")
    )
}

pub(super) fn form_node_count(form: &Form) -> usize {
    let metadata = form.metadata.iter().fold(0_usize, |count, entry| {
        count
            .saturating_add(form_node_count(&entry.key))
            .saturating_add(form_node_count(&entry.value))
    });
    let children = match &form.kind {
        FormKind::List(items)
        | FormKind::Vector(items)
        | FormKind::Map(items)
        | FormKind::Set(items) => items.iter().fold(0_usize, |count, item| {
            count.saturating_add(form_node_count(item))
        }),
        FormKind::ReaderMacro { form, .. } => form_node_count(form),
        _ => 0,
    };
    1_usize.saturating_add(metadata).saturating_add(children)
}

pub(super) fn none(span: Span) -> Form {
    Form::new(FormKind::None, span)
}

pub(super) fn boolean(value: bool, span: Span) -> Form {
    Form::new(FormKind::Bool(value), span)
}

pub(super) fn integer(value: usize, span: Span) -> Form {
    Form::new(FormKind::Integer(value.to_string()), span)
}

pub(super) fn string(value: &str, span: Span) -> Form {
    Form::new(FormKind::String(value.to_owned()), span)
}

pub(super) fn named_form(keyword: bool, spelling: &str, span: Span) -> Form {
    let name = Name {
        spelling: spelling.to_owned(),
        canonical: spelling.to_owned(),
    };
    Form::new(
        if keyword {
            FormKind::Keyword(name)
        } else {
            FormKind::Symbol(name)
        },
        span,
    )
}

pub(super) fn is_phase_one_declaration(name: &str) -> bool {
    is_phase_declaration(name)
}

/// Heads whose presence at module level establishes the dependency/phase
/// boundary before runtime macro expansion. They must be authored directly;
/// runtime declarations such as `def`, `defn`, `defstruct`, `extern`, and
/// `static-record` intentionally remain generatable by declaration macros.
pub(super) fn top_level_boundary_head(form: &Form) -> Option<&str> {
    let FormKind::List(items) = &form.kind else {
        return None;
    };
    let head = items.first().and_then(symbol_canonical)?;
    is_authored_boundary(head).then_some(head)
}

pub(super) fn generated_top_level_boundary_head(form: &Form) -> Option<&str> {
    if let Some(head) = top_level_boundary_head(form) {
        return Some(head);
    }
    let FormKind::List(items) = &form.kind else {
        return None;
    };
    if items.first().and_then(symbol_canonical) != Some("do") {
        return None;
    }
    items
        .iter()
        .skip(1)
        .find_map(generated_top_level_boundary_head)
}

pub(super) fn generated_declaration_sequence(form: &Form) -> Option<Vec<Form>> {
    let FormKind::List(items) = &form.kind else {
        return None;
    };
    if items.first().and_then(symbol_canonical) != Some("do") {
        return None;
    }
    let mut declarations = Vec::new();
    for item in items.iter().skip(1) {
        if let Some(nested) = generated_declaration_sequence(item) {
            declarations.extend(nested);
        } else if is_runtime_declaration(item) {
            declarations.push(item.clone());
        } else {
            return None;
        }
    }
    (!declarations.is_empty()).then_some(declarations)
}

pub(super) fn is_runtime_declaration(form: &Form) -> bool {
    let FormKind::List(items) = &form.kind else {
        return false;
    };
    items
        .first()
        .and_then(symbol_canonical)
        .is_some_and(is_macro_declaration)
}

pub(super) fn symbol_canonical(form: &Form) -> Option<&str> {
    match &form.kind {
        FormKind::Symbol(name) => Some(&name.canonical),
        _ => None,
    }
}

pub(super) fn list(items: Vec<Form>, span: Span) -> Form {
    Form::new(FormKind::List(items), span)
}

pub(super) fn vector(items: Vec<Form>, span: Span) -> Form {
    Form::new(FormKind::Vector(items), span)
}

pub(super) fn error_form(message: &str, span: Span) -> Form {
    Form::new(FormKind::Error(message.to_owned()), span)
}

pub(super) fn merge_call_metadata(
    call: &[crate::syntax::MetadataEntry],
    generated: &[crate::syntax::MetadataEntry],
) -> Vec<crate::syntax::MetadataEntry> {
    let mut metadata = generated.to_vec();
    for entry in call {
        if let Some(existing) = metadata
            .iter_mut()
            .find(|existing| crate::syntax::datum_eq(&existing.key, &entry.key))
        {
            *existing = entry.clone();
        } else {
            metadata.push(entry.clone());
        }
    }
    metadata
}
