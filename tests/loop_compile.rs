use std::{
    env, fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use _core::{
    compiler::{CompileOptions, compile},
    project::PythonVersion,
};

fn options() -> CompileOptions {
    CompileOptions::new("loop_compile", PythonVersion::default())
}

fn temporary_directory() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let path = env::temp_dir().join(format!("osiris-loop-{nonce}"));
    fs::create_dir_all(&path).expect("create temporary directory");
    path
}

#[test]
fn loop_recur_lowers_to_constant_stack_runtime() {
    let source = r#"
(module loop_compile)
(defn sum-down [[n Int] [total Int]] -> Int
  (loop [value n acc total]
    (if (= value 0)
      acc
      (recur (- value 1) (+ acc value)))))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let python = result.python.expect("loop should generate Python").source;
    assert!(
        python.contains("from osiris.prelude import loop as"),
        "{python}"
    );
    assert!(python.contains("recur as"), "{python}");
    assert!(python.contains("return _u0_osiris_loop"), "{python}");
    assert!(
        !python.contains("return sum_down("),
        "loop must not recurse: {python}"
    );
}

#[test]
fn function_recur_lowers_to_constant_stack_runtime() {
    let source = r#"
(module loop_compile)
(defn sum-down [[n Int] [total Int]] -> Int
  (if (= n 0)
    total
    (recur (- n 1) (+ total n))))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let python = result
        .python
        .expect("function recur should generate Python")
        .source;
    assert!(
        python.contains("from osiris.prelude import loop as"),
        "{python}"
    );
    assert!(python.contains("return _u0_osiris_loop"), "{python}");
    assert!(
        !python.contains("return sum_down("),
        "function recur must not recurse: {python}"
    );
}

#[test]
fn function_recur_executes_large_input_without_python_recursion() {
    let source = r#"
(module loop_compile)
(defn sum-down [[n Int] [total Int]] -> Int
  (if (= n 0)
    total
    (recur (- n 1) (+ total n))))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated Python").source;
    let root = temporary_directory();
    fs::write(root.join("loop_compile.py"), &generated).expect("write generated module");
    let smoke = root.join("smoke.py");
    fs::write(
        &smoke,
        "from loop_compile import sum_down\nassert sum_down(10000, 0) == 50005000\nprint('ok')\n",
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
        .expect("run generated function recur Python");
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
fn function_recur_checks_arity_and_type() {
    let arity = r#"
(module loop_compile)
(defn invalid [[value Int]] -> Int
  (if (= value 0) value (recur)))
"#;
    let result = compile(arity, &options());
    assert!(
        result
            .analysis
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-T0023"),
        "{:?}",
        result.analysis.diagnostics
    );

    let type_error = r#"
(module loop_compile)
(defn invalid [[value Int]] -> Int
  (if (= value 0) value (recur "wrong")))
"#;
    let result = compile(type_error, &options());
    assert!(
        result
            .analysis
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-T0001"),
        "{:?}",
        result.analysis.diagnostics
    );
}

#[test]
fn function_recur_must_be_in_tail_position() {
    let source = r#"
(module loop_compile)
(defn invalid [[value Int]] -> Int
  (+ 1 (recur (- value 1))))
"#;
    let result = compile(source, &options());
    assert!(
        result
            .analysis
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-T0023"),
        "{:?}",
        result.analysis.diagnostics
    );
}

#[test]
fn anonymous_fn_supports_function_recur() {
    let source = r#"
(module loop_compile)
(defn make-sum [[start Int]] -> Any
  (fn [^Int value]
    (if (= value 0)
      0
      (recur (- value 1)))))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let python = result.python.expect("anonymous fn should compile").source;
    assert!(python.contains("_u0_osiris_loop"), "{python}");
}

#[test]
fn loop_supports_destructured_state() {
    let source = r#"
(module loop_compile)
(defn pair-sum [[pair (Vector Int)]] -> Int
  (loop [[left right] pair]
    (if (= left 0)
      right
      (recur [(- left 1) (+ right left)]))))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let python = result.python.expect("loop should generate Python").source;
    assert!(python.contains("left ="), "{python}");
    assert!(python.contains("right ="), "{python}");
}

#[test]
fn loop_allows_an_empty_state_vector() {
    let source = "(module loop_compile) (def value (loop [] 42))";
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let python = result
        .python
        .expect("empty-state loop should generate Python")
        .source;
    assert!(python.contains("_u0_osiris_loop"), "{python}");
}

#[test]
fn nested_loops_bind_recur_to_the_nearest_callback() {
    let source = r#"
(module loop_compile)
(defn nested [[n Int]] -> Int
  (loop [outer n total 0]
    (if (= outer 0)
      total
      (let [inner
            (loop [value outer]
              (if (= value 0)
                0
                (recur (- value 1))))]
        (recur (- outer 1) (+ total inner))))))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
}

#[test]
fn recur_outside_loop_is_rejected() {
    let source = "(module loop_compile) (def value (recur 1))";
    let result = compile(source, &options());
    assert!(
        result
            .analysis
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-T0023")
    );
}

#[test]
fn recur_state_values_follow_loop_types() {
    let source = r#"
(module loop_compile)
(defn broken [[n Int]] -> Int
  (loop [value n]
    (if (= value 0)
      value
      (recur "not-an-int"))))
"#;
    let result = compile(source, &options());
    assert!(
        result
            .analysis
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-T0001")
    );
}

#[test]
fn recur_in_nested_lambda_cannot_capture_outer_loop() {
    let source = r#"
(module loop_compile)
(defn invalid [[n Int]] -> Int
  (loop [value n]
    (let [step (fn [] (recur value))]
      (if (= value 0) 0 (step)))))
"#;
    let result = compile(source, &options());
    assert!(
        result
            .analysis
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-T0023")
    );
}

#[test]
fn recur_must_be_in_tail_position() {
    let source = r#"
(module loop_compile)
(defn invalid [[n Int]] -> Int
  (loop [value n]
    (+ 1 (recur (- value 1)))))
"#;
    let result = compile(source, &options());
    assert!(
        result
            .analysis
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-T0023")
    );
}

#[test]
fn trampoline_and_lazy_seq_are_runtime_primitives() {
    let source = r#"
(module loop_compile)
(defn trampoline-id [[value Int]] -> Int
  (trampoline (fn [item] item) value))
(defn delayed-values [[value Int]] -> Any
  (lazy-seq [value]))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let python = result
        .python
        .expect("control primitives should generate Python")
        .source;
    assert!(python.contains("trampoline as"), "{python}");
    assert!(python.contains("lazy_seq as"), "{python}");
}

#[test]
fn trampoline_rejects_a_known_nonzero_arity_bounce() {
    let source = r#"
(module loop_compile)
(defn bad-step [[value Int]] -> (Fn [Int] -> Int)
  (fn [[argument Int]] -> Int argument))
(defn bad [[value Int]] -> Any
  (trampoline bad-step value))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "OSR-T0024" && diagnostic.message.contains("zero-argument callables")
        }),
        "expected a closed trampoline arity diagnostic, got {:?}",
        result.analysis.diagnostics
    );
}
