use std::{
    env, fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use _core::{compiler::CompileOptions, compiler::compile, project::PythonVersion};

fn options() -> CompileOptions {
    CompileOptions::new("parallel_compile", PythonVersion::default())
}

fn temporary_directory() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let path = env::temp_dir().join(format!("osiris-parallel-{nonce}"));
    fs::create_dir_all(&path).expect("create temporary directory");
    path
}

#[test]
fn parallel_forms_preserve_order_and_compile_through_future_abi() {
    let source = r#"
(module parallel_compile)
(defn increment [[value Int]] -> Int (+ value 1))
(defn add [[left Int] [right Int]] -> Int (+ left right))
(defn parallel-increment [[values (Vector Int)]] -> (Vector Int)
  (pmap increment values))
(defn parallel-anonymous [[values (Vector Int)]] -> (Vector Int)
  (pmap (fn [[value Int]] (+ value 1)) values))
(defn parallel-add
  [[left (Vector Int)] [right (Vector Int)]] -> (Vector Int)
  (pmap add left right))
(defn parallel-values [] -> (Vector Int)
  (pvalues (+ 1 2) (* 3 4) (- 20 5)))
(defn parallel-calls [] -> (Vector Int)
  (pcalls (fn [] 7) (fn [] 8) (fn [] 9)))
(defn parallel-failing [[values (Vector Int)]] -> Any
  (pmap (fn [value] (nth [] value)) values))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:#?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated Python").source;
    assert!(generated.contains("future_call"), "{generated}");
    assert!(generated.contains("deref"), "{generated}");

    let root = temporary_directory();
    fs::write(root.join("parallel_compile.py"), &generated).expect("write generated module");
    let smoke = root.join("smoke.py");
    fs::write(
        &smoke,
        r#"from parallel_compile import parallel_add, parallel_anonymous, parallel_calls, parallel_failing, parallel_increment, parallel_values

assert parallel_increment((1, 2, 3)) == (2, 3, 4)
assert parallel_anonymous((1, 2, 3)) == (2, 3, 4)
assert parallel_add((1, 2, 3), (10, 20, 30)) == (11, 22, 33)
assert parallel_values() == (3, 12, 15)
assert parallel_calls() == (7, 8, 9)
try:
    parallel_failing((0, 1, 2))
except IndexError:
    pass
else:
    raise AssertionError("pmap must propagate an ordered deref exception")
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
fn pmap_requires_a_collection() {
    let result = compile(
        "(module parallel_compile) (defn bad [] -> Any (pmap identity))",
        &options(),
    );
    assert!(result.analysis.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "OSR-M0007"
            && diagnostic
                .message
                .contains("pmap requires at least one collection")
    }));
    assert!(result.python.is_none());
}
