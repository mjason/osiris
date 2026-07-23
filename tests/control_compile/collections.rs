use super::*;

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
