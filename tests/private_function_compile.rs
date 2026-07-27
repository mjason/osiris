use std::{
    env, fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use osiris::{compiler::CompileOptions, compiler::compile, project::PythonVersion};

fn options() -> CompileOptions {
    CompileOptions::new("private_function_compile", PythonVersion::default())
}

fn temporary_directory() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let path = env::temp_dir().join(format!("osiris-private-function-{nonce}"));
    fs::create_dir_all(&path).expect("create temporary directory");
    path
}

/// Privacy is the absence of publication, not a declaration form: a helper that
/// is neither listed in `(export [...])` nor marked `^:export` stays off the
/// interface while remaining an ordinary function in the generated Python.
#[test]
fn an_unpublished_helper_stays_private_and_lowers_to_a_normal_function() {
    let source = r#"
(module private_function_compile)
(defn ^Int increment [^Int value] (+ value 1))
^{:doc "Increment an integer through a private helper."}
(defn ^Int public [^Int value] (increment value))
(export [public])
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:#?}",
        result.analysis.diagnostics
    );
    let exported = result
        .analysis
        .hir
        .bindings
        .iter()
        .filter(|binding| binding.public)
        .map(|binding| binding.source_spelling.as_str())
        .collect::<Vec<_>>();
    assert_eq!(exported, ["public"]);

    let generated = result.python.expect("generated Python").source;
    assert!(generated.contains("def increment"), "{generated}");
    assert!(generated.contains("def public"), "{generated}");

    let root = temporary_directory();
    fs::write(root.join("private_function_compile.py"), &generated)
        .expect("write generated module");
    let smoke = root.join("smoke.py");
    fs::write(
        &smoke,
        "from private_function_compile import public\nassert public(41) == 42\nprint('ok')\n",
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
        generated
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "ok\n");
    fs::remove_dir_all(root).expect("remove temporary directory");
}

/// The per-item marker is the other explicit way to publish, so it must reach
/// the same public surface the manifest does.
#[test]
fn an_export_marker_publishes_without_a_manifest_entry() {
    let source = r#"
(module private_function_compile)
(defn ^Int increment [^Int value] (+ value 1))
^{:doc "Increment an integer through a private helper." :export true}
(defn ^Int public [^Int value] (increment value))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:#?}",
        result.analysis.diagnostics
    );
    let exported = result
        .analysis
        .hir
        .bindings
        .iter()
        .filter(|binding| binding.public)
        .map(|binding| binding.source_spelling.as_str())
        .collect::<Vec<_>>();
    assert_eq!(exported, ["public"]);
}
