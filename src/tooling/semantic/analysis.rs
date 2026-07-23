use super::*;

pub(super) fn module_summary(module: &hir::Module) -> SemanticSummary {
    let mut summary = CallSummaries::pure_scalar();
    for item in &module.items {
        let item_summary = match &item.kind {
            ItemKind::Value(value) => value
                .value
                .as_ref()
                .map_or_else(CallSummaries::pure_scalar, |expr| expr.summaries.clone()),
            ItemKind::Function(function) => function
                .decorators
                .iter()
                .fold(function.summaries.clone(), |joined, decorator| {
                    joined.join(&decorator.summaries)
                }),
            ItemKind::Struct(structure) => {
                let decorators = structure
                    .decorators
                    .iter()
                    .fold(CallSummaries::pure_scalar(), |joined, decorator| {
                        joined.join(&decorator.summaries)
                    });
                structure.checks.iter().fold(decorators, |joined, check| {
                    joined.join(&check.condition.summaries)
                })
            }
            ItemKind::Expr(expr) => expr.summaries.clone(),
            ItemKind::Import(_) | ItemKind::StaticSchema(_) | ItemKind::StaticRecord(_) => {
                CallSummaries::pure_scalar()
            }
        };
        summary = summary.join(&item_summary);
    }
    SemanticSummary::from_call(&summary)
}

pub(super) fn collect_symbol_summaries(module: &hir::Module) -> BTreeMap<String, SemanticSummary> {
    let mut summaries = BTreeMap::new();
    for item in &module.items {
        match &item.kind {
            ItemKind::Function(function) => {
                summaries.insert(
                    function.binding.as_str().to_owned(),
                    SemanticSummary::from_call(&function.summaries),
                );
            }
            ItemKind::Value(value) => {
                let summary = value
                    .value
                    .as_ref()
                    .map_or_else(CallSummaries::pure_scalar, |expr| expr.summaries.clone());
                summaries.insert(
                    value.binding.as_str().to_owned(),
                    SemanticSummary::from_call(&summary),
                );
            }
            ItemKind::Struct(structure) => {
                let summary = structure
                    .checks
                    .iter()
                    .fold(CallSummaries::pure_scalar(), |joined, check| {
                        joined.join(&check.condition.summaries)
                    });
                summaries.insert(
                    structure.binding.as_str().to_owned(),
                    SemanticSummary::from_call(&summary),
                );
            }
            _ => {}
        }
    }
    summaries
}

pub(super) fn collect_references(analysis: &Analysis) -> BTreeMap<String, Vec<Span>> {
    let module = &analysis.hir;
    let mut references = BTreeMap::<String, Vec<Span>>::new();
    for item in &module.items {
        match &item.kind {
            ItemKind::Value(value) => {
                if let Some(expression) = &value.value {
                    collect_expr_references(expression, &mut references);
                }
            }
            ItemKind::Function(function) => {
                for decorator in &function.decorators {
                    collect_expr_references(decorator, &mut references);
                }
                collect_expr_references(&function.body, &mut references);
            }
            ItemKind::Struct(structure) => {
                for decorator in &structure.decorators {
                    collect_expr_references(decorator, &mut references);
                }
                for field in &structure.fields {
                    if let Some(default) = &field.default {
                        collect_expr_references(default, &mut references);
                    }
                }
                for check in &structure.checks {
                    collect_expr_references(&check.condition, &mut references);
                    if let Some(message) = &check.message {
                        collect_expr_references(message, &mut references);
                    }
                }
            }
            ItemKind::Expr(expression) => collect_expr_references(expression, &mut references),
            ItemKind::Import(_) | ItemKind::StaticSchema(_) | ItemKind::StaticRecord(_) => {}
        }
    }
    let mut targets = module
        .bindings
        .iter()
        .map(|binding| (binding.name.canonical.clone(), binding.name.id.clone()))
        .collect::<BTreeMap<_, _>>();
    for alias in &module.aliases {
        targets.insert(alias.canonical.clone(), alias.target.clone());
        targets.insert(alias.spelling.clone(), alias.target.clone());
    }
    for item in &analysis.surface.items {
        let crate::ast::ItemKind::PyDecorate(declaration) = &item.kind else {
            continue;
        };
        if let Some(binding) = targets.get(&declaration.target.canonical) {
            references
                .entry(binding.as_str().to_owned())
                .or_default()
                .push(declaration.target_span);
        }
    }
    for spans in references.values_mut() {
        spans.sort_by_key(|span| (span.start, span.end));
        spans.dedup();
    }
    references
}

pub(super) fn collect_expr_references(
    expression: &Expr,
    references: &mut BTreeMap<String, Vec<Span>>,
) {
    if let ExprKind::Binding(binding) = &expression.kind {
        references
            .entry(binding.as_str().to_owned())
            .or_default()
            .push(expression.span);
    }
    match &expression.kind {
        ExprKind::List(items)
        | ExprKind::Vector(items)
        | ExprKind::Set(items)
        | ExprKind::Do(items) => {
            for item in items {
                collect_expr_references(item, references);
            }
        }
        ExprKind::Map(entries) => {
            for (key, value) in entries {
                collect_expr_references(key, references);
                collect_expr_references(value, references);
            }
        }
        ExprKind::Call { callee, arguments } => {
            collect_expr_references(callee, references);
            for argument in arguments {
                match argument {
                    hir::CallArgument::Positional(value)
                    | hir::CallArgument::Keyword { value, .. } => {
                        collect_expr_references(value, references);
                    }
                }
            }
        }
        ExprKind::Operator { operands, .. } => {
            for operand in operands {
                collect_expr_references(operand, references);
            }
        }
        ExprKind::Attribute { value, .. } => collect_expr_references(value, references),
        ExprKind::Index { value, index } => {
            collect_expr_references(value, references);
            collect_expr_references(index, references);
        }
        ExprKind::Let { bindings, body } => {
            for binding in bindings {
                collect_expr_references(&binding.value, references);
            }
            collect_expr_references(body, references);
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_expr_references(condition, references);
            collect_expr_references(then_branch, references);
            collect_expr_references(else_branch, references);
        }
        ExprKind::Lambda { parameters, body } => {
            for parameter in parameters {
                if let Some(default) = &parameter.default {
                    collect_expr_references(default, references);
                }
            }
            collect_expr_references(body, references);
        }
        ExprKind::Try {
            body,
            catches,
            finally_body,
        } => {
            collect_expr_references(body, references);
            for catch in catches {
                collect_expr_references(&catch.body, references);
            }
            if let Some(finally_body) = finally_body {
                collect_expr_references(finally_body, references);
            }
        }
        ExprKind::Raise(value) => {
            if let Some(value) = value {
                collect_expr_references(value, references);
            }
        }
        ExprKind::None
        | ExprKind::Bool(_)
        | ExprKind::Integer(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Binding(_)
        | ExprKind::Error => {}
    }
}

pub(super) fn collect_records(module: &hir::Module) -> Vec<SemanticRecord> {
    module
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::StaticRecord(record) => Some(SemanticRecord {
                schema: record.schema.canonical.clone(),
                owner: record.owner.canonical.clone(),
                fields: json!(
                    record
                        .fields
                        .iter()
                        .map(|(name, value)| (name.canonical.clone(), form_json_expr(value)))
                        .collect::<BTreeMap<_, _>>()
                ),
                span: record.span,
                metadata: metadata_entries(&record.metadata),
                raw: serde_json::to_value(record).unwrap_or(JsonValue::Null),
            }),
            _ => None,
        })
        .collect()
}

pub(super) fn records_for_binding(
    records: &[SemanticRecord],
    canonical: &str,
) -> Vec<SemanticRecord> {
    records
        .iter()
        .filter(|record| record.owner == canonical)
        .cloned()
        .collect()
}

pub(super) fn form_json(form: &Form) -> JsonValue {
    serde_json::to_value(form).unwrap_or(JsonValue::Null)
}

pub(super) fn form_json_expr(expression: &ast::Expr) -> JsonValue {
    serde_json::to_value(expression).unwrap_or(JsonValue::Null)
}

pub(super) fn form_text(form: &Form) -> String {
    match &form.kind {
        FormKind::None => "none".to_owned(),
        FormKind::Bool(value) => value.to_string(),
        FormKind::Integer(value) | FormKind::Float(value) => value.clone(),
        FormKind::String(value) => value.clone(),
        FormKind::Keyword(name) | FormKind::Symbol(name) => name.spelling.clone(),
        FormKind::Error(message) => format!("#<error:{message}>"),
        FormKind::List(items) => format!(
            "({})",
            items.iter().map(form_text).collect::<Vec<_>>().join(" ")
        ),
        FormKind::Vector(items) => format!(
            "[{}]",
            items.iter().map(form_text).collect::<Vec<_>>().join(" ")
        ),
        FormKind::Map(items) => format!(
            "{{{}}}",
            items.iter().map(form_text).collect::<Vec<_>>().join(" ")
        ),
        FormKind::Set(items) => format!(
            "#{{{}}}",
            items.iter().map(form_text).collect::<Vec<_>>().join(" ")
        ),
        FormKind::ReaderMacro { form, .. } => form_text(form),
    }
}
