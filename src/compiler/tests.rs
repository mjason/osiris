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
