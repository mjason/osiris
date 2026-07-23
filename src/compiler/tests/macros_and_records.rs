#[test]
fn workspace_replays_exported_macro_and_private_helper() {
    let macros = r#"
            (module sample.macros)
            (defn-for-syntax make-add [value]
              (list '+ value 1))
            (defmacro add-one [value]
              (make-add value))
            (export [add-one])
        "#;
    let app = r#"
            (module sample.app)
            (import sample.macros :as macros)
            (export [increment])
            (defn increment [[value Int]] -> Int
              (macros/add-one value))
        "#;
    let app_options = CompileOptions::new("sample.app", PythonVersion::MINIMUM);
    let macro_options = CompileOptions::new("sample.macros", PythonVersion::MINIMUM);
    let inputs = [
        CompileInput::new(app, &app_options),
        CompileInput::new(macros, &macro_options),
    ];

    let result = compile_workspace(&inputs, &BTreeMap::new());

    assert!(!result.has_errors(), "{:?}", result.diagnostics);
    let python = &result.units[0]
        .python
        .as_ref()
        .expect("macro consumer should generate Python")
        .source;
    assert!(python.contains("return value + 1"), "{python}");
    assert!(
        result.units[0]
            .analysis
            .expansion_traces
            .iter()
            .any(|trace| trace.macro_name == "add-one")
    );
}

#[test]
fn workspace_validates_records_against_an_imported_schema() {
    let producer = r#"
            (module sample.producer)
            (import sample.schema :as schema)
            (export [owner])
            (def owner none)
            (static-record schema/Descriptor owner {:id "example.normalize"})
        "#;
    let schema = r#"
            (module sample.schema)
            (export [Descriptor])
            (defstatic-schema Descriptor
              :schema-id "sample/descriptor"
              :version 1
              :fields {:id {:type Str :required true}})
        "#;
    let producer_options = CompileOptions::new("sample.producer", PythonVersion::MINIMUM);
    let schema_options = CompileOptions::new("sample.schema", PythonVersion::MINIMUM);
    let inputs = [
        CompileInput::new(producer, &producer_options),
        CompileInput::new(schema, &schema_options),
    ];

    let result = compile_workspace(&inputs, &BTreeMap::new());

    assert!(!result.has_errors(), "{:?}", result.diagnostics);
    let records = &result.units[0]
        .records
        .as_ref()
        .expect("producer should emit a records projection")
        .sidecar
        .records;
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].record.schema.binding_id,
        "sample.schema::type::Descriptor"
    );
}

#[test]
fn workspace_isolates_same_named_macros_and_private_helpers() {
    let app = r#"
            (module sample.app)
            (import sample.alpha :as alpha)
            (import sample.beta :as beta)
            (defn calculate [[value Int]] -> Int
              (+ (alpha/wrap value) (beta/wrap value)))
        "#;
    let alpha = r#"
            (module sample.alpha)
            (defn-for-syntax helper [value] (list '+ value 1))
            (defmacro wrap [value] (helper value))
            (export [wrap])
        "#;
    let beta = r#"
            (module sample.beta)
            (defn-for-syntax helper [value] (list '* value 2))
            (defmacro wrap [value] (helper value))
            (export [wrap])
        "#;
    let app_options = CompileOptions::new("sample.app", PythonVersion::MINIMUM);
    let alpha_options = CompileOptions::new("sample.alpha", PythonVersion::MINIMUM);
    let beta_options = CompileOptions::new("sample.beta", PythonVersion::MINIMUM);
    let inputs = [
        CompileInput::new(app, &app_options),
        CompileInput::new(alpha, &alpha_options),
        CompileInput::new(beta, &beta_options),
    ];

    let result = compile_workspace(&inputs, &BTreeMap::new());

    assert!(!result.has_errors(), "{:?}", result.diagnostics);
    let python = &result.units[0]
        .python
        .as_ref()
        .expect("macro consumer should generate Python")
        .source;
    assert!(python.contains("value + 1"), "{python}");
    assert!(python.contains("value * 2"), "{python}");
    assert_eq!(
        result.units[0]
            .analysis
            .expansion_traces
            .iter()
            .filter(|trace| trace.macro_name == "wrap")
            .count(),
        2
    );
}
