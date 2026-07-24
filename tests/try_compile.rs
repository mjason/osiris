use std::{
    env, fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use osiris::{
    compiler::{CompileOptions, compile},
    project::PythonVersion,
};

fn options() -> CompileOptions {
    CompileOptions::new("try_compile", PythonVersion::default())
}

fn temporary_directory() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let path = env::temp_dir().join(format!("osiris-try-{nonce}"));
    fs::create_dir_all(&path).expect("create temporary directory");
    path
}

#[test]
fn body_only_try_lowers_without_an_invalid_python_try_statement() {
    let source = r#"
(module try_compile)
(defn ^Int value []
  (try (+ 40 2)))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let python = result
        .python
        .expect("body-only try should generate Python")
        .source;
    assert!(python.contains("def value"), "{python}");
    assert!(
        !python.contains("try:"),
        "body-only try emitted invalid Python: {python}"
    );
}

#[test]
fn malformed_catch_reports_a_diagnostic_without_panicking() {
    let source = r#"
(module try_compile)
(defn ^Int value []
  (try 1 (catch)))
"#;
    let result = compile(source, &options());
    assert!(
        result
            .analysis
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-A0004"),
        "expected a catch shape diagnostic, got {:?}",
        result.analysis.diagnostics
    );
}

#[test]
fn raise_rejects_a_statically_known_non_exception_value() {
    let source = r#"
(module try_compile)
(defn ^None value []
  (raise 1))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "OSR-T0033"
                && diagnostic
                    .message
                    .contains("raise expects an exception value")
        }),
        "expected a raise type diagnostic, got {:?}",
        result.analysis.diagnostics
    );
}

#[test]
fn catch_after_finally_is_rejected() {
    let source = r#"
(module try_compile)
(defn ^Int value []
  (try 1
    (finally 2)
    (catch Exception error 3)))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "OSR-A0004"
                && diagnostic
                    .message
                    .contains("catch clauses must appear before finally")
        }),
        "expected catch/finally ordering diagnostic, got {:?}",
        result.analysis.diagnostics
    );
}

#[test]
fn empty_finally_reports_a_shape_diagnostic() {
    let source = r#"
(module try_compile)
(defn ^Int value []
  (try 1 (finally)))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "OSR-A0004"
                && diagnostic.message.contains("finally body cannot be empty")
        }),
        "expected an empty-finally diagnostic, got {:?}",
        result.analysis.diagnostics
    );
}

#[test]
fn builtin_exception_types_compile_and_run_through_catch_finally() {
    let source = r#"
(module try_compile)
(py/import builtins :as py)
(defn ^Int recover []
  (try
    (raise (py.ValueError "boom"))
    (catch Exception error 7)
    (finally (py.len [1]))))
(defn ^Int recover-qualified []
  (try
    (raise (py.TypeError "boom"))
    (catch builtins/TypeError error 9)))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated Python").source;
    assert!(generated.contains("except Exception as"), "{generated}");
    assert!(generated.contains("except TypeError as"), "{generated}");
    let root = temporary_directory();
    fs::write(root.join("try_compile.py"), &generated).expect("write generated module");
    let smoke = root.join("smoke.py");
    fs::write(
        &smoke,
        "from try_compile import recover, recover_qualified\nassert recover() == 7\nassert recover_qualified() == 9\nprint('ok')\n",
    )
    .expect("write smoke script");
    let source_root = env!("CARGO_MANIFEST_DIR");
    let output = Command::new("python3")
        .arg(&smoke)
        .env(
            "PYTHONPATH",
            format!("{}:{source_root}/src", root.display()),
        )
        .output()
        .expect("run generated Python");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}\npython:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        generated
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "ok\n");
    fs::remove_dir_all(root).expect("remove temporary directory");
}
