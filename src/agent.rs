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
    lsc::{SourcePosition, ToolResult, WorkspaceService},
    project::{AgentConfig, ConfigError, ProjectConfig},
};

use client::call_provider;
use context::{ContextMaterial, collect_material};
use session::{
    MAX_SESSION_TURNS, SessionFile, SessionTurn, detect_locale, load_session, new_session_id,
    normalize_locale, save_session, validate_session_id,
};
#[cfg(test)]
use tools::parse_tool_calls;
use tools::{collect_source_references, evidence_from_result, run_tool_loop};

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
    let config = resolve_config(project.as_ref());
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
    let mut service = match project.as_ref() {
        Some(project) => match WorkspaceService::open(&project.root, Some(&locale)) {
            Ok(service) => Some(service),
            Err(error) => {
                language_service.push(LanguageServiceEvidence {
                    call_id: "initialize".to_owned(),
                    operation: "workspace-service".to_owned(),
                    status: "unavailable".to_owned(),
                    result: serde_json::Value::Null,
                    message: Some(error),
                });
                None
            }
        },
        None => None,
    };
    if let Some(at) = &options.at {
        let result = service.as_mut().map_or_else(
            || ToolResult {
                schema: "osiris.lsc-tool/v1".to_owned(),
                operation: "symbol-context".to_owned(),
                status: "unavailable".to_owned(),
                result: serde_json::Value::Null,
                message: Some("project language services are unavailable".to_owned()),
            },
            |service| service.position_context(at),
        );
        language_service.push(evidence_from_result("--at", result));
    }
    let prompt = build_prompt(
        &options.request,
        &locale,
        &material,
        &session,
        &language_service,
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
    let failed = response_issue_count(&response, &options.request);
    if failed > 0 {
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

fn resolve_config(project: Option<&ProjectConfig>) -> AgentConfig {
    let project = project
        .map(|project| project.agent.clone())
        .unwrap_or_default();
    AgentConfig {
        model: env::var("OSR_MODEL").unwrap_or(project.model),
        base_url: env::var("OSR_BASE_URL").unwrap_or(project.base_url),
        wire_api: env::var("OSR_WIRE_API").unwrap_or(project.wire_api),
    }
}

fn build_prompt(
    request: &str,
    locale: &str,
    material: &ContextMaterial,
    session: &SessionFile,
    language_service: &[LanguageServiceEvidence],
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
    format!(
        "You are Osiris Language Server Agent. Answer in locale {locale}. You explain the Osiris language and the current project, and provide project-adapted examples when requested. This is a read-only example and explanation assistant, never a coding agent. Retrieved syntax, manuals, standard API records, and compiler-owned language-service results are the authority. Never invent a project symbol, signature, definition, reference, or source location. Clearly label any interpretation not established by those facts. Do not propose file edits or shell commands beyond commands explicitly required by the requested workflow.\n\nYou may either request read-only language-service tools or return the final response. To request tools, return exactly {{\"toolCalls\":[{{\"id\":\"unique-id\",\"operation\":\"workspace-search\",\"arguments\":{{\"query\":\"concept or API name\"}}}}]}}. Available operations are workspace-search (conceptual symbol search), symbol-context (query one unambiguous API and obtain hover, definition, signature help, references, document symbols, and bounded source), and source-context (obtain one bounded top-level Osiris form for a URI/range already returned by a prior tool). Use at most four calls in a round. For a feature request whose implementation location is unknown, search the workspace using concise domain concepts, inspect the best relevant symbols, then write an example using the real project APIs. If a result is ambiguous, expose candidates or search with a qualified name; never guess. Do not request arbitrary paths.\n\nFor the final response, return one JSON object with fields answer (string), examples (array of objects with code), and references (array of strings). The compiler owns languageService, compiled, evaluated, diagnostics, result, and final references; omit those fields because model claims are discarded. Put all Osiris source examples in examples, never inline source in answer. A configuration or tooling explanation does not require a source example unless the user asks for one.\n\nConversation:\n{history}\n\nUser request:\n{request}\n\nRetrieved material:\n{}\n\nCompiler-owned language-service evidence already available:\n{}\n\nNon-negotiable compiler constraints: Every example code contains only valid Osiris source and is a complete compilable module using current syntax. Its first top-level form is exactly a standalone module declaration such as `(module example.point)`; close that form before writing declarations or expressions, and never put a body inside it. For a minimal example, follow the module declaration with only the forms the request needs; do not add defn, export, println, IO, or empty collection cases unless required. Make the requested value-producing expression the final top-level form so execution can capture its result. Use Python content only through documented Osiris interop such as `~python` when the request requires it; never return generated Python. If a defn is necessary, annotate its return type and every parameter type. Public osiris.core bindings and kernel operators are automatically referred; do not import osiris.core merely to access them. Kernel operators such as + are callable syntax but are not first-class function values. `(reduce + ...)` and `(map + ...)` are invalid Osiris examples. Always wrap an operator passed as a value in a typed callback; for integer reduction use exactly `(fn [^Int total ^Int value] (+ total value))`. Authored documentation snippets may omit required module and type context or show operator shorthand; adapt them instead of copying them.",
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
    failed_example_count(response)
        + usize::from(expects_example(request) && response.examples.is_empty())
}

fn expects_example(request: &str) -> bool {
    let request = request.to_lowercase();
    ["example", "示例", "例子", "サンプル"]
        .iter()
        .any(|marker| request.contains(marker))
}

fn parse_model_response(text: &str, session_id: &str) -> Result<LsaResponse, String> {
    let mut response: LsaResponse = serde_json::from_str(model_json_text(text))
        .map_err(|error| format!("LLM returned invalid LSA JSON: {error}"))?;
    response.session_id = session_id.to_owned();
    response.language_service.clear();
    Ok(response)
}

fn model_json_text(text: &str) -> &str {
    let trimmed = text.trim();
    trimmed
        .strip_prefix("```json")
        .and_then(|value| value.strip_suffix("```"))
        .map(str::trim)
        .unwrap_or(trimmed)
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
