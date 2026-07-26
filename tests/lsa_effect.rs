use std::{fs, path::PathBuf};

use osiris::agent::{LsaOptions, run};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EffectSuite {
    schema: String,
    cases: Vec<EffectCase>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EffectCase {
    id: String,
    locale: String,
    request: String,
    answer_contains_any: Vec<String>,
    reference_contains: String,
    #[serde(default)]
    example_contains: Option<String>,
    #[serde(default)]
    result_equals: Option<serde_json::Value>,
    minimum_examples: usize,
    #[serde(default)]
    follow_up: Option<EffectFollowUp>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EffectFollowUp {
    request: String,
    answer_contains_any: Vec<String>,
}

#[test]
fn fixed_effect_suite_is_well_formed() {
    let suite = load_suite();
    assert_eq!(suite.schema, "osiris-lsa-effect-suite/v1");
    assert!(!suite.cases.is_empty());
    for case in suite.cases {
        assert!(!case.id.trim().is_empty());
        assert!(!case.request.trim().is_empty());
        assert!(!case.answer_contains_any.is_empty());
        assert!(case.minimum_examples > 0);
        if let Some(follow_up) = case.follow_up {
            assert!(!follow_up.request.trim().is_empty());
            assert!(!follow_up.answer_contains_any.is_empty());
        }
    }
}

#[test]
#[ignore = "requires OSR_API_KEY and makes bounded live provider requests"]
fn live_lsa_effect_suite() {
    let suite = load_suite();
    let selected = std::env::var("OSR_LSA_EFFECT_CASE").ok();
    let mut executed = 0;
    for case in suite.cases.into_iter().filter(|case| {
        selected
            .as_ref()
            .is_none_or(|selected| selected == &case.id)
    }) {
        executed += 1;
        let response = run(&LsaOptions {
            request: case.request.clone(),
            session: None,
            locale: Some(case.locale.clone()),
            file: None,
        })
        .unwrap_or_else(|error| panic!("{}: {error}", case.id));

        assert_eq!(response.schema, "osiris-lsa/v1", "{}", case.id);
        assert!(!response.session_id.is_empty(), "{}", case.id);
        assert!(
            case.answer_contains_any.iter().any(|term| response
                .answer
                .to_lowercase()
                .contains(&term.to_lowercase())),
            "{}: answer did not contain any expected term: {}",
            case.id,
            response.answer
        );
        assert!(
            response
                .references
                .iter()
                .any(|reference| reference.contains(&case.reference_contains)),
            "{}: missing reference `{}`: {:?}",
            case.id,
            case.reference_contains,
            response.references
        );
        assert!(
            response.examples.len() >= case.minimum_examples,
            "{}: expected at least {} example(s)",
            case.id,
            case.minimum_examples
        );
        if let Some(expected) = &case.example_contains {
            assert!(
                response
                    .examples
                    .iter()
                    .any(|example| example.code.contains(expected)),
                "{}: no example contained `{expected}`: {:?}",
                case.id,
                response
                    .examples
                    .iter()
                    .map(|example| &example.code)
                    .collect::<Vec<_>>()
            );
        }
        assert!(
            response.examples.iter().all(|example| example.compiled),
            "{}: examples failed validation: {:?}",
            case.id,
            response
                .examples
                .iter()
                .flat_map(|example| &example.diagnostics)
                .collect::<Vec<_>>()
        );
        if let Some(expected) = &case.result_equals {
            assert!(
                response
                    .examples
                    .iter()
                    .any(|example| example.result.as_ref() == Some(expected)),
                "{}: no evaluated example returned {expected}: {:?}",
                case.id,
                response
                    .examples
                    .iter()
                    .map(|example| &example.result)
                    .collect::<Vec<_>>()
            );
        }
        assert!(
            response.examples.iter().all(|example| example.evaluated),
            "{}: examples were not executed successfully: {:?}",
            case.id,
            response
                .examples
                .iter()
                .flat_map(|example| &example.diagnostics)
                .collect::<Vec<_>>()
        );
        println!(
            "{}: session={} examples={}",
            case.id,
            response.session_id,
            response.examples.len()
        );
        if let Some(follow_up) = case.follow_up {
            validate_follow_up(&case.id, &case.locale, &response, follow_up);
        }
    }
    assert!(executed > 0, "no effect case matched {selected:?}");
}

fn validate_follow_up(
    case_id: &str,
    locale: &str,
    initial: &osiris::agent::LsaResponse,
    follow_up: EffectFollowUp,
) {
    let response = run(&LsaOptions {
        request: follow_up.request,
        session: Some(initial.session_id.clone()),
        locale: Some(locale.to_owned()),
        file: None,
    })
    .unwrap_or_else(|error| panic!("{case_id} follow-up: {error}"));

    assert_eq!(response.schema, "osiris-lsa/v1", "{case_id} follow-up");
    assert_eq!(
        response.session_id, initial.session_id,
        "{case_id} follow-up changed session"
    );
    assert!(
        follow_up.answer_contains_any.iter().any(|term| response
            .answer
            .to_lowercase()
            .contains(&term.to_lowercase())),
        "{case_id} follow-up answer did not contain any expected term: {}",
        response.answer
    );
    assert!(
        response.examples.iter().all(|example| example.compiled),
        "{case_id} follow-up returned an invalid example: {:?}",
        response
            .examples
            .iter()
            .flat_map(|example| &example.diagnostics)
            .collect::<Vec<_>>()
    );
    assert!(
        response.examples.iter().all(|example| example.evaluated),
        "{case_id} follow-up returned an unevaluated example"
    );
}

fn load_suite() -> EffectSuite {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lsa/effect-cases.jsonc");
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()));
    json5::from_str(&source)
        .unwrap_or_else(|error| panic!("invalid effect suite {}: {error}", path.display()))
}
