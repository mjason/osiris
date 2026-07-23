use std::{
    env, fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use osiris::{compiler::CompileOptions, compiler::compile, project::PythonVersion};

fn options() -> CompileOptions {
    CompileOptions::new("sequence_combinations_compile", PythonVersion::default())
}

fn temporary_directory() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let path = env::temp_dir().join(format!("osiris-sequence-combinations-{nonce}"));
    fs::create_dir_all(&path).expect("create temporary directory");
    path
}

#[test]
fn sequence_combinations_compile_and_run_through_typed_hir() {
    let source = r#"
(module sequence_combinations_compile)
(defn windowed [[values (Vector Int)]] -> (List (List Int))
  (partition 3 2 [9] values))
(defn all-windowed [[values (Vector Int)]] -> (List (List Int))
  (partition-all 3 2 values))
(defn grouped [[values (Vector Int)]] -> (List (List Int))
  (partition-by (fn [value] (>= value 0)) values))
(defn adjacent-unique [[values (Vector Int)]] -> (List Int)
  (dedupe values))
(defn woven [[values (Vector Int)]] -> (List Int)
  (interleave values [10 20]))
(defn separated [[values (Vector Int)]] -> (List Int)
  (interpose 0 values))
(defn tail [[values (Vector Int)]] -> (List Int)
  (take-last 2 values))
(defn initial [[values (Vector Int)]] -> (List Int)
  (drop-last 2 values))
(defn initial-one [[values (Vector Int)]] -> (List Int)
  (drop-last values))
(export [windowed all-windowed grouped adjacent-unique woven separated tail initial initial-one])
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:#?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated Python").source;
    for runtime_name in [
        "partition",
        "partition_all",
        "partition_by",
        "dedupe",
        "interleave",
        "interpose",
        "take_last",
        "drop_last",
    ] {
        assert!(
            generated.contains(runtime_name),
            "{runtime_name}:\n{generated}"
        );
    }

    let root = temporary_directory();
    fs::write(root.join("sequence_combinations_compile.py"), &generated)
        .expect("write generated module");
    let smoke = root.join("smoke.py");
    fs::write(
        &smoke,
        r#"from sequence_combinations_compile import adjacent_unique, all_windowed, grouped, initial, initial_one, separated, tail, windowed, woven

assert list(adjacent_unique((1, 1, 2, 1, 1))) == [1, 2, 1]
assert [list(group) for group in windowed((1, 2, 3, 4))] == [[1, 2, 3], [3, 4, 9]]
assert [list(group) for group in all_windowed((1, 2, 3, 4))] == [[1, 2, 3], [3, 4]]
assert [list(group) for group in grouped((-2, -1, 0, 1, -1))] == [[-2, -1], [0, 1], [-1]]
assert list(woven((1, 2, 3))) == [1, 10, 2, 20]
assert list(separated((1, 2, 3))) == [1, 0, 2, 0, 3]
assert list(tail((1, 2, 3, 4))) == [3, 4]
assert list(initial((1, 2, 3, 4))) == [1, 2]
assert list(initial_one((1, 2, 3, 4))) == [1, 2, 3]
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
fn sequence_combinations_report_arity_count_and_callback_errors() {
    let source = r#"
(module sequence_combinations_compile)
(defn bad-count [[values (Vector Int)]] -> Any
  (partition "three" values))
(defn bad-callback [[values (Vector Int)]] -> Any
  (partition-by (fn [] 1) values))
(defn bad-arity [[values (Vector Int)]] -> Any
  (interleave values))
"#;
    let result = compile(source, &options());
    assert!(
        result
            .analysis
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-T0001"),
        "{:#?}",
        result.analysis.diagnostics
    );
    assert!(
        result.analysis.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "OSR-T0041"
                && diagnostic
                    .message
                    .contains("sequence callback expects 1 argument")
        }),
        "{:#?}",
        result.analysis.diagnostics
    );
    assert!(
        result.analysis.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "OSR-T0041" && diagnostic.message.contains("interleave")
        }),
        "{:#?}",
        result.analysis.diagnostics
    );
    assert!(result.python.is_none());
}
