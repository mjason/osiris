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
(import osiris.concurrent :refer :all)
(defn ^Int future-add [^Int value]
  (let [task (future (+ value 1))]
    (deref task)))
(defn ^Int promise-value [^Int value]
  (let [result (promise)]
    (do
      (deliver result value)
      (deref result))))
(defn ^Int promise-timeout []
  (let [result (promise)]
    (deref result 0 42)))
(defn ^Int locked-add [^Int value]
  (let [guard (lock)]
    (locking guard (+ value 1))))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated Python");
    assert!(
        generated.source.contains("future_call"),
        "{}",
        generated.source
    );
    assert!(generated.source.contains("deref"), "{}", generated.source);
    assert!(!generated.source.contains("osiris.prelude"));
    let root = temporary_directory();
    fs::write(root.join("concurrency_compile.py"), &generated.source)
        .expect("write generated module");
    let support = generated
        .runtime_support
        .expect("runtime support should be linked");
    for (path, source) in osiris::backend::runtime_distribution_files(
        &support,
        osiris::project::PythonVersion::default(),
    )
    .expect("link runtime distribution")
    {
        let destination = root.join(path);
        fs::create_dir_all(destination.parent().expect("support parent"))
            .expect("create support directory");
        fs::write(destination, source).expect("write support file");
    }
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
    let output = Command::new("python3")
        .arg(&smoke)
        .env("PYTHONPATH", &root)
        .output()
        .expect("run generated Python");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}\npython:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        generated.source
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "ok\n");
    fs::remove_dir_all(root).expect("remove temporary directory");
}
