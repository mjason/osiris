use super::*;

#[test]
fn assert_uses_a_runtime_exception_and_keeps_message_lazy() {
    let source = r#"
(module control_compile)
(import osiris.core :refer :all)
(defn ^None check [^Any value]
  (assert value "assertion failed"))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated assert Python");
    assert!(
        generated
            .source
            .contains("assert_value as _u0_osiris_assert_value"),
        "{}",
        generated.source
    );
    let root = temporary_directory();
    write_generated_module(&root, "control_compile.py", &generated);
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
    let output = Command::new("python3")
        .arg(&smoke)
        .env("PYTHONPATH", &root)
        .output()
        .expect("run generated assert Python");
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
fn clojure_truthiness_applies_to_control_macros_over_any_values() {
    let source = r#"
(module control_compile)
(import osiris.core :refer :all)
(defn ^{:type (Option Str)} when-any [^Any value]
  (when value "yes"))
(defn ^Any and-any [^Any value]
  (and value 42))
(defn ^Any or-any [^Any value]
  (or value 42))
(defn ^Str cond-any [^Any value]
  (cond value "yes" :else "no"))
(defn ^{:type (Vector Any)} for-any [^{:type (Vector Any)} values]
  (forv [value values :when value] value))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated truthiness Python");
    let root = temporary_directory();
    write_generated_module(&root, "control_compile.py", &generated);
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
    let output = Command::new("python3")
        .arg(&smoke)
        .env("PYTHONPATH", &root)
        .output()
        .expect("run generated truthiness Python");
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
