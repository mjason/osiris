#[test]
fn workspace_compiles_a_two_module_runtime_cycle_with_provisional_interfaces() {
    let left = r#"
            (module cycle.left)
            (import cycle.right :as right)
            (export [left])
            ^{:doc "Call the right side."}
            (defn ^Int left [^Int value] (right/right value))
        "#;
    let right = r#"
            (module cycle.right)
            (import cycle.left :as left)
            (export [right])
            ^{:doc "Call the left side."}
            (defn ^Int right [^Int value] (left/left value))
        "#;
    let left_options = CompileOptions::new("cycle.left", PythonVersion::MINIMUM);
    let right_options = CompileOptions::new("cycle.right", PythonVersion::MINIMUM);
    let result = compile_workspace(
        &[
            CompileInput::new(left, &left_options),
            CompileInput::new(right, &right_options),
        ],
        &BTreeMap::new(),
    );
    assert!(!result.has_errors(), "{:?}", result.diagnostics);
    assert_eq!(result.units.len(), 2);
    assert!(result.units[0].python.is_some());
    assert!(result.units[1].python.is_some());
}

#[test]
fn runtime_scc_provisional_interfaces_preserve_struct_and_operator_shape() {
    let left = r#"
            (module capability.left)
            (import capability.right :as right)
            ^{:doc "A scalar series fixture."}
            (defstruct Series [value Float])
            ^{:osiris/operator :multiply}
            ^{:doc "Multiply a series fixture."}
            (defn ^Series multiply-series [^Series series ^Float multiplier] series)
            (export [Series multiply-series])
            ^{:doc "Dispatch scaling."}
            (defn ^Series dispatch [^Series series ^Float multiplier] (right/scale series multiplier))
            (export [dispatch])
        "#;
    let right = r#"
            (module capability.right)
            (import capability.left :refer [Series])
            (export [scale])
            ^{:doc "Scale a series fixture."}
            (defn ^Series scale [^Series series ^Float multiplier] series)
        "#;
    let left_options = CompileOptions::new("capability.left", PythonVersion::MINIMUM);
    let right_options = CompileOptions::new("capability.right", PythonVersion::MINIMUM);
    let result = compile_workspace(
        &[
            CompileInput::new(left, &left_options),
            CompileInput::new(right, &right_options),
        ],
        &BTreeMap::new(),
    );
    assert!(!result.has_errors(), "{:?}", result.diagnostics);
}

#[test]
fn runtime_scc_rebuilds_provisional_shape_after_declaration_macro_expansion() {
    let provider = r#"
            (module macro.cycle-provider)
            (import macro.cycle-consumer :as consumer)
            (defmacro emit-generated [] '(def ^{:type Int :doc "Generated value."} generated 1))
            (emit-generated)
            (export [generated run])
            ^{:doc "Run the generated provider."}
            (defn ^Int run [^Int value] (consumer/identity value))
        "#;
    let consumer = r#"
            (module macro.cycle-consumer)
            (import macro.cycle-provider :as provider)
            (export [identity])
            ^{:doc "Return a value unchanged."}
            (defn ^Int identity [^Int value] value)
        "#;
    let provider_options = CompileOptions::new("macro.cycle-provider", PythonVersion::MINIMUM);
    let consumer_options = CompileOptions::new("macro.cycle-consumer", PythonVersion::MINIMUM);
    let result = compile_workspace(
        &[
            CompileInput::new(provider, &provider_options),
            CompileInput::new(consumer, &consumer_options),
        ],
        &BTreeMap::new(),
    );
    assert!(!result.has_errors(), "{:?}", result.diagnostics);
    let provider_python = result.units[0]
        .python
        .as_ref()
        .expect("provider should generate Python")
        .source
        .as_str();
    assert!(provider_python.contains("generated"), "{provider_python}");
}

#[test]
fn workspace_still_rejects_a_phase1_cycle_before_runtime_scc_lowering() {
    let left = r#"
            (module cycle.phase-left)
            (import-for-syntax cycle.phase-right)
        "#;
    let right = r#"
            (module cycle.phase-right)
            (import-for-syntax cycle.phase-left)
        "#;
    let left_options = CompileOptions::new("cycle.phase-left", PythonVersion::MINIMUM);
    let right_options = CompileOptions::new("cycle.phase-right", PythonVersion::MINIMUM);
    let result = compile_workspace(
        &[
            CompileInput::new(left, &left_options),
            CompileInput::new(right, &right_options),
        ],
        &BTreeMap::new(),
    );
    assert!(result.has_errors());
    assert_eq!(result.diagnostics[0].diagnostic.code, "OSR-G0008");
}

#[test]
fn workspace_breaks_a_mixed_runtime_and_phase1_cycle_with_a_provisional_batch() {
    let runtime_importer = r#"
            (module mixed.runtime)
            (import mixed.syntax :as syntax)
            (export [run])
            ^{:doc "Run the syntax dependency."}
            (defn ^Int run [^Int value] (syntax/emit value))
        "#;
    let syntax_importer = r#"
            (module mixed.syntax)
            (import-for-syntax mixed.runtime)
            (export [emit])
            ^{:doc "Emit a value."}
            (defn ^Int emit [^Int value] value)
        "#;
    let runtime_options = CompileOptions::new("mixed.runtime", PythonVersion::MINIMUM);
    let syntax_options = CompileOptions::new("mixed.syntax", PythonVersion::MINIMUM);
    let result = compile_workspace(
        &[
            CompileInput::new(runtime_importer, &runtime_options),
            CompileInput::new(syntax_importer, &syntax_options),
        ],
        &BTreeMap::new(),
    );
    assert!(!result.has_errors(), "{:?}", result.diagnostics);
    assert_eq!(result.units.len(), 2);
}

#[test]
fn runtime_cycle_interface_hashes_are_stable_when_input_order_changes() {
    let left = r#"
            (module stable.left)
            (import stable.right :as right)
            (export [left])
            ^{:doc "Call the right side."}
            (defn ^Int left [^Int value] (right/right value))
        "#;
    let right = r#"
            (module stable.right)
            (import stable.left :as left)
            (export [right])
            ^{:doc "Call the left side."}
            (defn ^Int right [^Int value] (left/left value))
        "#;
    let left_options = CompileOptions::new("stable.left", PythonVersion::MINIMUM);
    let right_options = CompileOptions::new("stable.right", PythonVersion::MINIMUM);
    let forward = compile_workspace(
        &[
            CompileInput::new(left, &left_options),
            CompileInput::new(right, &right_options),
        ],
        &BTreeMap::new(),
    );
    let reverse = compile_workspace(
        &[
            CompileInput::new(right, &right_options),
            CompileInput::new(left, &left_options),
        ],
        &BTreeMap::new(),
    );
    assert!(!forward.has_errors(), "{:?}", forward.diagnostics);
    assert!(!reverse.has_errors(), "{:?}", reverse.diagnostics);
    let forward_left =
        interface::read(forward.units[0].interface.as_ref().expect("left interface"))
            .expect("left interface should parse");
    let reverse_left =
        interface::read(reverse.units[1].interface.as_ref().expect("left interface"))
            .expect("left interface should parse");
    assert_eq!(
        forward_left.semantic_interface_hash(),
        reverse_left.semantic_interface_hash()
    );
    assert_eq!(
        forward_left.tooling_metadata_hash(),
        reverse_left.tooling_metadata_hash()
    );
}

#[test]
fn runtime_sccs_are_scheduled_before_their_cross_scc_importers() {
    let app = r#"
            (module ordered.app)
            (import ordered.left :as left)
            (export [run])
            ^{:doc "Run the ordered application."}
            (defn ^Int run [^Int value] (left/left value))
        "#;
    let left = r#"
            (module ordered.left)
            (import ordered.right :as right)
            (export [left])
            ^{:doc "Call the right side."}
            (defn ^Int left [^Int value] (right/right value))
        "#;
    let right = r#"
            (module ordered.right)
            (import ordered.left :as left)
            (export [right])
            ^{:doc "Call the left side."}
            (defn ^Int right [^Int value] (left/left value))
        "#;
    let app_options = CompileOptions::new("ordered.app", PythonVersion::MINIMUM);
    let left_options = CompileOptions::new("ordered.left", PythonVersion::MINIMUM);
    let right_options = CompileOptions::new("ordered.right", PythonVersion::MINIMUM);
    let result = compile_workspace(
        &[
            CompileInput::new(app, &app_options),
            CompileInput::new(left, &left_options),
            CompileInput::new(right, &right_options),
        ],
        &BTreeMap::new(),
    );
    assert!(!result.has_errors(), "{:?}", result.diagnostics);
    assert_eq!(result.units.len(), 3);
    assert!(result.units.iter().all(|unit| unit.python.is_some()));
}

/// The per-item `^:export` marker is the one way a declaration macro can
/// publish a name, so the public surface a macro produces is invisible to the
/// first provisional interface, which is built from the unexpanded header, and
/// appears only in the second, built from the expanded surface. This is the
/// convergence OSR-M0008 warns about, so it is checked on the hardest shape:
/// the consumer sits in the same runtime SCC as the producer and therefore
/// resolves against a provisional interface rather than a published one.
#[test]
fn a_macro_generated_export_marker_converges_across_a_runtime_cycle() {
    let factory = r#"
            (module cycle.factory)
            (export [define-factor])
            ^{:doc "Declare one factor."}
            (defmacro define-factor [label]
              `(do ^{:doc "Factor name." :export true} (def ^Str factor-name ~label)
                   ^{:doc "Compute." :export true} (defn ^Int calculate [^Int value] value)))
        "#;
    let producer = r#"
            (module cycle.producer)
            (import-for-syntax cycle.factory :refer [define-factor])
            (import cycle.consumer :refer [pong])
            (export [ping])
            (define-factor "cycle")
            ^{:doc "Ping."}
            (defn ^Int ping [^Int value] (pong value))
        "#;
    let consumer = r#"
            (module cycle.consumer)
            (export [pong])
            (import cycle.producer :refer [calculate])
            ^{:doc "Pong."}
            (defn ^Int pong [^Int value] (calculate value))
        "#;
    let factory_options = CompileOptions::new("cycle.factory", PythonVersion::MINIMUM);
    let producer_options = CompileOptions::new("cycle.producer", PythonVersion::MINIMUM);
    let consumer_options = CompileOptions::new("cycle.consumer", PythonVersion::MINIMUM);
    let result = compile_workspace(
        &[
            CompileInput::new(factory, &factory_options),
            CompileInput::new(producer, &producer_options),
            CompileInput::new(consumer, &consumer_options),
        ],
        &BTreeMap::new(),
    );
    // Nothing here would compile if the marker did not reach the provisional
    // interface: `cycle.consumer` refers `calculate`, and an unpublished name
    // fails that import with OSR-H0011.
    assert!(!result.has_errors(), "{:?}", result.diagnostics);
    let producer = result.units[1]
        .interface
        .as_ref()
        .expect("the producer publishes an interface");
    assert!(producer.contains(r#":canonical "calculate""#), "{producer}");
    assert!(producer.contains(r#":canonical "factor-name""#), "{producer}");
}

/// The negative control for the test above: the same macro without markers
/// generates the same declarations, and they stay module-private.
#[test]
fn a_macro_generated_declaration_stays_private_without_the_marker() {
    let factory = r#"
            (module quiet.factory)
            (export [define-factor])
            ^{:doc "Declare one factor."}
            (defmacro define-factor [label]
              `(do ^{:doc "Factor name."} (def ^Str factor-name ~label)
                   ^{:doc "Compute."} (defn ^Int calculate [^Int value] value)))
        "#;
    let producer = r#"
            (module quiet.producer)
            (import-for-syntax quiet.factory :refer [define-factor])
            (define-factor "quiet")
        "#;
    let consumer = r#"
            (module quiet.consumer)
            (export [pong])
            (import quiet.producer :refer [calculate])
            ^{:doc "Pong."}
            (defn ^Int pong [^Int value] (calculate value))
        "#;
    let factory_options = CompileOptions::new("quiet.factory", PythonVersion::MINIMUM);
    let producer_options = CompileOptions::new("quiet.producer", PythonVersion::MINIMUM);
    let consumer_options = CompileOptions::new("quiet.consumer", PythonVersion::MINIMUM);
    let result = compile_workspace(
        &[
            CompileInput::new(factory, &factory_options),
            CompileInput::new(producer, &producer_options),
            CompileInput::new(consumer, &consumer_options),
        ],
        &BTreeMap::new(),
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.code == "OSR-H0011"),
        "{:?}",
        result.diagnostics
    );
}
