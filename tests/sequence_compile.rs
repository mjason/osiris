use std::{
    env, fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use osiris::{compiler::CompileOptions, compiler::compile, project::PythonVersion};

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
(import osiris.core :refer :all)
^{:doc "Sum a finite prefix."}
(defn ^Int take-sum [^{:type (Vector Int)} values]
  (reduce (fn [acc value] (+ acc value)) 0 (take 2 values)))
^{:doc "Sum without an explicit initial value."}
(defn ^Int sum [^{:type (Vector Int)} values]
  (reduce (fn [acc value] (+ acc value)) values))
^{:doc "Map values with their indexes."}
(defn ^{:type (Vector Int)} indexed [^{:type (Vector Int)} values]
  (mapv (fn [value] value)
    (map-indexed (fn [index value] (+ index value)) values)))
^{:doc "Iterate a successor function."}
(defn ^{:type (Vector Int)} iterated [^Int value]
  (mapv (fn [item] item)
    (take 3 (iterate (fn [current] (+ current 1)) value))))
^{:doc "Repeat a value."}
(defn ^{:type (Vector Int)} repeated [^Int value]
  (mapv (fn [item] item) (repeat 3 value)))
^{:doc "Take values from an unbounded repetition."}
(defn ^{:type (Vector Int)} repeated-forever [^Int value]
  (mapv (fn [item] item) (take 3 (repeat value))))
^{:doc "Invoke a producer without a finite repetition count."}
(defn ^{:type (Vector Int)} produced [^Int value]
  (mapv (fn [item] item) (take 2 (repeatedly (fn [] value)))))
^{:doc "Build a numeric sequence from its one-argument form."}
(defn ^{:type (Vector Int)} ranged []
  (mapv (fn [item] item) (range 4)))
^{:doc "Return intermediate reductions."}
(defn ^{:type (Vector Int)} accumulated [^{:type (Vector Int)} values]
  (mapv (fn [item] item)
    (reductions (fn [acc value] (+ acc value)) 0 values)))
^{:doc "Keep selected values."}
(defn ^Any kept [^{:type (Vector Int)} values]
  (keep (fn [value] (if (> value 1) value none)) values))
^{:doc "Concatenate values."}
(defn ^Any concatenated [^{:type (Vector Int)} values]
  (concat [0] values))
^{:doc "Lazily concatenate values."}
(defn ^Any lazy-concatenated [^{:type (Vector Int)} values]
  (lazy-cat [0] values))
^{:doc "Return the first selected value."}
(defn ^Any some-value [^{:type (Vector Int)} values]
  (some (fn [value] (if (> value 1) value none)) values))
^{:doc "Count realized values."}
(defn ^Int realized-count [^{:type (Vector Int)} values]
  (count (doall (take 2 values))))
(export [take-sum sum indexed iterated repeated repeated-forever produced ranged accumulated kept concatenated lazy-concatenated some-value realized-count])
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:#?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated Python");
    let root = temporary_directory();
    fs::write(root.join("sequence_compile.py"), &generated.source).expect("write generated module");
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
        r#"from sequence_compile import accumulated, concatenated, indexed, iterated, kept, lazy_concatenated, produced, ranged, realized_count, repeated, repeated_forever, some_value, sum, take_sum

assert take_sum((1, 2, 3)) == 3
assert sum((1, 2, 3)) == 6
assert indexed((10, 20, 30)) == (10, 21, 32)
assert iterated(4) == (4, 5, 6)
assert repeated(7) == (7, 7, 7)
assert repeated_forever(7) == (7, 7, 7)
assert produced(8) == (8, 8)
assert ranged() == (0, 1, 2, 3)
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
