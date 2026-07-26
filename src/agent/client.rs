use std::{io::Read, time::Duration};

use serde::{Deserialize, Serialize};

use crate::project::AgentConfig;

const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

pub(super) fn call_provider(
    config: &AgentConfig,
    api_key: &str,
    prompt: &str,
) -> Result<String, String> {
    match config.wire_api.as_str() {
        "responses" => call_responses(config, api_key, prompt),
        "chatCompletions" => call_chat_completions(config, api_key, prompt),
        wire_api => Err(format!(
            "unsupported LSA wire API `{wire_api}`; expected `responses` or `chatCompletions`"
        )),
    }
}

fn call_responses(config: &AgentConfig, api_key: &str, prompt: &str) -> Result<String, String> {
    let body = serde_json::json!({
        "model": config.model,
        "input": [{
            "role": "user",
            "content": [{"type": "input_text", "text": prompt}]
        }],
        "stream": config.stream
    });
    let value = post_json(config, api_key, "responses", body)?;
    extract_responses_text(&value)
}

fn call_chat_completions(
    config: &AgentConfig,
    api_key: &str,
    prompt: &str,
) -> Result<String, String> {
    let body = chat_completions_body(config, prompt);
    let value = post_json(config, api_key, "chat/completions", body)?;
    extract_chat_completions_text(&value)
}

pub(super) fn chat_completions_body(config: &AgentConfig, prompt: &str) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": config.model,
        "messages": [{"role": "user", "content": prompt}],
        "stream": config.stream
    });
    if config.model.to_ascii_lowercase().contains("deepseek") {
        body["thinking"] = serde_json::json!({
            "type": if config.thinking { "enabled" } else { "disabled" }
        });
        body["response_format"] = serde_json::json!({"type": "json_object"});
    }
    if let Some(reasoning_effort) = &config.reasoning_effort {
        body["reasoning_effort"] = serde_json::Value::String(reasoning_effort.clone());
    }
    body
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct NativeFunctionCall {
    pub(super) name: String,
    pub(super) arguments: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct NativeToolCall {
    pub(super) id: String,
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) function: NativeFunctionCall,
}

#[derive(Clone, Debug)]
pub(super) struct NativeChatTurn {
    pub(super) content: Option<String>,
    pub(super) tool_calls: Vec<NativeToolCall>,
}

pub(super) fn supports_native_tools(config: &AgentConfig) -> bool {
    config.wire_api == "chatCompletions" && config.model.to_ascii_lowercase().contains("deepseek")
}

pub(super) fn call_native_chat(
    config: &AgentConfig,
    api_key: &str,
    prompt: &str,
    continuation: &[serde_json::Value],
) -> Result<NativeChatTurn, String> {
    let mut messages = vec![
        serde_json::json!({
            "role": "system",
            "content": "You are the Osiris Language Server Agent. Follow the supplied facts and schemas. Return a JSON object when no tool call is needed."
        }),
        serde_json::json!({"role": "user", "content": prompt}),
    ];
    messages.extend_from_slice(continuation);
    let body = serde_json::json!({
        "model": config.model,
        "messages": messages,
        "tools": native_tool_definitions(),
        "tool_choice": "auto",
        "thinking": {"type": if config.thinking { "enabled" } else { "disabled" }},
        "response_format": {"type": "json_object"},
        "stream": config.stream
    });
    let mut body = body;
    if let Some(reasoning_effort) = &config.reasoning_effort {
        body["reasoning_effort"] = serde_json::Value::String(reasoning_effort.clone());
    }
    let value = post_json(config, api_key, "chat/completions", body)?;
    let message = value
        .pointer("/choices/0/message")
        .ok_or_else(|| "Chat Completions response did not contain a message".to_owned())?;
    let content = message
        .get("content")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let tool_calls: Vec<NativeToolCall> = message
        .get("tool_calls")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| format!("invalid native tool_calls response: {error}"))?
        .unwrap_or_default();
    if content.as_deref().is_none_or(str::is_empty) && tool_calls.is_empty() {
        return Err(
            "Chat Completions response contained neither JSON content nor tool calls".to_owned(),
        );
    }
    Ok(NativeChatTurn {
        content,
        tool_calls,
    })
}

fn native_tool_definitions() -> serde_json::Value {
    serde_json::json!([
        {
            "type": "function",
            "function": {
                "name": "workspace_search",
                "description": "Search symbols and documented concepts defined by the current Osiris project. Never use this to decide language syntax or standard-library capabilities.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "A concise project symbol or concept"},
                        "limit": {"type": "integer", "minimum": 1, "maximum": 6}
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "symbol_context",
                "description": "Inspect one unambiguous current-project API using hover, definition, signature, references, symbols, and bounded source facts.",
                "parameters": {
                    "type": "object",
                    "properties": {"query": {"type": "string"}},
                    "required": ["query"],
                    "additionalProperties": false
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "source_context",
                "description": "Read one bounded top-level Osiris form at a URI and range returned by an earlier project tool.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "uri": {"type": "string"},
                        "range": {"type": "object"}
                    },
                    "required": ["uri", "range"],
                    "additionalProperties": false
                }
            }
        }
    ])
}

fn post_json(
    config: &AgentConfig,
    api_key: &str,
    path: &str,
    body: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let url = format!("{}/{path}", config.base_url.trim_end_matches('/'));
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(120))
        .timeout_write(Duration::from_secs(30))
        .build();
    let response = agent
        .post(&url)
        .set("Authorization", &format!("Bearer {api_key}"))
        .set("Content-Type", "application/json")
        .send_json(body)
        .map_err(|error| provider_error(error, api_key, path))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not read OpenAI-compatible response body: {error}"))?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err("OpenAI-compatible response body exceeded the 4 MiB limit".to_owned());
    }
    if config.stream {
        parse_sse_response(&bytes, path)
    } else {
        serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid OpenAI-compatible response JSON: {error}"))
    }
}

pub(super) fn parse_sse_response(bytes: &[u8], path: &str) -> Result<serde_json::Value, String> {
    if path != "chat/completions" {
        return Err("streaming is currently supported only for chatCompletions".to_owned());
    }
    #[derive(Default)]
    struct FunctionCall {
        id: String,
        kind: String,
        name: String,
        arguments: String,
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|error| format!("OpenAI-compatible SSE was not UTF-8: {error}"))?;
    let mut content = String::new();
    let mut calls = std::collections::BTreeMap::<usize, FunctionCall>::new();
    for line in text.lines() {
        let Some(data) = line.strip_prefix("data:").map(str::trim) else {
            continue;
        };
        if data == "[DONE]" || data.is_empty() {
            continue;
        }
        let chunk: serde_json::Value = serde_json::from_str(data)
            .map_err(|error| format!("invalid OpenAI-compatible SSE JSON: {error}"))?;
        if let Some(value) = chunk
            .pointer("/choices/0/delta/content")
            .and_then(serde_json::Value::as_str)
        {
            content.push_str(value);
        }
        if let Some(tool_calls) = chunk
            .pointer("/choices/0/delta/tool_calls")
            .and_then(serde_json::Value::as_array)
        {
            for tool_call in tool_calls {
                let index = tool_call
                    .get("index")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| usize::try_from(value).ok())
                    .unwrap_or_default();
                let call = calls.entry(index).or_default();
                append_json_string(tool_call, "/id", &mut call.id);
                append_json_string(tool_call, "/type", &mut call.kind);
                append_json_string(tool_call, "/function/name", &mut call.name);
                append_json_string(tool_call, "/function/arguments", &mut call.arguments);
            }
        }
    }
    let tool_calls = calls
        .into_values()
        .map(|call| {
            serde_json::json!({
                "id": call.id,
                "type": if call.kind.is_empty() { "function" } else { &call.kind },
                "function": {"name": call.name, "arguments": call.arguments}
            })
        })
        .collect::<Vec<_>>();
    if content.is_empty() && tool_calls.is_empty() {
        return Err("OpenAI-compatible SSE contained no content or tool calls".to_owned());
    }
    Ok(serde_json::json!({
        "choices": [{"message": {"content": content, "tool_calls": tool_calls}}]
    }))
}

fn append_json_string(value: &serde_json::Value, pointer: &str, output: &mut String) {
    if let Some(fragment) = value.pointer(pointer).and_then(serde_json::Value::as_str) {
        output.push_str(fragment);
    }
}

pub(super) fn extract_responses_text(value: &serde_json::Value) -> Result<String, String> {
    if let Some(text) = value.get("output_text").and_then(serde_json::Value::as_str) {
        return Ok(text.to_owned());
    }
    let mut output = String::new();
    if let Some(items) = value.get("output").and_then(serde_json::Value::as_array) {
        for item in items {
            if let Some(contents) = item.get("content").and_then(serde_json::Value::as_array) {
                for content in contents {
                    if let Some(text) = content.get("text").and_then(serde_json::Value::as_str) {
                        output.push_str(text);
                    }
                }
            }
        }
    }
    non_empty_output(output, "Responses")
}

pub(super) fn extract_chat_completions_text(value: &serde_json::Value) -> Result<String, String> {
    let content = value
        .pointer("/choices/0/message/content")
        .ok_or_else(|| "Chat Completions response did not contain message content".to_owned())?;
    if let Some(text) = content.as_str() {
        return Ok(text.to_owned());
    }
    let output = content
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| part.get("text").and_then(serde_json::Value::as_str))
        .collect::<String>();
    non_empty_output(output, "Chat Completions")
}

fn non_empty_output(output: String, wire_api: &str) -> Result<String, String> {
    if output.is_empty() {
        Err(format!(
            "OpenAI-compatible {wire_api} response did not contain output text"
        ))
    } else {
        Ok(output)
    }
}

fn provider_error(error: ureq::Error, api_key: &str, path: &str) -> String {
    match error {
        ureq::Error::Status(status, response) => {
            let mut body = String::new();
            let _ = response
                .into_reader()
                .take(16 * 1024)
                .read_to_string(&mut body);
            let redacted = body.replace(api_key, "[REDACTED]");
            let detail = redacted.trim();
            if detail.is_empty() {
                format!("OpenAI-compatible {path} request failed with HTTP {status}")
            } else {
                format!("OpenAI-compatible {path} request failed with HTTP {status}: {detail}")
            }
        }
        ureq::Error::Transport(error) => {
            format!("OpenAI-compatible {path} request failed: {error}")
        }
    }
}
