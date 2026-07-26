//! Reusable Language Server Console boundary.
//!
//! LSC speaks JSON-RPC to the in-process language server and exposes bounded,
//! read-only composite operations for command-line clients and LSA. It is the
//! only layer outside the editor transport that understands LSP capabilities.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};

use crate::{
    lsp::{JsonRpcMachine, Position, Range},
    project::ProjectConfig,
};

mod context;
mod graph;
mod inputs;
mod source;
#[cfg(test)]
use crate::lsp::offset_to_position;
use graph::{CacheProbe, GraphStore};
use source::{first_source, json_position, path_to_uri};

const RESULT_SCHEMA: &str = "osiris.lsc-tool/v1";
const MAX_SEARCH_RESULTS: usize = 6;
const MAX_GRAPH_EDGES: usize = 16;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePosition {
    pub path: PathBuf,
    /// One-based line number for CLI stability.
    pub line: u32,
    /// One-based UTF-16 column number for LSP compatibility.
    pub column: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub schema: String,
    pub operation: String,
    pub status: String,
    pub result: JsonValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheReport {
    pub schema: String,
    pub status: String,
    pub path: String,
    pub reason: String,
    pub input_count: usize,
    pub reused_hashes: usize,
    pub hashed_inputs: usize,
}

impl ToolResult {
    fn ok(operation: &str, result: JsonValue) -> Self {
        Self {
            schema: RESULT_SCHEMA.to_owned(),
            operation: operation.to_owned(),
            status: "ok".to_owned(),
            result,
            message: None,
        }
    }

    fn status(
        operation: &str,
        status: &str,
        result: JsonValue,
        message: impl Into<String>,
    ) -> Self {
        Self {
            schema: RESULT_SCHEMA.to_owned(),
            operation: operation.to_owned(),
            status: status.to_owned(),
            result,
            message: Some(message.into()),
        }
    }
}

/// One project-scoped LSC service with lazy language-server initialization.
pub struct WorkspaceService {
    machine: Option<JsonRpcMachine>,
    project: ProjectConfig,
    anchor_uri: Option<String>,
    capabilities: JsonValue,
    next_request_id: u64,
    locale: String,
    graph: Option<GraphStore>,
}

impl WorkspaceService {
    pub fn open(root: &Path, locale: Option<&str>) -> Result<Self, String> {
        let project = ProjectConfig::discover(root).map_err(|error| error.to_string())?;
        match GraphStore::probe(&project)? {
            CacheProbe::Fresh { graph, .. } => Ok(Self::new(project, locale, Some(graph))),
            CacheProbe::Refresh { inputs, .. } => {
                let mut service = Self::new(project, locale, None);
                service.rebuild_graph(&inputs)?;
                Ok(service)
            }
        }
    }

    pub fn cache_status(root: &Path) -> Result<CacheReport, String> {
        let project = ProjectConfig::discover(root).map_err(|error| error.to_string())?;
        let (status, reason, inputs) = match GraphStore::probe(&project)? {
            CacheProbe::Fresh { inputs, .. } => ("fresh", "inputs-match", inputs),
            CacheProbe::Refresh { inputs, reason } => (reason, reason, inputs),
        };
        Ok(CacheReport {
            schema: "osiris.lsc-cache/v2".to_owned(),
            status: status.to_owned(),
            path: GraphStore::relative_path().to_owned(),
            reason: reason.to_owned(),
            input_count: inputs.entries.len(),
            reused_hashes: inputs.reused_hashes(),
            hashed_inputs: inputs.hashed_inputs(),
        })
    }

    pub fn rebuild_cache(root: &Path, locale: Option<&str>) -> Result<CacheReport, String> {
        let project = ProjectConfig::discover(root).map_err(|error| error.to_string())?;
        let inputs = inputs::fingerprint(&project, None)?;
        let input_count = inputs.entries.len();
        let reused_hashes = inputs.reused_hashes();
        let hashed_inputs = inputs.hashed_inputs();
        let mut service = Self::new(project, locale, None);
        service.rebuild_graph(&inputs)?;
        Ok(CacheReport {
            schema: "osiris.lsc-cache/v2".to_owned(),
            status: "rebuilt".to_owned(),
            path: GraphStore::relative_path().to_owned(),
            reason: "manual-full-rebuild".to_owned(),
            input_count,
            reused_hashes,
            hashed_inputs,
        })
    }

    fn new(project: ProjectConfig, locale: Option<&str>, graph: Option<GraphStore>) -> Self {
        Self {
            machine: None,
            project,
            anchor_uri: None,
            capabilities: JsonValue::Null,
            next_request_id: 1,
            locale: locale.unwrap_or("und").to_owned(),
            graph,
        }
    }

    fn ensure_language_service(&mut self) -> Result<(), String> {
        if self.machine.is_some() {
            return Ok(());
        }
        let anchor = first_source(&self.project)?;
        self.machine = Some(JsonRpcMachine::new());
        let site_roots = self
            .project
            .installed_package_roots()
            .into_iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>();
        let initialized = self.request(
            "initialize",
            json!({
                "rootUri": path_to_uri(&self.project.root)?,
                "locale": self.locale.clone(),
                "capabilities": {},
                "initializationOptions": {"siteRoots": site_roots},
            }),
        )?;
        self.capabilities = initialized
            .get("capabilities")
            .cloned()
            .unwrap_or(JsonValue::Null);
        self.notify("initialized", json!({}))?;
        self.anchor_uri = Some(self.open_document(&anchor)?);
        Ok(())
    }

    fn rebuild_graph(&mut self, inputs: &inputs::InputSnapshot) -> Result<(), String> {
        self.ensure_language_service()?;
        let anchor_uri = self
            .anchor_uri
            .clone()
            .ok_or_else(|| "language service has no workspace anchor".to_owned())?;
        let snapshot = self.request(
            "osiris/workspaceGraph",
            json!({"textDocument": {"uri": anchor_uri}}),
        )?;
        let snapshot = self.public_value(snapshot);
        self.graph = Some(GraphStore::replace(&self.project, &snapshot, inputs)?);
        Ok(())
    }

    #[must_use]
    pub fn capabilities(&self) -> &JsonValue {
        &self.capabilities
    }

    pub fn workspace_search(&mut self, query: &str, limit: Option<usize>) -> ToolResult {
        let operation = "workspace-search";
        let limit = limit.unwrap_or(MAX_SEARCH_RESULTS).min(MAX_SEARCH_RESULTS);
        if let Some(graph) = &self.graph {
            match graph.search(query, limit) {
                Ok(values) => {
                    let status = if values.is_empty() { "notFound" } else { "ok" };
                    return ToolResult {
                        schema: RESULT_SCHEMA.to_owned(),
                        operation: operation.to_owned(),
                        status: status.to_owned(),
                        result: JsonValue::Array(values),
                        message: (status == "notFound")
                            .then(|| format!("no project graph node matched `{query}`")),
                    };
                }
                Err(error) => {
                    return ToolResult::status(operation, "error", json!([]), error);
                }
            }
        }
        if !self.capability_enabled("workspaceSymbolProvider") {
            return ToolResult::status(
                operation,
                "unavailable",
                json!([]),
                "the language server does not advertise workspace symbols",
            );
        }
        match self.request("workspace/symbol", json!({"query": query})) {
            Ok(value) => {
                let mut values = value.as_array().cloned().unwrap_or_default();
                values.truncate(limit);
                let status = if values.is_empty() { "notFound" } else { "ok" };
                ToolResult {
                    schema: RESULT_SCHEMA.to_owned(),
                    operation: operation.to_owned(),
                    status: status.to_owned(),
                    result: JsonValue::Array(values),
                    message: (status == "notFound")
                        .then(|| format!("no workspace symbol matched `{query}`")),
                }
            }
            Err(error) => ToolResult::status(operation, "error", json!([]), error),
        }
    }

    pub fn symbol_context(&mut self, query: &str) -> ToolResult {
        let search = self.workspace_search(query, None);
        let Some(candidates) = search.result.as_array() else {
            return search;
        };
        if candidates.is_empty() {
            return search;
        }
        let best_score = candidates[0]
            .pointer("/data/score")
            .and_then(JsonValue::as_u64)
            .unwrap_or_default();
        let best = candidates
            .iter()
            .take_while(|candidate| {
                candidate
                    .pointer("/data/score")
                    .and_then(JsonValue::as_u64)
                    .unwrap_or_default()
                    == best_score
            })
            .cloned()
            .collect::<Vec<_>>();
        if best.len() != 1 {
            return ToolResult::status(
                "symbol-context",
                "ambiguous",
                json!({"query": query, "candidates": best}),
                format!("`{query}` has {} equally ranked definitions", best.len()),
            );
        }
        let candidate = &best[0];
        let Some(uri) = candidate
            .pointer("/location/uri")
            .and_then(JsonValue::as_str)
        else {
            return ToolResult::status(
                "symbol-context",
                "error",
                json!({"candidate": candidate}),
                "the selected workspace symbol has no source URI",
            );
        };
        let Some(position) = json_position(candidate.pointer("/location/range/start")) else {
            return ToolResult::status(
                "symbol-context",
                "error",
                json!({"candidate": candidate}),
                "the selected workspace symbol has no definition position",
            );
        };
        if let Err(error) = self.ensure_language_service() {
            return ToolResult::status(
                "symbol-context",
                "unavailable",
                json!({"query": query, "candidate": candidate}),
                error,
            );
        }
        let lsp_uri = match self.uri_for_lsp(uri) {
            Ok(uri) => uri,
            Err(error) => {
                return ToolResult::status(
                    "symbol-context",
                    "unavailable",
                    json!({"query": query, "candidate": candidate}),
                    error,
                );
            }
        };
        match self.context_at_uri(&lsp_uri, position) {
            Ok(mut context) => {
                if let Some(binding_id) = candidate
                    .pointer("/data/bindingId")
                    .and_then(JsonValue::as_str)
                    && let Some(graph) = &self.graph
                    && let Ok(neighborhood) = graph.neighborhood(binding_id, 1, MAX_GRAPH_EDGES)
                {
                    context["graphNeighborhood"] = JsonValue::Array(neighborhood);
                }
                let context = self.public_value(context);
                ToolResult::ok(
                    "symbol-context",
                    json!({"query": query, "candidate": candidate, "context": context}),
                )
            }
            Err(error) => ToolResult::status(
                "symbol-context",
                "error",
                json!({"query": query, "candidate": candidate}),
                error,
            ),
        }
    }

    pub fn position_context(&mut self, at: &SourcePosition) -> ToolResult {
        let operation = "symbol-context";
        if at.line == 0 || at.column == 0 {
            return ToolResult::status(
                operation,
                "error",
                JsonValue::Null,
                "line and column must be one-based positive values",
            );
        }
        if let Err(error) = self.ensure_language_service() {
            return ToolResult::status(operation, "unavailable", JsonValue::Null, error);
        }
        let path = match self.resolve_project_source(&at.path) {
            Ok(path) => path,
            Err(error) => {
                return ToolResult::status(operation, "unavailable", JsonValue::Null, error);
            }
        };
        let uri = match self.open_document(&path) {
            Ok(uri) => uri,
            Err(error) => return ToolResult::status(operation, "error", JsonValue::Null, error),
        };
        let position = Position {
            line: at.line - 1,
            character: at.column - 1,
        };
        match self.context_at_uri(&uri, position) {
            Ok(context) => ToolResult::ok(
                operation,
                self.public_value(json!({
                    "requestedAt": {"uri": uri, "position": position},
                    "context": context,
                })),
            ),
            Err(error) => ToolResult::status(operation, "notFound", JsonValue::Null, error),
        }
    }

    pub fn source_context(&self, uri: &str, range: Range) -> ToolResult {
        match self.extract_source_context(uri, range) {
            Ok(context) => ToolResult::ok("source-context", self.public_value(context)),
            Err(error) => {
                ToolResult::status("source-context", "unavailable", JsonValue::Null, error)
            }
        }
    }

    fn open_document(&mut self, path: &Path) -> Result<String, String> {
        let canonical = self.resolve_project_source(path)?;
        let source = fs::read_to_string(&canonical)
            .map_err(|error| format!("could not read '{}': {error}", canonical.display()))?;
        let uri = path_to_uri(&canonical)?;
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "osiris",
                    "version": 1,
                    "text": source,
                }
            }),
        )?;
        Ok(uri)
    }

    fn resolve_project_source(&self, path: &Path) -> Result<PathBuf, String> {
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.project.root.join(path)
        };
        let canonical = fs::canonicalize(&path)
            .map_err(|error| format!("could not resolve '{}': {error}", path.display()))?;
        let under_source = self
            .project
            .source_roots
            .iter()
            .any(|root| fs::canonicalize(root).is_ok_and(|root| canonical.starts_with(root)));
        if !under_source
            || self.project.is_excluded(&canonical)
            || canonical.extension().and_then(|value| value.to_str()) != Some("osr")
        {
            return Err(format!(
                "'{}' is outside the configured, non-excluded Osiris source scope",
                path.display()
            ));
        }
        Ok(canonical)
    }

    fn request(&mut self, method: &str, params: JsonValue) -> Result<JsonValue, String> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        let machine = self
            .machine
            .as_mut()
            .ok_or_else(|| "language service is not initialized".to_owned())?;
        let outcome = machine.handle(
            &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string(),
        );
        let response = outcome
            .response
            .ok_or_else(|| format!("language server returned no response for `{method}`"))?;
        if let Some(error) = response.get("error") {
            return Err(format!(
                "language server `{method}` failed: {}",
                error["message"].as_str().unwrap_or("unknown error")
            ));
        }
        Ok(response.get("result").cloned().unwrap_or(JsonValue::Null))
    }

    fn notify(&mut self, method: &str, params: JsonValue) -> Result<(), String> {
        let machine = self
            .machine
            .as_mut()
            .ok_or_else(|| "language service is not initialized".to_owned())?;
        let outcome = machine
            .handle(&json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string());
        if let Some(error) = outcome
            .response
            .and_then(|response| response.get("error").cloned())
        {
            return Err(format!(
                "language server `{method}` failed: {}",
                error["message"].as_str().unwrap_or("unknown error")
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
