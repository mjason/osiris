use serde_json::Value as JsonValue;

use super::{SEMANTIC_DOCUMENT_VERSION, SemanticDocument};
use crate::{
    compiler::{CompileOptions, analyze},
    project::PythonVersion,
};

#[test]
fn semantic_projection_is_versioned_and_keeps_aliases_and_layers() {
    let source = r#"(module demo)
^{:doc "v"} (def value 1)
(alias 中文值 value)
(export [value 中文值])
"#;
    let analysis = analyze(source, &CompileOptions::new("demo", PythonVersion::MINIMUM));
    let document = SemanticDocument::from_analysis_at_version(&analysis, "demo.osr", 7);
    assert_eq!(document.version, SEMANTIC_DOCUMENT_VERSION);
    assert_eq!(document.document_version, 7);
    let value = document
        .symbols
        .iter()
        .find(|symbol| symbol.canonical == "value")
        .expect("value symbol");
    assert!(value.aliases.iter().any(|alias| alias.spelling == "中文值"));
    assert!(!value.metadata.authored.is_empty());
    let json: JsonValue =
        serde_json::from_str(&document.to_json().expect("json")).expect("valid json");
    assert_eq!(json["version"], SEMANTIC_DOCUMENT_VERSION);
    assert!(json["operation_graph"]["nodes"].is_array());
}

#[test]
fn authored_layer_keeps_metadata_from_a_macro_call_site() {
    let source = r#"(module demo)
(defmacro define-one [name]
  `(def ~name 1))
^{:agent/intent :demo/create}
(define-one value)
"#;
    let analysis = analyze(source, &CompileOptions::new("demo", PythonVersion::MINIMUM));
    assert!(
        analysis.diagnostics.is_empty(),
        "{:#?}",
        analysis.diagnostics
    );
    let document = SemanticDocument::from_analysis(&analysis, "demo.osr");
    assert!(
        document.authored.iter().any(|entry| {
            entry.key_text.trim_start_matches(':') == "agent/intent"
                && entry.value_text.trim_start_matches(':') == "demo/create"
        }),
        "{:#?}",
        document
            .authored
            .iter()
            .map(|entry| (&entry.key_text, &entry.value_text))
            .collect::<Vec<_>>()
    );
}

#[test]
fn operation_nodes_have_localized_labels_and_spans() {
    let analysis = analyze(
        r#"(module demo)
(def value (+ 1 2))"#,
        &CompileOptions::new("demo", PythonVersion::MINIMUM),
    );
    let document = SemanticDocument::from_analysis(&analysis, "demo.osr");
    assert!(
        document
            .operations
            .iter()
            .any(|node| node.span.end > node.span.start)
    );
    assert!(
        document
            .operations
            .iter()
            .all(|node| !node.labels.default.is_empty())
    );
}
