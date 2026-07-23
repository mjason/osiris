use super::*;

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
