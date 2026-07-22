use std::{
    env, fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use _core::{compiler::CompileOptions, compiler::compile, project::PythonVersion};

fn options() -> CompileOptions {
    CompileOptions::new("predicates_compile", PythonVersion::default())
}

fn temporary_directory() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let path = env::temp_dir().join(format!("osiris-predicates-{nonce}"));
    fs::create_dir_all(&path).expect("create temporary directory");
    path
}

#[test]
fn sequence_predicates_lower_to_typed_runtime_calls() {
    let source = r#"
(module predicates_compile)
(defn seq-value [[value Any]] -> Bool (seq? value))
(defn coll-value [[value Any]] -> Bool (coll? value))
(defn sequential-value [[value Any]] -> Bool (sequential? value))
(defn qualified-seq-value [[value Any]] -> Bool (osiris.prelude/seq? value))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:#?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated Python").source;
    assert!(generated.contains("seq_q"), "{generated}");
    assert!(generated.contains("coll_q"), "{generated}");
    assert!(generated.contains("sequential_q"), "{generated}");

    let root = temporary_directory();
    fs::write(root.join("predicates_compile.py"), &generated).expect("write generated module");
    let smoke = root.join("smoke.py");
    fs::write(
        &smoke,
        r#"from osiris.prelude import sequence
from predicates_compile import coll_value, qualified_seq_value, seq_value, sequential_value

lazy = sequence(iter((1, 2)))
assert seq_value([]) is True
assert seq_value(()) is False
assert seq_value(lazy) is True
assert seq_value(iter((1, 2))) is False
assert seq_value(None) is False

assert coll_value([]) is True
assert coll_value(()) is True
assert coll_value({"value": 1}) is True
assert coll_value({1, 2}) is True
assert coll_value(lazy) is True
assert coll_value("text") is False

assert sequential_value([]) is True
assert sequential_value(()) is True
assert sequential_value(lazy) is True
assert sequential_value({"value": 1}) is False
assert sequential_value("text") is False
assert qualified_seq_value(lazy) is True
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

#[test]
fn sequence_predicates_reject_invalid_arity() {
    let source = r#"
(module predicates_compile)
(defn invalid [[value Any]] -> Bool (seq? value value))
"#;
    let result = compile(source, &options());
    assert!(
        result
            .analysis
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-T0041"),
        "{:#?}",
        result.analysis.diagnostics
    );
    assert!(result.python.is_none());
}
