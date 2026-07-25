//! Project-internal language compatibility tests.
//!
//! These fixtures are deliberately small and domain-neutral.  They exercise
//! the public compiler pipeline as a user would: read source, compile it,
//! stage the generated distribution, and execute the resulting Python.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

use osiris::{
    compiler::{self, CompileOptions},
    formatter,
    project::PythonVersion,
    reader,
    syntax::FormKind,
};
use serde_json::Value;

static NEXT_TEMPORARY_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

fn fixture(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/compatibility")
        .join(relative)
}

fn source(relative: &str) -> String {
    fs::read_to_string(fixture(relative)).unwrap_or_else(|error| {
        panic!("could not read compatibility fixture `{relative}`: {error}")
    })
}

fn expected(relative: &str) -> Value {
    let path = fixture(relative);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("could not read `{}`: {error}", path.display()));
    json5::from_str(&text)
        .unwrap_or_else(|error| panic!("could not parse `{}`: {error}", path.display()))
}

fn options(module: &str) -> CompileOptions {
    CompileOptions::new(module, PythonVersion::default())
}

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn new() -> Self {
        let sequence = NEXT_TEMPORARY_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "osiris-compatibility-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("compatibility temporary directory should be created");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Execution {
    status: Option<i32>,
    stdout: String,
    stderr: String,
    generated: String,
}

fn run_generated(source: &str, module: &str) -> Execution {
    let result = compiler::compile(source, &options(module));
    assert!(
        result.analysis.diagnostics.is_empty(),
        "source should compile without diagnostics: {:#?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("compiler should generate Python");
    let temporary = TemporaryDirectory::new();

    let module_path = temporary.path().join(compiler::python_module_path(module));
    fs::create_dir_all(module_path.parent().expect("generated module parent"))
        .expect("generated module parent should be created");
    fs::write(&module_path, &generated.source).expect("generated module should be written");

    if let Some(support) = generated.runtime_support.as_ref() {
        for (path, contents) in
            osiris::backend::runtime_distribution_files(support, PythonVersion::default())
                .expect("runtime distribution should be linkable")
        {
            let destination = temporary.path().join(path);
            fs::create_dir_all(destination.parent().expect("runtime file parent"))
                .expect("runtime file parent should be created");
            fs::write(destination, contents).expect("runtime file should be written");
        }
    }

    for embedded in &result.analysis.embedded_python {
        let destination = temporary
            .path()
            .join(compiler::python_module_path(&embedded.logical_module));
        fs::create_dir_all(destination.parent().expect("embedded module parent"))
            .expect("embedded module parent should be created");
        fs::write(destination, &embedded.source).expect("embedded module should be written");
    }

    let output = Command::new("python3")
        .arg(&module_path)
        .env("PYTHONPATH", temporary.path())
        .output()
        .expect("python3 should execute generated source");
    Execution {
        status: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        generated: generated.source,
    }
}

#[test]
fn reader_cases_preserve_embedded_syntax() {
    let document = reader::read(&source("reader/embedded-json/input.osr"));
    assert!(
        document.diagnostics.is_empty(),
        "{:#?}",
        document.diagnostics
    );
    let embedded = document
        .forms
        .iter()
        .find(|form| matches!(form.kind, FormKind::EmbeddedLanguage { .. }))
        .expect("expected an embedded language form");
    let FormKind::EmbeddedLanguage {
        language,
        label,
        body,
        ..
    } = &embedded.kind
    else {
        unreachable!("embedded form was found by the match above");
    };
    let expected = expected("reader/embedded-json/expected.json");
    assert_eq!(language, expected["language"].as_str().unwrap());
    assert_eq!(label.canonical, expected["label"].as_str().unwrap());
    assert_eq!(body, expected["body"].as_str().unwrap());
}

fn assert_behavior_case(case: &str, module: &str) {
    let execution = run_generated(&source(&format!("behavior/{case}/input.osr")), module);
    let expected = expected(&format!("behavior/{case}/expected.json"));
    assert_eq!(
        execution.status,
        expected["status"].as_i64().map(|value| value as i32)
    );
    assert_eq!(execution.stdout, expected["stdout"].as_str().unwrap());
    assert_eq!(execution.stderr, expected["stderr"].as_str().unwrap());
}

#[test]
fn behavior_map_basic() {
    assert_behavior_case("map-basic", "compat.map-basic");
}

#[test]
fn behavior_order_events() {
    assert_behavior_case("order-events", "compat.order-events");
}

#[test]
fn behavior_embedded_python() {
    assert_behavior_case("embedded-python", "compat.embedded-python");
}

#[test]
fn behavior_cases_preserve_expected_failures() {
    let execution = run_generated(
        &source("behavior/expected-error/input.osr"),
        "compat.expected-error",
    );
    let expected = expected("behavior/expected-error/expected.json");
    assert_ne!(execution.status, Some(0));
    assert!(
        execution
            .stderr
            .contains(expected["stderrContains"].as_str().unwrap()),
        "stderr did not contain the expected exception: {}\nGenerated Python:\n{}",
        execution.stderr,
        execution.generated
    );
}

#[test]
fn diagnostic_cases_keep_stable_codes() {
    let document = reader::read(&source("diagnostics/unterminated-embedded/input.osr"));
    let expected = expected("diagnostics/unterminated-embedded/expected.json");
    let actual = document
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();
    let wanted = expected["codes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|code| code.as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(actual, wanted);
}

#[test]
fn formatter_cases_are_idempotent() {
    let input = source("formatter/threading/input.osr");
    let expected = source("formatter/threading/expected.osr");
    let formatted = formatter::format_source(&input).expect("fixture should be formattable");
    assert_eq!(formatted, expected);
    assert_eq!(
        formatter::format_source(&formatted).expect("formatted source should remain formattable"),
        formatted
    );
}
