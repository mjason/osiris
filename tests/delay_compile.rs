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
    CompileOptions::new("delay_compile", PythonVersion::default())
}

fn temporary_directory() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let path = env::temp_dir().join(format!("osiris-delay-{nonce}"));
    fs::create_dir_all(&path).expect("create temporary directory");
    path
}

#[test]
fn delay_force_and_realized_compile_to_readable_python() {
    let source = r#"
(module delay_compile)
(import osiris.core :refer [delay force realized?])
(defn ^Int delayed [^Int value]
  (let [value* (delay (+ value 1))]
    (if (realized? value*)
      0
      (force value*))))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("delay should generate Python");
    assert!(generated.source.contains("delay"), "{}", generated.source);
    assert!(generated.source.contains("force"), "{}", generated.source);
    let root = temporary_directory();
    fs::write(root.join("delay_compile.py"), &generated.source).expect("write generated module");
    let support = generated
        .runtime_support
        .expect("delay should link private runtime support");
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
        "from delay_compile import delayed\nassert delayed(41) == 42\nprint('ok')\n",
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
