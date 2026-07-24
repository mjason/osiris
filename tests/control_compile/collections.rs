use super::*;

#[test]
fn authored_map_and_set_literals_keep_boolean_and_integer_values_distinct() {
    let source = r#"
(module logical_literals)
(defn ^Any values-by-key []
  {false "false" 0 "zero" true "true" 1 "one"})
(defn ^Any distinct-values []
  #{false 0 true 1})
"#;
    let result = compile(
        source,
        &CompileOptions::new("logical_literals", PythonVersion::default()),
    );
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:#?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated logical collection Python");
    assert!(
        generated
            .source
            .contains("logical_map as _osr_logical_map_")
    );
    assert!(
        generated
            .source
            .contains("logical_set as _osr_logical_set_")
    );

    let root = temporary_directory();
    write_generated_module(&root, "logical_literals.py", &generated);
    let smoke = root.join("smoke.py");
    fs::write(
        &smoke,
        r#"from logical_literals import distinct_values, values_by_key

mapping = values_by_key()
assert list(mapping.items()) == [
    (False, "false"),
    (0, "zero"),
    (True, "true"),
    (1, "one"),
]
assert mapping[False] == "false"
assert mapping[0] == "zero"
assert mapping[True] == "true"
assert mapping[1] == "one"
assert list(distinct_values()) == [False, 0, True, 1]
print("ok")
"#,
    )
    .expect("write logical collection smoke script");
    let output = Command::new("python3")
        .arg(&smoke)
        .env("PYTHONPATH", &root)
        .output()
        .expect("run generated logical collections");
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
fn collection_control_compiles_and_executes_in_python() {
    let source = r#"
(module control_compile)
(import osiris.core :refer :all)
(defn ^{:type (Vector Int)} cartesian [^{:type (Vector Int)} lefts ^{:type (Vector Int)} rights]
  (forv [left lefts right rights] (+ left right)))
(defn ^{:type (Vector Int)} selected [^{:type (Vector Int)} values]
  (forv [value values :when (> value 1)] value))
(defn ^{:type (Vector Int)} pairwise [^{:type (Vector Int)} left ^{:type (Vector Int)} right]
  (mapv (fn [a b] (+ a b)) left right))
(defn ^{:type (Vector Int)} flattened [^{:type (Vector Int)} values]
  (mapcatv (fn [value] [value (- value 1)]) values))
(defn ^{:type (Vector Int)} selected-values [^{:type (Vector Int)} values]
  (filterv (fn [value] (> value 1)) values))
(defn ^Int total [^{:type (Vector Int)} values]
  (reduce (fn [acc value] (+ acc value)) 0 values))
(defn ^Int folded [^{:type (Vector Int)} values]
  (fold (fn [acc value] (+ acc value)) 10 values))
(defn ^Int total-before-four [^{:type (Vector Int)} values]
  (reduce
    (fn [acc value]
      (if (= value 4) (reduced acc) (+ acc value)))
    0
    values))
(defn ^Int folded-before-four [^{:type (Vector Int)} values]
  (fold
    (fn [acc value]
      (if (= value 4) (reduced acc) (+ acc value)))
    10
    values))
(defn ^Int reduced-roundtrip [^Int value]
  (unreduced (reduced value)))
(defn ^Bool reduced-marker? [^Int value]
  (reduced? (reduced value)))
(defn ^{:type (Union Int (Reduced Int))} stop-at-four [^Int acc ^Int value]
  (if (= value 4) (reduced acc) (+ acc value)))
(defn ^Int named-total-before-four [^{:type (Vector Int)} values]
  (reduce stop-at-four 0 values))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated Python");
    let root = temporary_directory();
    write_generated_module(&root, "control_compile.py", &generated);
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
fn mapcat_supports_multiple_collections_and_stops_at_the_shortest() {
    let source = r#"
(module control_compile)
(import osiris.core :refer :all)
(defn ^{:type (Vector Int)} pair-flatten [^{:type (Vector Int)} left ^{:type (Vector Int)} right]
  (mapcatv (fn [a b] [a b (+ a b)]) left right))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:#?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated Python");
    let root = temporary_directory();
    write_generated_module(&root, "control_compile.py", &generated);
    let smoke = root.join("smoke.py");
    fs::write(
        &smoke,
        "from control_compile import pair_flatten\nassert pair_flatten((1, 2), (10,)) == (1, 10, 11)\n",
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
    fs::remove_dir_all(root).expect("remove temporary directory");
}

#[test]
fn reduce_rejects_a_reduced_value_with_the_wrong_accumulator_type() {
    let source = r#"
(module bad_reduced)
(import osiris.core :refer :all)
(defn ^Int bad [^{:type (Vector Int)} values]
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
(import osiris.core :refer :all)
^{:doc "Reduce one integer value."}
(defn ^{:type (Union Int (Reduced Int))} step [^Int acc ^Int value]
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
        python.contains("from __osiris_runtime__ import Reduced as _u0_osiris_Reduced"),
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
(import osiris.core :refer :all)
(defn ^Any first-value [^Any values]
  (when-first [value values] value))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated when-first Python");
    let root = temporary_directory();
    write_generated_module(&root, "control_compile.py", &generated);
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
    let output = Command::new("python3")
        .arg(&smoke)
        .env("PYTHONPATH", &root)
        .output()
        .expect("run generated when-first Python");
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
