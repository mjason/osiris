use std::{
    env, fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use osiris::{compiler::CompileOptions, compiler::compile, project::PythonVersion};

fn options() -> CompileOptions {
    CompileOptions::new("distinct_compile", PythonVersion::default())
}

fn temporary_directory() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let path = env::temp_dir().join(format!("osiris-distinct-{nonce}"));
    fs::create_dir_all(&path).expect("create temporary directory");
    path
}

#[test]
fn distinct_lowers_to_a_lazy_sequence_and_composes_with_take() {
    let source = r#"
(module distinct_compile)
(defn unique [[values (Vector Int)]] -> (Vector Int)
  (mapv (fn [value] value) (distinct values)))
(defn prefix [] -> (Vector Int)
  (mapv (fn [value] value)
    (take 3 (distinct (cycle [1 1 2 2 3])))))
(export [unique prefix])
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:#?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated Python").source;
    assert!(generated.contains("distinct"), "{generated}");

    let root = temporary_directory();
    fs::write(root.join("distinct_compile.py"), &generated).expect("write generated module");
    let smoke = root.join("smoke.py");
    fs::write(
        &smoke,
        r#"from distinct_compile import prefix, unique

assert unique((1, 1, 2, 1, 3, 2)) == (1, 2, 3)
assert prefix() == (1, 2, 3)
print("ok")
"#,
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

#[test]
fn distinct_rejects_an_invalid_arity_before_codegen() {
    let source = r#"
(module distinct_compile)
(defn invalid [[values (Vector Int)]] -> Any
  (distinct values values))
"#;
    let result = compile(source, &options());
    assert!(
        result
            .analysis
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-T0041"),
        "{:#?}",
        result.analysis.diagnostics
    );
    assert!(result.python.is_none());
}
