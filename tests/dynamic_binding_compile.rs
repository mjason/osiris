use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use osiris::{
    compiler::{CompileInput, CompileOptions, compile, compile_workspace},
    interface,
    project::PythonVersion,
    syntax::FormKind,
};

fn options() -> CompileOptions {
    CompileOptions::new("dynamic_binding_compile", PythonVersion::default())
}

fn temporary_directory() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let path = env::temp_dir().join(format!("osiris-dynamic-binding-{nonce}"));
    fs::create_dir_all(&path).expect("create temporary directory");
    path
}

fn dynamic_metadata(binding: &interface::PublicBinding) -> bool {
    binding.metadata.iter().any(|entry| {
        matches!(
            &entry.key.kind,
            FormKind::Keyword(name) | FormKind::Symbol(name)
                if name.canonical.trim_start_matches(':') == "dynamic"
        ) && matches!(entry.value.kind, FormKind::Bool(true))
    })
}

fn write_runtime_support(root: &std::path::Path, generated: &osiris::backend::GeneratedPython) {
    let Some(support) = &generated.runtime_support else {
        return;
    };
    for (path, source) in osiris::backend::runtime_distribution_files(
        support,
        osiris::project::PythonVersion::default(),
    )
    .expect("link runtime distribution")
    {
        let destination = root.join(path);
        fs::create_dir_all(destination.parent().expect("support parent"))
            .expect("create support directory");
        fs::write(destination, source).expect("write support file");
    }
}

#[test]
fn dynamic_binding_is_typed_nested_restored_and_propagated_to_futures() {
    let source = r#"
(module dynamic_binding_compile)
(import osiris.core :refer [assert binding])
(import osiris.concurrent :refer :all)
(extern python "dynamic_support"
  (defn ^Int record [^Int value]))

(def ^{:dynamic true :type Int :doc "Named dynamic value."} *named* 1)
^{:dynamic true :doc "Outer dynamic value."} (def ^Int *outer* 10)

^{:doc "Return dynamic roots."}
(defn ^{:type (Vector Int)} roots []
  [*named* *outer*])

^{:doc "Exercise nested dynamic bindings."}
(defn ^Any nested []
  [*named*
   (binding [*named* (record 2)
             *outer* (record *named*)]
     [*named* *outer* (binding [*named* 3] *named*)])
   *named*])

^{:doc "Return the restored value after an exception."}
(defn ^Int restored-after-throw []
  (do
    (try
      (binding [*named* 9]
        (assert false "boom"))
      (catch AssertionError error none))
    *named*))

^{:doc "Capture a dynamic value in a future."}
(defn ^Int future-context []
  (let [gate (promise)
        task (binding [*named* 11]
               (future (do (deref gate) *named*)))]
    (do
      (deliver gate none)
      (deref task))))

(export [*named* *outer* roots nested restored-after-throw future-context])
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:#?}",
        result.analysis.diagnostics
    );

    let interface = interface::read(result.interface.as_deref().expect("generated interface"))
        .expect("read generated interface");
    for name in ["*named*", "*outer*"] {
        let binding = interface
            .bindings
            .iter()
            .find(|binding| binding.canonical == name)
            .unwrap_or_else(|| panic!("missing {name} interface binding"));
        assert!(dynamic_metadata(binding), "{name} lost :dynamic metadata");
    }

    let generated = result.python.expect("generated Python");
    assert!(
        generated.source.contains("dynamic_get"),
        "{}",
        generated.source
    );
    assert!(
        generated.source.contains("binding_values"),
        "{}",
        generated.source
    );

    let root = temporary_directory();
    fs::write(root.join("dynamic_binding_compile.py"), &generated.source)
        .expect("write generated module");
    write_runtime_support(&root, &generated);
    fs::write(
        root.join("dynamic_support.py"),
        "events = []\ndef record(value):\n    events.append(value)\n    return value\n",
    )
    .expect("write dynamic support module");
    let smoke = root.join("smoke.py");
    fs::write(
        &smoke,
        r#"import dynamic_support
from dynamic_binding_compile import future_context, nested, restored_after_throw, roots

assert roots() == (1, 10)
assert nested() == (1, (2, 1, 3), 1)
assert dynamic_support.events == [2, 1]
assert restored_after_throw() == 1
assert future_context() == 11
assert roots() == (1, 10)
print("ok")
"#,
    )
    .expect("write smoke script");
    let output = Command::new("python3")
        .arg(&smoke)
        .env("PYTHONPATH", &root)
        .output()
        .expect("run generated Python");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}\npython:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        generated.source
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "ok\n");
    fs::remove_dir_all(root).expect("remove temporary directory");
}

#[test]
fn binding_rejects_non_dynamic_local_and_wrong_typed_values() {
    let source = r#"
(module dynamic_binding_compile)
(import osiris.core :refer [binding])
(def ^{:dynamic true :type Int} *value* 1)
(def ^Int ordinary 2)
(defn ^Int wrong-target []
  (binding [ordinary 3] ordinary))
(defn ^Int wrong-local []
  (let [*value* 2]
    (binding [*value* 3] *value*)))
(defn ^Int wrong-type []
  (binding [*value* "bad"] *value*))
"#;
    let result = compile(source, &options());
    let messages = result
        .analysis
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("is not a `^:dynamic` top-level Value")),
        "{:#?}",
        result.analysis.diagnostics
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("resolves to a local value")),
        "{:#?}",
        result.analysis.diagnostics
    );
    assert!(
        result.analysis.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "OSR-T0001"
                && diagnostic.message.contains("expected `Int`, found `Str`")
        }),
        "{:#?}",
        result.analysis.diagnostics
    );
}

#[test]
fn dynamic_vars_require_root_values() {
    let result = compile(
        "(module dynamic_binding_compile) (def ^{:dynamic true :type Int} *missing*)",
        &options(),
    );
    assert!(result.analysis.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "OSR-T0042" && diagnostic.message.contains("requires an initial value")
    }));
}

#[test]
fn imported_dynamic_vars_keep_their_binding_identity() {
    let provider = r#"
(module dynamic.provider)
(def ^{:dynamic true :type Int :doc "Dynamic scale."} *scale* 2)
^{:doc "Scale an integer."}
(defn ^Int scaled [^Int value] (* value *scale*))
(export [*scale* scaled])
"#;
    let consumer = r#"
(module dynamic.consumer)
(import osiris.core :refer [binding])
(import dynamic.provider :as provider)
^{:doc "Return values under several dynamic bindings."}
(defn ^{:type (Vector Int)} roots []
  [(provider/scaled 3)
   (binding [provider/*scale* 5]
     (provider/scaled 3))
   (provider/scaled 3)])
(export [roots])
"#;
    let provider_options = CompileOptions::new("dynamic.provider", PythonVersion::default())
        .with_expected_module_name("dynamic.provider");
    let consumer_options = CompileOptions::new("dynamic.consumer", PythonVersion::default())
        .with_expected_module_name("dynamic.consumer");
    let inputs = [
        CompileInput::new(provider, &provider_options),
        CompileInput::new(consumer, &consumer_options),
    ];
    let workspace = compile_workspace(&inputs, &BTreeMap::new());
    assert!(!workspace.has_errors(), "{:#?}", workspace.diagnostics);

    let root = temporary_directory();
    let package = root.join("dynamic");
    fs::create_dir_all(&package).expect("create generated package");
    fs::write(package.join("__init__.py"), "").expect("write package marker");
    let mut runtime_package = None;
    let mut runtime_helpers = BTreeSet::new();
    for unit in &workspace.units {
        let module = unit
            .analysis
            .hir
            .name
            .rsplit('.')
            .next()
            .expect("module leaf");
        let generated = unit.python.as_ref().expect("generated Python");
        fs::write(package.join(format!("{module}.py")), &generated.source)
            .expect("write generated module");
        if let Some(support) = &generated.runtime_support {
            if let Some(package) = &runtime_package {
                assert_eq!(package, &support.package);
            } else {
                runtime_package = Some(support.package.clone());
            }
            runtime_helpers.extend(support.helpers.iter().cloned());
        }
    }
    if let Some(runtime_package) = runtime_package {
        for (path, source) in
            osiris::backend::runtime_support_files(&runtime_package, &runtime_helpers)
        {
            let destination = root.join(path);
            fs::create_dir_all(destination.parent().expect("support parent"))
                .expect("create support directory");
            fs::write(destination, source).expect("write support file");
        }
    }
    let smoke = root.join("smoke.py");
    fs::write(
        &smoke,
        "from dynamic.consumer import roots\nassert roots() == (6, 15, 6)\nprint('ok')\n",
    )
    .expect("write smoke script");
    let output = Command::new("python3")
        .arg(&smoke)
        .env("PYTHONPATH", &root)
        .output()
        .expect("run generated Python");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}\nprovider:\n{}\nconsumer:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        workspace.units[0]
            .python
            .as_ref()
            .expect("provider Python")
            .source,
        workspace.units[1]
            .python
            .as_ref()
            .expect("consumer Python")
            .source,
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "ok\n");
    fs::remove_dir_all(root).expect("remove temporary directory");
}
