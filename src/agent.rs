//! Rust-native Language Server Agent (LSA).
//!
//! LSA is intentionally an example and explanation assistant, not a coding
//! agent. It retrieves compiler-owned material, asks one OpenAI Responses API
//! request, and validates returned Osiris examples before presenting them.

use std::{
    env, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use oxilangtag::LanguageTag;
use serde::{Deserialize, Serialize};

use crate::{
    formatter,
    project::{AgentConfig, ConfigError, ProjectConfig},
};

use client::call_provider;
use context::{ContextMaterial, collect_material};

mod client;
mod context;
mod evaluator;

const MAX_SESSION_BYTES: u64 = 1024 * 1024;
const MAX_SESSION_TURNS: usize = 100;
const SESSION_SCHEMA: &str = "osiris-lsa-session/v1";
const RESPONSE_SCHEMA: &str = "osiris-lsa/v1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LsaOptions {
    pub request: String,
    pub session: Option<String>,
    pub locale: Option<String>,
    pub file: Option<PathBuf>,
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

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionFile {
    schema: String,
    session_id: String,
    #[serde(default)]
    locale: Option<String>,
    #[serde(default)]
    turns: Vec<SessionTurn>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionTurn {
    role: String,
    content: String,
}

/// Execute one LSA request and return a stable JSON-ready response.
pub fn run(options: &LsaOptions) -> Result<LsaResponse, String> {
    if options.request.trim().is_empty() {
        return Err("lsa requires a non-empty request".to_owned());
    }
    let project = match ProjectConfig::discover(Path::new(".")) {
        Ok(project) => Some(project),
        Err(ConfigError::NotFound(_)) => None,
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
    let material = collect_material(&root, options.file.as_deref(), &options.request)?;
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
    let prompt = build_prompt(&options.request, &locale, &material, &session);
    let model_text = call_provider(&config, &api_key, &prompt)?;
    let mut response = validate_response(
        parse_model_response(&model_text, &session_id)?,
        project.as_ref(),
    );
    let failed = response_issue_count(&response, &options.request);
    if failed > 0 {
        let repair_prompt = build_repair_prompt(&response, &options.request, &locale, &material)?;
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
        "You are Osiris Language Server Agent. Answer in locale {locale}. You only explain Osiris and provide examples. Retrieved Osiris syntax and standard API records are the authority for language facts: never deny or replace a form that they explicitly define, and use its exact documented spelling. Do not propose file edits or shell commands. Return one JSON object with fields answer (string), examples (array of objects with code), and references (array of strings). The compiler owns compiled, evaluated, diagnostics, and result; omit those fields because model claims are discarded. Put all source examples in examples, never inline source in answer.\n\nConversation:\n{history}\n\nUser request:\n{request}\n\nRetrieved material:\n{}\n\nNon-negotiable compiler constraints: Every example code contains only valid Osiris source and is a complete compilable module using current syntax. Its first top-level form is exactly a standalone module declaration such as `(module example.point)`; close that form before writing declarations or expressions, and never put a body inside it. For a minimal example, follow the module declaration with only the forms the request needs; do not add defn, export, println, IO, or empty collection cases unless required. Make the requested value-producing expression the final top-level form so execution can capture its result. Use Python content only through documented Osiris interop such as `~python` when the request requires it; never return generated Python. If a defn is necessary, annotate its return type and every parameter type. Public osiris.core bindings and kernel operators are automatically referred; do not import osiris.core merely to access them. Kernel operators such as + are callable syntax but are not first-class function values. `(reduce + ...)` and `(map + ...)` are invalid Osiris examples. Always wrap an operator passed as a value in a typed callback; for integer reduction use exactly `(fn [^Int total ^Int value] (+ total value))`. Authored documentation snippets may omit required module and type context or show operator shorthand; adapt them instead of copying them.",
        material.text,
    )
}

fn build_repair_prompt(
    response: &LsaResponse,
    request: &str,
    locale: &str,
    material: &ContextMaterial,
) -> Result<String, String> {
    let response = serde_json::to_string(response).map_err(|error| error.to_string())?;
    Ok(format!(
        "You are repairing Osiris examples after compiler validation and execution. Return one replacement JSON object with answer, examples, and references, in locale {locale}. Include only valid Osiris source in each example; omit compiled, evaluated, diagnostics, and result because the compiler owns that evidence. Never return generated Python; Python content is allowed only through documented Osiris interop such as `~python` when required. The retrieved material below is authoritative: correct any previous factual claim that contradicts it and use the exact documented form requested by the user. Preserve every requirement from the original request. Replace every failed example with a complete module that fixes every diagnostic and ends with the requested value-producing expression. Its first top-level form must be a standalone declaration such as `(module example.point)` with the closing parenthesis immediately after the one module name; declarations and expressions are later sibling forms, never a module body. If the user requested an example and the previous examples list is empty, add at least one complete example. Keep only the forms needed by the request: remove defn, export, println, output helpers, and empty collection cases unless essential. If a declaration remains, annotate its return and every parameter type. `(reduce + ...)` and `(map + ...)` are invalid because operators are not function values. For integer reduction replace them with `(reduce (fn [^Int total ^Int value] (+ total value)) initial values)`. Return JSON only.\n\nOriginal user request:\n{request}\n\nValidated previous response:\n{response}\n\nAuthoritative retrieved material:\n{}",
        material.text,
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
    let trimmed = text.trim();
    let json = trimmed
        .strip_prefix("```json")
        .and_then(|value| value.strip_suffix("```"))
        .map(str::trim)
        .unwrap_or(trimmed);
    let mut response: LsaResponse = serde_json::from_str(json)
        .map_err(|error| format!("LLM returned invalid LSA JSON: {error}"))?;
    response.session_id = session_id.to_owned();
    Ok(response)
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

fn load_session(path: &Path, session_id: &str) -> Result<SessionFile, String> {
    if !path.is_file() {
        return Ok(SessionFile {
            schema: SESSION_SCHEMA.to_owned(),
            session_id: session_id.to_owned(),
            ..SessionFile::default()
        });
    }
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.len() > MAX_SESSION_BYTES {
        return Err("LSA session exceeded the 1 MiB limit".to_owned());
    }
    let source = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let session: SessionFile = json5::from_str(&source).map_err(|error| error.to_string())?;
    if session.schema != SESSION_SCHEMA {
        return Err(format!(
            "unsupported LSA session schema `{}`",
            session.schema
        ));
    }
    if session.session_id != session_id {
        return Err("session file id does not match requested session".to_owned());
    }
    validate_session(&session)?;
    Ok(session)
}

fn save_session(path: &Path, session: &SessionFile) -> Result<(), String> {
    validate_session(session)?;
    fs::create_dir_all(path.parent().expect("session path parent"))
        .map_err(|error| error.to_string())?;
    let contents = serde_json::to_string_pretty(session).map_err(|error| error.to_string())?;
    if contents.len() as u64 > MAX_SESSION_BYTES {
        return Err("LSA session exceeded the 1 MiB limit".to_owned());
    }
    let temporary = path.with_extension("jsonc.tmp");
    fs::write(&temporary, format!("{contents}\n")).map_err(|error| error.to_string())?;
    fs::rename(temporary, path).map_err(|error| error.to_string())
}

fn validate_session(session: &SessionFile) -> Result<(), String> {
    if session.turns.len() > MAX_SESSION_TURNS {
        return Err(format!(
            "LSA session exceeded the {MAX_SESSION_TURNS}-turn limit"
        ));
    }
    if session
        .turns
        .iter()
        .any(|turn| !matches!(turn.role.as_str(), "user" | "assistant"))
    {
        return Err("LSA session contains an unsupported turn role".to_owned());
    }
    Ok(())
}

fn validate_session_id(session_id: &str) -> Result<(), String> {
    if session_id.is_empty()
        || session_id.len() > 128
        || matches!(session_id, "." | "..")
        || !session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err("session id may contain only letters, numbers, '-', '_' and '.'".to_owned());
    }
    Ok(())
}

fn new_session_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("session-{nanos}")
}

fn detect_locale(request: &str) -> String {
    if request
        .chars()
        .any(|character| ('\u{3040}'..='\u{30ff}').contains(&character))
    {
        return "ja".to_owned();
    }
    if request
        .chars()
        .any(|character| ('\u{ac00}'..='\u{d7af}').contains(&character))
    {
        return "ko".to_owned();
    }
    if request
        .chars()
        .any(|character| ('\u{4e00}'..='\u{9fff}').contains(&character))
    {
        "zh-CN".to_owned()
    } else {
        "en".to_owned()
    }
}

fn normalize_locale(locale: &str) -> Result<String, String> {
    LanguageTag::parse_and_normalize(locale)
        .map(|tag| tag.to_string())
        .map_err(|error| format!("invalid BCP 47 locale `{locale}`: {error}"))
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
            Ok(output)
        }
    }
}

#[cfg(test)]
#[path = "agent/tests.rs"]
mod tests;
