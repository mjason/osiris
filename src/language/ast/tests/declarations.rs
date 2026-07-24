#[test]
fn no_module_header_yields_none_name() {
    let lowered = lower_document(&read("(def x 1)"));
    assert!(lowered.module.name.is_none());
}

#[test]
fn scalar_form_kinds_are_not_lost() {
    let lowered = lower_document(&read("none true 1 1.0 \"s\" :k"));
    assert_eq!(lowered.module.items.len(), 6);
    assert!(matches!(
        lowered.module.items[0].kind,
        ItemKind::Expr(ref expr) if matches!(expr.kind, ExprKind::None)
    ));
    assert!(matches!(
        lowered.module.items[5].kind,
        ItemKind::Expr(ref expr) if matches!(expr.kind, ExprKind::Keyword(_))
    ));
}

#[test]
fn python_decorators_are_explicit_executable_declarations() {
    let lowered = lower_document(&read(
        r#"(py/decorate publish
               register
               (register :extra-data {"columns" ["value"]}))"#,
    ));
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let ItemKind::PyDecorate(declaration) = &lowered.module.items[0].kind else {
        panic!("expected py/decorate declaration");
    };
    assert_eq!(declaration.target.canonical, "publish");
    assert_eq!(declaration.decorators.len(), 2);
    assert!(matches!(declaration.decorators[0].kind, ExprKind::Name(_)));
    assert!(matches!(declaration.decorators[1].kind, ExprKind::Call(_)));
}

#[test]
fn python_decorator_requires_a_target_and_expression() {
    let lowered = lower_document(&read("(py/decorate) (py/decorate publish)"));
    assert_eq!(lowered.module.items.len(), 2);
    assert!(
        lowered
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("requires a target"))
    );
    assert!(
        lowered
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("at least one decorator"))
    );
}

#[test]
fn def_uses_metadata_for_type_only_declarations() {
    let lowered = lower_document(&read(
        "(def array (np.asarray [1 2]))\n\
             (def point (Point :x 1))\n\
             (def ^{:type (Array Float [:time])} declared)",
    ));
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);

    for index in 0..2 {
        let ItemKind::Def(definition) = &lowered.module.items[index].kind else {
            panic!("expected def");
        };
        assert!(definition.type_annotation.is_none());
        assert!(matches!(
            definition.value.as_ref().map(|value| &value.kind),
            Some(ExprKind::Call(_))
        ));
    }
    let ItemKind::Def(declared) = &lowered.module.items[2].kind else {
        panic!("expected def");
    };
    assert!(declared.type_annotation.is_some());
    assert!(declared.value.is_none());
}

#[test]
fn malformed_declaration_still_has_a_recoverable_item() {
    let lowered = lower_document(&read("(defn 1) (def okay 2)"));
    assert!(
        lowered
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == super::AST_INVALID_NAME)
    );
    assert_eq!(lowered.module.items.len(), 2);
    assert!(matches!(lowered.module.items[1].kind, ItemKind::Def(_)));
}

#[test]
fn map_and_set_expression_nodes_are_structured() {
    let lowered = lower_document(&read("{:a 1} #{:x :y}"));
    assert_eq!(lowered.module.items.len(), 2);
    assert!(matches!(
        lowered.module.items[0].kind,
        ItemKind::Expr(ref expr) if matches!(expr.kind, ExprKind::Map(_))
    ));
    assert!(matches!(
        lowered.module.items[1].kind,
        ItemKind::Expr(ref expr) if matches!(expr.kind, ExprKind::Set(_))
    ));
}

#[test]
fn static_record_uses_a_single_map_with_keyword_fields() {
    let lowered = lower_document(&read(
        "(static-record component/ComponentDescriptor normalize
               {:component/id \"example.normalize\"
                :component/enabled true})",
    ));
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let record = match &lowered.module.items[0].kind {
        ItemKind::StaticRecord(record) => record,
        other => panic!("expected static-record, got {other:?}"),
    };
    assert_eq!(record.fields.len(), 2);
    assert_eq!(record.fields[0].0.canonical, ":component/id");
    assert!(matches!(record.fields[0].1.kind, ExprKind::String(_)));
    assert_eq!(record.fields[1].0.canonical, ":component/enabled");
    assert!(matches!(record.fields[1].1.kind, ExprKind::Bool(true)));
}

#[test]
fn static_record_recovers_flat_fields_with_a_shape_diagnostic() {
    let lowered = lower_document(&read("(static-record Schema owner :field 1)"));
    assert!(
        lowered
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == super::AST_WRONG_SHAPE
                && diagnostic.message.contains("single map"))
    );
    let record = match &lowered.module.items[0].kind {
        ItemKind::StaticRecord(record) => record,
        other => panic!("expected static-record, got {other:?}"),
    };
    assert_eq!(record.fields.len(), 1);
    assert_eq!(record.fields[0].0.canonical, ":field");
}

#[test]
fn extern_contains_nested_declarations() {
    let lowered = lower_document(&read(
        "(extern python \"math\" (defn ^Bool isfinite [^Float value]))",
    ));
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let external = match &lowered.module.items[0].kind {
        ItemKind::Extern(external) => external,
        _ => panic!("expected extern"),
    };
    assert_eq!(external.items.len(), 1);
    assert!(matches!(
        external.items[0].kind,
        ItemKind::Defn(ref function) if function.body.is_empty()
    ));
}

#[test]
fn extern_contract_is_lowered_as_closed_static_data() {
    let lowered = lower_document(&read(
        r#"(extern python "host.data"
                 (defn ^Series moving-average [^Series values ^Int n]
                   :contract
                   {:id "host.data/moving-average-v1"
                    :effects [:io :host/cache]
                    :temporal {:past "2*(n-1)"
                               :future 0
                               :availability :published}
                    :data {:schema "measurements"
                           :axes [:time]
                           :alignment :labelled
                           :ordered-by [:source :time]
                           :unique-by [:source :time]
                           :preserves-length true
                           :materializes false
                           :reshapes false
                           :nulls-possible true
                           :nan-possible true
                           :nonfinite-possible true
                           :nonfinite-policy :preserve-nonfinite}}))"#,
    ));
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let ItemKind::Extern(external) = &lowered.module.items[0].kind else {
        panic!("expected extern");
    };
    let ItemKind::Defn(function) = &external.items[0].kind else {
        panic!("expected extern function");
    };
    let contract = function.contract.as_ref().expect("contract");
    assert_eq!(contract.id, "host.data/moving-average-v1");
    assert!(contract.summaries.effects.effects.contains(&Effect::Io));
    assert!(
        contract
            .summaries
            .effects
            .effects
            .contains(&Effect::Custom("host/cache".to_owned()))
    );
    assert_eq!(
        contract.summaries.temporal.past,
        TemporalBound::Symbolic("2*(n-1)".to_owned())
    );
    assert_eq!(contract.summaries.temporal.future, TemporalBound::Finite(0));
    assert_eq!(
        contract.summaries.temporal.availability,
        Availability::Named("published".to_owned())
    );
    assert_eq!(contract.summaries.data.alignment, Alignment::Labelled);
    assert_eq!(
        contract.summaries.data.ordered_by,
        Some(vec!["source".to_owned(), "time".to_owned()])
    );
    assert_eq!(
        contract.summaries.data.unique_by,
        Some(vec!["source".to_owned(), "time".to_owned()])
    );
    assert_eq!(contract.summaries.data.preserves_length, Some(true));
    assert_eq!(contract.summaries.data.nan_possible, Some(true));
    assert_eq!(
        contract.summaries.data.nonfinite_policy.as_deref(),
        Some("preserve-nonfinite")
    );
}

#[test]
fn malformed_extern_contract_fails_closed() {
    let lowered = lower_document(&read(
        r#"(extern python "host.series"
                 (defn ^Series lead [^Series values]
                   :contract
                   {:id "host.series/lead-v1"
                    :id "duplicate"
                    :temporal {:future -1}
                    :executable-analyzer "host.analyze"}))"#,
    ));
    assert!(
        lowered
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == super::AST_INVALID_CONTRACT)
    );
    assert!(
        lowered
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == super::AST_UNKNOWN_CLAUSE)
    );
    let ItemKind::Extern(external) = &lowered.module.items[0].kind else {
        panic!("expected extern");
    };
    let ItemKind::Defn(function) = &external.items[0].kind else {
        panic!("expected extern function");
    };
    assert!(function.contract.is_none());
}

#[test]
fn runtime_function_still_requires_a_body() {
    let lowered = lower_document(&read("(defn ^Int incomplete [^Int value])"));
    assert!(lowered.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == super::AST_WRONG_SHAPE
            && diagnostic.message == "function body cannot be empty"
    }));
}

#[test]
fn error_nodes_are_serializable_without_panicking() {
    let lowered = lower_document(&read("(if true)"));
    let json = serde_json::to_string(&lowered).expect("AST should serialize");
    assert!(json.contains("diagnostics"));
    assert!(lowered
            .module
            .items
            .iter()
            .any(|item| matches!(&item.kind, ItemKind::Expr(expr) if matches!(expr.kind, ExprKind::If { .. }))));
}
