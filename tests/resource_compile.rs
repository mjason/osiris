use std::{
    env, fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use osiris::{
    compiler::{CompileOptions, compile},
    project::PythonVersion,
};

fn options() -> CompileOptions {
    CompileOptions::new("resource_compile", PythonVersion::default())
}

fn temporary_directory() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let path = env::temp_dir().join(format!("osiris-resource-{nonce}"));
    fs::create_dir_all(&path).expect("create temporary directory");
    path
}

#[test]
fn with_open_closes_resources_in_reverse_order() {
    let source = r#"
(module resource_compile)
(py/import builtins :as py)
(defn read-resource [] -> Any
  (with-open [first (py.open "/dev/null" "w")
              second (py.open "/dev/null" "w")]
    (py.str first.closed)))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let generated = result
        .python
        .expect("with-open should generate Python")
        .source;
    assert!(generated.contains("close as"), "{generated}");
    let root = temporary_directory();
    fs::write(root.join("resource_compile.py"), &generated).expect("write generated module");
    let smoke = root.join("smoke.py");
    fs::write(
        &smoke,
        "from resource_compile import read_resource\nassert read_resource() == 'False'\nprint('ok')\n",
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
