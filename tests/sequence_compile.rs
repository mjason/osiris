use std::{
    env, fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use _core::{compiler::CompileOptions, compiler::compile, project::PythonVersion};

fn options() -> CompileOptions {
    CompileOptions::new("sequence_compile", PythonVersion::default())
}

fn temporary_directory() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let path = env::temp_dir().join(format!("osiris-sequence-{nonce}"));
    fs::create_dir_all(&path).expect("create temporary directory");
    path
}

#[test]
fn lazy_sequence_helpers_compile_and_run_through_typed_hir() {
    let source = r#"
(module sequence_compile)
(defn take-sum [[values (Vector Int)]] -> Int
  (reduce (fn [acc value] (+ acc value)) 0 (take 2 values)))
(defn indexed [[values (Vector Int)]] -> (Vector Int)
  (mapv (fn [value] value)
    (map-indexed (fn [index value] (+ index value)) values)))
(defn iterated [[value Int]] -> (Vector Int)
  (mapv (fn [item] item)
    (take 3 (iterate (fn [current] (+ current 1)) value))))
(defn repeated [[value Int]] -> (Vector Int)
  (mapv (fn [item] item) (repeat 3 value)))
(defn accumulated [[values (Vector Int)]] -> (Vector Int)
  (mapv (fn [item] item)
    (reductions (fn [acc value] (+ acc value)) 0 values)))
(defn kept [[values (Vector Int)]] -> Any
  (keep (fn [value] (if (> value 1) value none)) values))
(defn concatenated [[values (Vector Int)]] -> Any
  (concat [0] values))
(defn lazy-concatenated [[values (Vector Int)]] -> Any
  (lazy-cat [0] values))
(defn some-value [[values (Vector Int)]] -> Any
  (some (fn [value] (if (> value 1) value none)) values))
(defn realized-count [[values (Vector Int)]] -> Int
  (count (doall (take 2 values))))
(export [take-sum indexed iterated repeated accumulated kept concatenated lazy-concatenated some-value realized-count])
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:#?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated Python").source;
    let root = temporary_directory();
    fs::write(root.join("sequence_compile.py"), &generated).expect("write generated module");
    let smoke = root.join("smoke.py");
    fs::write(
        &smoke,
        r#"from sequence_compile import accumulated, concatenated, indexed, iterated, kept, lazy_concatenated, realized_count, repeated, some_value, take_sum

assert take_sum((1, 2, 3)) == 3
assert indexed((10, 20, 30)) == (10, 21, 32)
assert iterated(4) == (4, 5, 6)
assert repeated(7) == (7, 7, 7)
assert accumulated((1, 2, 3)) == (0, 1, 3, 6)
assert list(kept((0, 1, 2, 3))) == [2, 3]
assert list(concatenated((1, 2))) == [0, 1, 2]
assert list(lazy_concatenated((1, 2))) == [0, 1, 2]
assert some_value((0, 1, 2)) == 2
assert realized_count((1, 2, 3)) == 2
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
