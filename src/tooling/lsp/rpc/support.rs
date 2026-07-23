fn result_outcome(result: JsonValue) -> DispatchOutcome {
    DispatchOutcome {
        result: Some(result),
        notifications: Vec::new(),
    }
}

fn ensure_document(state: &LspState, uri: &str) -> Result<(), LspStateError> {
    state
        .document(uri)
        .map(|_| ())
        .ok_or_else(|| document_not_found(uri))
}

pub(in crate::lsp) fn document_not_found(uri: &str) -> LspStateError {
    LspStateError::new(DOCUMENT_NOT_FOUND, format!("document {uri} is not open"))
}

fn serialize_value<T: Serialize>(value: T) -> Result<JsonValue, LspStateError> {
    serde_json::to_value(value).map_err(|error| {
        LspStateError::new(
            INTERNAL_ERROR,
            format!("failed to serialize response: {error}"),
        )
    })
}

fn decode_params<T: for<'de> Deserialize<'de>>(params: &JsonValue) -> Result<T, LspStateError> {
    serde_json::from_value(params.clone())
        .map_err(|error| LspStateError::new(INVALID_PARAMS, format!("invalid params: {error}")))
}

fn required_uri(params: &JsonValue) -> Result<&str, LspStateError> {
    find_string_ref(params, &["uri"])
        .or_else(|| find_string_ref(params, &["textDocument", "uri"]))
        .ok_or_else(|| LspStateError::new(INVALID_PARAMS, "params require a document uri"))
}

fn find_string(value: &JsonValue, path: &[&str]) -> Option<String> {
    find_string_ref(value, path).map(str::to_owned)
}

fn find_string_ref<'a>(value: &'a JsonValue, path: &[&str]) -> Option<&'a str> {
    find_value(value, path)?.as_str()
}

fn find_value<'a>(value: &'a JsonValue, path: &[&str]) -> Option<&'a JsonValue> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    Some(current)
}

fn publish_diagnostics_notification(params: PublishDiagnosticsParams) -> JsonValue {
    json!({
        "jsonrpc": JSON_RPC_VERSION,
        "method": "textDocument/publishDiagnostics",
        "params": params,
    })
}

fn rpc_success(id: JsonValue, result: JsonValue) -> JsonValue {
    json!({
        "jsonrpc": JSON_RPC_VERSION,
        "id": id,
        "result": result,
    })
}

fn rpc_error(id: JsonValue, code: i64, message: &str, data: Option<JsonValue>) -> JsonValue {
    let mut error = json!({
        "code": code,
        "message": message,
    });
    if let Some(data) = data {
        error["data"] = data;
    }
    json!({
        "jsonrpc": JSON_RPC_VERSION,
        "id": id,
        "error": error,
    })
}

fn initialize_result() -> JsonValue {
    json!({
        "capabilities": {
            "positionEncoding": "utf-16",
            "textDocumentSync": {
                "openClose": true,
                "change": 2,
            },
            "hoverProvider": true,
            "completionProvider": {
                "triggerCharacters": ["/", ":", "."],
            },
            "signatureHelpProvider": {
                "triggerCharacters": [" ", ":"],
                "retriggerCharacters": [" ", ":"],
            },
            "definitionProvider": true,
            "referencesProvider": true,
            "renameProvider": {
                "prepareProvider": true,
            },
            "diagnosticProvider": {
                "interFileDependencies": true,
                "workspaceDiagnostics": false,
            },
            "experimental": {
                "osirisSemanticView": true,
                "osirisSyntaxSnapshot": true,
                "osirisInspect": true,
                "osirisExpandPreview": true,
            },
        },
        "serverInfo": {
            "name": LSP_SERVER_NAME,
            "version": crate::version(),
        },
    })
}
