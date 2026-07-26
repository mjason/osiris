use std::{fs, path::PathBuf};

use serde_json::{Value as JsonValue, json};

use crate::lsp::{Position, Range};

use super::{
    WorkspaceService,
    source::{
        bound_document_symbols, json_position, percent_decode, source_context_from_text,
        uri_to_path,
    },
};

const MAX_REFERENCE_CONTEXTS: usize = 5;

impl WorkspaceService {
    pub(super) fn context_at_uri(
        &mut self,
        uri: &str,
        position: Position,
    ) -> Result<JsonValue, String> {
        if uri.starts_with("file://") {
            let path = uri_to_path(uri)?;
            let _ = self.resolve_project_source(&path)?;
            self.open_document(&path)?;
        }
        let params = || json!({"textDocument": {"uri": uri}, "position": position});
        let hover = self.optional_request("hoverProvider", "textDocument/hover", params());
        let definition =
            self.optional_request("definitionProvider", "textDocument/definition", params());
        let references = self.optional_request(
            "referencesProvider",
            "textDocument/references",
            json!({
                "textDocument": {"uri": uri},
                "position": position,
                "context": {"includeDeclaration": true},
            }),
        );
        let mut document_symbols = self.optional_request(
            "documentSymbolProvider",
            "textDocument/documentSymbol",
            json!({"textDocument": {"uri": uri}}),
        );
        bound_document_symbols(&mut document_symbols, position);

        let mut signature = self.optional_request(
            "signatureHelpProvider",
            "textDocument/signatureHelp",
            params(),
        );
        if signature["status"] == "ok" && signature["value"].is_null() {
            if let Some(locations) = references["value"].as_array() {
                for location in locations.iter().take(MAX_REFERENCE_CONTEXTS) {
                    let Some(reference_uri) = location["uri"].as_str() else {
                        continue;
                    };
                    let Some(reference_position) = json_position(location.pointer("/range/start"))
                    else {
                        continue;
                    };
                    let candidate = self.optional_request(
                        "signatureHelpProvider",
                        "textDocument/signatureHelp",
                        json!({
                            "textDocument": {"uri": reference_uri},
                            "position": reference_position,
                        }),
                    );
                    if !candidate["value"].is_null() {
                        signature = candidate;
                        break;
                    }
                }
            }
            if signature["value"].is_null() {
                signature["status"] = json!("notApplicable");
                signature["message"] = json!(
                    "no call site produced signature help; use hover/type facts for the declaration"
                );
            }
        }

        let definition_context = definition["value"].as_object().and_then(|location| {
            let uri = location.get("uri")?.as_str()?;
            let range = serde_json::from_value(location.get("range")?.clone()).ok()?;
            self.extract_source_context(uri, range).ok()
        });
        let reference_contexts = references["value"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|location| {
                let uri = location["uri"].as_str()?;
                let range = serde_json::from_value(location["range"].clone()).ok()?;
                self.extract_source_context(uri, range).ok()
            })
            .take(MAX_REFERENCE_CONTEXTS)
            .collect::<Vec<_>>();

        Ok(json!({
            "at": {"uri": uri, "position": position},
            "hover": hover,
            "definition": definition,
            "signatureHelp": signature,
            "references": references,
            "documentSymbols": document_symbols,
            "definitionSource": definition_context,
            "referenceSources": reference_contexts,
        }))
    }

    fn optional_request(&mut self, capability: &str, method: &str, params: JsonValue) -> JsonValue {
        if !self.capability_enabled(capability) {
            return json!({
                "status": "unavailable",
                "value": null,
                "message": format!("server capability `{capability}` is unavailable"),
            });
        }
        match self.request(method, params) {
            Ok(value) => json!({"status": "ok", "value": value}),
            Err(error) => json!({"status": "error", "value": null, "message": error}),
        }
    }

    pub(super) fn capability_enabled(&self, name: &str) -> bool {
        !matches!(
            self.capabilities.get(name),
            None | Some(JsonValue::Null) | Some(JsonValue::Bool(false))
        )
    }

    pub(super) fn extract_source_context(
        &self,
        uri: &str,
        range: Range,
    ) -> Result<JsonValue, String> {
        if uri.starts_with("osiris-stdlib:///") {
            let source = crate::stdlib::source_artifact_by_uri(uri)
                .ok_or_else(|| format!("standard source `{uri}` is unavailable"))?;
            return source_context_from_text(uri, &source, range);
        }
        let path = self.resolve_project_source(&self.path_from_source_uri(uri)?)?;
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("could not read '{}': {error}", path.display()))?;
        source_context_from_text(uri, &source, range)
    }

    pub(super) fn uri_for_lsp(&mut self, uri: &str) -> Result<String, String> {
        if uri.starts_with("osiris-workspace:///") {
            let path = self.path_from_source_uri(uri)?;
            self.open_document(&path)
        } else {
            Ok(uri.to_owned())
        }
    }

    fn path_from_source_uri(&self, uri: &str) -> Result<PathBuf, String> {
        if let Some(relative) = uri.strip_prefix("osiris-workspace:///") {
            let relative = percent_decode(relative)?;
            let relative = PathBuf::from(relative);
            if relative.is_absolute()
                || relative
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
            {
                return Err(format!("invalid workspace source URI `{uri}`"));
            }
            return Ok(self.project.root.join(relative));
        }
        uri_to_path(uri)
    }

    pub(super) fn public_value(&self, mut value: JsonValue) -> JsonValue {
        let root =
            fs::canonicalize(&self.project.root).unwrap_or_else(|_| self.project.root.clone());
        let file_prefix = format!("file://{}/", root.display());
        fn rewrite(value: &mut JsonValue, prefix: &str) {
            match value {
                JsonValue::String(text) => {
                    if let Some(relative) = text.strip_prefix(prefix) {
                        *text = format!("osiris-workspace:///{relative}");
                    }
                }
                JsonValue::Array(values) => {
                    for value in values {
                        rewrite(value, prefix);
                    }
                }
                JsonValue::Object(values) => {
                    for value in values.values_mut() {
                        rewrite(value, prefix);
                    }
                }
                _ => {}
            }
        }
        rewrite(&mut value, &file_prefix);
        value
    }
}
