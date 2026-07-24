use super::*;

pub(super) fn render_standard_hover(
    records: Vec<crate::stdlib::StandardApiSelection>,
) -> Result<(JsonValue, String, bool), String> {
    let (_, text, _) = render_standard_api(records.clone(), false)?;
    let projections = records
        .into_iter()
        .map(|record| {
            json!({
                "schema": "osiris.hover/v1",
                "bindingId": record.api.binding_id,
                "kind": record.api.kind,
                "label": record.label,
                "canonical": {
                    "name": record.api.canonical,
                    "qualified": format!("{}/{}", record.api.namespace, record.api.canonical),
                },
                "documentation": {
                    "default": record.api.documentation.default,
                    "translations": record.api.documentation.translations,
                    "selection": {
                        "requestedLocale": record.requested_locale,
                        "resolvedLocale": record.resolved_locale,
                        "text": record.selected_documentation,
                    },
                },
                "usage": record.api.call_shapes,
                "examples": record.api.examples,
                "type": record.api.signature,
                "source": {
                    "uri": record.api.source.uri,
                    "line": record.api.source.line,
                    "column": record.api.source.column,
                    "provenance": record.provenance,
                },
                "semantic": {
                    "effects": record.api.effects,
                    "evaluation": record.api.evaluation,
                    "exceptions": record.api.exceptions,
                },
                "authoredSpelling": null,
            })
        })
        .collect::<Vec<_>>();
    Ok((
        if projections.len() == 1 {
            projections.into_iter().next().expect("one projection")
        } else {
            JsonValue::Array(projections)
        },
        text,
        false,
    ))
}

pub(super) fn standard_api_query(
    request: &LscRequest,
    label: &str,
    signatures_only: bool,
) -> Result<(JsonValue, String, bool), String> {
    let query = required_single(&request.arguments, label)?;
    let records = crate::stdlib::query_api(query, request.locale.as_deref());
    if records.is_empty() {
        return Err(format!("standard API `{query}` was not found"));
    }
    render_standard_api(records, signatures_only)
}

pub(super) fn standard_definition(
    request: &LscRequest,
) -> Result<(JsonValue, String, bool), String> {
    let query = required_single(&request.arguments, "API-NAME-OR-BINDING-ID")?;
    let records = crate::stdlib::query_api(query, request.locale.as_deref());
    if records.is_empty() {
        return Err(format!("standard API `{query}` was not found"));
    }
    let locations = records
        .into_iter()
        .map(|record| Location {
            uri: record.api.source.uri,
            range: crate::lsp::Range {
                start: Position {
                    line: record.api.source.line.saturating_sub(1),
                    character: record.api.source.column.saturating_sub(1),
                },
                end: Position {
                    line: record.api.source.line.saturating_sub(1),
                    character: record.api.source.column.saturating_sub(1)
                        + record.api.canonical.chars().count() as u32,
                },
            },
        })
        .collect::<Vec<_>>();
    let text = locations.iter().map(render_location).collect();
    Ok((
        serde_json::to_value(locations).map_err(|error| error.to_string())?,
        text,
        false,
    ))
}

pub(super) fn render_standard_api(
    records: Vec<crate::stdlib::StandardApiSelection>,
    signatures_only: bool,
) -> Result<(JsonValue, String, bool), String> {
    let mut text = String::new();
    for record in &records {
        let locale = record
            .resolved_locale
            .as_deref()
            .or(record.requested_locale.as_deref())
            .unwrap_or("default");
        let chinese = locale == "zh" || locale.starts_with("zh-");
        if signatures_only {
            for shape in &record.api.call_shapes {
                let _ = writeln!(text, "{shape}");
            }
            let _ = writeln!(text, "type: {}", record.api.signature);
        } else {
            let kind = if chinese && record.api.kind == crate::name::BindingKind::Function {
                "函数".to_owned()
            } else {
                format!("{:?}", record.api.kind)
            };
            let _ = writeln!(text, "{} · {}", record.label, kind);
            let _ = writeln!(text, "\n{}", record.selected_documentation);
            if !record.api.call_shapes.is_empty() {
                text.push_str(if chinese { "\n用法\n" } else { "\nUsage\n" });
                for shape in &record.api.call_shapes {
                    let _ = writeln!(text, "  {shape}");
                }
            }
            if !record.api.examples.is_empty() {
                text.push_str(if chinese {
                    "\n示例\n"
                } else {
                    "\nExamples\n"
                });
                for example in &record.api.examples {
                    for line in example {
                        let _ = writeln!(text, "  {line}");
                    }
                    text.push('\n');
                }
            }
            let type_heading = if chinese { "类型" } else { "Type" };
            let _ = writeln!(text, "{type_heading}\n  {}", record.api.signature);
            if let Some(behavior) = lsc_evaluation_behavior(record.api.evaluation, chinese) {
                let heading = if chinese { "行为" } else { "Behavior" };
                let _ = writeln!(text, "\n{heading}\n  {behavior}");
            }
            let canonical_heading = if chinese {
                "规范名称"
            } else {
                "Canonical name"
            };
            let _ = writeln!(
                text,
                "\n{canonical_heading}\n  {}/{}",
                record.api.namespace, record.api.canonical
            );
        }
        if records.len() > 1 {
            text.push('\n');
        }
    }
    serde_json::to_value(records)
        .map(|value| (value, text, false))
        .map_err(|error| error.to_string())
}

fn lsc_evaluation_behavior(evaluation: &str, chinese: bool) -> Option<&'static str> {
    match (evaluation, chinese) {
        ("consumer", true) => Some("立即消费输入集合。"),
        ("consumer", false) => Some("Consumes its input eagerly."),
        ("lazy", true) => Some("按需生成结果。"),
        ("lazy", false) => Some("Produces results lazily."),
        _ => None,
    }
}
