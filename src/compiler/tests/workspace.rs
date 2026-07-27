use std::{collections::BTreeMap, time::Instant};

use super::{
    CompileInput, CompileOptions, analyze, analyze_workspace, analyze_workspace_recovering,
    compile, compile_workspace,
};
use crate::{hir, interface, project::PythonVersion, types::Type};

fn options() -> CompileOptions {
    CompileOptions::new("example", PythonVersion::MINIMUM)
}

#[test]
fn workspace_analysis_skips_every_emitted_artifact() {
    let options = options();
    let input = CompileInput::new("(def value 1)", &options);
    let workspace = analyze_workspace(&[input], &BTreeMap::new());

    assert!(!workspace.has_errors(), "{:?}", workspace.diagnostics);
    let unit = workspace.units.first().expect("analyzed workspace unit");
    assert!(unit.python.is_none());
    assert!(unit.interface.is_none());
    assert!(unit.source_map.is_none());
    assert!(unit.records.is_none());
}

#[test]
fn workspace_analyzes_a_wide_independent_dependency_layer() {
    const MODULE_COUNT: usize = 512;

    let sources = (0..MODULE_COUNT)
        .map(|index| format!("(module scale.m{index:04})\n(def value {index})\n"))
        .collect::<Vec<_>>();
    let options = (0..MODULE_COUNT)
        .map(|index| CompileOptions::new(format!("scale.m{index:04}"), PythonVersion::MINIMUM))
        .collect::<Vec<_>>();
    let inputs = sources
        .iter()
        .zip(&options)
        .map(|(source, options)| CompileInput::new(source, options))
        .collect::<Vec<_>>();

    let started = Instant::now();
    let workspace = analyze_workspace(&inputs, &BTreeMap::new());
    let elapsed = started.elapsed();
    assert!(!workspace.has_errors(), "{:?}", workspace.diagnostics);
    assert_eq!(workspace.units.len(), MODULE_COUNT);
    eprintln!("analyzed {MODULE_COUNT} independent modules in {elapsed:?}");
}

#[test]
fn non_strict_compilation_accepts_complete_inferred_public_signatures() {
    let source = "(export [answer]) ^{:doc \"Answer.\"} (defn answer [] 42)";
    let strict = compile(source, &options());
    assert!(
        strict
            .analysis
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-T0018")
    );

    let inferred = compile(source, &options().with_strict(false));
    assert!(
        inferred.analysis.diagnostics.is_empty(),
        "{:?}",
        inferred.analysis.diagnostics
    );
    assert!(inferred.interface.is_some());
    assert!(inferred.python.is_some());
}

#[test]
fn non_strict_public_inference_rejects_implicit_dynamic_boundaries() {
    let source = "(py/import host :as host) (export [answer]) ^{:doc \"Answer.\"} (defn answer [] (host.answer))";
    let result = compile(source, &options().with_strict(false));
    assert!(result.analysis.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "OSR-T0051" && diagnostic.message.contains("annotate it explicitly")
    }));
    assert!(result.interface.is_none());

    let explicit = compile(
        "(py/import host :as host) (export [answer]) ^{:doc \"Answer.\"} (defn ^Any answer [] (host.answer))",
        &options().with_strict(false),
    );
    assert!(!explicit.has_errors(), "{:?}", explicit.analysis.diagnostics);
    assert!(explicit.interface.is_some());
}

#[test]
fn trust_policy_hash_partitions_analysis_and_build_artifacts() {
    let source = "(defn ^Int value [] 1)";
    let first_policy = hir::ContractTrustPolicy::untrusted(format!("sha256:{}", "1".repeat(64)));
    let second_policy = hir::ContractTrustPolicy::untrusted(format!("sha256:{}", "2".repeat(64)));
    let first = compile(
        source,
        &CompileOptions::new("example", PythonVersion::MINIMUM)
            .with_trust_policy(first_policy.clone()),
    );
    let second = compile(
        source,
        &CompileOptions::new("example", PythonVersion::MINIMUM)
            .with_trust_policy(second_policy.clone()),
    );
    assert!(!first.has_errors(), "{:?}", first.analysis.diagnostics);
    assert!(!second.has_errors(), "{:?}", second.analysis.diagnostics);
    assert_ne!(first.analysis.cache_key, second.analysis.cache_key);
    assert_ne!(first.build_hash, second.build_hash);
    assert_eq!(first.interface, second.interface);
    assert_eq!(
        first
            .source_map
            .as_ref()
            .expect("source map")
            .trust_policy_hash,
        first_policy.hash
    );
    assert_eq!(
        second.source_map.as_ref().expect("source map").build_hash,
        second.build_hash
    );
}

#[test]
fn analysis_combines_frontend_diagnostics_in_source_order() {
    let result = analyze("(def first missing)\n(def second [1 2)\n", &options());

    assert!(result.has_errors());
    assert!(
        result
            .diagnostics
            .windows(2)
            .all(|pair| pair[0].span.start <= pair[1].span.start)
    );
}

#[test]
fn frontend_errors_prevent_python_generation() {
    let result = compile("(def value missing)\n", &options());

    assert!(result.has_errors());
    assert!(result.python.is_none());
}

#[test]
fn explicit_module_must_match_the_project_source_identity() {
    let options = CompileOptions::new("nested.expected", PythonVersion::MINIMUM)
        .with_expected_module_name("nested.expected");

    let result = compile("(module nested.other)\n(def value 1)\n", &options);

    assert!(result.has_errors());
    assert!(result.python.is_none());
    assert!(
        result
            .analysis
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-G0011"
                && diagnostic.message.contains("nested.expected"))
    );
}

#[test]
fn invalid_static_records_fail_before_codegen() {
    let source = r#"
            (module example)
            (export [owner S])
            ^{:doc "Schema S."}
            (defstatic-schema S
              :schema-id "example/schema"
              :version 1
              :fields {:id {:type Str :required true}})
            ^{:doc "Record owner."}
            (def ^Any owner none)
            (static-record S owner {:id 42})
        "#;
    let result = compile(source, &options());

    assert!(result.has_errors());
    assert!(result.python.is_none());
    assert!(
        result
            .analysis
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code.starts_with("OSR-S"))
    );
}

#[test]
fn public_static_records_are_bound_to_the_interface_provider() {
    let source = r#"
            (module example)
            (export [owner S])
            ^{:doc "Schema S."}
            (defstatic-schema S
              :schema-id "example/schema"
              :version 1
              :fields {:id {:type Str :required true}})
            ^{:doc "Record owner."}
            (def ^Any owner none)
            (static-record S owner {:id "alpha"})
        "#;
    let options = options().with_provider("example-dist", "1.2.3");
    let result = compile(source, &options);

    assert!(!result.has_errors(), "{:?}", result.analysis.diagnostics);
    let sidecar = result.records.expect("records sidecar should be built");
    assert_eq!(sidecar.sidecar.records.len(), 1);
    let occurrence = &sidecar.sidecar.records[0].occurrence;
    assert_eq!(occurrence.distribution, "example-dist");
    assert_eq!(occurrence.version, "1.2.3");
    assert_eq!(occurrence.interface_member_id, "example");
    assert_eq!(
        occurrence.semantic_interface_hash,
        sidecar.sidecar.interface_semantic_hashes[0]
    );
    let rendered = result
        .interface
        .as_ref()
        .expect("interface should be rendered");
    let decoded = interface::read(rendered).expect("rendered interface should parse");
    assert_eq!(
        occurrence.semantic_interface_hash,
        decoded.semantic_interface_hash()
    );
    assert_ne!(
        occurrence.semantic_interface_hash, decoded.hashes.semantic_body,
        "record occurrence must use the published graph hash, not the local body hash"
    );
}

#[test]
fn workspace_compiles_typed_dependencies_before_importers() {
    let app = r#"
            (module app)
            (import dep.core :as dep)
            (export [call])
            ^{:doc "Call the dependency."}
            (defn ^Int call [] (dep/add 1 :value 2))
        "#;
    let dependency = r#"
            (module dep.core)
            (export [add])
            ^{:doc "Add two integers."}
            (defn ^Int add [^Int x ^Int value] (+ x value))
        "#;
    let app_options = CompileOptions::new("app", PythonVersion::MINIMUM);
    let dependency_options = CompileOptions::new("dep.core", PythonVersion::MINIMUM);
    let inputs = [
        CompileInput::new(app, &app_options),
        CompileInput::new(dependency, &dependency_options),
    ];

    let result = compile_workspace(&inputs, &BTreeMap::new());

    assert!(!result.has_errors(), "{:?}", result.diagnostics);
    assert_eq!(result.units.len(), 2);
    assert_eq!(result.units[0].analysis.hir.name, "app");
    assert_eq!(result.units[1].analysis.hir.name, "dep.core");
    let python = &result.units[0]
        .python
        .as_ref()
        .expect("app should generate Python")
        .source;
    assert!(python.contains("from dep.core import add"), "{python}");
    assert!(python.contains("return add(1, value=2)"), "{python}");

    let app_interface = interface::read(
        result.units[0]
            .interface
            .as_ref()
            .expect("app interface should be rendered"),
    )
    .expect("app interface should parse");
    let dependency_interface = interface::read(
        result.units[1]
            .interface
            .as_ref()
            .expect("dependency interface should be rendered"),
    )
    .expect("dependency interface should parse");
    assert_eq!(
        result.units[0]
            .records
            .as_ref()
            .expect("app records should be rendered")
            .sidecar
            .interface_semantic_hashes,
        vec![app_interface.semantic_interface_hash().to_owned()]
    );
    assert_ne!(
        app_interface.semantic_interface_hash(),
        app_interface.hashes.semantic_body,
        "workspace interfaces must publish an SCC/group hash"
    );
    assert_ne!(
        dependency_interface.semantic_interface_hash(),
        dependency_interface.hashes.semantic_body,
        "even a dependency-only singleton retains a standalone group hash"
    );
    let dependency = app_interface
        .graph
        .external_dependencies
        .iter()
        .find(|dependency| dependency.to == "dep.core")
        .expect("app graph should retain its dependency hash");
    assert_eq!(
        dependency.semantic_interface_hash,
        dependency_interface.semantic_interface_hash()
    );
}

#[test]
fn recovering_workspace_keeps_healthy_imports_when_another_module_is_invalid() {
    let dependency = r#"
            (module dep.core)
            (export [add-one])
            ^{:doc "Increment an integer."}
            (defn ^Int add-one [^Int x] (+ x 1))
        "#;
    let app = r#"
            (module app)
            (import dep.core :as dep)
            (def answer (dep/add-one 41))
        "#;
    let broken = r#"
            (module broken)
            (defn ^Int invalid [^Int x])
        "#;
    let dependency_options = CompileOptions::new("dep.core", PythonVersion::MINIMUM);
    let app_options = CompileOptions::new("app", PythonVersion::MINIMUM);
    let broken_options = CompileOptions::new("broken", PythonVersion::MINIMUM);
    let inputs = [
        CompileInput::new(dependency, &dependency_options),
        CompileInput::new(app, &app_options),
        CompileInput::new(broken, &broken_options),
    ];

    let strict = compile_workspace(&inputs, &BTreeMap::new());
    assert!(strict.has_errors());
    assert!(strict.units.is_empty());

    let recovered = analyze_workspace_recovering(&inputs, &BTreeMap::new());
    assert_eq!(recovered.len(), inputs.len());
    assert!(
        recovered[1].diagnostics.is_empty(),
        "{:?}",
        recovered[1].diagnostics
    );
    assert_eq!(recovered[1].hir.name, "app");
    let imported = recovered[1]
        .hir
        .bindings
        .iter()
        .find(|binding| binding.name.id.as_str() == "dep.core::function::add-one")
        .expect("dependency function should remain available to the app");
    let Type::Fn(signature) = &imported.ty else {
        panic!("imported dependency binding should retain its function signature");
    };
    assert_eq!(signature.parameters, vec![Type::Int]);
    assert_eq!(*signature.return_type, Type::Int);
    assert!(!recovered[2].diagnostics.is_empty());
    assert_eq!(recovered[2].hir.name, "broken");
}

#[test]
fn workspace_graph_hashes_are_stable_when_input_order_changes() {
    let app = r#"
            (module app)
            (import dep.core :as dep)
            (export [call])
            ^{:doc "Call the dependency."}
            (defn ^Int call [] (dep/add 1 :value 2))
        "#;
    let dependency = r#"
            (module dep.core)
            (export [add])
            ^{:doc "Add two integers."}
            (defn ^Int add [^Int x ^Int value] (+ x value))
        "#;
    let app_options = CompileOptions::new("app", PythonVersion::MINIMUM);
    let dependency_options = CompileOptions::new("dep.core", PythonVersion::MINIMUM);
    let forward = compile_workspace(
        &[
            CompileInput::new(app, &app_options),
            CompileInput::new(dependency, &dependency_options),
        ],
        &BTreeMap::new(),
    );
    let reverse = compile_workspace(
        &[
            CompileInput::new(dependency, &dependency_options),
            CompileInput::new(app, &app_options),
        ],
        &BTreeMap::new(),
    );
    assert!(!forward.has_errors(), "{:?}", forward.diagnostics);
    assert!(!reverse.has_errors(), "{:?}", reverse.diagnostics);

    let forward_app = interface::read(
        forward.units[0]
            .interface
            .as_ref()
            .expect("forward app interface"),
    )
    .expect("forward app interface should parse");
    let forward_dep = interface::read(
        forward.units[1]
            .interface
            .as_ref()
            .expect("forward dependency interface"),
    )
    .expect("forward dependency interface should parse");
    let reverse_app = interface::read(
        reverse.units[1]
            .interface
            .as_ref()
            .expect("reverse app interface"),
    )
    .expect("reverse app interface should parse");
    let reverse_dep = interface::read(
        reverse.units[0]
            .interface
            .as_ref()
            .expect("reverse dependency interface"),
    )
    .expect("reverse dependency interface should parse");

    assert_eq!(
        forward_app.semantic_interface_hash(),
        reverse_app.semantic_interface_hash()
    );
    assert_eq!(
        forward_app.tooling_metadata_hash(),
        reverse_app.tooling_metadata_hash()
    );
    assert_eq!(
        forward_dep.semantic_interface_hash(),
        reverse_dep.semantic_interface_hash()
    );
    assert_eq!(
        forward_app.graph.external_dependencies,
        reverse_app.graph.external_dependencies
    );
}

/// A memo must be an optimization only: reusing module analyses has to produce
/// exactly what a cold run produces, including after an edit that changes one
/// module's interface and after an edit that only changes a body.
#[test]
fn memoized_workspace_analysis_matches_a_cold_run_across_edits() {
    fn provider_options() -> CompileOptions {
        CompileOptions::new("demo.provider", PythonVersion::MINIMUM)
            .with_expected_module_name("demo.provider")
    }
    fn consumer_options() -> CompileOptions {
        CompileOptions::new("demo.consumer", PythonVersion::MINIMUM)
            .with_expected_module_name("demo.consumer")
    }

    let provider_options = provider_options();
    let consumer_options = consumer_options();
    let consumer = "(module demo.consumer)\n\
         (import demo.provider :as provider)\n\
         (def answer (provider/step 1))\n";

    // Each revision changes the provider: a body-only edit, then a signature
    // change, then back. The consumer is untouched throughout.
    let revisions = [
        "(module demo.provider)\n(export [step])\n^{:doc \"Step.\"} (defn ^Int step [^Int x] (+ x 1))\n",
        "(module demo.provider)\n(export [step])\n^{:doc \"Step.\"} (defn ^Int step [^Int x] (+ x 2))\n",
        "(module demo.provider)\n(export [step])\n^{:doc \"Step.\"} (defn ^Int step [^Int x ^Int y] (+ x y))\n",
        "(module demo.provider)\n(export [step])\n^{:doc \"Step.\"} (defn ^Int step [^Int x] (+ x 1))\n",
    ];

    let memo = super::WorkspaceMemo::new();
    for (revision, provider) in revisions.iter().enumerate() {
        let inputs = [
            CompileInput::new(provider, &provider_options),
            CompileInput::new(consumer, &consumer_options),
        ];
        let cold = analyze_workspace(&inputs, &BTreeMap::new());
        let warm = super::analyze_workspace_with_memo(&inputs, &BTreeMap::new(), &memo);

        assert_eq!(
            cold.has_errors(),
            warm.has_errors(),
            "revision {revision} error state"
        );
        assert_eq!(
            cold.diagnostics
                .iter()
                .map(|located| (located.input_index, located.diagnostic.code))
                .collect::<Vec<_>>(),
            warm.diagnostics
                .iter()
                .map(|located| (located.input_index, located.diagnostic.code))
                .collect::<Vec<_>>(),
            "revision {revision} diagnostics"
        );
        assert_eq!(
            cold.units
                .iter()
                .map(|unit| (unit.analysis.hir.name.clone(), unit.build_hash.clone()))
                .collect::<Vec<_>>(),
            warm.units
                .iter()
                .map(|unit| (unit.analysis.hir.name.clone(), unit.build_hash.clone()))
                .collect::<Vec<_>>(),
            "revision {revision} units"
        );
    }
}

#[test]
fn memoized_workspace_analysis_reuses_untouched_modules() {
    let provider_options = CompileOptions::new("demo.provider", PythonVersion::MINIMUM)
        .with_expected_module_name("demo.provider");
    let consumer_options = CompileOptions::new("demo.consumer", PythonVersion::MINIMUM)
        .with_expected_module_name("demo.consumer");
    let provider =
        "(module demo.provider)\n(export [step])\n^{:doc \"Step.\"} (defn ^Int step [^Int x] (+ x 1))\n";
    let memo = super::WorkspaceMemo::new();

    let first = "(module demo.consumer)\n(import demo.provider :as provider)\n(def answer (provider/step 1))\n";
    let inputs = [
        CompileInput::new(provider, &provider_options),
        CompileInput::new(first, &consumer_options),
    ];
    let cold = super::analyze_workspace_with_memo(&inputs, &BTreeMap::new(), &memo);
    assert!(!cold.has_errors(), "{:?}", cold.diagnostics);
    assert_eq!(memo.stats().analyzed, 2, "a cold run analyzes every module");
    assert_eq!(memo.stats().reused, 0);

    // Editing only the consumer must leave the provider reusable.
    let second = "(module demo.consumer)\n(import demo.provider :as provider)\n(def answer (provider/step 2))\n";
    let inputs = [
        CompileInput::new(provider, &provider_options),
        CompileInput::new(second, &consumer_options),
    ];
    let warm = super::analyze_workspace_with_memo(&inputs, &BTreeMap::new(), &memo);
    assert!(!warm.has_errors(), "{:?}", warm.diagnostics);
    assert_eq!(memo.stats().reused, 1, "the untouched provider is reused");
    assert_eq!(memo.stats().analyzed, 1, "only the edited module is analyzed");
}
