use std::{
    env, fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use osiris::{compiler::CompileOptions, compiler::compile, project::PythonVersion};

fn options() -> CompileOptions {
    CompileOptions::new("time_compile", PythonVersion::default())
}

fn temporary_directory() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let path = env::temp_dir().join(format!("osiris-time-{nonce}"));
    fs::create_dir_all(&path).expect("create temporary directory");
    path
}

#[test]
fn time_evaluates_multiple_body_forms_and_returns_the_last_value() {
    let source = r#"
(module time_compile)
(import osiris.core :refer [time])
(defn ^Int timed []
  (time
    (let [ignored 1] ignored)
    (+ 40 2)))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:#?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("generated Python");
    assert!(
        generated.source.contains("time_value"),
        "{}",
        generated.source
    );

    let root = temporary_directory();
    fs::write(root.join("time_compile.py"), &generated.source).expect("write generated module");
    let support = generated
        .runtime_support
        .expect("time should link private runtime support");
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
        "from time_compile import timed\nassert timed() == 42\n",
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
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Elapsed time:"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    fs::remove_dir_all(root).expect("remove temporary directory");
}

#[test]
fn time_requires_a_body() {
    let source = "(module time_compile) (import osiris.core :refer [time]) (def value (time))";
    let result = compile(source, &options());
    assert!(
        result
            .analysis
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-M0007"),
        "{:#?}",
        result.analysis.diagnostics
    );
    assert!(result.python.is_none());
}
