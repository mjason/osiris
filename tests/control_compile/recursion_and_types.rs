use super::*;

#[test]
fn clojure_type_metadata_reaches_python_annotations() {
    let source = r#"
(module control_compile)
(import osiris.core :refer :all)
(defn ^{:type (Vector Int)} increment-all [^{:type (Vector Int)} values]
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
(import osiris.core :refer :all)
(defn ^Any even-step [^Int value]
  (if (= value 0)
    true
    (fn [] (odd-step (- value 1)))))
(defn ^Any odd-step [^Int value]
  (if (= value 0)
    false
    (fn [] (even-step (- value 1)))))
(defn ^Any parity [^Int value]
  (trampoline even-step value))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated trampoline Python");
    let root = temporary_directory();
    write_generated_module(&root, "control_compile.py", &generated);
    let smoke = root.join("smoke.py");
    fs::write(
        &smoke,
        "from control_compile import parity\nassert parity(10001) is False\nprint('ok')\n",
    )
    .expect("write smoke script");
    let output = Command::new("python3")
        .arg(&smoke)
        .env("PYTHONPATH", &root)
        .output()
        .expect("run generated trampoline Python");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}\npython:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        generated.source
    );
    fs::remove_dir_all(root).expect("remove temporary directory");
}

#[test]
fn derived_clojure_control_macros_compile_and_execute() {
    let source = r#"
(module control_compile)
(import osiris.core :refer :all)
(extern python "control_support"
  (defn ^None record [^Int value])
  (defn ^Bool ready [])
  (defn ^None tick []))

(defn ^Int increment [^Int value] (+ value 1))
(defn ^Int double [^Int value] (* value 2))

(defn ^Int value-or [^{:type (Option Int)} value ^Int fallback]
  (if-let [found value] found fallback))

(defn ^Str binding-truth [^Any value]
  (if-let [found value] "bound" "missing"))

(defn ^Str some-binding-truth [^Any value]
  (if-some [found value] "bound" "missing"))

(defn ^{:type (Option Int)} maybe-pipeline [^{:type (Option Int)} value]
  (some-> value increment double))

(defn ^{:type (Option Bool)} maybe-bool [^{:type (Option Bool)} value]
  (some-> value))

(defn ^Str selected-label [^Int value]
  (case value (1 2) "small" 3 "three" "other"))

(defn ^Int named-thread [^Int value]
  (as-> value current (+ current 1) (* current current)))

(defn ^{:type (Vector Int)} conditional-map [^{:type (Vector Int)} values ^Bool ready]
  (cond->> values ready (mapv increment)))

(defn ^{:type (Vector Int)} prefix-before-three [^{:type (Vector Int)} values]
  (forv [value values :while (< value 3)] value))

(defn ^{:type (Vector Int)} nested-prefix [^{:type (Vector Int)} lefts ^{:type (Vector Int)} rights]
  (forv [left lefts
        right rights
        :while (< right left)]
    (+ left right)))

(defn ^Str negative-label [^Bool ready]
  (if-not ready "wait" "run"))

(defn ^{:type (Option Str)} unless-ready [^Bool ready]
  (when-not ready "wait"))

(defn ^Int commented []
  (do (comment (unknown-call)) 7))

(defn ^{:type (Option Str)} first-ran [^{:type (Vector (Option Int))} values]
  (when-first [value values] "ran"))

(defn ^None none-first []
  (when-first [value none] none))

(defn ^Int recur-through-if-let [^{:type (Option Int)} value ^Int remaining ^Int total]
  (if (= remaining 0)
    total
    (if-let [present value]
      (recur value (- remaining 1) (+ total present))
      total)))

(defn ^None run-doseq [^{:type (Vector Int)} values]
  (doseq [value values :when (> value 0)]
    (record value)))

(defn ^None run-dotimes [^Int count]
  (dotimes [index count]
    (record index)))

(defn ^None run-while []
  (while (ready)
    (tick)))

(defn ^Int run-doto [^Int value]
  (doto value record record))

(defn ^None run-doseq-while [^{:type (Vector Int)} lefts ^{:type (Vector Int)} rights]
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
    let generated = result.python.expect("generated control Python");
    assert!(
        generated.source.contains("truthy as _u0_osiris_truthy"),
        "{}",
        generated.source
    );
    assert!(
        generated.source.contains("present as _u0_osiris_present"),
        "{}",
        generated.source
    );
    assert!(generated.source.contains("seq"), "{}", generated.source);
    assert!(
        generated.source.contains("doseq as _u0_osiris_doseq"),
        "{}",
        generated.source
    );
    assert!(
        generated.source.contains("return _u0_osiris_doseq"),
        "{}",
        generated.source
    );
    let root = temporary_directory();
    write_generated_module(&root, "control_compile.py", &generated);
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
    let output = Command::new("python3")
        .arg(&smoke)
        .env("PYTHONPATH", &root)
        .output()
        .expect("run generated control Python");
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
