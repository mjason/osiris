#[test]
fn full_and_ranged_changes_update_versions_once() {
    let mut state = LspState::new();
    state.did_open(URI, 4, "(module demo)\n(def value 1)\n");
    let changed = state
        .did_change_full(URI, 5, "(module demo)\n(def value missing)\n")
        .expect("new version");
    assert!(!changed.diagnostics.is_empty());
    assert_eq!(state.analysis_runs(), 2);
    assert_eq!(state.document_version(URI), Some(5));

    let stale = state.did_change_full(URI, 5, "(module demo)");
    assert!(stale.is_err());
    assert_eq!(state.analysis_runs(), 2);

    let repaired = state
        .did_change(
            URI,
            6,
            &[TextDocumentContentChangeEvent {
                range: Some(Range {
                    start: Position {
                        line: 1,
                        character: 11,
                    },
                    end: Position {
                        line: 1,
                        character: 18,
                    },
                }),
                range_length: Some(7),
                text: "1".to_owned(),
            }],
        )
        .expect("range change");
    assert!(repaired.diagnostics.is_empty());
    assert_eq!(state.analysis_runs(), 3);
}

#[test]
fn document_changes_reuse_incremental_reader_node_identities() {
    let uri = "file:///workspace/incremental.osr";
    let mut state = LspState::new();
    state.did_open(uri, 1, "(def first 1)\n(def second 2)\n");
    let retained = state
        .document(uri)
        .expect("open document")
        .analysis
        .document
        .node_id(&NodePath::top_level(1))
        .expect("second form identity");

    state
        .did_change_full(uri, 2, "; note\n(def first 10)\n(def second 2)\n")
        .expect("changed document");
    let changed = state.document(uri).expect("changed document");
    assert_eq!(
        changed.analysis.document.node_id(&NodePath::top_level(1)),
        Some(retained)
    );
}

#[test]
fn strict_unicode_lints_are_lsp_warnings_not_compiler_errors() {
    let uri = "file:///workspace/unicode-lint.osr";
    let mut state = LspState::new();
    let diagnostics = state.did_open(uri, 1, "(def pаypal 1)\n");
    assert!(
        !state
            .document(uri)
            .expect("open document")
            .analysis
            .has_errors()
    );
    assert!(diagnostics.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == CONFUSABLE_IDENTIFIER && diagnostic.severity == 2
    }));
    assert!(diagnostics.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == MIXED_SCRIPT_IDENTIFIER && diagnostic.severity == 2
    }));
    assert!(
        diagnostics
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic.code == CONFUSABLE_IDENTIFIER
                    || diagnostic.code == MIXED_SCRIPT_IDENTIFIER
            })
            .all(|diagnostic| diagnostic.data["nodeId"].is_u64())
    );
}

#[test]
fn positions_use_lsp_utf16_units() {
    let source = "a😀中\nvalue";
    let offset = source.find('中').expect("Chinese character");
    let position = offset_to_position(source, offset);
    assert_eq!(
        position,
        Position {
            line: 0,
            character: 3,
        }
    );
    assert_eq!(position_to_offset(source, position), Some(offset));
    assert_eq!(
        position_to_offset(
            source,
            Position {
                line: 0,
                character: 2,
            }
        ),
        None
    );
}

#[test]
fn json_rpc_transcript_recovers_and_exposes_semantics() {
    let mut machine = JsonRpcMachine::new();
    let initialize = machine.handle(
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "initializationOptions": { "displayLocale": "zh-CN" }
            }
        })
        .to_string(),
    );
    assert_eq!(initialize.response.as_ref().expect("response")["id"], 1);

    let opened = machine.handle(
        &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": URI,
                    "languageId": "osiris",
                    "version": 1,
                    "text": source(),
                }
            }
        })
        .to_string(),
    );
    assert!(opened.response.is_none());
    assert_eq!(
        opened.notifications[0]["method"],
        "textDocument/publishDiagnostics"
    );

    let semantic = machine.handle(
        &json!({
            "jsonrpc": "2.0",
            "id": "semantic",
            "method": "osiris/semanticView",
            "params": { "textDocument": { "uri": URI }, "locale": "zh-CN" }
        })
        .to_string(),
    );
    let result = &semantic.response.as_ref().expect("response")["result"];
    assert_eq!(result["version"], 1);
    assert_eq!(result["document_version"], 1);
    assert!(result["symbols"].is_array());
    assert!(result["operation_graph"]["nodes"].is_array());

    let inspected = machine.handle(
        &json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "osiris/inspect",
            "params": { "uri": URI, "symbol": "加一" }
        })
        .to_string(),
    );
    assert_eq!(
        inspected.response.as_ref().expect("response")["result"]["canonical"],
        "add-one"
    );

    let malformed = handle_json_rpc(&mut machine.state, "{broken");
    assert_eq!(
        malformed.response.as_ref().expect("parse response")["error"]["code"],
        PARSE_ERROR
    );
    assert_eq!(machine.state.analysis_runs(), 1);

    let syntax = machine.handle(
        &json!({
            "jsonrpc": "2.0",
            "id": "syntax",
            "method": "osiris/syntaxSnapshot",
            "params": { "textDocument": { "uri": URI } }
        })
        .to_string(),
    );
    let syntax_result = &syntax.response.as_ref().expect("response")["result"];
    assert_eq!(syntax_result["version"], 1);
    assert_eq!(syntax_result["documentVersion"], 1);
    assert!(
        syntax_result["nodes"]
            .as_array()
            .is_some_and(|nodes| !nodes.is_empty())
    );
    assert!(syntax_result["nodes"][0]["id"].is_u64());
}

#[test]
fn expand_preview_includes_macro_trace() {
    let mut state = LspState::new();
    state.did_open(URI, 1, "(module demo)\n(def result (-> 1 (+ 2)))\n");
    let preview = state.expand_preview(URI).expect("preview");
    assert!(preview.text.contains("(+ 1 2)"));
    assert!(!preview.macro_traces.is_empty());
    assert_eq!(
        preview.macro_traces[0].macro_binding_id,
        "osiris.prelude::macro::->"
    );
    let serialized = serde_json::to_value(preview).expect("preview serializes");
    assert_eq!(
        serialized["macroTraces"][0]["macro_binding_id"],
        "osiris.prelude::macro::->"
    );
}

#[test]
fn malformed_request_has_json_rpc_error_shape() {
    let mut state = LspState::new();
    let output = handle_json_rpc(&mut state, "[]");
    let response: &JsonValue = output.response.as_ref().expect("error response");
    assert_eq!(response["jsonrpc"], "2.0");
    assert!(response["error"]["code"].is_number());
}
