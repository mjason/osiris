use crate::{
    compiler::{CompileOptions, compile},
    printer::render_document_text,
    project::PythonVersion,
    reader::read,
    syntax::{Form, FormKind, METADATA_TARGET_LIMITS},
};

use super::{ExpansionOptions, ImportedPhaseModule, expand, expand_with_imported_phase_modules};

fn expanded(source: &str) -> String {
    let result = expand(&read(source), ExpansionOptions::default());
    assert!(
        result.document.diagnostics.is_empty(),
        "{:?}",
        result.document.diagnostics
    );
    render_document_text(&result.document)
}

fn imported_module(
    namespace: &str,
    source: &str,
    macro_names: &[(&str, &str)],
) -> ImportedPhaseModule {
    let document = read(source);
    assert!(
        document.diagnostics.is_empty(),
        "{:?}",
        document.diagnostics
    );
    ImportedPhaseModule::new(
        namespace,
        document.forms,
        macro_names
            .iter()
            .map(|(visible, target)| ((*visible).to_owned(), (*target).to_owned()))
            .collect(),
    )
}

#[test]
fn imported_modules_isolate_same_named_helpers_and_macros() {
    let first = imported_module(
        "dep.first",
        "(defn-for-syntax helper [value] `(from-first ~value))\n\
             (defmacro wrap [value] (helper value))",
        &[("first/wrap", "wrap")],
    );
    let second = imported_module(
        "dep.second",
        "(defn-for-syntax helper [value] `(from-second ~value))\n\
             (defmacro wrap [value] (helper value))",
        &[("second/wrap", "wrap"), ("wrap", "wrap")],
    );
    let result = expand_with_imported_phase_modules(
        &read("(first/wrap x) (wrap y)"),
        &[first, second],
        ExpansionOptions::default(),
    );
    assert!(
        result.document.diagnostics.is_empty(),
        "{:?}",
        result.document.diagnostics
    );
    assert_eq!(
        render_document_text(&result.document),
        "(from-first x)\n(from-second y)\n"
    );
    assert_eq!(
        result
            .traces
            .iter()
            .map(|trace| trace.macro_binding_id.as_str())
            .collect::<Vec<_>>(),
        ["dep.first::macro::wrap", "dep.second::macro::wrap"]
    );
}

#[test]
fn local_macro_trace_uses_the_declared_module_binding_id() {
    let result = expand(
        &read("(module local.core) (defmacro wrap [value] value) (wrap 1)"),
        ExpansionOptions::default(),
    );

    assert!(result.document.diagnostics.is_empty());
    assert_eq!(result.traces.len(), 1);
    assert_eq!(result.traces[0].macro_binding_id, "local.core::macro::wrap");
}

#[test]
fn declaration_macros_can_generate_python_decorators() {
    let output = expanded(
        "(defmacro defregistered [name]\n\
           `(do (defn ~name [] none) (py/decorate ~name register)))\n\
         (defregistered handler)",
    );
    assert!(output.contains("(defn handler [] none)"), "{output}");
    assert!(output.contains("(py/decorate handler register)"), "{output}");
}

#[test]
fn imported_macros_require_an_explicit_visible_name() {
    let module = imported_module(
        "dep.first",
        "(defn-for-syntax helper [value] `(wrapped ~value))\n\
             (defmacro wrap [value] (helper value))",
        &[("first/wrap", "wrap")],
    );
    let result = expand_with_imported_phase_modules(
        &read("(wrap short) (wrong/wrap wrong) (first/wrap right)"),
        &[module],
        ExpansionOptions::default(),
    );
    assert!(
        result.document.diagnostics.is_empty(),
        "{:?}",
        result.document.diagnostics
    );
    assert_eq!(
        render_document_text(&result.document),
        "(wrap short)\n(wrong/wrap wrong)\n(wrapped right)\n"
    );
}

#[test]
fn imported_syntax_quote_resolves_exported_names_at_the_definition_site() {
    let module = imported_module(
        "dep.component",
        "(defmacro declare [name] `(static-record Descriptor ~name {}))",
        &[("component/declare", "declare")],
    )
    .with_definition_names(
        [("Descriptor".to_owned(), "Descriptor".to_owned())]
            .into_iter()
            .collect(),
    );
    let result = expand_with_imported_phase_modules(
        &read("(component/declare normalize)"),
        &[module],
        ExpansionOptions::default(),
    );
    assert!(
        result.document.diagnostics.is_empty(),
        "{:?}",
        result.document.diagnostics
    );
    assert_eq!(
        render_document_text(&result.document),
        "(static-record dep.component/Descriptor normalize {})\n"
    );
}

#[test]
fn duplicate_identical_namespace_loads_merge_explicit_exposures() {
    let source = "(defn-for-syntax helper [value] `(wrapped ~value))\n\
                      (defmacro wrap [value] (helper value))";
    let qualified = imported_module("dep.shared", source, &[("shared/wrap", "wrap")]);
    let referred = imported_module("dep.shared", source, &[("wrap", "wrap")]);
    let result = expand_with_imported_phase_modules(
        &read("(shared/wrap one) (wrap two)"),
        &[qualified, referred],
        ExpansionOptions::default(),
    );
    assert!(
        result.document.diagnostics.is_empty(),
        "{:?}",
        result.document.diagnostics
    );
    assert_eq!(
        render_document_text(&result.document),
        "(wrapped one)\n(wrapped two)\n"
    );
}

#[test]
fn inconsistent_duplicate_namespace_loads_are_rejected() {
    let first = imported_module(
        "dep.shared",
        "(defmacro wrap [value] `(first ~value))",
        &[("shared/wrap", "wrap")],
    );
    let second = imported_module(
        "dep.shared",
        "(defmacro wrap [value] `(second ~value))",
        &[("shared/wrap", "wrap")],
    );
    let result = expand_with_imported_phase_modules(
        &read("(shared/wrap value)"),
        &[first, second],
        ExpansionOptions::default(),
    );
    assert!(
        result
            .document
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-M0003")
    );
    assert_eq!(
        render_document_text(&result.document),
        "(shared/wrap value)\n"
    );
}

#[test]
fn imported_modules_share_the_existing_expansion_budget() {
    let first = imported_module(
        "dep.first",
        "(defmacro wrap [value] `(first ~value))",
        &[("first/wrap", "wrap")],
    );
    let second = imported_module(
        "dep.second",
        "(defmacro wrap [value] `(second ~value))",
        &[("second/wrap", "wrap")],
    );
    let result = expand_with_imported_phase_modules(
        &read("(first/wrap one) (second/wrap two)"),
        &[first, second],
        ExpansionOptions {
            once: false,
            max_expansions: 1,
        },
    );
    assert!(
        result
            .document
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-M0002")
    );
}

#[test]
fn macros_cannot_generate_top_level_boundary_declarations() {
    for declaration in [
        "module dep.generated",
        "import dep.generated",
        "import-for-syntax dep.generated",
        "py/import dep.generated",
        "export [value]",
        "alias local target",
        "defmacro generated [] `1",
        "defn-for-syntax generated [] 1",
        "defstatic-schema Generated :schema-id \"generated\" :version 1",
    ] {
        let source = format!("(defmacro emit [] '({declaration}))\n(emit)\n(def value 1)");
        let result = expand(&read(&source), ExpansionOptions::default());
        assert!(
            result
                .document
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "OSR-M0008"),
            "generated declaration should be rejected: {declaration}\n{:?}",
            result.document.diagnostics
        );
        assert!(matches!(
            result.document.forms.get(1).map(|form| &form.kind),
            Some(FormKind::Error(message))
                if message == "macro-generated top-level declaration"
        ));
        assert!(matches!(
            result.document.forms.get(2).map(|form| &form.kind),
            Some(FormKind::List(items))
                if items.first().and_then(super::symbol_canonical) == Some("def")
        ));
    }
}

#[test]
fn authored_boundary_declarations_are_not_macro_expanded() {
    let source = "(defmacro module-name [] 'generated)\n(module (module-name))";
    let result = expand(&read(source), ExpansionOptions::default());
    assert!(
        result.document.diagnostics.is_empty(),
        "{:?}",
        result.document.diagnostics
    );
    assert!(result.traces.is_empty());
    assert!(matches!(
        result.document.forms.get(1).map(|form| &form.kind),
        Some(FormKind::List(items))
            if items.first().and_then(super::symbol_canonical) == Some("module")
                && matches!(items.get(1).map(|form| &form.kind), Some(FormKind::List(_)))
    ));
}

#[test]
fn declaration_macros_can_still_generate_runtime_declarations() {
    let source = "(defmacro emit [] '(def generated 1))\n(emit)";
    let result = expand(&read(source), ExpansionOptions::default());
    assert!(
        result.document.diagnostics.is_empty(),
        "{:?}",
        result.document.diagnostics
    );
    assert!(matches!(
        result.document.forms.get(1).map(|form| &form.kind),
        Some(FormKind::List(items))
            if items.first().and_then(super::symbol_canonical) == Some("def")
    ));
}

#[test]
fn declaration_macros_can_generate_ordered_declaration_sequences() {
    let source = r#"
            (defmacro emit []
              '(do
                 (def generated 1)
                 (do
                   (defn generated-fn [] -> Int generated)
                   (static-record Schema generated {:id "generated"}))))
            (emit)
        "#;
    let result = expand(&read(source), ExpansionOptions::default());
    assert!(
        result.document.diagnostics.is_empty(),
        "{:?}",
        result.document.diagnostics
    );
    assert_eq!(result.document.forms.len(), 4);
    assert_eq!(
        result.document.forms[1..]
            .iter()
            .filter_map(|form| match &form.kind {
                FormKind::List(items) => items.first().and_then(super::symbol_canonical),
                _ => None,
            })
            .collect::<Vec<_>>(),
        ["def", "defn", "static-record"]
    );
}

#[test]
fn declaration_sequences_cannot_hide_module_graph_declarations() {
    let source = "(defmacro emit [] '(do (def value 1) (do (import hidden))))\n(emit)";
    let result = expand(&read(source), ExpansionOptions::default());
    assert!(
        result
            .document
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-M0008")
    );
    assert!(matches!(
        result.document.forms.get(1).map(|form| &form.kind),
        Some(FormKind::Error(message))
            if message == "macro-generated top-level declaration"
    ));
}
