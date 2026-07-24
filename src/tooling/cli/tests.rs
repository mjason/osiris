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
    let api = &result["result"][0];
    assert_eq!(api["bindingId"], "osiris.collection::function::frequencies");
    assert_eq!(api["requestedLocale"], "zh-CN");
    assert_eq!(api["resolvedLocale"], "zh-CN");
    assert_eq!(api["evaluation"], "consumer");
    assert!(
        api["selectedDocumentation"]
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
    assert_eq!(value["result"][0]["requestedLocale"], "zh-CN-x-agent");
    assert_eq!(value["result"][0]["resolvedLocale"], "zh-CN");
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
    assert!(hover["result"][0]["requestedLocale"].is_null());
    assert!(hover["result"][0]["resolvedLocale"].is_null());
    assert!(
        hover["result"][0]["selectedDocumentation"]
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
