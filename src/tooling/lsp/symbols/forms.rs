pub(super) fn semantic_symbol_accepts(symbol: &SemanticSymbol, spelling: &str) -> bool {
    let spelling = spelling.nfc().collect::<String>();
    symbol.canonical == spelling
        || symbol.source_spelling.nfc().eq(spelling.chars())
        || symbol
            .aliases
            .iter()
            .any(|alias| alias.spelling.nfc().eq(spelling.chars()) || alias.canonical == spelling)
}

pub(super) fn exact_container_form(forms: &[Form], span: Span) -> Option<&Form> {
    for form in forms {
        if form.span == span || form.datum_span == span {
            return Some(form);
        }
        if form.span.start <= span.start
            && span.end <= form.span.end
            && let Some(found) = exact_container_form(form_children(form), span)
        {
            return Some(found);
        }
    }
    None
}

pub(super) fn exact_symbol_form(forms: &[Form], span: Span) -> Option<&Form> {
    let form = exact_container_form(forms, span)?;
    matches!(form.kind, FormKind::Symbol(_)).then_some(form)
}

pub(super) fn definition_name_form<'form>(
    forms: &'form [Form],
    span: Span,
    expected: &str,
) -> Option<&'form Form> {
    let container = exact_container_form(forms, span)?;
    if symbol_form_matches(container, expected) {
        return Some(container);
    }
    if let FormKind::List(items) = &container.kind {
        let declaration = items.first().and_then(form_name).is_some_and(|head| {
            matches!(
                head,
                "def" | "defn" | "defstruct" | "defstatic-schema" | "defmacro" | "defn-for-syntax"
            )
        });
        if declaration
            && let Some(name) = items
                .get(1)
                .filter(|form| symbol_form_matches(form, expected))
        {
            return Some(name);
        }
    }
    form_children(container)
        .iter()
        .find(|form| symbol_form_matches(form, expected))
}

pub(super) fn symbol_form_matches(form: &Form, expected: &str) -> bool {
    let FormKind::Symbol(name) = &form.kind else {
        return false;
    };
    name.spelling.nfc().eq(expected.nfc())
}

pub(super) fn rename_member_from_form(source: &str, form: &Form) -> Option<(Span, String)> {
    if !matches!(form.kind, FormKind::Symbol(_))
        || form.datum_span.end > source.len()
        || !source.is_char_boundary(form.datum_span.start)
        || !source.is_char_boundary(form.datum_span.end)
    {
        return None;
    }
    let raw = &source[form.datum_span.start..form.datum_span.end];
    let relative = raw
        .char_indices()
        .filter(|(_, character)| matches!(character, '/' | '.'))
        .map(|(offset, character)| offset + character.len_utf8())
        .next_back()
        .unwrap_or(0);
    let spelling = raw.get(relative..)?.to_owned();
    if spelling.is_empty() {
        return None;
    }
    Some((
        Span::new(form.datum_span.start + relative, form.datum_span.end),
        spelling,
    ))
}

pub(super) fn list_item(form: &Form, index: usize) -> Option<&Form> {
    let FormKind::List(items) = &form.kind else {
        return None;
    };
    items.get(index)
}

pub(super) fn collection_items(form: &Form) -> Option<&[Form]> {
    match &form.kind {
        FormKind::List(items) | FormKind::Vector(items) => Some(items),
        _ => None,
    }
}

pub(super) fn import_member_forms(form: &Form) -> Vec<&Form> {
    let FormKind::List(items) = &form.kind else {
        return Vec::new();
    };
    let mut members = Vec::new();
    let mut index = 2;
    while index + 1 < items.len() {
        let is_members = matches!(
            &items[index].kind,
            FormKind::Keyword(name) if matches!(name.canonical.as_str(), ":refer" | ":only")
        );
        if is_members && let Some(values) = collection_items(&items[index + 1]) {
            members.extend(values);
        }
        index += 2;
    }
    members
}

pub(super) fn form_children(form: &Form) -> &[Form] {
    match &form.kind {
        FormKind::List(items)
        | FormKind::Vector(items)
        | FormKind::Map(items)
        | FormKind::Set(items) => items,
        FormKind::ReaderMacro { form, .. } => std::slice::from_ref(form),
        _ => &[],
    }
}
