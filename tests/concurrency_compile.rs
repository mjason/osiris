use std::{
    env, fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use osiris::{compiler::CompileOptions, compiler::compile, project::PythonVersion};

fn options() -> CompileOptions {
    CompileOptions::new("concurrency_compile", PythonVersion::default())
}

fn temporary_directory() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let path = env::temp_dir().join(format!("osiris-concurrency-{nonce}"));
    fs::create_dir_all(&path).expect("create temporary directory");
    path
}

#[test]
fn future_promise_and_locking_compile_and_run() {
    let source = r#"
(module concurrency_compile)
(defn future-add [[value Int]] -> Int
  (let [task (future (+ value 1))]
    (deref task)))
(defn promise-value [[value Int]] -> Int
  (let [result (promise)]
    (do
      (deliver result value)
      (deref result))))
(defn promise-timeout [] -> Int
  (let [result (promise)]
    (deref result 0 42)))
(defn locked-add [[value Int]] -> Int
  (let [guard (lock)]
    (locking guard (+ value 1))))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated Python").source;
    assert!(generated.contains("future_call as"), "{generated}");
    assert!(generated.contains("deref as"), "{generated}");
    let root = temporary_directory();
    fs::write(root.join("concurrency_compile.py"), &generated).expect("write generated module");
    let smoke = root.join("smoke.py");
    fs::write(
        &smoke,
        r#"from concurrency_compile import future_add, locked_add, promise_timeout, promise_value

assert future_add(41) == 42
assert promise_value(7) == 7
assert promise_timeout() == 42
assert locked_add(8) == 9
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
