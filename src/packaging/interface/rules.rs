use super::*;

pub(in crate::interface) fn contains_dynamic_operator_type(ty: &Type) -> bool {
    match ty {
        Type::Any | Type::Unknown | Type::Error => true,
        Type::Option(inner) | Type::List(inner) | Type::Vector(inner) | Type::Set(inner) => {
            contains_dynamic_operator_type(inner)
        }
        Type::Union(members) | Type::Tuple(members) => {
            members.iter().any(contains_dynamic_operator_type)
        }
        Type::Map(key, value) => {
            contains_dynamic_operator_type(key) || contains_dynamic_operator_type(value)
        }
        Type::Fn(function) => {
            function
                .parameters
                .iter()
                .any(contains_dynamic_operator_type)
                || contains_dynamic_operator_type(&function.return_type)
        }
        Type::Nominal { args, .. } => args.iter().any(contains_dynamic_operator_type),
        _ => false,
    }
}

pub(in crate::interface) fn is_publishable_operator(operator: ScalarOperator) -> bool {
    matches!(
        operator,
        ScalarOperator::Add
            | ScalarOperator::Subtract
            | ScalarOperator::Multiply
            | ScalarOperator::TrueDivide
            | ScalarOperator::Less
            | ScalarOperator::LessEqual
            | ScalarOperator::Greater
            | ScalarOperator::GreaterEqual
            | ScalarOperator::Equal
            | ScalarOperator::NotEqual
            | ScalarOperator::Negate
            | ScalarOperator::Positive
            | ScalarOperator::Abs
    )
}

pub(in crate::interface) fn operator_arity(operator: ScalarOperator) -> usize {
    usize::from(matches!(
        operator,
        ScalarOperator::Add
            | ScalarOperator::Subtract
            | ScalarOperator::Multiply
            | ScalarOperator::TrueDivide
            | ScalarOperator::FloorDivide
            | ScalarOperator::Remainder
            | ScalarOperator::Less
            | ScalarOperator::LessEqual
            | ScalarOperator::Greater
            | ScalarOperator::GreaterEqual
            | ScalarOperator::Equal
            | ScalarOperator::NotEqual
    )) + 1
}

pub(in crate::interface) fn phase_helper_closure(
    declaration: &Form,
    helper_forms: &BTreeMap<String, Form>,
    helper_names: &BTreeSet<String>,
) -> InterfaceResult<Vec<String>> {
    let mut pending = phase_direct_helper_references(declaration, helper_names)?;
    let mut closure = BTreeSet::new();
    while let Some(name) = pending.pop_first() {
        if !closure.insert(name.clone()) {
            continue;
        }
        let helper = helper_forms.get(&name).ok_or_else(|| {
            InterfaceError::new("OSR-I0060", format!("missing phase-1 helper `{name}`"))
        })?;
        pending.extend(phase_direct_helper_references(helper, helper_names)?);
    }
    Ok(closure.into_iter().collect())
}

pub(in crate::interface) fn phase_direct_helper_references(
    declaration: &Form,
    helper_names: &BTreeSet<String>,
) -> InterfaceResult<BTreeSet<String>> {
    let (_, parameters, body) = match phase_declaration_head(declaration)? {
        "defmacro" => phase_declaration_parts(declaration, "defmacro")?,
        "defn-for-syntax" => phase_declaration_parts(declaration, "defn-for-syntax")?,
        _ => {
            return Err(InterfaceError::new(
                "OSR-I0059",
                "phase-1 IR must be a macro or syntax function declaration",
            ));
        }
    };
    let mut bound = BTreeSet::new();
    collect_pattern_bindings(parameters, &mut bound);
    let mut references = BTreeSet::new();
    for form in body {
        collect_phase_references(form, &bound, &mut references);
    }
    references.retain(|name| helper_names.contains(name));
    Ok(references)
}

pub(in crate::interface) fn phase_declaration_head(form: &Form) -> InterfaceResult<&str> {
    let FormKind::List(items) = &form.kind else {
        return Err(InterfaceError::new(
            "OSR-I0059",
            "phase-1 IR must be a list",
        ));
    };
    items
        .first()
        .and_then(form_symbol)
        .ok_or_else(|| InterfaceError::new("OSR-I0059", "phase-1 IR requires a symbol head"))
}

pub(in crate::interface) fn phase_declaration_parts<'a>(
    form: &'a Form,
    expected: &str,
) -> InterfaceResult<(&'a str, &'a Form, &'a [Form])> {
    let FormKind::List(items) = &form.kind else {
        return Err(InterfaceError::new(
            "OSR-I0059",
            "phase-1 declaration must be a list",
        ));
    };
    if items.first().and_then(form_symbol) != Some(expected) {
        return Err(InterfaceError::new(
            "OSR-I0059",
            format!("expected `{expected}` phase-1 declaration"),
        ));
    }
    let name = items
        .get(1)
        .and_then(form_symbol)
        .ok_or_else(|| InterfaceError::new("OSR-I0059", "phase-1 declaration has no name"))?;
    let mut index = 2;
    if matches!(
        items.get(index).map(|form| &form.kind),
        Some(FormKind::String(_))
    ) {
        index += 1;
    }
    let parameters = items.get(index).ok_or_else(|| {
        InterfaceError::new("OSR-I0059", "phase-1 declaration has no parameter vector")
    })?;
    index += 1;
    if items.get(index).and_then(form_symbol) == Some("->") {
        if items.get(index + 1).is_none() {
            return Err(InterfaceError::new(
                "OSR-I0059",
                "phase-1 return annotation has no type",
            ));
        }
        index += 2;
    }
    if index >= items.len() {
        return Err(InterfaceError::new(
            "OSR-I0059",
            "phase-1 declaration has no body",
        ));
    }
    Ok((name, parameters, &items[index..]))
}

pub(in crate::interface) fn phase_parameter_arity(
    parameters: &Form,
) -> InterfaceResult<(usize, bool)> {
    let FormKind::Vector(items) = &parameters.kind else {
        return Err(InterfaceError::new(
            "OSR-I0059",
            "macro parameters must be a vector",
        ));
    };
    let mut minimum = 0;
    let mut variadic = false;
    let mut index = 0;
    while index < items.len() {
        if form_symbol(&items[index]) == Some("&") {
            if variadic || index + 2 != items.len() {
                return Err(InterfaceError::new(
                    "OSR-I0059",
                    "`&` must precede the final macro parameter",
                ));
            }
            variadic = true;
            index += 2;
        } else {
            minimum += 1;
            index += 1;
        }
    }
    Ok((minimum, variadic))
}

pub(in crate::interface) fn form_symbol(form: &Form) -> Option<&str> {
    match &form.kind {
        FormKind::Symbol(name) => Some(&name.canonical),
        _ => None,
    }
}

pub(in crate::interface) fn collect_pattern_bindings(
    pattern: &Form,
    bindings: &mut BTreeSet<String>,
) {
    match &pattern.kind {
        FormKind::Symbol(name) if name.canonical != "_" && name.canonical != "&" => {
            bindings.insert(name.canonical.clone());
        }
        FormKind::Vector(items) => {
            for item in items {
                collect_pattern_bindings(item, bindings);
            }
        }
        _ => {}
    }
}

pub(in crate::interface) fn collect_phase_references(
    form: &Form,
    bound: &BTreeSet<String>,
    references: &mut BTreeSet<String>,
) {
    match &form.kind {
        FormKind::Symbol(name) => {
            if !bound.contains(&name.canonical) {
                references.insert(name.canonical.clone());
            }
        }
        FormKind::List(items)
        | FormKind::Vector(items)
        | FormKind::Map(items)
        | FormKind::Set(items) => {
            for item in items {
                collect_phase_references(item, bound, references);
            }
        }
        FormKind::ReaderMacro {
            macro_kind: ReaderMacroKind::Quote,
            ..
        } => {}
        FormKind::ReaderMacro {
            macro_kind: ReaderMacroKind::SyntaxQuote,
            form,
        } => {
            collect_syntax_quote_references(form, bound, references, 1);
        }
        FormKind::ReaderMacro { form, .. } => {
            collect_phase_references(form, bound, references);
        }
        FormKind::None
        | FormKind::Bool(_)
        | FormKind::Integer(_)
        | FormKind::Float(_)
        | FormKind::String(_)
        | FormKind::Keyword(_)
        | FormKind::Error(_) => {}
    }
}

pub(in crate::interface) fn collect_syntax_quote_references(
    form: &Form,
    bound: &BTreeSet<String>,
    references: &mut BTreeSet<String>,
    depth: usize,
) {
    match &form.kind {
        FormKind::ReaderMacro {
            macro_kind: ReaderMacroKind::Unquote | ReaderMacroKind::UnquoteSplicing,
            form,
        } if depth == 1 => collect_phase_references(form, bound, references),
        FormKind::ReaderMacro {
            macro_kind: ReaderMacroKind::SyntaxQuote,
            form,
        } => collect_syntax_quote_references(form, bound, references, depth + 1),
        FormKind::ReaderMacro {
            macro_kind: ReaderMacroKind::Unquote | ReaderMacroKind::UnquoteSplicing,
            form,
        } if depth > 1 => collect_syntax_quote_references(form, bound, references, depth - 1),
        FormKind::ReaderMacro {
            macro_kind: ReaderMacroKind::Quote,
            ..
        } => {}
        FormKind::List(items)
        | FormKind::Vector(items)
        | FormKind::Map(items)
        | FormKind::Set(items) => {
            for item in items {
                collect_syntax_quote_references(item, bound, references, depth);
            }
        }
        _ => {}
    }
}
