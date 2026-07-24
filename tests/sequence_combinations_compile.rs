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
(import osiris.core :refer [dedupe drop-last interleave interpose nth partition partition-all partition-by take-last])
^{:doc "Partition values into windows."}
(defn ^{:type (List (List Int))} windowed [^{:type (Vector Int)} values]
  (partition 3 2 [9] values))
^{:doc "Partition values with the default step."}
(defn ^{:type (List (List Int))} default-windowed [^{:type (Vector Int)} values]
  (partition 2 values))
^{:doc "Partition all values into windows."}
(defn ^{:type (List (List Int))} all-windowed [^{:type (Vector Int)} values]
  (partition-all 3 2 values))
^{:doc "Partition all values with the default step."}
(defn ^{:type (List (List Int))} default-all-windowed [^{:type (Vector Int)} values]
  (partition-all 3 values))
^{:doc "Partition values by a key."}
(defn ^{:type (List (List Int))} grouped [^{:type (Vector Int)} values]
  (partition-by (fn [value] (>= value 0)) values))
^{:doc "Remove adjacent duplicate values."}
(defn ^{:type (List Int)} adjacent-unique [^{:type (Vector Int)} values]
  (dedupe values))
^{:doc "Interleave values."}
(defn ^{:type (List Int)} woven [^{:type (Vector Int)} values]
  (interleave values [10 20]))
^{:doc "Interpose a separator."}
(defn ^{:type (List Int)} separated [^{:type (Vector Int)} values]
  (interpose 0 values))
^{:doc "Take the final values."}
(defn ^{:type (List Int)} tail [^{:type (Vector Int)} values]
  (take-last 2 values))
^{:doc "Drop final values."}
(defn ^{:type (List Int)} initial [^{:type (Vector Int)} values]
  (drop-last 2 values))
^{:doc "Drop one final value."}
(defn ^{:type (List Int)} initial-one [^{:type (Vector Int)} values]
  (drop-last values))
^{:doc "Return an indexed value or an explicit fallback."}
(defn ^Any indexed-or [^{:type (Vector Int)} values ^Int index ^Any fallback]
  (nth values index fallback))
^{:doc "Return an indexed value or raise when it is absent."}
(defn ^Any indexed [^{:type (Vector Int)} values ^Int index]
  (nth values index))
(export [windowed default-windowed all-windowed default-all-windowed grouped adjacent-unique woven separated tail initial initial-one indexed-or indexed])
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:#?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated Python");
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
            generated.source.contains(runtime_name),
            "{runtime_name}:\n{}",
            generated.source
        );
    }

    let root = temporary_directory();
    fs::write(
        root.join("sequence_combinations_compile.py"),
        &generated.source,
    )
    .expect("write generated module");
    let support = generated
        .runtime_support
        .expect("sequence functions should link private runtime support");
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
        r#"from sequence_combinations_compile import adjacent_unique, all_windowed, default_all_windowed, default_windowed, grouped, indexed, indexed_or, initial, initial_one, separated, tail, windowed, woven

assert list(adjacent_unique((1, 1, 2, 1, 1))) == [1, 2, 1]
assert [list(group) for group in windowed((1, 2, 3, 4))] == [[1, 2, 3], [3, 4, 9]]
assert [list(group) for group in default_windowed((1, 2, 3, 4))] == [[1, 2], [3, 4]]
assert [list(group) for group in all_windowed((1, 2, 3, 4))] == [[1, 2, 3], [3, 4]]
assert [list(group) for group in default_all_windowed((1, 2, 3, 4))] == [[1, 2, 3], [4]]
assert [list(group) for group in grouped((-2, -1, 0, 1, -1))] == [[-2, -1], [0, 1], [-1]]
assert list(woven((1, 2, 3))) == [1, 10, 2, 20]
assert list(separated((1, 2, 3))) == [1, 0, 2, 0, 3]
assert list(tail((1, 2, 3, 4))) == [3, 4]
assert list(initial((1, 2, 3, 4))) == [1, 2]
assert list(initial_one((1, 2, 3, 4))) == [1, 2, 3]
assert indexed_or((), 0, None) is None
try:
    indexed((), 0)
except IndexError:
    pass
else:
    raise AssertionError("two-argument nth must not synthesize a none fallback")
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
fn sequence_combinations_report_arity_count_and_callback_errors() {
    let source = r#"
(module sequence_combinations_compile)
(import osiris.core :refer [interleave partition partition-by])
(defn ^Any bad-count [^{:type (Vector Int)} values]
  (partition "three" values))
(defn ^Any bad-callback [^{:type (Vector Int)} values]
  (partition-by (fn [] 1) values))
(defn ^Any bad-arity [^{:type (Vector Int)} values]
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
