use std::{fmt::Write as _, path::Path};

use oxilangtag::LanguageTag;
use serde_json::{Value as JsonValue, json};

use super::*;
use crate::lsp::{Location, LspState, Position};

mod support;

use support::*;

const LSC_SCHEMA: &str = "osiris.lsc/v1";

#[derive(Clone, Copy, Eq, PartialEq)]
enum LscFormat {
    Text,
    Json,
}

struct LscRequest {
    operation: String,
    arguments: Vec<String>,
    locale: Option<String>,
    format: LscFormat,
}

pub(super) fn run_lsc(arguments: &[String]) -> CliOutcome {
    let request = match parse_request(arguments) {
        Ok(request) => request,
        Err(message) => return CliOutcome::usage_error(message),
    };
    match execute(&request) {
        Ok((result, text, failed)) => {
            render_result(&request.operation, result, text, failed, request.format)
        }
        Err(message) => CliOutcome::failure(1, String::new(), format!("osr: {message}\n")),
    }
}

fn parse_request(arguments: &[String]) -> Result<LscRequest, String> {
    let Some(operation) = arguments.first() else {
        return Err("missing OPERATION for 'lsc'".to_owned());
    };
    let supported = [
        "diagnostics",
        "hover",
        "completion",
        "signature",
        "definition",
        "references",
        "rename",
        "expand",
        "syntax",
        "semantic",
        "symbol",
    ];
    if !supported.contains(&operation.as_str()) {
        return Err(format!("unknown lsc operation '{operation}'"));
    }
    let mut locale = None;
    let mut format = LscFormat::Text;
    let mut rest = Vec::new();
    let mut index = 1;
    while let Some(argument) = arguments.get(index) {
        match argument.as_str() {
            "--locale" => {
                if locale.is_some() {
                    return Err("duplicate option '--locale' for 'lsc'".to_owned());
                }
                let raw = arguments
                    .get(index + 1)
                    .ok_or_else(|| "missing value for '--locale'".to_owned())?;
                let tag = LanguageTag::parse_and_normalize(raw)
                    .map_err(|error| format!("invalid BCP 47 locale '{raw}': {error}"))?;
                locale = Some(tag.to_string());
                index += 1;
            }
            "--format" => {
                let raw = arguments
                    .get(index + 1)
                    .ok_or_else(|| "missing value for '--format'".to_owned())?;
                format = match raw.as_str() {
                    "text" => LscFormat::Text,
                    "json" => LscFormat::Json,
                    _ => return Err("--format must be 'text' or 'json'".to_owned()),
                };
                index += 1;
            }
            _ => rest.push(argument.clone()),
        }
        index += 1;
    }
    Ok(LscRequest {
        operation: operation.clone(),
        arguments: rest,
        locale,
        format,
    })
}

fn execute(request: &LscRequest) -> Result<(JsonValue, String, bool), String> {
    match request.operation.as_str() {
        "diagnostics" => diagnostics(request),
        "hover" => hover(request),
        "completion" => positioned(request, |state, uri, position, locale| {
            let value = serde_json::to_value(state.completion(uri, position, locale))
                .map_err(|error| error.to_string())?;
            let text = value
                .as_array()
                .into_iter()
                .flatten()
                .map(|item| {
                    format!(
                        "{}\t{}\n",
                        item["label"].as_str().unwrap_or(""),
                        item["detail"].as_str().unwrap_or("")
                    )
                })
                .collect();
            Ok((value, text))
        }),
        "signature"
            if request
                .arguments
                .first()
                .is_some_and(|value| value == "--at") =>
        {
            positioned(request, |state, uri, position, locale| {
                let result = state.signature_help(uri, position, locale);
                let text = result
                    .as_ref()
                    .map(|help| {
                        help.signatures
                            .iter()
                            .map(|signature| format!("{}\n", signature.label))
                            .collect()
                    })
                    .unwrap_or_default();
                serde_json::to_value(result)
                    .map(|value| (value, text))
                    .map_err(|error| error.to_string())
            })
        }
        "signature" => standard_api_query(request, "API-NAME-OR-BINDING-ID", true),
        "definition"
            if request
                .arguments
                .first()
                .is_some_and(|value| value == "--at") =>
        {
            positioned(request, |state, uri, position, _| {
                location_result(state.definition(uri, position))
            })
        }
        "definition" => standard_definition(request),
        "references" => positioned(request, |state, uri, position, _| {
            locations_result(state.references(uri, position))
        }),
        "rename" => rename(request),
        "expand" => source_view(request, SourceView::Expand),
        "syntax" => source_view(request, SourceView::Syntax),
        "semantic" => source_view(request, SourceView::Semantic),
        "symbol" => symbol(request),
        _ => unreachable!("operation was validated"),
    }
}

fn diagnostics(request: &LscRequest) -> Result<(JsonValue, String, bool), String> {
    let path = optional_single_path(&request.arguments)?.unwrap_or_else(|| ".".to_owned());
    let (state, uri) = open(&path)?;
    let result = state
        .diagnostics(&uri)
        .ok_or_else(|| "document analysis is unavailable".to_owned())?;
    let mut text = String::new();
    for diagnostic in &result.diagnostics {
        let _ = writeln!(
            text,
            "{}:{}:{} {} {}",
            uri,
            diagnostic.range.start.line + 1,
            diagnostic.range.start.character + 1,
            diagnostic.code,
            diagnostic.message
        );
    }
    let failed = result
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == 1);
    let value = serde_json::to_value(result).map_err(|error| error.to_string())?;
    Ok((value, text, failed))
}

fn hover(request: &LscRequest) -> Result<(JsonValue, String, bool), String> {
    if request
        .arguments
        .first()
        .is_some_and(|value| value == "--at")
    {
        return positioned(request, |state, uri, position, locale| {
            let hover = state.hover(uri, position, locale);
            let text = hover
                .as_ref()
                .map(|hover| markdown_hover_to_plain(&hover.contents.value))
                .unwrap_or_default();
            let symbol = state
                .semantic_symbol_at(uri, position)
                .ok_or_else(|| "no symbol exists at the requested position".to_owned())?;
            if let Some(standard) = crate::stdlib::query_api(&symbol.binding_id, locale)
                .into_iter()
                .next()
            {
                let mut value =
                    serde_json::to_value(standard).map_err(|error| error.to_string())?;
                value["range"] = serde_json::to_value(hover.and_then(|value| value.range))
                    .map_err(|error| error.to_string())?;
                return Ok((value, text));
            }

            let document = state
                .semantic_document(uri)
                .ok_or_else(|| "document analysis is unavailable".to_owned())?;
            let mut value = serde_json::to_value(symbol).map_err(|error| error.to_string())?;
            let object = value
                .as_object_mut()
                .ok_or_else(|| "semantic symbol did not serialize as an object".to_owned())?;
            let (selected_doc, doc_locale) = symbol.documentation.for_locale(locale);
            let (selected_name, name_locale) = symbol.names.for_locale(locale);
            let requested_locale = locale.map(ToOwned::to_owned);
            let available_locales = symbol
                .documentation
                .translations
                .keys()
                .chain(symbol.names.localized.keys())
                .cloned()
                .collect::<std::collections::BTreeSet<_>>();
            object.insert("schema".to_owned(), json!("osiris.local-symbol/v1"));
            object.insert("module".to_owned(), json!(document.module));
            object.insert(
                "documentVersion".to_owned(),
                json!(document.document_version),
            );
            object.insert("provenance".to_owned(), json!("workspace-source"));
            object.insert("requestedLocale".to_owned(), json!(requested_locale));
            object.insert("resolvedLocale".to_owned(), json!(doc_locale));
            object.insert("selectedDocumentation".to_owned(), json!(selected_doc));
            object.insert("label".to_owned(), json!(selected_name));
            object.insert("resolvedNameLocale".to_owned(), json!(name_locale));
            object.insert("availableLocales".to_owned(), json!(available_locales));
            if let Some(documentation) = object
                .get_mut("documentation")
                .and_then(JsonValue::as_object_mut)
            {
                documentation.insert(
                    "selection".to_owned(),
                    json!({
                        "requestedLocale": locale,
                        "resolvedLocale": doc_locale,
                        "text": selected_doc,
                    }),
                );
            }
            if let Some(names) = object.get_mut("names").and_then(JsonValue::as_object_mut) {
                names.insert(
                    "selection".to_owned(),
                    json!({
                        "requestedLocale": locale,
                        "resolvedLocale": name_locale,
                        "label": selected_name,
                    }),
                );
            }
            Ok((value, text))
        });
    }
    let query = required_single(&request.arguments, "API-NAME-OR-BINDING-ID")?;
    let standard = crate::stdlib::query_api(query, request.locale.as_deref());
    if !standard.is_empty() {
        return render_standard_api(standard, false);
    }
    symbol_query(query, request.locale.as_deref())
}

fn symbol(request: &LscRequest) -> Result<(JsonValue, String, bool), String> {
    let query = required_single(&request.arguments, "NAME-OR-BINDING-ID")?;
    symbol_query(query, request.locale.as_deref())
}

fn symbol_query(query: &str, locale: Option<&str>) -> Result<(JsonValue, String, bool), String> {
    let standard = crate::stdlib::query_api(query, locale);
    if !standard.is_empty() {
        return render_standard_api(standard, false);
    }
    let (mut state, uri) = open(".")?;
    if let Some(locale) = locale {
        state.set_display_locale(locale);
    }
    let symbols = state.symbols(&uri, Some(query)).unwrap_or_default();
    let mut text = String::new();
    for symbol in &symbols {
        let _ = writeln!(
            text,
            "{}\t{}\t{}",
            symbol["binding_id"].as_str().unwrap_or(""),
            symbol["canonical"].as_str().unwrap_or(""),
            symbol["type"].as_str().unwrap_or("")
        );
    }
    Ok((JsonValue::Array(symbols), text, false))
}

fn standard_api_query(
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

fn standard_definition(request: &LscRequest) -> Result<(JsonValue, String, bool), String> {
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

fn render_standard_api(
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

fn markdown_hover_to_plain(markdown: &str) -> String {
    let mut plain = String::new();
    let mut in_code = false;
    for line in markdown.lines() {
        if line.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        let line = line.replace("**", "").replace('`', "");
        if in_code && !line.is_empty() {
            plain.push_str("  ");
        }
        plain.push_str(&line);
        plain.push('\n');
    }
    plain
}

fn positioned<F>(request: &LscRequest, query: F) -> Result<(JsonValue, String, bool), String>
where
    F: FnOnce(&LspState, &str, Position, Option<&str>) -> Result<(JsonValue, String), String>,
{
    let at = parse_at_only(&request.arguments)?;
    let (state, uri) = open(&at.path)?;
    let (value, text) = query(&state, &uri, at.position, request.locale.as_deref())?;
    Ok((value, text, false))
}

fn rename(request: &LscRequest) -> Result<(JsonValue, String, bool), String> {
    let (at, new_name) = parse_rename_arguments(&request.arguments)?;
    let (state, uri) = open(&at.path)?;
    let edit = state
        .rename(&uri, at.position, &new_name)
        .map_err(|error| error.to_string())?;
    let value = serde_json::to_value(&edit).map_err(|error| error.to_string())?;
    let text = edit.as_ref().map(render_workspace_edit).unwrap_or_default();
    Ok((value, text, false))
}

enum SourceView {
    Expand,
    Syntax,
    Semantic,
}

fn source_view(
    request: &LscRequest,
    view: SourceView,
) -> Result<(JsonValue, String, bool), String> {
    let path = required_single(&request.arguments, "PATH")?;
    let (state, uri) = open(path)?;
    let document = state
        .document(&uri)
        .ok_or_else(|| "document analysis is unavailable".to_owned())?;
    let (value, text) = match view {
        SourceView::Expand => {
            let preview = state
                .expand_preview(&uri)
                .ok_or_else(|| "expansion is unavailable".to_owned())?;
            (
                serde_json::to_value(&preview).map_err(|error| error.to_string())?,
                preview.text,
            )
        }
        SourceView::Syntax => (
            json!({
                "version": document.analysis.document.format_version,
                "documentVersion": document.version,
                "source": document.text,
                "tokens": document.analysis.document.tokens,
                "forms": document.analysis.document.forms,
                "nodes": document.analysis.document.nodes,
                "diagnostics": document.analysis.document.diagnostics,
            }),
            document.text.clone(),
        ),
        SourceView::Semantic => (
            serde_json::to_value(&document.semantic).map_err(|error| error.to_string())?,
            render_semantic(&document.semantic),
        ),
    };
    Ok((value, text, document.analysis.has_errors()))
}

fn open(path: &str) -> Result<(LspState, String), String> {
    let path = select_source(Path::new(path))?;
    let canonical = fs::canonicalize(&path)
        .map_err(|error| format!("could not resolve '{}': {error}", path.display()))?;
    let source = fs::read_to_string(&canonical)
        .map_err(|error| format!("could not read '{}': {error}", canonical.display()))?;
    let uri = format!("file://{}", canonical.display());
    let mut state = LspState::new();
    // LSC without --locale selects the authored :default slot and must not
    // inherit a project's displayLocale. `und` deliberately misses tagged
    // translations while remaining a valid internal BCP 47 request.
    state.set_display_locale("und");
    state.did_open(&uri, 1, source);
    Ok((state, uri))
}

fn select_source(path: &Path) -> Result<PathBuf, String> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    let project = ProjectConfig::discover(path).map_err(|error| error.to_string())?;
    first_project_source(&project)
}
