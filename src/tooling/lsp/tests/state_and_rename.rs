use std::{
    collections::BTreeMap,
    fs,
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::{Value as JsonValue, json};

use crate::{
    compiler::{self, CompileInput, CompileOptions},
    interface,
    name::{BindingKind, CONFUSABLE_IDENTIFIER, MIXED_SCRIPT_IDENTIFIER},
    project::PythonVersion,
    syntax::NodePath,
    types::Type,
};

use super::{
    JsonRpcMachine, LspState, OpenDocument, PARSE_ERROR, Position, ProjectDocumentAnalysis, Range,
    TextDocumentContentChangeEvent, build_single_symbol_index, collect_function_interfaces,
    collect_macro_interfaces, handle_json_rpc, offset_to_position, position_to_offset,
};

const URI: &str = "file:///workspace/demo.osr";
static NEXT_WORKSPACE: AtomicUsize = AtomicUsize::new(0);

fn source() -> &'static str {
    r#"(module demo)
(defn add-one [[x Int]] -> Int (+ x 1))
(alias 加一 add-one)
(export [add-one 加一])
(def result (加一 2))
"#
}

#[test]
fn state_reuses_one_analysis_for_all_queries() {
    let mut state = LspState::new();
    let diagnostics = state.did_open(URI, 1, source());
    assert!(diagnostics.diagnostics.is_empty());
    assert_eq!(state.analysis_runs(), 1);

    let position = Position {
        line: 4,
        character: 14,
    };
    let hover = state
        .hover(URI, position, Some("zh-CN"))
        .expect("hover on alias");
    assert!(hover.contents.value.contains("add-one"));
    let definition = state
        .definition(URI, position)
        .expect("definition from alias");
    assert_eq!(definition.range.start.line, 1);
    assert!(state.references(URI, position).len() >= 2);

    let completion = state.completion(
        URI,
        Position {
            line: 5,
            character: 0,
        },
        Some("zh-CN"),
    );
    let localized = completion
        .iter()
        .find(|item| item.label == "加一")
        .expect("Chinese alias completion");
    assert_eq!(localized.insert_text, "加一");
    assert_eq!(state.analysis_runs(), 1);
}

#[test]
fn python_decorator_targets_participate_in_navigation() {
    const DECORATOR_URI: &str = "file:///workspace/decorated.osr";
    let source = r#"(module decorated)
(py/import host.runtime :as host)
(defn publish [] -> Int 1)
(alias 发布 publish)
(py/decorate 发布 host.register)
"#;
    let mut state = LspState::new();
    let diagnostics = state.did_open(DECORATOR_URI, 1, source);
    assert!(diagnostics.diagnostics.is_empty(), "{diagnostics:?}");
    let target = offset_to_position(
        source,
        source.rfind("发布 host.register").expect("decorator target"),
    );
    let definition = state
        .definition(DECORATOR_URI, target)
        .expect("decorator target definition");
    assert_eq!(definition.range.start.line, 2);
    assert!(state.references(DECORATOR_URI, target).len() >= 2);
}

#[test]
fn rename_keeps_canonical_and_alias_spelling_groups_separate() {
    let mut state = LspState::new();
    let diagnostics = state.did_open(URI, 1, source());
    assert!(diagnostics.diagnostics.is_empty(), "{diagnostics:?}");

    let canonical_position = offset_to_position(
        source(),
        source().find("add-one [[x").expect("canonical declaration"),
    );
    let prepared = state
        .prepare_rename(URI, canonical_position)
        .expect("canonical prepare range");
    assert_eq!(
        &source()[position_to_offset(source(), prepared.start).expect("start")
            ..position_to_offset(source(), prepared.end).expect("end")],
        "add-one"
    );
    let canonical = state
        .rename(URI, canonical_position, "increment")
        .expect("valid rename")
        .expect("workspace edit");
    let canonical_edits = canonical.changes.get(URI).expect("same document edits");
    assert_eq!(canonical_edits.len(), 3);
    assert!(canonical_edits.iter().all(|edit| {
        let start = position_to_offset(source(), edit.range.start).expect("edit start");
        let end = position_to_offset(source(), edit.range.end).expect("edit end");
        &source()[start..end] == "add-one" && edit.new_text == "increment"
    }));

    let alias_position = offset_to_position(
        source(),
        source().find("加一 2").expect("Chinese alias call"),
    );
    let alias = state
        .rename(URI, alias_position, "增加")
        .expect("valid alias rename")
        .expect("alias workspace edit");
    let alias_edits = alias.changes.get(URI).expect("same document edits");
    assert_eq!(alias_edits.len(), 3);
    assert!(alias_edits.iter().all(|edit| {
        let start = position_to_offset(source(), edit.range.start).expect("edit start");
        let end = position_to_offset(source(), edit.range.end).expect("edit end");
        &source()[start..end] == "加一" && edit.new_text == "增加"
    }));

    for invalid in [
        "if",
        "defn",
        "cond",
        "when-first",
        "loop",
        "recur",
        "time",
        "lazy-seq",
        "future-call",
        "binding",
        "with-open",
        "two names",
        "alpha/score",
        ":keyword",
    ] {
        let error = state
            .rename(URI, canonical_position, invalid)
            .expect_err("invalid or reserved rename target");
        assert_eq!(error.code, super::INVALID_PARAMS, "{invalid}");
    }
}

#[test]
fn json_rpc_advertises_and_dispatches_prepare_and_rename() {
    let mut machine = JsonRpcMachine::new();
    let initialized = machine.handle(
        &json!({
            "jsonrpc": "2.0",
            "id": "initialize",
            "method": "initialize",
            "params": {}
        })
        .to_string(),
    );
    assert_eq!(
        initialized.response.as_ref().expect("initialize response")["result"]["capabilities"]["renameProvider"]
            ["prepareProvider"],
        true
    );
    machine.state.did_open(URI, 1, source());
    let position = offset_to_position(source(), source().find("加一 2").expect("alias call"));

    let prepared = machine.handle(
        &json!({
            "jsonrpc": "2.0",
            "id": "prepare",
            "method": "textDocument/prepareRename",
            "params": {
                "textDocument": { "uri": URI },
                "position": position,
            }
        })
        .to_string(),
    );
    assert!(prepared.response.as_ref().expect("prepare response")["result"].is_object());

    let renamed = machine.handle(
        &json!({
            "jsonrpc": "2.0",
            "id": "rename",
            "method": "textDocument/rename",
            "params": {
                "textDocument": { "uri": URI },
                "position": position,
                "newName": "增加",
            }
        })
        .to_string(),
    );
    let response = renamed.response.as_ref().expect("rename response");
    assert_eq!(
        response["result"]["changes"][URI].as_array().map(Vec::len),
        Some(3)
    );
}

#[test]
fn rename_validates_collisions_normalizes_nfc_and_emits_utf16_ranges() {
    let source = r#"(module demo)
(defn first [[x Int]] -> Int x)
(alias 第一 first)
(export [first 第一])
(defn second [[x Int]] -> Int x)
(def value (do "😀" (第一 1)))
"#;
    let mut state = LspState::new();
    let diagnostics = state.did_open(URI, 1, source);
    assert!(diagnostics.diagnostics.is_empty(), "{diagnostics:?}");
    let first = offset_to_position(source, source.find("first [[x").expect("first"));
    let alias_offset = source.rfind("第一 1").expect("alias call");
    let alias = offset_to_position(source, alias_offset);

    for (position, collision) in [(first, "second"), (first, "第一"), (alias, "first")] {
        let error = state
            .rename(URI, position, collision)
            .expect_err("obvious declaration collision");
        assert_eq!(error.code, super::INVALID_PARAMS);
    }

    let prepared = state
        .prepare_rename(URI, alias)
        .expect("UTF-16 prepare range");
    let line_start = source[..alias_offset]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    assert_eq!(
        prepared.start.character,
        source[line_start..alias_offset].encode_utf16().count() as u32
    );
    assert_eq!(prepared.end.character, prepared.start.character + 2);

    let edit = state
        .rename(URI, alias, "e\u{301}")
        .expect("NFC rename")
        .expect("NFC edits");
    let edits = edit.changes.get(URI).expect("document edits");
    assert_eq!(edits.len(), 3);
    assert!(edits.iter().all(|edit| edit.new_text == "é"));
    assert!(edits.windows(2).all(|pair| {
        (pair[0].range.start.line, pair[0].range.start.character)
            < (pair[1].range.start.line, pair[1].range.start.character)
    }));
}

#[test]
fn signature_help_capability_and_local_typed_call_use_localized_parameters() {
    let source = r#"(module demo)
(defn rolling
  [[values Float]
   ^{:osiris/names {"zh-CN" {:preferred 周期 :aliases [窗口]}}}
   [window Int = 14]]
  -> Float
  values)
(def result (rolling 1.0 ))
"#;
    let mut machine = JsonRpcMachine::new();
    let initialized = machine.handle(
        &json!({
            "jsonrpc": "2.0",
            "id": "initialize",
            "method": "initialize",
            "params": {}
        })
        .to_string(),
    );
    assert_eq!(
        initialized.response.as_ref().expect("initialize response")["result"]["capabilities"]["signatureHelpProvider"]
            ["triggerCharacters"],
        json!([" ", ":"])
    );
    let diagnostics = machine.state.did_open(URI, 1, source);
    assert!(diagnostics.diagnostics.is_empty(), "{diagnostics:?}");
    let cursor = source.find("(rolling 1.0 )").expect("call") + "(rolling 1.0 ".len();
    let position = offset_to_position(source, cursor);

    let response = machine.handle(
        &json!({
            "jsonrpc": "2.0",
            "id": "signature",
            "method": "textDocument/signatureHelp",
            "params": {
                "textDocument": { "uri": URI },
                "position": position,
                "locale": "zh-CN"
            }
        })
        .to_string(),
    );

    let result = &response.response.as_ref().expect("signature response")["result"];
    assert_eq!(result["activeSignature"], 0);
    assert_eq!(result["activeParameter"], 1);
    assert_eq!(
        result["signatures"][0]["label"],
        "rolling(values: Float, 周期: Int = 14) -> Float"
    );
    assert_eq!(
        result["signatures"][0]["parameters"][1]["label"],
        "周期: Int = 14"
    );
}
