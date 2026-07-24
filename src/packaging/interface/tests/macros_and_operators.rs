#[test]
fn public_macro_ir_round_trips_and_replays() {
    let (surface, typed) = macro_modules(MACRO_SOURCE);
    let encoded = emit(&typed, &surface).expect("macro interface should emit");
    let decoded = read(&encoded).expect("macro interface should read");

    assert_eq!(decoded.macros.len(), 1);
    assert_eq!(decoded.macros[0].canonical, "public-pipeline");
    assert_eq!(decoded.macros[0].minimum_arity, 1);
    assert!(decoded.macros[0].variadic);
    assert_eq!(decoded.phase_helpers.len(), 2);
    assert_eq!(
        decoded
            .phase_helpers
            .iter()
            .map(|helper| helper.canonical.as_str())
            .collect::<Vec<_>>(),
        ["helper", "helper-two"]
    );
    assert!(!encoded.contains("unused-helper"));
    assert!(!encoded.contains("hidden-macro"));
    assert!(!encoded.contains("/home/"));
    assert_eq!(render(&decoded).unwrap(), encoded);

    let imported = decoded.imported_phase_forms();
    let input = source_reader::read("(public-pipeline 1)");
    let expanded = macro_expand::expand_with_imported_phase_forms(
        &input,
        &imported,
        macro_expand::ExpansionOptions::default(),
    );
    assert!(
        expanded.document.diagnostics.is_empty(),
        "{:?}",
        expanded.document.diagnostics
    );
    let rendered = crate::printer::render_document_text(&expanded.document);
    assert!(rendered.contains("inc"), "{rendered}");
}

#[test]
fn macro_ir_tampering_is_rejected_and_changes_semantic_hash() {
    let (surface, typed) = macro_modules(MACRO_SOURCE);
    let encoded = emit(&typed, &surface).unwrap();
    let tampered = encoded.replacen("helper-two", "missing-helper", 1);
    assert!(matches!(
        read(&tampered).unwrap_err().code,
        "OSR-I0059" | "OSR-I0060" | "OSR-I0015"
    ));

    let changed_source = MACRO_SOURCE.replacen("'inc", "'dec", 1);
    let (changed_surface, changed_typed) = macro_modules(&changed_source);
    let changed = read(&emit(&changed_typed, &changed_surface).unwrap()).unwrap();
    let original = read(&encoded).unwrap();
    assert_ne!(
        changed.hashes.interface_body,
        original.hashes.interface_body
    );
    assert_ne!(changed.hashes.semantic_body, original.hashes.semantic_body);
}

#[test]
fn operator_instance_round_trips_and_is_semantic() {
    let (surface, typed) = operator_modules(OPERATOR_SOURCE);
    let encoded = emit(&typed, &surface).expect("operator interface should emit");
    let decoded = read(&encoded).expect("operator interface should read");

    assert_eq!(decoded.operator_instances.len(), 1);
    let instance = &decoded.operator_instances[0];
    assert_eq!(instance.operator, crate::types::ScalarOperator::Multiply);
    assert_eq!(instance.operands.len(), 2);
    assert_eq!(render(&decoded).unwrap(), encoded);
    assert!(encoded.contains(":operator-instances"));

    let changed_source = OPERATOR_SOURCE.replace(":multiply", ":subtract");
    let (changed_surface, changed_typed) = operator_modules(&changed_source);
    let changed = read(&emit(&changed_typed, &changed_surface).unwrap()).unwrap();
    assert_ne!(changed.hashes.interface_body, decoded.hashes.interface_body);
    assert_ne!(changed.hashes.semantic_body, decoded.hashes.semantic_body);
}

#[test]
fn operator_instance_tampering_is_rejected_before_hash_acceptance() {
    let (surface, typed) = operator_modules(OPERATOR_SOURCE);
    let encoded = emit(&typed, &surface).unwrap();

    let invalid_id = encoded.replacen("::operator::multiply\"", "::operator::subtract\"", 1);
    assert_eq!(read(&invalid_id).unwrap_err().code, "OSR-I0068");

    let invalid_signature = encoded.replacen(":float] :result", ":any] :result", 1);
    assert!(matches!(
        read(&invalid_signature).unwrap_err().code,
        "OSR-I0069" | "OSR-I0071"
    ));
}

#[test]
fn operator_instance_requires_an_owned_nominal_operand() {
    let source = r#"
            (module sample.operators)
            ^{:doc "Add scalar fixtures." :osiris/operator :add}
            (defn ^Float add-scalars [^Float left ^Float right] left)
            (export [add-scalars])
        "#;
    let (surface, typed) = operator_modules(source);
    assert_eq!(emit(&typed, &surface).unwrap_err().code, "OSR-I0064");
}

#[test]
fn duplicate_operator_operand_tuple_is_rejected() {
    let source = OPERATOR_SOURCE.replace(
        "(export [Series multiply-series])",
        r#"
            ^{:doc "Multiply a series fixture again." :osiris/operator :multiply}
            (defn ^{:type (Series Float)} multiply-series-again [^{:type (Series Float)} series ^Float multiplier]
              series)
            (export [Series multiply-series multiply-series-again])
            "#,
    );
    let (surface, typed) = operator_modules(&source);
    assert_eq!(emit(&typed, &surface).unwrap_err().code, "OSR-I0065");
}

#[test]
fn duplicate_and_private_entries_are_rejected() {
    let (surface, typed) = modules();
    let encoded = emit(&typed, &surface).unwrap();
    let duplicate = encoded.replacen(
        ":module \"sample.core\"",
        ":module \"sample.core\" :module \"duplicate\"",
        1,
    );
    assert_eq!(read(&duplicate).unwrap_err().code, "OSR-I0043");

    let private = encoded.replacen(":visibility :public", ":visibility :private", 1);
    assert_eq!(read(&private).unwrap_err().code, "OSR-I0053");
}

#[test]
fn incompatible_abi_is_rejected() {
    let (surface, typed) = modules();
    let encoded = emit(&typed, &surface).unwrap();
    let incompatible = encoded.replacen("\"osiris-compiler-v0\"", "\"osiris-compiler-v999\"", 1);
    assert_eq!(read(&incompatible).unwrap_err().code, "OSR-I0013");
}

#[test]
fn reading_runtime_locator_does_not_import_python() {
    let (surface, mut typed) = modules();
    let binding = typed
        .bindings
        .iter_mut()
        .find(|binding| binding.name.canonical == "distance")
        .unwrap();
    binding.runtime = Some(hir::RuntimeBinding {
        module: "module_that_cannot_exist_for_osiris_test".to_owned(),
        name: "distance".to_owned(),
        python_module: true,
    });
    let encoded = emit(&typed, &surface).unwrap();
    let decoded = read(&encoded).unwrap();
    assert_eq!(
        decoded.bindings[0]
            .runtime
            .as_ref()
            .map(|runtime| runtime.module.as_str()),
        Some("module_that_cannot_exist_for_osiris_test")
    );
}
