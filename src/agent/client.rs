use std::{io::Read, time::Duration};

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
        "stream": false
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
        "stream": false
    });
    if config.model.to_ascii_lowercase().contains("deepseek") {
        body["thinking"] = serde_json::json!({"type": "disabled"});
    }
    body
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
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid OpenAI-compatible response JSON: {error}"))
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
