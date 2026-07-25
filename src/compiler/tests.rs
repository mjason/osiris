include!("tests/workspace.rs");
include!("tests/macros_and_records.rs");
include!("tests/cycles.rs");

#[test]
fn analysis_reports_public_rich_metadata_contracts_before_codegen() {
    let options = super::CompileOptions::new(
        "metadata_contract",
        crate::types::PythonVersion::DEFAULT_TARGET,
    );
    let missing = super::analyze(
        "(module metadata-contract) (def ^Int value 1) (export [value])",
        &options,
    );
    assert!(
        missing
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-I0087")
    );

    let invalid = super::analyze(
        r#"(module metadata-contract)
           ^{:doc {:default "Value." "not_a_locale" "Translation."}}
           (def ^Int value 1)
           (export [value])"#,
        &options,
    );
    assert!(
        invalid
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-I0085")
    );
}

#[test]
fn embedded_content_bindings_and_documentation_references_lower_statically() {
    let options =
        super::CompileOptions::new("embedded.docs", crate::types::PythonVersion::DEFAULT_TARGET);
    let source = r#"(module embedded.docs)

~json<settings>
{"theme": "dark"}
</settings>

~markdown<identity-doc>
Return the supplied value.
</identity-doc>

~osiris<identity-example>
(identity 4)
;; => 4
</identity-example>

^{:doc {:default identity-doc}
  :examples [identity-example]}
(defn ^Int identity [^Int value] value)

(def ^Str copied-settings settings)

(export [identity])
"#;
    let result = super::compile(source, &options);
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated Python").source;
    assert!(
        generated.contains("settings: str = '{\"theme\": \"dark\"}'"),
        "{generated}"
    );
    assert!(!generated.contains("identity_doc"));
    assert!(!generated.contains("identity_example"));
    let interface = result.interface.as_deref().expect("interface");
    assert!(interface.contains("Return the supplied value."));
    assert!(interface.contains("(identity 4)"));
    assert!(interface.contains("osiris/content-references"));
    let semantic =
        crate::semantic::SemanticDocument::from_analysis(&result.analysis, "embedded/docs.osr");
    let identity = semantic
        .symbols
        .iter()
        .find(|symbol| {
            symbol.canonical == "identity" && symbol.binding_id.contains("embedded.docs")
        })
        .expect("identity symbol");
    assert_eq!(identity.content_references.len(), 2);
    assert!(identity.content_references.iter().any(|reference| {
        reference.field == "doc/default"
            && reference.language == "markdown"
            && reference.label == "identity-doc"
            && reference.content_hash.starts_with("sha256:")
    }));
}

#[test]
fn documentation_examples_reject_the_pre_release_string_vector_shape() {
    let result = super::compile(
        r#"(module embedded.legacy)
^{:doc "Identity."
  :examples [["(identity 1)" ";; => 1"]]}
(defn ^Int identity [^Int value] value)
(export [identity])
"#,
        &super::CompileOptions::new(
            "embedded.legacy",
            crate::types::PythonVersion::DEFAULT_TARGET,
        ),
    );
    assert!(result.analysis.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "OSR-A0012"
            && diagnostic
                .message
                .contains("unquoted `~osiris` binding name")
    }));
}

#[test]
fn embedded_python_is_formatted_relocated_and_imported_by_handle() {
    let options = super::CompileOptions::new(
        "embedded.python",
        crate::types::PythonVersion::DEFAULT_TARGET,
    )
    .with_provider("demo-provider", "0.1.0");
    let source = r#"(module embedded.python)

~python<text-backend>
def normalize(value:str)->str:
 return value.strip().casefold()
</text-backend>

(extern python text-backend
  (defn ^Str normalize [^Str value]))

(normalize "  Hello  ")
"#;
    let result = super::compile(source, &options);
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    assert_eq!(result.analysis.embedded_python.len(), 1);
    let embedded = &result.analysis.embedded_python[0];
    assert!(embedded.logical_module.contains(".packages.demo_provider_"));
    assert!(
        embedded
            .logical_module
            .ends_with(".embedded.python.text_backend"),
        "{}",
        embedded.logical_module
    );
    assert!(
        embedded
            .source
            .contains("def normalize(value: str) -> str:")
    );
    let generated = result.python.expect("generated Python").source;
    assert!(generated.contains(&format!("from {} import", embedded.logical_module)));
    assert!(generated.contains("normalize,"));
}

#[test]
fn embedded_python_links_only_the_static_provider_closure() {
    let options = super::CompileOptions::new(
        "embedded.graph",
        crate::types::PythonVersion::DEFAULT_TARGET,
    );
    let source = r#"(module embedded.graph)

~python<helper>
def suffix(value: str) -> str:
    return value + "!"
</helper>

~python<backend>
import helper

def normalize(value: str) -> str:
    return helper.suffix(value.strip())
</backend>

~python<unused>
def ignored() -> str:
    return "unused"
</unused>

(extern python backend
  (defn ^Str normalize [^Str value]))
"#;
    let result = super::compile(source, &options);
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    assert_eq!(result.analysis.embedded_python.len(), 2);
    assert!(
        result
            .analysis
            .embedded_python
            .iter()
            .all(|artifact| artifact.handle != "unused")
    );
    let backend = result
        .analysis
        .embedded_python
        .iter()
        .find(|artifact| artifact.handle == "backend")
        .expect("backend artifact");
    assert!(backend.source.contains(" as helper"), "{}", backend.source);
}

#[test]
fn embedded_python_rejects_dynamic_module_discovery() {
    let result = super::compile(
        r#"(module embedded.dynamic)
~python<backend>
def load(name: str):
    return __import__(name)
</backend>
(extern python backend
  (defn ^Any load [^Str name]))
"#,
        &super::CompileOptions::new(
            "embedded.dynamic",
            crate::types::PythonVersion::DEFAULT_TARGET,
        ),
    );
    assert!(result.analysis.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "OSR-B0003" && diagnostic.message.contains("static import statements")
    }));
}
