use std::{fmt::Write as _, path::Path};

use oxilangtag::LanguageTag;
use serde_json::{Value as JsonValue, json};

use super::*;
use crate::lsp::{Location, LspState, Position};

mod standard;
pub(super) mod support;

use standard::*;
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
        "workspace-search",
        "symbol-context",
        "source-context",
        "cache",
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
        "workspace-search" => graph_workspace_search(request),
        "symbol-context" => graph_symbol_context(request),
        "source-context" => graph_source_context(request),
        "cache" => graph_cache(request),
        _ => unreachable!("operation was validated"),
    }
}

fn graph_cache(request: &LscRequest) -> Result<(JsonValue, String, bool), String> {
    let report = match request.arguments.as_slice() {
        [operation] if operation == "status" => {
            crate::lsc::WorkspaceService::cache_status(Path::new("."))?
        }
        [operation] if operation == "rebuild" => {
            crate::lsc::WorkspaceService::rebuild_cache(Path::new("."), request.locale.as_deref())?
        }
        [] => return Err("cache requires 'status' or 'rebuild'".to_owned()),
        [operation] => return Err(format!("unknown lsc cache operation '{operation}'")),
        _ => return Err("cache accepts exactly one operation: 'status' or 'rebuild'".to_owned()),
    };
    let value = serde_json::to_value(&report).map_err(|error| error.to_string())?;
    let text = format!(
        "cache {}: {} ({} inputs, {} reused, {} hashed)\n",
        report.status, report.path, report.input_count, report.reused_hashes, report.hashed_inputs
    );
    Ok((value, text, false))
}

fn graph_workspace_search(request: &LscRequest) -> Result<(JsonValue, String, bool), String> {
    let query = required_single(&request.arguments, "QUERY")?;
    let mut service =
        crate::lsc::WorkspaceService::open(Path::new("."), request.locale.as_deref())?;
    render_tool_result(service.workspace_search(query, None))
}

fn graph_symbol_context(request: &LscRequest) -> Result<(JsonValue, String, bool), String> {
    let mut service =
        crate::lsc::WorkspaceService::open(Path::new("."), request.locale.as_deref())?;
    let result = if request
        .arguments
        .first()
        .is_some_and(|value| value == "--at")
    {
        let at = parse_at_only(&request.arguments)?;
        service.position_context(&crate::lsc::SourcePosition {
            path: at.path.into(),
            line: at.position.line + 1,
            column: at.position.character + 1,
        })
    } else {
        service.symbol_context(required_single(&request.arguments, "API-NAME")?)
    };
    render_tool_result(result)
}

fn graph_source_context(request: &LscRequest) -> Result<(JsonValue, String, bool), String> {
    let at = parse_at_only(&request.arguments)?;
    let mut service =
        crate::lsc::WorkspaceService::open(Path::new("."), request.locale.as_deref())?;
    render_tool_result(service.position_context(&crate::lsc::SourcePosition {
        path: at.path.into(),
        line: at.position.line + 1,
        column: at.position.character + 1,
    }))
}

fn render_tool_result(result: crate::lsc::ToolResult) -> Result<(JsonValue, String, bool), String> {
    let failed = matches!(result.status.as_str(), "error" | "unavailable");
    let value = serde_json::to_value(&result).map_err(|error| error.to_string())?;
    let text = serde_json::to_string_pretty(&value).map_err(|error| error.to_string())? + "\n";
    Ok((value, text, failed))
}

fn diagnostics(request: &LscRequest) -> Result<(JsonValue, String, bool), String> {
    let path = optional_single_path(&request.arguments)?.unwrap_or_else(|| ".".to_owned());
    let (mut state, uri) = open(&path)?;
    if let Some(locale) = &request.locale {
        state.set_display_locale(locale);
    }
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
            let value = state
                .hover_machine_projection(uri, position, locale)
                .ok_or_else(|| "no symbol exists at the requested position".to_owned())?;
            Ok((value, text))
        });
    }
    let query = required_single(&request.arguments, "API-NAME-OR-BINDING-ID")?;
    let standard = crate::stdlib::query_api(query, request.locale.as_deref());
    if !standard.is_empty() {
        return render_standard_hover(standard);
    }
    symbol_query(query, request.locale.as_deref(), true)
}

fn symbol(request: &LscRequest) -> Result<(JsonValue, String, bool), String> {
    let query = required_single(&request.arguments, "NAME-OR-BINDING-ID")?;
    symbol_query(query, request.locale.as_deref(), false)
}

fn symbol_query(
    query: &str,
    locale: Option<&str>,
    hover: bool,
) -> Result<(JsonValue, String, bool), String> {
    let standard = crate::stdlib::query_api(query, locale);
    if !standard.is_empty() {
        return if hover {
            render_standard_hover(standard)
        } else {
            render_standard_api(standard, false)
        };
    }
    let (mut state, uri) = open(".")?;
    if let Some(locale) = locale {
        state.set_display_locale(locale);
    }
    let symbols = state.symbols(&uri, Some(query)).unwrap_or_default();
    if hover && symbols.len() == 1 {
        let binding_id = symbols[0]["binding_id"]
            .as_str()
            .ok_or_else(|| "matched symbol has no stable binding identity".to_owned())?;
        let (hover, mut machine) = state
            .hover_for_binding(&uri, binding_id, locale)
            .ok_or_else(|| "matched symbol has no hover projection".to_owned())?;
        let mut human = markdown_hover_to_plain(&hover.contents.value);
        let queried = queried_spelling(&symbols[0], query, &machine);
        if queried["role"] == "migration" {
            let replacement = queried["replacement"].as_str().unwrap_or_default();
            if locale.is_some_and(|locale| locale == "zh" || locale.starts_with("zh-")) {
                let _ = writeln!(
                    human,
                    "\n迁移提示\n  {query} 是兼容旧源码的别名；请改用 {replacement}。"
                );
            } else {
                let _ = writeln!(
                    human,
                    "\nMigration\n  {query} is a source-compatibility alias; use {replacement}."
                );
            }
        }
        machine["queriedSpelling"] = queried;
        return Ok((machine, human, false));
    }
    if hover && symbols.len() > 1 {
        let candidates = symbols
            .iter()
            .map(|symbol| {
                json!({
                    "bindingId": symbol["binding_id"],
                    "canonical": symbol["canonical"],
                    "kind": symbol["kind"],
                })
            })
            .collect::<Vec<_>>();
        let mut text = format!("ambiguous name `{query}`; candidates:\n");
        for candidate in &candidates {
            let _ = writeln!(
                text,
                "  {}",
                candidate["bindingId"].as_str().unwrap_or_default()
            );
        }
        return Ok((
            json!({"ambiguous": true, "candidates": candidates}),
            text,
            false,
        ));
    }
    if hover && symbols.is_empty() {
        return Err(format!("symbol `{query}` was not found"));
    }
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

fn queried_spelling(symbol: &JsonValue, query: &str, hover: &JsonValue) -> JsonValue {
    let spelling = query
        .rmatch_indices(['/', '.'])
        .next()
        .map_or(query, |(index, separator)| {
            &query[index + separator.len()..]
        });
    let canonical = symbol["canonical"].as_str().unwrap_or_default();
    let mut role = (spelling == canonical).then_some("canonical");
    if let Some(localized) = symbol["names"]["localized"].as_object() {
        for entry in localized.values() {
            if entry["preferred"] == spelling {
                role = Some("preferred");
                break;
            }
            if entry["aliases"]
                .as_array()
                .is_some_and(|aliases| aliases.iter().any(|alias| alias == spelling))
            {
                role = Some("migration");
                break;
            }
        }
    }
    if role.is_none()
        && let Some(alias) = symbol["aliases"].as_array().and_then(|aliases| {
            aliases
                .iter()
                .find(|alias| alias["spelling"] == spelling || alias["canonical"] == spelling)
        })
    {
        role = alias["role"].as_str();
    }
    let replacement = (role == Some("migration"))
        .then(|| hover["label"].as_str().unwrap_or(canonical).to_owned());
    json!({
        "text": spelling,
        "role": role.unwrap_or("canonical"),
        "replacement": replacement,
    })
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
