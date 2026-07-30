//! A provider's installed private runtime is linked, never imported.
//!
//! An external `.osri` bakes the provider's installed runtime location into
//! `:runtime :module`. Consumer codegen must relocate that under the consumer
//! module's own runtime package and record the link for the CLI to copy the
//! file (OEP-0002-R033A/R033G) — while a workspace-internal provider keeps its
//! plain same-build import.

use std::collections::BTreeMap;

use osiris::{
    compiler::{CompileInput, CompileOptions, compile, compile_workspace},
    interface,
    project::PythonVersion,
};

const PROVIDER_SOURCE: &str = r#"
(module prov.core)

(py/embed backend "backend/impl.py")

(extern python backend
  ^{:doc "Advance a value by one." :export true}
  (defn ^Int step [^Int value]
    :contract {:id "prov/step-v1" :effects []}))
"#;

const CONSUMER_SOURCE: &str = r#"
(module app.main)
(import prov.core :refer [step])

^{:doc "Uses the provider." :export true}
(defn ^Int advance [^Int value] (step value))
"#;

fn provider_interface() -> interface::Interface {
    let options = CompileOptions::new("prov.core", PythonVersion::new(3, 11))
        .with_expected_module_name("prov.core")
        .with_provider("prov", "1.0")
        .with_embedded_sources(BTreeMap::from([(
            "backend/impl.py".to_owned(),
            "def step(value):\n    return value + 1\n".to_owned(),
        )]));
    let result = compile(PROVIDER_SOURCE, &options);
    assert!(
        !result.has_errors(),
        "provider fixture must compile: {:?}",
        result.analysis.diagnostics
    );
    interface::read(&result.interface.expect("provider interface")).expect("readable interface")
}

#[test]
fn an_external_provider_runtime_is_relocated_and_recorded() {
    let provider = provider_interface();
    let runtime = provider
        .bindings
        .iter()
        .find(|binding| binding.canonical == "step")
        .and_then(|binding| binding.runtime.as_ref())
        .expect("provider step has a runtime binding");
    assert!(
        runtime.module.contains(".__osiris_runtime__.packages."),
        "fixture must exercise an installed-runtime module, got `{}`",
        runtime.module
    );

    let options = CompileOptions::new("app.main", PythonVersion::new(3, 11))
        .with_expected_module_name("app.main")
        .with_provider("app", "1.0");
    let inputs = [CompileInput::new(CONSUMER_SOURCE, &options)];
    let external = BTreeMap::from([("prov.core".to_owned(), provider)]);
    let workspace = compile_workspace(&inputs, &external);
    assert!(
        !workspace.has_errors(),
        "consumer must compile: {:?}",
        workspace.diagnostics
    );
    let generated = workspace.units[0]
        .python
        .as_ref()
        .expect("consumer generates Python");

    assert!(
        !generated.source.contains("from prov.__osiris_runtime__"),
        "generated code must not import the provider's installed runtime:\n{}",
        generated.source
    );
    assert!(
        generated
            .source
            .contains("from app.__osiris_runtime__.packages.prov_"),
        "generated code must import the relocated copy:\n{}",
        generated.source
    );
    let support = generated
        .runtime_support
        .as_ref()
        .expect("linking requests runtime support");
    let (relocated, original) = support
        .external_modules
        .iter()
        .next()
        .expect("the link is recorded for the caller to copy");
    assert!(relocated.starts_with("app.__osiris_runtime__.packages.prov_"));
    assert!(original.starts_with("prov.__osiris_runtime__.packages.prov_"));
}

#[test]
fn a_workspace_internal_provider_keeps_its_same_build_import() {
    let provider_options = CompileOptions::new("prov.core", PythonVersion::new(3, 11))
        .with_expected_module_name("prov.core")
        .with_provider("app", "1.0")
        .with_embedded_sources(BTreeMap::from([(
            "backend/impl.py".to_owned(),
            "def step(value):\n    return value + 1\n".to_owned(),
        )]));
    let consumer_options = CompileOptions::new("app.main", PythonVersion::new(3, 11))
        .with_expected_module_name("app.main")
        .with_provider("app", "1.0");
    let inputs = [
        CompileInput::new(PROVIDER_SOURCE, &provider_options),
        CompileInput::new(CONSUMER_SOURCE, &consumer_options),
    ];
    let workspace = compile_workspace(&inputs, &BTreeMap::new());
    assert!(
        !workspace.has_errors(),
        "workspace must compile: {:?}",
        workspace.diagnostics
    );
    let consumer = workspace
        .units
        .iter()
        .find(|unit| unit.analysis.hir.name == "app.main")
        .expect("consumer unit");
    let generated = consumer.python.as_ref().expect("consumer generates Python");
    // Same build, same output tree: the provider's runtime is emitted by this
    // compilation, so the import stays as authored and nothing is linked.
    assert!(
        generated
            .source
            .contains("from prov.__osiris_runtime__.packages."),
        "first-party runtime import must stay in place:\n{}",
        generated.source
    );
    assert!(
        generated
            .runtime_support
            .as_ref()
            .is_none_or(|support| support.external_modules.is_empty()),
        "nothing external to link inside one workspace"
    );
}
