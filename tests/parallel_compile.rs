use std::{
    env, fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use osiris::{compiler::CompileOptions, compiler::compile, project::PythonVersion};

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
(import osiris.core :refer [nth])
(import osiris.concurrent :refer :all)
(defn ^Int increment [^Int value] (+ value 1))
(defn ^Int add [^Int left ^Int right] (+ left right))
(defn ^{:type (Vector Int)} parallel-increment [^{:type (Vector Int)} values]
  (pmap increment values))
(defn ^{:type (Vector Int)} parallel-anonymous [^{:type (Vector Int)} values]
  (pmap (fn [^Int value] (+ value 1)) values))
(defn ^{:type (Vector Int)} parallel-add [^{:type (Vector Int)} left ^{:type (Vector Int)} right]
  (pmap add left right))
(defn ^{:type (Vector Int)} parallel-values []
  (pvalues (+ 1 2) (* 3 4) (- 20 5)))
(defn ^{:type (Vector Int)} parallel-calls []
  (pcalls (fn [] 7) (fn [] 8) (fn [] 9)))
(defn ^Any parallel-failing [^{:type (Vector Int)} values]
  (pmap (fn [value] (nth [] value)) values))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:#?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated Python");
    assert!(
        generated.source.contains("future_call"),
        "{}",
        generated.source
    );
    assert!(generated.source.contains("deref"), "{}", generated.source);

    let root = temporary_directory();
    fs::write(root.join("parallel_compile.py"), &generated.source).expect("write generated module");
    let support = generated
        .runtime_support
        .expect("parallel forms should link private runtime support");
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

#[test]
fn pmap_requires_a_collection() {
    let result = compile(
        "(module parallel_compile) (import osiris.core :refer [identity]) (import osiris.concurrent :refer [pmap]) (defn ^Any bad [] (pmap identity))",
        &options(),
    );
    assert!(result.analysis.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "OSR-T0020"
            && diagnostic
                .message
                .contains("expects a function and at least one collection")
    }));
    assert!(result.python.is_none());
}
