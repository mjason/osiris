use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{
    lsc::{ToolResult, WorkspaceService},
    project::AgentConfig,
};

use super::{
    LanguageServiceEvidence,
    client::{call_native_chat, call_provider, supports_native_tools},
    model_json_text,
};

const MAX_TOOL_ROUNDS: usize = 4;
const MAX_TOOL_CALLS_PER_ROUND: usize = 4;
const MAX_TOTAL_TOOL_CALLS: usize = 8;
const MAX_TOOL_EVIDENCE_BYTES: usize = 384 * 1024;

pub(super) enum WorkspaceToolService {
    Pending { root: PathBuf, locale: String },
    Ready(Box<WorkspaceService>),
    Unavailable(String),
}

impl WorkspaceToolService {
    pub(super) fn pending(root: &Path, locale: &str) -> Self {
        Self::Pending {
            root: root.to_path_buf(),
            locale: locale.to_owned(),
        }
    }

    #[cfg(test)]
    pub(super) fn unavailable(message: impl Into<String>) -> Self {
        Self::Unavailable(message.into())
    }

    pub(super) fn get(&mut self) -> Result<&mut WorkspaceService, String> {
        if let Self::Pending { root, locale } = self {
            match WorkspaceService::open(root, Some(locale)) {
                Ok(service) => *self = Self::Ready(Box::new(service)),
                Err(error) => *self = Self::Unavailable(error),
            }
        }
        match self {
            Self::Ready(service) => Ok(service.as_mut()),
            Self::Unavailable(error) => Err(error.clone()),
            Self::Pending { .. } => unreachable!("pending service was initialized"),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolCallEnvelope {
    tool_calls: Vec<LsaToolCall>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LsaToolCall {
    pub(super) id: String,
    pub(super) operation: String,
    #[serde(default)]
    pub(super) arguments: serde_json::Value,
}

pub(super) fn run_tool_loop(
    config: &AgentConfig,
    api_key: &str,
    base_prompt: &str,
    service: &mut WorkspaceToolService,
    evidence: &mut Vec<LanguageServiceEvidence>,
) -> Result<String, String> {
    if supports_native_tools(config) {
        return run_native_tool_loop(config, api_key, base_prompt, service, evidence);
    }
    let mut prompt = base_prompt.to_owned();
    let initial_evidence = evidence.len();
    for round in 0..=MAX_TOOL_ROUNDS {
        let model_text = call_provider(config, api_key, &prompt)?;
        let Some(calls) = parse_tool_calls(&model_text)? else {
            return Ok(model_text);
        };
        if round == MAX_TOOL_ROUNDS {
            return Err(format!(
                "LSA exceeded the bounded limit of {MAX_TOOL_ROUNDS} language-service rounds"
            ));
        }
        if calls.is_empty() {
            return Err(
                "LLM returned an empty toolCalls list instead of a final answer".to_owned(),
            );
        }
        if calls.len() > MAX_TOOL_CALLS_PER_ROUND {
            return Err(format!(
                "LLM requested {} tools in one round; the limit is {MAX_TOOL_CALLS_PER_ROUND}",
                calls.len()
            ));
        }
        if evidence.len().saturating_sub(initial_evidence) + calls.len() > MAX_TOTAL_TOOL_CALLS {
            return Err(format!(
                "LLM exceeded the bounded limit of {MAX_TOTAL_TOOL_CALLS} language-service calls"
            ));
        }
        for call in calls {
            if call.id.trim().is_empty() {
                return Err("LLM language-service tool call has an empty id".to_owned());
            }
            if evidence.iter().any(|item| item.call_id == call.id) {
                return Err(format!(
                    "LLM reused language-service tool call id `{}`",
                    call.id
                ));
            }
            let result = compact_tool_result(execute_tool(service, &call, evidence));
            evidence.push(evidence_from_result(&call.id, result));
            ensure_evidence_limit(evidence)?;
        }
        let serialized = serde_json::to_string_pretty(evidence)
            .map_err(|error| format!("could not encode language-service evidence: {error}"))?;
        prompt = format!(
            "{base_prompt}\n\nLanguage-service tool results from prior rounds (compiler-owned, authoritative, and bounded):\n{serialized}\n\nContinue by returning either another toolCalls object or the final answer object. Do not repeat a completed tool call."
        );
    }
    unreachable!("bounded loop always returns")
}

fn run_native_tool_loop(
    config: &AgentConfig,
    api_key: &str,
    prompt: &str,
    service: &mut WorkspaceToolService,
    evidence: &mut Vec<LanguageServiceEvidence>,
) -> Result<String, String> {
    let initial_evidence = evidence.len();
    let mut continuation = Vec::new();
    for round in 0..=MAX_TOOL_ROUNDS {
        let turn = call_native_chat(config, api_key, prompt, &continuation)?;
        if turn.tool_calls.is_empty() {
            return turn
                .content
                .filter(|content| !content.trim().is_empty())
                .ok_or_else(|| "native tool loop ended without JSON content".to_owned());
        }
        if round == MAX_TOOL_ROUNDS {
            return Err(format!(
                "LSA exceeded the bounded limit of {MAX_TOOL_ROUNDS} language-service rounds"
            ));
        }
        if turn.tool_calls.len() > MAX_TOOL_CALLS_PER_ROUND {
            return Err(format!(
                "LLM requested {} tools in one round; the limit is {MAX_TOOL_CALLS_PER_ROUND}",
                turn.tool_calls.len()
            ));
        }
        if evidence.len().saturating_sub(initial_evidence) + turn.tool_calls.len()
            > MAX_TOTAL_TOOL_CALLS
        {
            return Err(format!(
                "LLM exceeded the bounded limit of {MAX_TOTAL_TOOL_CALLS} language-service calls"
            ));
        }
        continuation.push(serde_json::json!({
            "role": "assistant",
            "content": turn.content,
            "tool_calls": &turn.tool_calls,
        }));
        for native in turn.tool_calls {
            let operation = match native.function.name.as_str() {
                "workspace_search" => "workspace-search",
                "symbol_context" => "symbol-context",
                "source_context" => "source-context",
                name => {
                    return Err(format!("LLM requested unknown native tool `{name}`"));
                }
            };
            let arguments = serde_json::from_str(&native.function.arguments).map_err(|error| {
                format!(
                    "LLM returned invalid arguments for native tool `{}`: {error}",
                    native.function.name
                )
            })?;
            let call = LsaToolCall {
                id: native.id.clone(),
                operation: operation.to_owned(),
                arguments,
            };
            if evidence.iter().any(|item| item.call_id == call.id) {
                return Err(format!(
                    "LLM reused language-service tool call id `{}`",
                    call.id
                ));
            }
            let result = compact_tool_result(execute_tool(service, &call, evidence));
            let content = serde_json::to_string(&result)
                .map_err(|error| format!("could not encode native tool result: {error}"))?;
            evidence.push(evidence_from_result(&call.id, result));
            ensure_evidence_limit(evidence)?;
            continuation.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": native.id,
                "content": content,
            }));
        }
    }
    unreachable!("bounded loop always returns")
}

fn ensure_evidence_limit(evidence: &[LanguageServiceEvidence]) -> Result<(), String> {
    if serde_json::to_vec(evidence).is_ok_and(|encoded| encoded.len() > MAX_TOOL_EVIDENCE_BYTES) {
        Err(format!(
            "language-service evidence exceeded the {} KiB limit",
            MAX_TOOL_EVIDENCE_BYTES / 1024
        ))
    } else {
        Ok(())
    }
}

fn compact_tool_result(mut result: ToolResult) -> ToolResult {
    result.result = match result.operation.as_str() {
        "workspace-search" => result
            .result
            .as_array()
            .map(|values| {
                serde_json::Value::Array(
                    values
                        .iter()
                        .take(4)
                        .map(compact_symbol_candidate)
                        .collect(),
                )
            })
            .unwrap_or(result.result),
        "symbol-context" => compact_symbol_context(&result.result),
        _ => result.result,
    };
    result
}

fn compact_symbol_candidate(value: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "id": value.get("id"),
        "name": value.get("name"),
        "module": value.get("module"),
        "kind": value.get("kind"),
        "names": value.get("names"),
        "aliases": value.get("aliases"),
        "type": value.get("type"),
        "documentation": value.get("documentation"),
        "examples": value.get("examples"),
        "location": value.get("location"),
        "bindingId": value.pointer("/data/bindingId"),
    })
}

fn compact_symbol_context(value: &serde_json::Value) -> serde_json::Value {
    let context = value.get("context");
    let references = context
        .and_then(|context| context.get("references"))
        .and_then(|references| references.get("items"))
        .and_then(serde_json::Value::as_array)
        .map(|items| serde_json::Value::Array(items.iter().take(8).cloned().collect()))
        .unwrap_or_else(|| serde_json::json!([]));
    serde_json::json!({
        "query": value.get("query"),
        "candidate": value.get("candidate").map(compact_symbol_candidate),
        "context": {
            "hover": context.and_then(|value| value.get("hover")),
            "signatureHelp": context.and_then(|value| value.get("signatureHelp")),
            "definition": context.and_then(|value| value.get("definition")),
            "definitionSource": context.and_then(|value| value.get("definitionSource")),
            "references": references,
        }
    })
}

pub(super) fn parse_tool_calls(text: &str) -> Result<Option<Vec<LsaToolCall>>, String> {
    let json = model_json_text(text)?;
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|error| format!("LLM returned invalid LSA JSON: {error}"))?;
    if value.get("toolCalls").is_none() {
        return Ok(None);
    }
    let envelope: ToolCallEnvelope = serde_json::from_value(value)
        .map_err(|error| format!("LLM returned invalid language-service tool calls: {error}"))?;
    Ok(Some(envelope.tool_calls))
}

fn execute_tool(
    service: &mut WorkspaceToolService,
    call: &LsaToolCall,
    evidence: &[LanguageServiceEvidence],
) -> ToolResult {
    let service = match service.get() {
        Ok(service) => service,
        Err(error) => {
            return ToolResult {
                schema: "osiris.lsc-tool/v1".to_owned(),
                operation: call.operation.clone(),
                status: "unavailable".to_owned(),
                result: serde_json::Value::Null,
                message: Some(error),
            };
        }
    };
    match call.operation.as_str() {
        "workspace-search" => {
            let Some(query) = call
                .arguments
                .get("query")
                .and_then(serde_json::Value::as_str)
            else {
                return invalid_tool_arguments(&call.operation, "arguments.query must be a string");
            };
            let limit = call
                .arguments
                .get("limit")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| usize::try_from(value).ok());
            service.workspace_search(query, limit)
        }
        "symbol-context" => {
            let Some(query) = call
                .arguments
                .get("query")
                .and_then(serde_json::Value::as_str)
            else {
                return invalid_tool_arguments(&call.operation, "arguments.query must be a string");
            };
            service.symbol_context(query)
        }
        "source-context" => {
            let Some(uri) = call
                .arguments
                .get("uri")
                .and_then(serde_json::Value::as_str)
            else {
                return invalid_tool_arguments(&call.operation, "arguments.uri must be a string");
            };
            let Some(range) = call.arguments.get("range") else {
                return invalid_tool_arguments(&call.operation, "arguments.range is required");
            };
            if !evidence
                .iter()
                .any(|item| json_contains_source(&item.result, uri, range))
            {
                return invalid_tool_arguments(
                    &call.operation,
                    "source-context requires a URI and range returned by an earlier tool",
                );
            }
            match serde_json::from_value(range.clone()) {
                Ok(range) => service.source_context(uri, range),
                Err(_) => {
                    invalid_tool_arguments(&call.operation, "arguments.range must be an LSP range")
                }
            }
        }
        operation => ToolResult {
            schema: "osiris.lsc-tool/v1".to_owned(),
            operation: operation.to_owned(),
            status: "unavailable".to_owned(),
            result: serde_json::Value::Null,
            message: Some(format!(
                "unknown read-only LSA tool operation `{operation}`"
            )),
        },
    }
}

fn json_contains_source(value: &serde_json::Value, uri: &str, range: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Array(values) => values
            .iter()
            .any(|value| json_contains_source(value, uri, range)),
        serde_json::Value::Object(values) => {
            (values.get("uri").and_then(serde_json::Value::as_str) == Some(uri)
                && values.get("range").is_some_and(|value| value == range))
                || values
                    .values()
                    .any(|value| json_contains_source(value, uri, range))
        }
        _ => false,
    }
}

fn invalid_tool_arguments(operation: &str, message: &str) -> ToolResult {
    ToolResult {
        schema: "osiris.lsc-tool/v1".to_owned(),
        operation: operation.to_owned(),
        status: "error".to_owned(),
        result: serde_json::Value::Null,
        message: Some(message.to_owned()),
    }
}

pub(super) fn evidence_from_result(call_id: &str, result: ToolResult) -> LanguageServiceEvidence {
    LanguageServiceEvidence {
        call_id: call_id.to_owned(),
        operation: result.operation,
        status: result.status,
        result: result.result,
        message: result.message,
    }
}

pub(super) fn collect_source_references(value: &serde_json::Value, references: &mut Vec<String>) {
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                collect_source_references(value, references);
            }
        }
        serde_json::Value::Object(values) => {
            if let Some(uri) = values.get("uri").and_then(serde_json::Value::as_str) {
                references.push(uri.to_owned());
            }
            for value in values.values() {
                collect_source_references(value, references);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_search_compaction_keeps_four_small_candidates() {
        let result = ToolResult {
            schema: "osiris.lsc-tool/v1".to_owned(),
            operation: "workspace-search".to_owned(),
            status: "ok".to_owned(),
            result: serde_json::Value::Array(
                (0..6)
                    .map(|index| {
                        serde_json::json!({
                            "id": index,
                            "name": format!("item-{index}"),
                            "location": {"uri": format!("file:///{index}.osr")},
                            "noise": "discard me"
                        })
                    })
                    .collect(),
            ),
            message: None,
        };

        let compact = compact_tool_result(result);
        let candidates = compact.result.as_array().unwrap();
        assert_eq!(candidates.len(), 4);
        assert!(candidates[0].get("location").is_some());
        assert!(candidates[0].get("noise").is_none());
    }

    #[test]
    fn symbol_context_compaction_keeps_facts_and_bounds_references() {
        let result = ToolResult {
            schema: "osiris.lsc-tool/v1".to_owned(),
            operation: "symbol-context".to_owned(),
            status: "ok".to_owned(),
            result: serde_json::json!({
                "query": "Point",
                "candidate": {"name": "Point", "location": {"uri": "file:///point.osr"}},
                "context": {
                    "hover": {"markdown": "Point docs"},
                    "signatureHelp": {"signatures": ["Point"]},
                    "definition": [{"uri": "file:///point.osr"}],
                    "definitionSource": "(defstruct Point [])",
                    "references": {"items": (0..12).map(|index| serde_json::json!({"line": index})).collect::<Vec<_>>()},
                    "referenceSources": ["large source"],
                    "graphNeighborhood": {"large": true}
                }
            }),
            message: None,
        };

        let compact = compact_tool_result(result);
        let context = compact.result.get("context").unwrap();
        assert!(context.get("hover").is_some());
        assert!(context.get("signatureHelp").is_some());
        assert!(context.get("definition").is_some());
        assert!(context.get("definitionSource").is_some());
        assert_eq!(context["references"].as_array().unwrap().len(), 8);
        assert!(context.get("referenceSources").is_none());
        assert!(context.get("graphNeighborhood").is_none());
    }
}
