use super::*;

pub(super) fn tick_budget(
    budget: &mut EvalBudget,
    depth: usize,
    span: Span,
) -> Result<(), EvalError> {
    if depth > DEFAULT_MAX_EVAL_DEPTH {
        return Err(EvalError::new(
            "OSR-M0005",
            format!(
                "phase-1 evaluation exceeded the recursion depth limit of {DEFAULT_MAX_EVAL_DEPTH}"
            ),
            span,
        ));
    }
    budget.steps = budget.steps.saturating_add(1);
    if budget.steps > DEFAULT_MAX_EVAL_STEPS {
        return Err(EvalError::new(
            "OSR-M0005",
            format!("phase-1 evaluation exceeded the step limit of {DEFAULT_MAX_EVAL_STEPS}"),
            span,
        ));
    }
    Ok(())
}

pub(super) fn require_form_arity(
    items: &[Form],
    expected: usize,
    name: &str,
    span: Span,
) -> Result<(), EvalError> {
    if items.len() == expected {
        Ok(())
    } else {
        Err(EvalError::evaluation(
            format!(
                "`{name}` expects {} argument(s)",
                expected.saturating_sub(1)
            ),
            span,
        ))
    }
}

pub(super) fn require_value_arity(
    arguments: &[Value],
    expected: usize,
    name: &str,
    span: Span,
) -> Result<(), EvalError> {
    if arguments.len() == expected {
        Ok(())
    } else {
        Err(EvalError::evaluation(
            format!("`{name}` expects {expected} argument(s)"),
            span,
        ))
    }
}

pub(super) struct BindContext<'expander, 'environment, 'budget> {
    pub(super) expander: &'expander mut Expander,
    pub(super) environment: &'environment mut Environment,
    pub(super) budget: &'budget mut EvalBudget,
    pub(super) span: Span,
    pub(super) depth: usize,
}

pub(super) fn bind_parameters(
    context: &mut BindContext<'_, '_, '_>,
    parameters: &Parameters,
    arguments: &[Value],
    macro_call: bool,
) -> Result<(), EvalError> {
    let valid = arguments.len() >= parameters.fixed.len()
        && (parameters.rest.is_some() || arguments.len() == parameters.fixed.len());
    if !valid {
        let expectation = if parameters.rest.is_some() {
            format!("at least {}", parameters.fixed.len())
        } else {
            parameters.fixed.len().to_string()
        };
        return Err(EvalError::new(
            if macro_call { "OSR-M0001" } else { "OSR-M0004" },
            format!(
                "phase-1 call expects {expectation} argument(s), received {}",
                arguments.len()
            ),
            context.span,
        ));
    }
    for (pattern, value) in parameters.fixed.iter().zip(arguments) {
        bind_pattern(context, pattern, value.clone())?;
    }
    if let Some(rest) = &parameters.rest {
        let remaining = arguments[parameters.fixed.len()..]
            .iter()
            .cloned()
            .map(|value| value.into_data(context.span))
            .collect::<Result<Vec<_>, _>>()?;
        bind_pattern(context, rest, Value::Data(list(remaining, context.span)))?;
    }
    Ok(())
}

pub(super) fn bind_pattern(
    context: &mut BindContext<'_, '_, '_>,
    pattern: &Pattern,
    value: Value,
) -> Result<(), EvalError> {
    match pattern {
        Pattern::Bind(name) => {
            context.environment.insert(name.clone(), value);
            Ok(())
        }
        Pattern::Ignore => Ok(()),
        Pattern::Vector(parameters) => {
            let data = value.into_data(context.span)?;
            let items = sequence_items(&data, context.span)?;
            for (index, pattern) in parameters.fixed.iter().enumerate() {
                let item = items
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| none(context.span));
                bind_pattern(context, pattern, Value::Data(item))?;
            }
            if let Some(rest) = &parameters.rest {
                let remaining = items.into_iter().skip(parameters.fixed.len()).collect();
                bind_pattern(context, rest, Value::Data(list(remaining, context.span)))?;
            }
            Ok(())
        }
        Pattern::Map(pattern) => {
            let data = value.into_data(context.span)?;
            let entries = match &data.kind {
                FormKind::None => &[][..],
                FormKind::Map(entries) => entries.as_slice(),
                _ => {
                    return Err(EvalError::evaluation(
                        "map destructuring requires a phase-1 map or none",
                        context.span,
                    ));
                }
            };

            if let Some(whole) = &pattern.whole {
                bind_pattern(context, whole, Value::Data(data.clone()))?;
            }

            for entry in &pattern.entries {
                let found = entries
                    .chunks_exact(2)
                    .find(|pair| crate::syntax::datum_eq(&pair[0], &entry.lookup))
                    .map(|pair| pair[1].clone());
                let selected = if let Some(found) = found {
                    found
                } else if let Pattern::Bind(name) = &entry.binding {
                    match pattern.defaults.get(name) {
                        Some(default) => context
                            .expander
                            .eval(
                                default,
                                context.environment,
                                context.budget,
                                context.depth + 1,
                            )?
                            .into_data(default.span)?,
                        None => none(context.span),
                    }
                } else {
                    none(context.span)
                };
                bind_pattern(context, &entry.binding, Value::Data(selected))?;
            }
            Ok(())
        }
    }
}

pub(super) fn is_truthy(value: &Value) -> bool {
    !matches!(
        value,
        Value::Data(Form {
            kind: FormKind::None | FormKind::Bool(false),
            ..
        })
    )
}

pub(super) fn values_into_forms(arguments: Vec<Value>, span: Span) -> Result<Vec<Form>, EvalError> {
    arguments
        .into_iter()
        .map(|value| value.into_data(span))
        .collect()
}

pub(super) fn value_callable(value: Value, span: Span) -> Result<Callable, EvalError> {
    match value {
        Value::Callable(callable) => Ok(callable),
        Value::Data(_) | Value::Reduced(_) => {
            Err(EvalError::evaluation("expected a phase-1 function", span))
        }
    }
}

pub(super) fn sequence_items(form: &Form, span: Span) -> Result<Vec<Form>, EvalError> {
    match &form.kind {
        FormKind::None => Ok(Vec::new()),
        FormKind::List(items)
        | FormKind::Vector(items)
        | FormKind::Map(items)
        | FormKind::Set(items) => Ok(items.clone()),
        _ => Err(EvalError::evaluation("expected a phase-1 collection", span)),
    }
}

pub(super) fn collection_count(form: &Form, span: Span) -> Result<usize, EvalError> {
    match &form.kind {
        FormKind::None => Ok(0),
        FormKind::String(value) => Ok(value.chars().count()),
        FormKind::List(items) | FormKind::Vector(items) | FormKind::Set(items) => Ok(items.len()),
        FormKind::Map(items) => Ok(items.len() / 2),
        _ => Err(EvalError::evaluation("expected a phase-1 collection", span)),
    }
}

pub(super) fn unique_forms(forms: Vec<Form>) -> Vec<Form> {
    let mut unique = Vec::new();
    for form in forms {
        if !unique
            .iter()
            .any(|existing| crate::syntax::datum_eq(existing, &form))
        {
            unique.push(form);
        }
    }
    unique
}

pub(super) fn conj(mut collection: Form, values: Vec<Form>, span: Span) -> Result<Form, EvalError> {
    match &mut collection.kind {
        FormKind::List(items) => {
            for value in values {
                items.insert(0, value);
            }
        }
        FormKind::Vector(items) => items.extend(values),
        FormKind::Set(items) => {
            items.extend(values);
            *items = unique_forms(std::mem::take(items));
        }
        FormKind::Map(items) => {
            for value in values {
                match value.kind {
                    FormKind::Vector(pair) if pair.len() == 2 => {
                        assoc_items(items, pair[0].clone(), pair[1].clone());
                    }
                    FormKind::Map(entries) if entries.len() % 2 == 0 => {
                        for pair in entries.chunks_exact(2) {
                            assoc_items(items, pair[0].clone(), pair[1].clone());
                        }
                    }
                    _ => {
                        return Err(EvalError::evaluation(
                            "conjoining into a map requires [key value] pairs or maps",
                            span,
                        ));
                    }
                }
            }
        }
        FormKind::None => return Ok(list(values.into_iter().rev().collect(), span)),
        _ => return Err(EvalError::evaluation("`conj` expects a collection", span)),
    }
    collection.span = span;
    collection.datum_span = span;
    Ok(collection)
}

pub(super) fn assoc_form(mut map: Form, pairs: Vec<Form>, span: Span) -> Result<Form, EvalError> {
    let FormKind::Map(items) = &mut map.kind else {
        return Err(EvalError::evaluation("`assoc` expects a map", span));
    };
    for pair in pairs.chunks_exact(2) {
        assoc_items(items, pair[0].clone(), pair[1].clone());
    }
    map.span = span;
    map.datum_span = span;
    Ok(map)
}

pub(super) fn assoc_items(items: &mut Vec<Form>, key: Form, value: Form) {
    if let Some(index) = items
        .chunks_exact(2)
        .position(|pair| crate::syntax::datum_eq(&pair[0], &key))
    {
        items[index * 2] = key;
        items[index * 2 + 1] = value;
    } else {
        items.push(key);
        items.push(value);
    }
}

pub(super) fn dissoc_form(mut map: Form, keys: &[Form], span: Span) -> Result<Form, EvalError> {
    let FormKind::Map(items) = &mut map.kind else {
        return Err(EvalError::evaluation("`dissoc` expects a map", span));
    };
    let mut retained = Vec::new();
    for pair in items.chunks_exact(2) {
        if !keys
            .iter()
            .any(|key| crate::syntax::datum_eq(key, &pair[0]))
        {
            retained.extend_from_slice(pair);
        }
    }
    *items = retained;
    map.span = span;
    map.datum_span = span;
    Ok(map)
}

pub(super) fn get_from_collection(collection: &Form, key: &Form) -> Option<Form> {
    match &collection.kind {
        FormKind::Map(items) => items
            .chunks_exact(2)
            .find(|pair| crate::syntax::datum_eq(&pair[0], key))
            .map(|pair| pair[1].clone()),
        FormKind::Vector(items) | FormKind::List(items) => form_to_usize(key, key.span)
            .ok()
            .and_then(|index| items.get(index).cloned()),
        _ => None,
    }
}

pub(super) fn collection_contains(collection: &Form, key: &Form) -> bool {
    match &collection.kind {
        FormKind::Map(items) => items
            .chunks_exact(2)
            .any(|pair| crate::syntax::datum_eq(&pair[0], key)),
        FormKind::Set(items) => items.iter().any(|item| crate::syntax::datum_eq(item, key)),
        FormKind::Vector(items) | FormKind::List(items) => {
            form_to_usize(key, key.span).is_ok_and(|index| index < items.len())
        }
        _ => false,
    }
}

pub(super) fn metadata_map(form: &Form) -> Form {
    let items = form
        .metadata
        .iter()
        .flat_map(|entry| [entry.key.clone(), entry.value.clone()])
        .collect();
    Form::new(FormKind::Map(items), form.span)
}

pub(super) fn with_metadata(
    mut target: Form,
    metadata: &Form,
    span: Span,
) -> Result<Form, EvalError> {
    if !target.supports_metadata() {
        return Err(EvalError::evaluation(
            "metadata can only be attached to syntax forms that support metadata",
            span,
        ));
    }
    let normalized = match &metadata.kind {
        FormKind::None => Vec::new(),
        FormKind::Map(items) if items.len() % 2 == 0 => {
            let entries = items.len() / 2;
            if entries > METADATA_TARGET_LIMITS.max_entries {
                return Err(EvalError::new(
                    "OSR-M0009",
                    format!(
                        "metadata for one syntax target exceeds the entry count limit of {} (found {entries})",
                        METADATA_TARGET_LIMITS.max_entries
                    ),
                    span,
                ));
            }
            items
                .chunks_exact(2)
                .map(|pair| MetadataEntry {
                    key: pair[0].clone(),
                    value: pair[1].clone(),
                })
                .collect()
        }
        _ => {
            return Err(EvalError::evaluation(
                "metadata must be a map or none",
                span,
            ));
        }
    };
    if normalized.iter().any(|entry| {
        !metadata_datum_is_serializable(&entry.key) || !metadata_datum_is_serializable(&entry.value)
    }) {
        return Err(EvalError::evaluation(
            "metadata must contain only serializable phase-1 data",
            span,
        ));
    }
    if let Err(exceeded) = check_metadata_resources(&normalized, METADATA_TARGET_LIMITS) {
        return Err(EvalError::new(
            "OSR-M0009",
            format!(
                "metadata for one syntax target exceeds the {} limit of {} (found {})",
                exceeded.resource, exceeded.limit, exceeded.actual
            ),
            span,
        ));
    }
    target.metadata = normalized;
    Ok(target)
}
