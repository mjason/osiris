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

#[test]
fn defn_dash_keeps_private_metadata_and_lowers_to_a_normal_function() {
    let source = r#"
(module private_function_compile)
(import osiris.core :refer [defn-])
(defn- ^Int increment [^Int value] (+ value 1))
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

#[test]
fn malformed_defn_dash_reports_a_macro_diagnostic() {
    let result = compile(
        "(module private_function_compile) (import osiris.core :refer [defn-]) (defn- 1 [] 1)",
        &options(),
    );
    assert!(result.analysis.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "OSR-M0007"
            && diagnostic.message.contains("defn- requires a symbol name")
    }));
    assert!(result.python.is_none());
}
