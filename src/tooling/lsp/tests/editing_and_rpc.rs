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
fn public_metadata_contract_errors_are_published_during_analysis() {
    let mut state = LspState::new();
    let diagnostics = state.did_open(
        "file:///workspace/metadata-contract.osr",
        1,
        "(module metadata-contract)\n(def ^Int value 1)\n(export [value])\n",
    );
    assert!(
        diagnostics
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-I0087")
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
                "locale": "zh-CN"
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

    let symbols = machine.handle(
        &json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "osiris/symbol",
            "params": { "uri": URI, "symbol": "加一" }
        })
        .to_string(),
    );
    assert_eq!(
        symbols.response.as_ref().expect("response")["result"][0]["canonical"],
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
fn standard_hover_definition_and_virtual_source_share_one_artifact() {
    let mut machine = JsonRpcMachine::new();
    machine.handle(
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "locale": "zh-CN" }
        })
        .to_string(),
    );
    machine.handle(
        &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": URI,
                "languageId": "osiris",
                "version": 1,
                "text": "(module demo)\n(import osiris.concurrent :refer [pmap])\n(defn ^{:type (Vector Int)} run\n [^{:type (Fn [Int] -> Int)} function ^{:type (Vector Int)} values]\n  (pmap function values))\n"
            }}
        })
        .to_string(),
    );
    let position = json!({ "line": 4, "character": 4 });
    let hover = machine.handle(
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "textDocument/hover",
            "params": { "textDocument": { "uri": URI }, "position": position }
        })
        .to_string(),
    );
    let hover_text = hover.response.as_ref().unwrap()["result"]["contents"]["value"]
        .as_str()
        .unwrap();
    assert!(
        hover_text.contains("立即提交映射任务，保持结果顺序，并传播解引用失败。"),
        "{hover_text}"
    );
    assert!(hover_text.contains("osiris.concurrent::function::pmap"));

    let definition = machine.handle(
        &json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "textDocument/definition",
            "params": { "textDocument": { "uri": URI }, "position": position }
        })
        .to_string(),
    );
    let location = &definition.response.as_ref().unwrap()["result"];
    let standard_uri = location["uri"].as_str().unwrap();
    assert!(standard_uri.starts_with("osiris-stdlib:///"));

    let source = machine.handle(
        &json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "osiris/standardSource",
            "params": { "uri": standard_uri }
        })
        .to_string(),
    );
    let source = &source.response.as_ref().unwrap()["result"];
    assert_eq!(source["uri"], standard_uri);
    let line = location["range"]["start"]["line"].as_u64().unwrap() as usize;
    assert!(source["text"].as_str().unwrap().lines().nth(line).unwrap().contains("pmap"));
}

#[test]
fn expand_preview_includes_macro_trace() {
    let mut state = LspState::new();
    state.did_open(
        URI,
        1,
        "(module demo)\n(import osiris.core :refer :all)\n(def result (-> 1 (+ 2)))\n",
    );
    let preview = state.expand_preview(URI).expect("preview");
    assert!(preview.text.contains("(+ 1 2)"));
    assert!(!preview.macro_traces.is_empty());
    assert_eq!(
        preview.macro_traces[0].macro_binding_id,
        "osiris.core::macro::->"
    );
    let serialized = serde_json::to_value(preview).expect("preview serializes");
    assert_eq!(
        serialized["macroTraces"][0]["macro_binding_id"],
        "osiris.core::macro::->"
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
