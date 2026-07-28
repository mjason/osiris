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

/// Macros are erased before typed HIR, so nothing downstream of the semantic
/// model can see them unless the projection reconstructs them from the surface
/// declaration and the spans expansion recorded.
#[test]
fn macros_are_projected_as_symbols_with_their_call_sites() {
    let source = r#"(module demo)
^{:doc "Double the expression."} (defmacro twice [x] `(+ ~x ~x))
(def a (twice 3))
(def b (twice 4))
"#;
    let analysis = analyze(source, &CompileOptions::new("demo", PythonVersion::MINIMUM));
    assert!(
        analysis.diagnostics.is_empty(),
        "{:#?}",
        analysis.diagnostics
    );

    let document = SemanticDocument::from_analysis(&analysis, "demo.osr");
    let macro_symbol = document
        .symbols
        .iter()
        .find(|symbol| symbol.binding_id == "demo::macro::twice")
        .expect("the macro is a semantic symbol");

    assert_eq!(macro_symbol.kind, crate::name::BindingKind::Macro);
    assert_eq!(
        macro_symbol.documentation.default.as_deref(),
        Some("Double the expression.")
    );
    assert_eq!(
        macro_symbol.references.len(),
        2,
        "both call sites are recorded: {:?}",
        macro_symbol.references
    );

    // Occurrences must name the macro, not span the whole call, so that they
    // mean what they mean for every other symbol.
    for span in &macro_symbol.references {
        assert_eq!(&source[span.start..span.end], "twice");
    }
    assert_eq!(
        &source[macro_symbol.definition.start..macro_symbol.definition.end],
        "twice",
        "the definition names the macro rather than the whole declaration"
    );

    // A position inside the call but outside the name must not answer with the
    // macro; the argument is not part of it.
    let argument = source.find("twice 3").expect("call") + "twice ".len();
    assert!(
        document
            .symbol_at(argument)
            .is_none_or(|symbol| symbol.binding_id != "demo::macro::twice"),
        "the macro answers for its argument position"
    );
}

/// A module that only calls a macro still needs a symbol for it, or a reference
/// cannot resolve in the file the reader has open.
#[test]
fn a_macro_called_from_another_module_is_projected_at_its_call_site() {
    let source = "(module demo.caller)\n(import demo.lib :refer [twice])\n(def a (twice 3))\n";
    let provider = "(module demo.lib)\n(export [twice])\n^{:doc \"Double.\"} (defmacro twice [x] `(+ ~x ~x))\n";
    let provider_options = CompileOptions::new("demo.lib", PythonVersion::MINIMUM)
        .with_expected_module_name("demo.lib");
    let caller_options = CompileOptions::new("demo.caller", PythonVersion::MINIMUM)
        .with_expected_module_name("demo.caller");
    let inputs = [
        crate::compiler::CompileInput::new(provider, &provider_options),
        crate::compiler::CompileInput::new(source, &caller_options),
    ];
    let analyses =
        crate::compiler::analyze_workspace_recovering(&inputs, &std::collections::BTreeMap::new());
    let caller = analyses.last().expect("caller analysis");

    let document = SemanticDocument::from_analysis(caller, "caller.osr");
    let macro_symbol = document
        .symbols
        .iter()
        .find(|symbol| symbol.binding_id == "demo.lib::macro::twice")
        .expect("the referenced macro is a semantic symbol in the calling module");

    assert_eq!(macro_symbol.kind, crate::name::BindingKind::Macro);
    assert_eq!(macro_symbol.references.len(), 1);
    let span = macro_symbol.references[0];
    assert_eq!(&source[span.start..span.end], "twice");
}
