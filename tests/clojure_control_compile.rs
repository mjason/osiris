use std::{
    env, fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use osiris::{compiler::CompileOptions, compiler::compile, project::PythonVersion};

fn options() -> CompileOptions {
    CompileOptions::new("clojure_control_compile", PythonVersion::default())
}

fn temporary_directory() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let path = env::temp_dir().join(format!("osiris-clojure-control-{nonce}"));
    fs::create_dir_all(&path).expect("create temporary directory");
    path
}

#[test]
fn sequence_and_concurrency_forms_compile_to_readable_python() {
    let source = r#"
(module clojure_control_compile)
(defn sequence-values [[values (Vector Int)]] -> Any
  (let [lazy-values (take 3 (iterate (fn [value] (+ value 1)) (nth values 0 0)))]
    [(first (cons 99 values))
     (rest values)
     (next values)
     (nth values 1 "missing")
     (count values)
     (seq values)
     (empty values)
     (drop 1 values)
     (take-while (fn [value] (< value 3)) values)
     (drop-while (fn [value] (< value 2)) values)
     (keep (fn [value] (if (> value 1) value none)) values)
     (keep-indexed (fn [index value] (if (= index 1) value none)) values)
     (remove (fn [value] (= value 1)) values)
     (removev (fn [value] (= value 1)) values)
     (map-indexed (fn [index value] (+ index value)) values)
     lazy-values
     (repeat 2 7)
     (repeatedly 2 (fn [] 8))
     (cycle [1 2])
     (sequence values)
     (reductions (fn [acc value] (+ acc value)) 0 values)
     (some (fn [value] (if (= value 2) value none)) values)
     (every? (fn [value] (> value 0)) values)
     (not-every? (fn [value] (> value 0)) values)
     (not-any? (fn [value] (= value 9)) values)]))
(defn side-effects [[values (Vector Int)]] -> None
  (run! (fn [value] value) values)
  (dorun (doall values)))
(defn bounded-realize [[values (Vector Int)]] -> (Vector Int)
  (doall 2 values))
(defn bounded-run [[values (Vector Int)]] -> None
  (dorun 2 values))
(defn async-value [] -> Any
  (let [task (future (+ 40 2))]
    [(future-done? task) (deref task 1000 "timeout")]))
(defn promise-value [] -> Any
  (let [value (promise)]
    [(deliver value 7) (deref value 0 "timeout") (realized? value)]))
(defn lock-value [] -> Int
  (locking (lock) (+ 20 22)))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:#?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated Python").source;
    assert!(
        generated.contains("from osiris.prelude import"),
        "{generated}"
    );
    let root = temporary_directory();
    fs::write(root.join("clojure_control_compile.py"), &generated).expect("write generated module");
    let smoke = root.join("smoke.py");
    fs::write(
        &smoke,
        r#"import itertools
from clojure_control_compile import async_value, bounded_realize, bounded_run, lock_value, promise_value, sequence_values, side_effects

values = (1, 2, 3)
result = sequence_values(values)
assert result[0] == 99
assert result[1] == [2, 3]
assert list(result[2]) == [2, 3]
assert result[3] == 2
assert result[4] == 3
assert result[5] is not None
assert result[6] == ()
assert list(result[7]) == [2, 3]
assert list(result[8]) == [1, 2]
assert list(result[9]) == [2, 3]
assert list(result[10]) == [2, 3]
assert list(result[11]) == [2]
assert list(result[12]) == [2, 3]
assert result[13] == (2, 3)
assert list(result[14]) == [1, 3, 5]
assert list(result[15]) == [1, 2, 3]
assert list(result[16]) == [7, 7]
assert list(result[17]) == [8, 8]
assert list(itertools.islice(result[18], 5)) == [1, 2, 1, 2, 1]
assert list(result[19]) == [1, 2, 3]
assert list(result[20]) == [0, 1, 3, 6]
assert result[21] == 2
assert result[22] is True
assert result[23] is False
assert result[24] is True
side_effects(values)
assert bounded_realize(values) == values
assert bounded_run(values) is None
assert async_value()[1] == 42
promise_result = promise_value()
assert promise_result[1] == 7
assert promise_result[2] is True
assert lock_value() == 42
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
