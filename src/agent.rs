//! Rust-native Language Server Agent (LSA).
//!
//! LSA is intentionally an example and explanation assistant, not a coding
//! agent. It retrieves compiler-owned material, asks one OpenAI Responses API
//! request, and validates returned Osiris examples before presenting them.

use std::{
    env,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    formatter,
    lsc::{SourcePosition, ToolResult},
    project::{AgentConfig, ConfigError, ProjectConfig},
};

use client::{call_provider, supports_native_tools};
use context::{ContextMaterial, collect_material};
use session::{
    MAX_SESSION_TURNS, SessionFile, SessionTurn, detect_locale, load_session, new_session_id,
    normalize_locale, save_session, validate_session_id,
};
#[cfg(test)]
use tools::parse_tool_calls;
use tools::{WorkspaceToolService, collect_source_references, evidence_from_result, run_tool_loop};

mod client;
mod context;
mod evaluator;
mod session;
mod tools;

const RESPONSE_SCHEMA: &str = "osiris-lsa/v1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LsaOptions {
    pub request: String,
    pub session: Option<String>,
    pub locale: Option<String>,
    pub file: Option<PathBuf>,
    pub at: Option<SourcePosition>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputFormat {
    Json,
    Text,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LsaResponse {
    #[serde(default)]
    pub schema: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub answer: String,
    #[serde(default)]
    pub examples: Vec<LsaExample>,
    #[serde(default)]
    pub references: Vec<String>,
    /// Compiler-owned LSC/LSP evidence. Provider-authored values are discarded.
    #[serde(default)]
    pub language_service: Vec<LanguageServiceEvidence>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageServiceEvidence {
    pub call_id: String,
    pub operation: String,
    pub status: String,
    pub result: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LsaExample {
    pub code: String,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub compiled: bool,
    #[serde(default)]
    pub evaluated: bool,
    #[serde(default)]
    pub diagnostics: Vec<String>,
}

/// Execute one LSA request and return a stable JSON-ready response.
pub fn run(options: &LsaOptions) -> Result<LsaResponse, String> {
    if options.request.trim().is_empty() {
        return Err("lsa requires a non-empty request".to_owned());
    }
    let project = match ProjectConfig::discover(Path::new(".")) {
        Ok(project) => Some(project),
        Err(ConfigError::NotFound(_)) => {
            return Err(
                "lsa requires an Osiris project with pyproject.toml and osiris.jsonc".to_owned(),
            );
        }
        Err(error) => return Err(format!("could not load project configuration: {error}")),
    };
    let root = project.as_ref().map_or_else(
        || env::current_dir().map_err(|error| error.to_string()),
        |value| Ok(value.root.clone()),
    )?;
    let dotenv = root.join(".env");
    if dotenv.is_file() {
        dotenvy::from_path(&dotenv)
            .map_err(|error| format!("could not load '{}': {error}", dotenv.display()))?;
    }
    let config = resolve_config(project.as_ref())?;
    let api_key = env::var("OSR_API_KEY").map_err(|_| {
        "OSR_API_KEY is not set; configure it in the environment or .env".to_owned()
    })?;
    let session_id = options.session.clone().unwrap_or_else(new_session_id);
    validate_session_id(&session_id)?;
    let session_path = root
        .join(".osiris")
        .join("cache")
        .join("agent")
        .join(&session_id)
        .join("session.jsonc");
    let mut session = load_session(&session_path, &session_id)?;
    let material = collect_material(
        &root,
        options.file.as_deref(),
        &options.request,
        project.as_ref(),
    )?;
    let locale = options
        .locale
        .as_deref()
        .map(normalize_locale)
        .transpose()?
        .or_else(|| {
            project
                .as_ref()
                .and_then(|project| project.display_locale.clone())
        })
        .unwrap_or_else(|| detect_locale(&options.request));
    let mut language_service = Vec::new();
    let mut service = WorkspaceToolService::pending(&root, &locale);
    if let Some(at) = &options.at {
        let result = match service.get() {
            Ok(service) => service.position_context(at),
            Err(error) => ToolResult {
                schema: "osiris.lsc-tool/v1".to_owned(),
                operation: "symbol-context".to_owned(),
                status: "unavailable".to_owned(),
                result: serde_json::Value::Null,
                message: Some(error),
            },
        };
        language_service.push(evidence_from_result("--at", result));
    }
    let prompt = build_prompt(
        &options.request,
        &locale,
        &material,
        &session,
        &language_service,
        supports_native_tools(&config),
    );
    let model_text = run_tool_loop(
        &config,
        &api_key,
        &prompt,
        &mut service,
        &mut language_service,
    )?;
    let mut response = validate_response(
        parse_model_response(&model_text, &session_id)?,
        project.as_ref(),
    );
    if response.answer.trim().is_empty()
        && let Some(answer) = language_service_fallback_answer(&locale, &language_service)
    {
        response.answer = answer;
    }
    let failed = response_issue_count(&response, &options.request);
    if failed_example_count(&response) > 0
        || (expects_example(&options.request) && response.examples.is_empty())
    {
        let repair_prompt = build_repair_prompt(
            &response,
            &options.request,
            &locale,
            &material,
            &language_service,
        )?;
        if let Ok(repaired_text) = call_provider(&config, &api_key, &repair_prompt)
            && let Ok(repaired) = parse_model_response(&repaired_text, &session_id)
        {
            let repaired = validate_response(repaired, project.as_ref());
            if response_issue_count(&repaired, &options.request) < failed {
                response = repaired;
            }
        }
    }
    retain_usable_examples(&mut response, &options.request);
    if response_issue_count(&response, &options.request) > 0 {
        response.answer = validation_failure_answer(&locale, &response);
    }
    // References are retrieval evidence. Provider-authored labels are not trusted.
    response.references = material.references;
    response.language_service = language_service;
    for evidence in &response.language_service {
        collect_source_references(&evidence.result, &mut response.references);
    }
    response.references.sort();
    response.references.dedup();
    session.locale = Some(locale);
    if session.turns.len() > MAX_SESSION_TURNS - 2 {
        let remove = session.turns.len() - (MAX_SESSION_TURNS - 2);
        session.turns.drain(..remove);
    }
    session.turns.push(SessionTurn {
        role: "user".to_owned(),
        content: options.request.clone(),
    });
    session.turns.push(SessionTurn {
        role: "assistant".to_owned(),
        content: serde_json::to_string(&response).map_err(|error| error.to_string())?,
    });
    save_session(&session_path, &session)?;
    Ok(response)
}

fn language_service_fallback_answer(
    locale: &str,
    evidence: &[LanguageServiceEvidence],
) -> Option<String> {
    let result = evidence
        .iter()
        .rev()
        .find(|item| item.operation == "symbol-context" && item.status == "ok")?
        .result
        .get("context")?
        .clone();
    let hover = result
        .pointer("/hover/value/contents/value")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty());
    let signature = result
        .pointer("/signatureHelp/value/signatures/0/label")
        .and_then(serde_json::Value::as_str);
    let definition = result
        .pointer("/definition/value")
        .and_then(format_source_location);
    if hover.is_none() && signature.is_none() && definition.is_none() {
        return None;
    }
    let mut sections = Vec::new();
    if let Some(hover) = hover {
        sections.push(hover.to_owned());
    }
    if hover.is_none()
        && let Some(signature) = signature
    {
        sections.push(format!("Signature: `{signature}`"));
    }
    if let Some(definition) = definition {
        let label = match locale.split('-').next().unwrap_or(locale) {
            "zh" => "定义位置",
            "ja" => "定義位置",
            _ => "Definition",
        };
        sections.push(format!("**{label}** `{definition}`"));
    }
    Some(sections.join("\n\n"))
}

fn format_source_location(value: &serde_json::Value) -> Option<String> {
    let uri = value.get("uri")?.as_str()?;
    let line = value
        .pointer("/range/start/line")
        .and_then(serde_json::Value::as_u64)
        .map(|line| line + 1);
    let character = value
        .pointer("/range/start/character")
        .and_then(serde_json::Value::as_u64)
        .map(|character| character + 1);
    Some(match (line, character) {
        (Some(line), Some(character)) => format!("{uri}:{line}:{character}"),
        _ => uri.to_owned(),
    })
}

fn resolve_config(project: Option<&ProjectConfig>) -> Result<AgentConfig, String> {
    let project = project
        .map(|project| project.agent.clone())
        .unwrap_or_default();
    let config = AgentConfig {
        model: env::var("OSR_MODEL").unwrap_or(project.model),
        base_url: env::var("OSR_BASE_URL").unwrap_or(project.base_url),
        wire_api: env::var("OSR_WIRE_API").unwrap_or(project.wire_api),
        thinking: env_bool("OSR_THINKING")?.unwrap_or(project.thinking),
        reasoning_effort: env::var("OSR_REASONING_EFFORT")
            .ok()
            .or(project.reasoning_effort),
        stream: env_bool("OSR_STREAM")?.unwrap_or(project.stream),
    };
    if config.stream && config.wire_api != "chatCompletions" {
        return Err("LSA streaming currently requires wireApi `chatCompletions`".to_owned());
    }
    if config
        .reasoning_effort
        .as_deref()
        .is_some_and(|value| !matches!(value, "low" | "medium" | "high"))
    {
        return Err("OSR_REASONING_EFFORT must be low, medium, or high".to_owned());
    }
    Ok(config)
}

fn env_bool(name: &str) -> Result<Option<bool>, String> {
    match env::var(name) {
        Ok(value) => match value.to_ascii_lowercase().as_str() {
            "true" | "1" => Ok(Some(true)),
            "false" | "0" => Ok(Some(false)),
            _ => Err(format!("{name} must be true, false, 1, or 0")),
        },
        Err(env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(format!("could not read {name}: {error}")),
    }
}

fn build_prompt(
    request: &str,
    locale: &str,
    material: &ContextMaterial,
    session: &SessionFile,
    language_service: &[LanguageServiceEvidence],
    native_tools: bool,
) -> String {
    let history = session
        .turns
        .iter()
        .rev()
        .take(6)
        .rev()
        .map(|turn| format!("{}: {}", turn.role, turn.content))
        .collect::<Vec<_>>()
        .join("\n");
    let tool_protocol = if native_tools {
        "Use the API-provided functions for workspace_search, symbol_context, and source_context. Never encode a tool request inside the final JSON object."
            .to_owned()
    } else {
        "Legacy tool request format: return exactly {\"toolCalls\":[{\"id\":\"unique-id\",\"operation\":\"workspace-search\",\"arguments\":{\"query\":\"qualified symbol or project concept\"}}]}. Operations: workspace-search, symbol-context, and source-context."
            .to_owned()
    };
    let output_gate = if native_tools {
        "OUTPUT GATE: Re-read the user request and compiler-owned evidence. If the user requests current-project APIs, project DSL, project symbols, or their source and no relevant language-service evidence is shown, call an API-provided workspace tool now. Do not return final JSON yet."
    } else {
        "OUTPUT GATE: Re-read the user request and compiler-owned evidence. If the user requests current-project APIs, project DSL, project symbols, or their source and no relevant language-service evidence is shown, the only permitted response is a toolCalls JSON object. Do not return an answer or examples yet."
    };
    format!(
        "You are Osiris Language Server Agent. Answer in locale {locale}. You explain Osiris and the current project and provide project-adapted examples. You are a read-only example and explanation assistant, not a coding agent. Retrieved syntax, manuals, standard API records, and compiler-owned language-service results are authoritative. Never invent symbols, modules, signatures, locations, or references; label model interpretation as inference.\n\nFACT BOUNDARY: Retrieved syntax and standard API records define the Osiris language. Workspace tools describe only the current project's symbols. Never use workspace search to decide whether Osiris supports syntax or a standard API. A workspace notFound result says nothing about language capabilities.\n\nReturn either read-only language-service tool calls or the final response. Answer language, syntax, standard-library, configuration, and generic example questions directly from retrieved material. Use workspace tools only when the answer genuinely depends on current-project symbols or source. If the request asks you to use, explain, locate, or adapt a current-project API and no relevant compiler-owned language-service evidence is present, you MUST request workspace tools before answering. Never substitute generic Osiris APIs for an uninspected project API. Never search generic concepts such as recursion, hello, print function, Python APIs, or facts already present in retrieved material.\n\n{tool_protocol} Use at most four calls per round. For project-specific feature requests, first search concise domain concepts, then inspect the best symbols with symbol-context before writing source. Expose ambiguity; never guess or request arbitrary paths.\n\nFinal format is one JSON object with answer (string), examples (array of objects with code), and references (array of strings). The exact shape is JSON like {{\"answer\":\"Concise factual answer.\",\"examples\":[{{\"code\":\"(module example.main)\\n\\n42\\n\"}}],\"references\":[]}}. Return JSON directly without reasoning, commentary, or Markdown fences. Always include all three keys; use an empty examples array when no example is requested or useful. The compiler owns languageService, compiled, evaluated, diagnostics, result, and final references; omit them. Put Osiris source only in examples. Return exactly one minimal example unless the user explicitly requests multiple distinct examples.\n\nConversation:\n{history}\n\nUser request:\n{request}\n\nRetrieved material:\n{}\n\nCompiler-owned language-service evidence:\n{}\n\nExample constraints: Each example is valid Osiris and a complete compilable module. Its first form is exactly a standalone module declaration such as `(module example.point)`; close that form and never put a body inside it. Include only required forms, and finish with the requested value expression. If the example defines a function, invoke it in the final top-level form so evaluation captures a real result, unless the user explicitly requests declaration-only source. Function return metadata precedes the function name: `(defn ^Int factorial [^Int n] ...)`. For Hello World, use the string `\"Hello, World!\"` as the final expression; Osiris has no implicit `io`, `print`, or `println` module. Do not add defn, export, output, IO, or empty cases unless required and documented in retrieved material. Never return generated Python; use documented `~python` interop only when explicitly required. A defn needs return and parameter types. Public osiris.core bindings and kernel operators are automatically referred. Operators such as + are not first-class values: `(reduce + ...)` and `(map + ...)` are invalid. Wrap them in a typed callback, for example `(fn [^Int total ^Int value] (+ total value))`. Adapt incomplete documentation snippets to these constraints.\n\n{output_gate}",
        material.text,
        serde_json::to_string_pretty(language_service).unwrap_or_else(|_| "[]".to_owned()),
    )
}

fn build_repair_prompt(
    response: &LsaResponse,
    request: &str,
    locale: &str,
    material: &ContextMaterial,
    language_service: &[LanguageServiceEvidence],
) -> Result<String, String> {
    let response = serde_json::to_string(response).map_err(|error| error.to_string())?;
    Ok(format!(
        "You are repairing Osiris examples after compiler validation and execution. Return one replacement JSON object with answer, examples, and references, in locale {locale}. Include only valid Osiris source in each example; omit languageService, compiled, evaluated, diagnostics, and result because the compiler owns that evidence. Never invent project symbols or locations beyond the language-service evidence. Never return generated Python; Python content is allowed only through documented Osiris interop such as `~python` when required. The retrieved material below is authoritative: correct any previous factual claim that contradicts it and use the exact documented form requested by the user. Preserve every requirement from the original request. Replace every failed example with a complete module that fixes every diagnostic and ends with the requested value-producing expression. Its first top-level form must be a standalone declaration such as `(module example.point)` with the closing parenthesis immediately after the one module name; declarations and expressions are later sibling forms, never a module body. If the user requested an example and the previous examples list is empty, add at least one complete example. Keep only the forms needed by the request: remove defn, export, println, output helpers, and empty collection cases unless essential. If a declaration remains, annotate its return and every parameter type. `(reduce + ...)` and `(map + ...)` are invalid because operators are not function values. For integer reduction replace them with `(reduce (fn [^Int total ^Int value] (+ total value)) initial values)`. Return JSON only.\n\nOriginal user request:\n{request}\n\nValidated previous response:\n{response}\n\nAuthoritative retrieved material:\n{}\n\nCompiler-owned language-service evidence:\n{}",
        material.text,
        serde_json::to_string_pretty(language_service).unwrap_or_else(|_| "[]".to_owned()),
    ))
}

fn validate_response(mut response: LsaResponse, project: Option<&ProjectConfig>) -> LsaResponse {
    response.schema = RESPONSE_SCHEMA.to_owned();
    response.examples = response
        .examples
        .into_iter()
        .map(|example| validate_example_in_workspace(example, project))
        .collect();
    response
}

fn failed_example_count(response: &LsaResponse) -> usize {
    response
        .examples
        .iter()
        .filter(|example| !example.compiled || !example.evaluated)
        .count()
}

fn response_issue_count(response: &LsaResponse, request: &str) -> usize {
    usize::from(response.answer.trim().is_empty())
        + failed_example_count(response)
        + usize::from(expects_example(request) && response.examples.is_empty())
}

fn retain_usable_examples(response: &mut LsaResponse, request: &str) {
    let successful = response
        .examples
        .iter()
        .filter(|example| example.compiled && example.evaluated)
        .count();
    if successful > 0 || !expects_example(request) {
        response
            .examples
            .retain(|example| example.compiled && example.evaluated);
    }
}

fn expects_example(request: &str) -> bool {
    let request = request.to_lowercase();
    if [
        "no example",
        "without an example",
        "without examples",
        "do not generate an example",
        "do not generate examples",
        "don't generate an example",
        "不要例子",
        "不要示例",
        "不要生成例子",
        "不要生成示例",
        "无需例子",
        "无需示例",
        "例子は不要",
        "サンプルは不要",
    ]
    .iter()
    .any(|marker| request.contains(marker))
    {
        return false;
    }
    [
        "example",
        "how to write",
        "write a",
        "implement",
        "示例",
        "例子",
        "如何写",
        "怎么写",
        "实现",
        "サンプル",
        "書き方",
    ]
    .iter()
    .any(|marker| request.contains(marker))
}

fn validation_failure_answer(locale: &str, response: &LsaResponse) -> String {
    let count = failed_example_count(response);
    match locale.split('-').next().unwrap_or(locale) {
        "zh" if count == 0 => {
            "模型没有返回完整、可验证的 LSA 答案。该响应未作为正确答案交付。".to_owned()
        }
        "zh" => format!("未能生成通过 Osiris 编译与执行验证的示例（{count} 个候选失败）。下面只保留编译器诊断，不应把候选代码视为正确答案。"),
        "ja" if count == 0 => {
            "モデルは完全で検証可能な LSA 応答を返しませんでした。この応答は正解として提示されません。".to_owned()
        }
        "ja" => format!(
            "Osiris のコンパイルと実行検証に合格する例を生成できませんでした（{count} 件の候補が失敗）。候補コードを正解として扱わず、コンパイラ診断を確認してください。"
        ),
        _ if count == 0 => {
            "The model did not return a complete, verifiable LSA response. It was not presented as a correct answer."
                .to_owned()
        }
        _ => format!(
            "Could not produce an example that passed Osiris compilation and evaluation ({count} candidate(s) failed). The candidate code is retained only with compiler diagnostics and must not be treated as a correct answer."
        ),
    }
}

fn parse_model_response(text: &str, session_id: &str) -> Result<LsaResponse, String> {
    let mut response: LsaResponse = serde_json::from_str(model_json_text(text)?)
        .map_err(|error| format!("LLM returned invalid LSA JSON: {error}"))?;
    response.session_id = session_id.to_owned();
    response.language_service.clear();
    Ok(response)
}

fn model_json_text(text: &str) -> Result<&str, String> {
    let trimmed = text.trim();
    if serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
        return Ok(trimmed);
    }
    for (start, ch) in trimmed.char_indices() {
        if ch != '{' {
            continue;
        }
        let mut depth = 0_u32;
        let mut in_string = false;
        let mut escaped = false;
        for (offset, current) in trimmed[start..].char_indices() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if current == '\\' {
                    escaped = true;
                } else if current == '"' {
                    in_string = false;
                }
                continue;
            }
            match current {
                '"' => in_string = true,
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        let candidate = &trimmed[start..start + offset + current.len_utf8()];
                        if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
                            return Ok(candidate);
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    Err("LLM response did not contain a valid JSON object".to_owned())
}

fn validate_example_in_workspace(
    mut example: LsaExample,
    project: Option<&ProjectConfig>,
) -> LsaExample {
    // Compilation and evaluation are compiler-owned evidence.
    example.result = None;
    example.evaluated = false;
    let formatted = match formatter::format_source(&example.code) {
        Ok(formatted) => formatted,
        Err(error) => {
            example.diagnostics = error
                .diagnostics
                .iter()
                .map(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message))
                .collect();
            example.compiled = false;
            return example;
        }
    };
    example.code = formatted;
    let surface = crate::ast::lower_document(&crate::reader::read(&example.code));
    if surface.module.name.is_none() {
        example.compiled = false;
        example.diagnostics = vec!["OSR-A0002: example must declare a module".to_owned()];
        return example;
    }
    let workspace = match crate::cli::compile_evaluation_workspace(&example.code, project) {
        Ok(workspace) => workspace,
        Err(diagnostics) => {
            example.compiled = false;
            example.diagnostics = diagnostics;
            return example;
        }
    };
    example.compiled = true;
    match evaluator::evaluate(&workspace) {
        Ok(value) => {
            example.result = value;
            example.evaluated = true;
            example.diagnostics.clear();
        }
        Err(error) => {
            example.evaluated = false;
            example.diagnostics = vec![format!("OSR-A0005: {error}")];
        }
    }
    example
}

#[cfg(test)]
fn validate_example(example: LsaExample) -> LsaExample {
    validate_example_in_workspace(example, None)
}

pub fn render(response: &LsaResponse, format: OutputFormat) -> Result<String, String> {
    match format {
        OutputFormat::Json => {
            let mut output =
                serde_json::to_string_pretty(response).map_err(|error| error.to_string())?;
            output.push('\n');
            Ok(output)
        }
        OutputFormat::Text => {
            let mut output = format!(
                "sessionId: {}\n\n{}\n",
                response.session_id, response.answer
            );
            for example in &response.examples {
                output.push_str("\n```osiris\n");
                output.push_str(&example.code);
                output.push_str("```\n");
                output.push_str(if example.compiled {
                    "validated: compiled\n"
                } else {
                    "validated: failed\n"
                });
                if example.evaluated {
                    let result = example
                        .result
                        .as_ref()
                        .map_or_else(|| "null".to_owned(), serde_json::Value::to_string);
                    output.push_str(&format!("result: {result}\n"));
                }
            }
            if !response.language_service.is_empty() {
                output.push_str("\nLanguage service evidence:\n");
                for evidence in &response.language_service {
                    output.push_str(&format!(
                        "- {} [{}] {}\n",
                        evidence.operation, evidence.status, evidence.call_id
                    ));
                    if let Some(message) = &evidence.message {
                        output.push_str(&format!("  {message}\n"));
                    }
                }
            }
            if !response.references.is_empty() {
                output.push_str("\nSources:\n");
                for reference in &response.references {
                    output.push_str(&format!("- {reference}\n"));
                }
            }
            Ok(output)
        }
    }
}

#[cfg(test)]
#[path = "agent/tests.rs"]
mod tests;
