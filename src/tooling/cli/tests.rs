use super::{InspectFormat, InspectView, parse_inspect_arguments, run_cli};

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
fn inspect_accepts_syntax_and_format_in_any_order() {
    let arguments = arguments(&["--format", "json", "--syntax", "source.osr"]);
    assert_eq!(
        parse_inspect_arguments(&arguments),
        Ok(("source.osr", InspectFormat::Json, InspectView::Syntax))
    );
}

#[test]
fn inspect_rejects_invalid_format() {
    let arguments = arguments(&["source.osr", "--format", "yaml"]);
    assert!(parse_inspect_arguments(&arguments).is_err());
}
