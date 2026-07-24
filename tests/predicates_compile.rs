use std::{
    env, fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use osiris::{compiler::CompileOptions, compiler::compile, project::PythonVersion};

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
(import osiris.core :as core :refer [coll? seq? sequence sequential?])
(defn ^Bool seq-value [^Any value] (seq? value))
(defn ^Bool coll-value [^Any value] (coll? value))
(defn ^Bool sequential-value [^Any value] (sequential? value))
(defn ^Bool qualified-seq-value [^Any value] (core/seq? value))
(defn ^Any lazy-values [] (sequence [1 2]))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:#?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated Python");
    assert!(generated.source.contains("seq_p"), "{}", generated.source);
    assert!(generated.source.contains("coll_p"), "{}", generated.source);
    assert!(
        generated.source.contains("sequential_p"),
        "{}",
        generated.source
    );

    let root = temporary_directory();
    fs::write(root.join("predicates_compile.py"), &generated.source)
        .expect("write generated module");
    let support = generated
        .runtime_support
        .expect("sequence predicates should link private runtime support");
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
        r#"from predicates_compile import coll_value, lazy_values, qualified_seq_value, seq_value, sequential_value

lazy = lazy_values()
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
fn sequence_predicates_reject_invalid_arity() {
    let source = r#"
(module predicates_compile)
(import osiris.core :refer [seq?])
(defn ^Bool invalid [^Any value] (seq? value value))
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
