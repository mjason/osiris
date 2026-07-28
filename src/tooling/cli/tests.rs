use super::run_cli;

fn arguments(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

#[test]
fn bare_source_path_remains_an_error() {
    let outcome = run_cli(&arguments(&["source.osr"]));
    assert_eq!(outcome.exit_code, 2);
    assert!(outcome.stderr.contains("unexpected arguments"));
}

#[test]
fn lsc_requires_a_known_operation() {
    let outcome = run_cli(&arguments(&["lsc", "inspect"]));
    assert_eq!(outcome.exit_code, 2);
    assert!(outcome.stderr.contains("unknown lsc operation"));
}

#[test]
fn lsc_rejects_invalid_format() {
    let outcome = run_cli(&arguments(&["lsc", "diagnostics", "--format", "yaml"]));
    assert_eq!(outcome.exit_code, 2);
    assert!(outcome.stderr.contains("--format must be"));
}

#[test]
fn lsc_queries_embedded_standard_apis_without_a_workspace() {
    let outcome = run_cli(&arguments(&[
        "lsc",
        "hover",
        "osiris.collection/frequencies",
        "--locale",
        "zh-cn",
        "--format",
        "json",
    ]));

    assert_eq!(outcome.exit_code, 0, "{}", outcome.stderr);
    let result: serde_json::Value = serde_json::from_str(&outcome.stdout).unwrap();
    let api = &result["result"];
    assert_eq!(api["bindingId"], "osiris.collection::function::frequencies");
    assert_eq!(
        api["documentation"]["selection"]["requestedLocale"],
        "zh-CN"
    );
    assert_eq!(api["documentation"]["selection"]["resolvedLocale"], "zh-CN");
    assert_eq!(api["semantic"]["evaluation"], "consumer");
    assert!(
        api["documentation"]["selection"]["text"]
            .as_str()
            .unwrap()
            .contains("逻辑相等")
    );
}

#[test]
fn lsc_signature_accepts_a_standard_api_identity() {
    let outcome = run_cli(&arguments(&["lsc", "signature", "osiris.concurrent/pmap"]));

    assert_eq!(outcome.exit_code, 0, "{}", outcome.stderr);
    assert!(outcome.stdout.contains("(pmap function collections...)"));
    assert!(outcome.stdout.contains("Fn["));
}

#[test]
fn lsc_locales_are_strict_bcp47_and_use_lookup_fallback() {
    let invalid = run_cli(&arguments(&[
        "lsc",
        "hover",
        "osiris.core/map",
        "--locale",
        "zh_CN",
    ]));
    assert_eq!(invalid.exit_code, 2);
    assert!(invalid.stderr.contains("invalid BCP 47 locale"));

    let fallback = run_cli(&arguments(&[
        "lsc",
        "hover",
        "osiris.core/map",
        "--locale",
        "zh-CN-x-agent",
        "--format",
        "json",
    ]));
    assert_eq!(fallback.exit_code, 0, "{}", fallback.stderr);
    let value: serde_json::Value = serde_json::from_str(&fallback.stdout).unwrap();
    assert_eq!(
        value["result"]["documentation"]["selection"]["requestedLocale"],
        "zh-CN-x-agent"
    );
    assert_eq!(
        value["result"]["documentation"]["selection"]["resolvedLocale"],
        "zh-CN"
    );
}

#[test]
fn lsc_uses_authored_default_and_reports_the_embedded_source_location() {
    let hover = run_cli(&arguments(&[
        "lsc",
        "hover",
        "osiris.concurrent/pmap",
        "--format",
        "json",
    ]));
    assert_eq!(hover.exit_code, 0, "{}", hover.stderr);
    let hover: serde_json::Value = serde_json::from_str(&hover.stdout).unwrap();
    assert!(hover["result"]["documentation"]["selection"]["requestedLocale"].is_null());
    assert!(hover["result"]["documentation"]["selection"]["resolvedLocale"].is_null());
    assert!(
        hover["result"]["documentation"]["selection"]["text"]
            .as_str()
            .unwrap()
            .starts_with("Eagerly submit mapped tasks")
    );

    let definition = run_cli(&arguments(&[
        "lsc",
        "definition",
        "osiris.concurrent/pmap",
        "--format",
        "json",
    ]));
    assert_eq!(definition.exit_code, 0, "{}", definition.stderr);
    let definition: serde_json::Value = serde_json::from_str(&definition.stdout).unwrap();
    let location = &definition["result"][0];
    let uri = location["uri"].as_str().unwrap();
    let source = crate::stdlib::source_artifact_by_uri(uri).expect("standard source");
    let line = location["range"]["start"]["line"].as_u64().unwrap() as usize;
    assert!(source.lines().nth(line).unwrap().contains("pmap"));
}

/// The Osiris-to-Python name mapping is fixed by OEP-0001-R005A and is what a
/// Python caller has to type, so it is worth being able to ask for directly
/// rather than reading it out of generated output.
#[test]
fn lsc_name_reports_the_python_spelling() {
    for (osiris, python) in [
        ("rolling-mean", "rolling_mean"),
        ("missing?", "missing_p"),
        ("reset!", "reset_bang"),
        ("column*", "column_u2a_"),
        ("均线", "均线"),
        ("class", "class_"),
        ("2value", "_2value"),
    ] {
        let outcome = run_cli(&arguments(&["lsc", "name", osiris]));
        assert_eq!(outcome.exit_code, 0, "{osiris}: {}", outcome.stderr);
        assert_eq!(outcome.stdout.trim(), python, "{osiris}");
    }
}

#[test]
fn lsc_name_distinguishes_a_module_path_from_an_identifier() {
    let outcome = run_cli(&arguments(&[
        "lsc",
        "name",
        "dm.dsl.pandas",
        "--format",
        "json",
    ]));
    assert_eq!(outcome.exit_code, 0, "{}", outcome.stderr);
    let value: serde_json::Value = serde_json::from_str(&outcome.stdout).unwrap();
    assert_eq!(value["result"]["pythonIdentifier"], "dm_u2e_dsl_u2e_pandas");
    assert_eq!(value["result"]["pythonModulePath"], "dm.dsl.pandas");
}

#[test]
fn lsc_name_requires_a_name() {
    let outcome = run_cli(&arguments(&["lsc", "name"]));
    assert_eq!(outcome.exit_code, 1);
    assert!(outcome.stderr.contains("requires an Osiris NAME"));
}
