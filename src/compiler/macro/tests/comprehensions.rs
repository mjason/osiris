#[test]
fn for_expands_multiple_bindings_to_nested_flat_maps() {
    let output = expanded("(for [left lefts right (rights-for left)] (combine left right))");
    assert_eq!(
        output,
        "(osiris.prelude/mapcatv (fn [left] (osiris.prelude/mapv (fn [right] (do (combine left right))) (rights-for left))) lefts)\n"
    );
    assert_eq!(output.matches("lefts").count(), 1);
    assert_eq!(output.matches("(rights-for left)").count(), 1);
}

#[test]
fn for_let_and_when_preserve_clause_order_and_single_evaluation() {
    let output = expanded(
        "(for [item items :let [score (score item)] :when (eligible? score) detail (details item)] (emit item detail score))",
    );
    assert_eq!(output.matches("(score item)").count(), 1);
    assert_eq!(output.matches("(eligible? score)").count(), 1);
    assert_eq!(output.matches("(details item)").count(), 1);
    assert_eq!(output.matches("(emit item detail score)").count(), 1);
    assert!(output.contains("(fn [item] (let [score (score item)]"));
    assert!(output.contains(
            "(if (osiris.prelude/truthy* (eligible? score)) (osiris.prelude/mapv (fn [detail] (do (emit item detail score))) (details item)) [])"
        ));
}

#[test]
fn for_uses_hygienic_temporaries_for_destructured_bindings() {
    let output = expanded("(for [[left right] pairs {:keys [value]} rows] (+ left right value))");
    assert!(output.contains("(fn [item__osr_g0]"));
    assert!(output.contains("(let [[left right] item__osr_g0]"));
    assert!(output.contains("(fn [item__osr_g1]"));
    assert!(output.contains("(let [{:keys [value]} item__osr_g1]"));

    let mixed = expanded("(for [group groups {:keys [value]} group] value)");
    assert!(mixed.contains("(fn [group]"));
    assert!(mixed.contains("(fn [item__osr_g0]"));
    assert!(!mixed.contains("item__osr_g1"));
}

#[test]
fn and_and_or_preserve_short_circuit_single_evaluation() {
    let and_output = expanded("(and (first?) (second?) (third?))");
    assert_eq!(and_output.matches("(first?)").count(), 1);
    assert_eq!(and_output.matches("(second?)").count(), 1);
    assert_eq!(and_output.matches("(third?)").count(), 1);
    assert!(and_output.contains("(let [and__osr_g0 (first?)]"));
    assert!(and_output.contains("(if (osiris.prelude/truthy* and__osr_g0)"));

    let or_output = expanded("(or (first?) (second?) (third?))");
    assert_eq!(or_output.matches("(first?)").count(), 1);
    assert_eq!(or_output.matches("(second?)").count(), 1);
    assert_eq!(or_output.matches("(third?)").count(), 1);
    assert!(or_output.contains("(let [or__osr_g0 (first?)]"));
    assert!(or_output.contains("(if (osiris.prelude/truthy* or__osr_g0) or__osr_g0"));
}

#[test]
fn malformed_control_macros_report_macro_diagnostics() {
    for source in [
        "(cond ready?)",
        "(cond :else 1 later? 2)",
        "(for [] x)",
        "(for [x xs y] x)",
        "(for [:when ready? x xs] x)",
        "(for [x xs :while] x)",
        "(for [x xs :when] x)",
        "(for [x xs :let {}] x)",
        "(for [x xs :let [y]] x)",
        "(for [x xs])",
        "(condp = value 1 :one)",
        "(condp = value 1 :>>)",
        "(letfn value body)",
        "(letfn [f (fn [])])",
    ] {
        let result = expand(&read(source), ExpansionOptions::default());
        assert!(
            result
                .document
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "OSR-M0007"),
            "expected macro diagnostic for {source}: {:?}",
            result.document.diagnostics
        );
    }
}

#[test]
fn malformed_for_clauses_have_specific_diagnostics() {
    for (source, expected) in [
        (
            "(for [] value)",
            "for requires at least one pattern/collection pair",
        ),
        (
            "(for [item items detail] value)",
            "for binding pattern requires a collection expression",
        ),
        (
            "(for [item items :when] value)",
            "for :when requires a predicate expression",
        ),
        (
            "(for [item items :let {}] value)",
            "for :let requires a binding vector",
        ),
        (
            "(for [item items :let [score]] value)",
            "for :let requires an even number of binding forms",
        ),
        (
            "(for [item items :let [1 score]] value)",
            "for :let binding pattern must be a symbol, vector, or map",
        ),
        (
            "(for [item items :while] value)",
            "for :while requires a predicate expression",
        ),
        (
            "(for [item items :until ready?] value)",
            "unsupported for modifier :until; expected :let, :when, or :while",
        ),
    ] {
        let result = expand(&read(source), ExpansionOptions::default());
        assert!(
            result.document.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "OSR-M0007" && diagnostic.message == expected
            }),
            "expected `{expected}` for {source}, got: {:?}",
            result.document.diagnostics
        );
    }
}

#[test]
fn malformed_doseq_clauses_have_specific_diagnostics() {
    for (source, expected) in [
        (
            "(doseq [] value)",
            "doseq requires at least one pattern/collection pair",
        ),
        (
            "(doseq [item items :while] value)",
            "doseq :while requires a predicate expression",
        ),
        (
            "(doseq [item items :let [value]] value)",
            "doseq :let requires an even number of binding forms",
        ),
        (
            "(doseq [item items :until ready?] value)",
            "unsupported doseq modifier :until; expected :let, :when, or :while",
        ),
    ] {
        let result = expand(&read(source), ExpansionOptions::default());
        assert!(
            result.document.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "OSR-M0007" && diagnostic.message == expected
            }),
            "expected `{expected}` for {source}, got: {:?}",
            result.document.diagnostics
        );
    }
}

#[test]
fn macro_declarations_keep_syntax_quote_templates_unexpanded() {
    let source = "(defmacro pipeline [x] `(~'-> ~x (f)))";
    assert_eq!(expanded(source), format!("{source}\n"));
}

#[test]
fn malformed_threading_call_is_recoverable() {
    let result = expand(
        &read("(-> value ()) (def okay 1)"),
        ExpansionOptions::default(),
    );
    assert!(
        result
            .document
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-M0007")
    );
    assert_eq!(result.document.forms.len(), 2);
}
