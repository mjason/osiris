use std::{
    env, fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use _core::{compiler::CompileOptions, compiler::compile, project::PythonVersion};

fn options() -> CompileOptions {
    CompileOptions::new("control_compile", PythonVersion::default())
}

fn temporary_directory() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let path = env::temp_dir().join(format!("osiris-control-{nonce}"));
    fs::create_dir_all(&path).expect("create temporary directory");
    path
}

#[test]
fn collection_control_compiles_and_executes_in_python() {
    let source = r#"
(module control_compile)
(defn cartesian [[lefts (Vector Int)] [rights (Vector Int)]] -> (Vector Int)
  (for [left lefts right rights] (+ left right)))
(defn selected [[values (Vector Int)]] -> (Vector Int)
  (for [value values :when (> value 1)] value))
(defn pairwise [[left (Vector Int)] [right (Vector Int)]] -> (Vector Int)
  (mapv (fn [a b] (+ a b)) left right))
(defn flattened [[values (Vector Int)]] -> (Vector Int)
  (mapcatv (fn [value] [value (- value 1)]) values))
(defn selected-values [[values (Vector Int)]] -> (Vector Int)
  (filterv (fn [value] (> value 1)) values))
(defn total [[values (Vector Int)]] -> Int
  (reduce (fn [acc value] (+ acc value)) 0 values))
(defn folded [[values (Vector Int)]] -> Int
  (fold (fn [acc value] (+ acc value)) 10 values))
(defn total-before-four [[values (Vector Int)]] -> Int
  (reduce
    (fn [acc value]
      (if (= value 4) (reduced acc) (+ acc value)))
    0
    values))
(defn folded-before-four [[values (Vector Int)]] -> Int
  (fold
    (fn [acc value]
      (if (= value 4) (reduced acc) (+ acc value)))
    10
    values))
(defn reduced-roundtrip [[value Int]] -> Int
  (unreduced (reduced value)))
(defn reduced-marker? [[value Int]] -> Bool
  (reduced? (reduced value)))
(defn stop-at-four [[acc Int] [value Int]] -> (Union Int (Reduced Int))
  (if (= value 4) (reduced acc) (+ acc value)))
(defn named-total-before-four [[values (Vector Int)]] -> Int
  (reduce stop-at-four 0 values))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated Python").source;
    let root = temporary_directory();
    fs::write(root.join("control_compile.py"), &generated).expect("write generated module");
    let smoke = root.join("smoke.py");
    fs::write(
        &smoke,
        r#"from control_compile import cartesian, folded, folded_before_four, flattened, named_total_before_four, pairwise, reduced_marker_p, reduced_roundtrip, selected, selected_values, total, total_before_four

assert cartesian((1, 2), (10, 20)) == (11, 21, 12, 22)
assert selected((0, 1, 2, 3)) == (2, 3)
assert pairwise((1, 2), (10, 20)) == (11, 22)
assert flattened((3, 4)) == (3, 2, 4, 3)
assert selected_values((0, 1, 2, 3)) == (2, 3)
assert total((1, 2, 3)) == 6
assert folded((1, 2, 3)) == 16
assert total_before_four((1, 2, 3, 4, 100)) == 6
assert folded_before_four((1, 2, 3, 4, 100)) == 16
assert named_total_before_four((1, 2, 3, 4, 100)) == 6
assert reduced_roundtrip(7) == 7
assert reduced_marker_p(7) is True
print("ok")
"#,
    )
    .expect("write smoke script");
    let source_root = env!("CARGO_MANIFEST_DIR").to_owned();
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
fn mapcat_supports_multiple_collections_and_stops_at_the_shortest() {
    let source = r#"
(module control_compile)
(defn pair-flatten [[left (Vector Int)] [right (Vector Int)]] -> (Vector Int)
  (mapcatv (fn [a b] [a b (+ a b)]) left right))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:#?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated Python").source;
    let root = temporary_directory();
    fs::write(root.join("control_compile.py"), &generated).expect("write generated module");
    let smoke = root.join("smoke.py");
    fs::write(
        &smoke,
        "from control_compile import pair_flatten\nassert pair_flatten((1, 2), (10,)) == (1, 10, 11)\n",
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
    fs::remove_dir_all(root).expect("remove temporary directory");
}

#[test]
fn reduce_rejects_a_reduced_value_with_the_wrong_accumulator_type() {
    let source = r#"
(module bad_reduced)
(defn bad [[values (Vector Int)]] -> Int
  (reduce
    (fn [acc value]
      (if (= value 0) (reduced "wrong") acc))
    0
    values))
"#;
    let result = compile(source, &options());
    assert!(
        result
            .analysis
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-T0001"),
        "{:?}",
        result.analysis.diagnostics
    );
    assert!(result.python.is_none());
}

#[test]
fn public_reducer_can_expose_the_reduced_protocol_type() {
    let source = r#"
(module reduced_interface)
(defn step [[acc Int] [value Int]] -> (Union Int (Reduced Int))
  (if (= value 0) (reduced acc) (+ acc value)))
(export [step])
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    assert!(result.interface.is_some());
    let python = result.python.expect("generated Python").source;
    assert!(
        python.contains("from osiris.prelude import Reduced as _u0_osiris_Reduced"),
        "{python}"
    );
    assert!(
        python.contains("Union[int, _u0_osiris_Reduced[int]]"),
        "{python}"
    );
}

#[test]
fn when_first_accepts_general_iterables_without_losing_the_first_value() {
    let source = r#"
(module control_compile)
(defn first-value [[values Any]] -> Any
  (when-first [value values] value))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated when-first Python").source;
    let root = temporary_directory();
    fs::write(root.join("control_compile.py"), &generated).expect("write generated module");
    let smoke = root.join("smoke.py");
    fs::write(
        &smoke,
        r#"from control_compile import first_value

assert first_value(iter(())) is None
assert first_value(iter((False, 2))) is False
assert first_value(iter((None, 2))) is None
assert first_value((7, 8)) == 7
print("ok")
"#,
    )
    .expect("write when-first smoke script");
    let source_root = env!("CARGO_MANIFEST_DIR");
    let output = Command::new("python3")
        .arg(&smoke)
        .env(
            "PYTHONPATH",
            format!("{}:{source_root}/src", root.display()),
        )
        .output()
        .expect("run generated when-first Python");
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
fn clojure_type_metadata_reaches_python_annotations() {
    let source = r#"
(module control_compile)
(defn ^{:type (Vector Int)} increment-all
  [^{:type (Vector Int)} values]
  (mapv (fn [^Int value] (+ value 1)) values))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let python = result.python.expect("metadata-typed Python").source;
    assert!(
        python.contains("def increment_all(values: tuple[int, ...]) -> tuple[int, ...]:"),
        "{python}"
    );
}

#[test]
fn trampoline_handles_mutual_bounce_functions() {
    let source = r#"
(module control_compile)
(defn even-step [[value Int]] -> Any
  (if (= value 0)
    true
    (fn [] (odd-step (- value 1)))))
(defn odd-step [[value Int]] -> Any
  (if (= value 0)
    false
    (fn [] (even-step (- value 1)))))
(defn parity [[value Int]] -> Any
  (trampoline even-step value))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated trampoline Python").source;
    let root = temporary_directory();
    fs::write(root.join("control_compile.py"), &generated).expect("write generated module");
    let smoke = root.join("smoke.py");
    fs::write(
        &smoke,
        "from control_compile import parity\nassert parity(10001) is False\nprint('ok')\n",
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
        .expect("run generated trampoline Python");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}\npython:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        generated
    );
    fs::remove_dir_all(root).expect("remove temporary directory");
}

#[test]
fn derived_clojure_control_macros_compile_and_execute() {
    let source = r#"
(module control_compile)
(extern python "control_support"
  (defn record [[value Int]] -> None)
  (defn ready [] -> Bool)
  (defn tick [] -> None))

(defn increment [[value Int]] -> Int (+ value 1))
(defn double [[value Int]] -> Int (* value 2))

(defn value-or [[value (Option Int)] [fallback Int]] -> Int
  (if-let [found value] found fallback))

(defn binding-truth [[value Any]] -> Str
  (if-let [found value] "bound" "missing"))

(defn some-binding-truth [[value Any]] -> Str
  (if-some [found value] "bound" "missing"))

(defn maybe-pipeline [[value (Option Int)]] -> (Option Int)
  (some-> value increment double))

(defn maybe-bool [[value (Option Bool)]] -> (Option Bool)
  (some-> value))

(defn selected-label [[value Int]] -> Str
  (case value (1 2) "small" 3 "three" "other"))

(defn named-thread [[value Int]] -> Int
  (as-> value current (+ current 1) (* current current)))

(defn conditional-map
  [[values (Vector Int)] [ready Bool]] -> (Vector Int)
  (cond->> values ready (mapv increment)))

(defn prefix-before-three [[values (Vector Int)]] -> (Vector Int)
  (for [value values :while (< value 3)] value))

(defn nested-prefix
  [[lefts (Vector Int)] [rights (Vector Int)]] -> (Vector Int)
  (for [left lefts
        right rights
        :while (< right left)]
    (+ left right)))

(defn negative-label [[ready Bool]] -> Str
  (if-not ready "wait" "run"))

(defn unless-ready [[ready Bool]] -> (Option Str)
  (when-not ready "wait"))

(defn commented [] -> Int
  (do (comment (unknown-call)) 7))

(defn first-ran [[values (Vector (Option Int))]] -> (Option Str)
  (when-first [value values] "ran"))

(defn none-first [] -> None
  (when-first [value none] none))

(defn recur-through-if-let
  [[value (Option Int)] [remaining Int] [total Int]] -> Int
  (if (= remaining 0)
    total
    (if-let [present value]
      (recur value (- remaining 1) (+ total present))
      total)))

(defn run-doseq [[values (Vector Int)]] -> None
  (doseq [value values :when (> value 0)]
    (record value)))

(defn run-dotimes [[count Int]] -> None
  (dotimes [index count]
    (record index)))

(defn run-while [] -> None
  (while (ready)
    (tick)))

(defn run-doto [[value Int]] -> Int
  (doto value record record))

(defn run-doseq-while
  [[lefts (Vector Int)] [rights (Vector Int)]] -> None
  (doseq [left lefts
          right rights
          :while (< right left)]
    (record (+ (* left 10) right))))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated control Python").source;
    assert!(
        generated.contains("truthy as _u0_osiris_truthy"),
        "{generated}"
    );
    assert!(
        generated.contains("present as _u0_osiris_present"),
        "{generated}"
    );
    assert!(generated.contains("seq as _u0_osiris_seq"), "{generated}");
    assert!(
        generated.contains("doseq as _u0_osiris_doseq"),
        "{generated}"
    );
    assert!(generated.contains("return _u0_osiris_doseq"), "{generated}");
    let root = temporary_directory();
    fs::write(root.join("control_compile.py"), &generated).expect("write generated module");
    fs::write(
        root.join("control_support.py"),
        r#"events = []
ticks = 0

def record(value):
    events.append(value)

def ready():
    return ticks < 3

def tick():
    global ticks
    events.append(100 + ticks)
    ticks += 1
"#,
    )
    .expect("write control support module");
    let smoke = root.join("smoke.py");
    fs::write(
        &smoke,
        r#"import control_support as support
from control_compile import (
    binding_truth,
    commented,
    conditional_map,
    first_ran,
    maybe_bool,
    maybe_pipeline,
    named_thread,
    negative_label,
    nested_prefix,
    none_first,
    recur_through_if_let,
    prefix_before_three,
    run_doseq,
    run_dotimes,
    run_doto,
    run_doseq_while,
    run_while,
    selected_label,
    some_binding_truth,
    unless_ready,
    value_or,
)

assert value_or(None, 7) == 7
assert value_or(4, 7) == 4
assert binding_truth(False) == "missing"
assert binding_truth(()) == "bound"
assert some_binding_truth(False) == "bound"
assert some_binding_truth(None) == "missing"
assert maybe_pipeline(None) is None
assert maybe_pipeline(3) == 8
assert maybe_bool(False) is False
assert selected_label(1) == "small"
assert selected_label(2) == "small"
assert selected_label(3) == "three"
assert selected_label(9) == "other"
assert named_thread(3) == 16
assert conditional_map((1, 2), False) == (1, 2)
assert conditional_map((1, 2), True) == (2, 3)
assert prefix_before_three((1, 2, 3, 1)) == (1, 2)
assert nested_prefix((2, 4), (1, 3, 2, 5)) == (3, 5, 7, 6)
assert negative_label(False) == "wait"
assert negative_label(True) == "run"
assert unless_ready(False) == "wait"
assert unless_ready(True) is None
assert commented() == 7
assert first_ran(()) is None
assert first_ran((None,)) == "ran"
assert none_first() is None
assert recur_through_if_let(3, 5, 0) == 15

run_doseq((-1, 2, 0, 3))
run_dotimes(3)
run_while()
assert run_doto(9) == 9
run_doseq_while((2, 4), (1, 3, 2, 5))
assert support.events == [
    2, 3,
    0, 1, 2,
    100, 101, 102,
    9, 9,
    21, 41, 43, 42,
]
print("ok")
"#,
    )
    .expect("write control smoke script");
    let source_root = env!("CARGO_MANIFEST_DIR");
    let output = Command::new("python3")
        .arg(&smoke)
        .env(
            "PYTHONPATH",
            format!("{}:{source_root}/src", root.display()),
        )
        .output()
        .expect("run generated control Python");
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
fn assert_uses_a_runtime_exception_and_keeps_message_lazy() {
    let source = r#"
(module control_compile)
(defn check [[value Any]] -> None
  (assert value "assertion failed"))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated assert Python").source;
    assert!(
        generated.contains("assert_value as _u0_osiris_assert_value"),
        "{generated}"
    );
    let root = temporary_directory();
    fs::write(root.join("control_compile.py"), &generated).expect("write generated module");
    let smoke = root.join("smoke.py");
    fs::write(
        &smoke,
        r#"from control_compile import check

assert check(True) is None
try:
    check(False)
except AssertionError as error:
    assert str(error) == "assertion failed"
else:
    raise AssertionError("assert did not raise")
print("ok")
"#,
    )
    .expect("write assert smoke script");
    let source_root = env!("CARGO_MANIFEST_DIR");
    let output = Command::new("python3")
        .arg(&smoke)
        .env(
            "PYTHONPATH",
            format!("{}:{source_root}/src", root.display()),
        )
        .output()
        .expect("run generated assert Python");
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
fn clojure_truthiness_applies_to_control_macros_over_any_values() {
    let source = r#"
(module control_compile)
(defn when-any [[value Any]] -> (Option Str)
  (when value "yes"))
(defn and-any [[value Any]] -> Any
  (and value 42))
(defn or-any [[value Any]] -> Any
  (or value 42))
(defn cond-any [[value Any]] -> Str
  (cond value "yes" :else "no"))
(defn for-any [[values (Vector Any)]] -> (Vector Any)
  (for [value values :when value] value))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated truthiness Python").source;
    let root = temporary_directory();
    fs::write(root.join("control_compile.py"), &generated).expect("write generated module");
    let smoke = root.join("smoke.py");
    fs::write(
        &smoke,
        r#"from control_compile import and_any, cond_any, for_any, or_any, when_any

assert when_any(0) == "yes"
assert when_any(False) is None
assert and_any(0) == 42
assert or_any(0) == 0
assert cond_any("") == "yes"
assert for_any((0, False, None, "")) == (0, "")
print("ok")
"#,
    )
    .expect("write truthiness smoke script");
    let source_root = env!("CARGO_MANIFEST_DIR");
    let output = Command::new("python3")
        .arg(&smoke)
        .env(
            "PYTHONPATH",
            format!("{}:{source_root}/src", root.display()),
        )
        .output()
        .expect("run generated truthiness Python");
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
