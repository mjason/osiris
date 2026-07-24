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
    CompileOptions::new("condp_compile", PythonVersion::default())
}

fn temporary_directory() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let path = env::temp_dir().join(format!("osiris-condp-{nonce}"));
    fs::create_dir_all(&path).expect("create temporary directory");
    path
}

#[test]
fn condp_supports_operator_predicates_and_result_handlers() {
    let source = r#"
(module condp_compile)
(import osiris.core :refer [condp])
(defn ^Int bump [^Int value] (+ value 1))
(defn ^{:type (Option Int)} match-plus-ten [^Int test ^Int value]
  (if (= test value) (+ value 10) none))
(defn ^Int render [^Int matched] (+ matched 100))
(defn ^Int choose-operator [^Int value]
  (condp = (bump value)
    2 7
    :else -1))
(defn ^Int choose-handler [^Int value]
  (condp match-plus-ten (bump value)
    2 :>> render
    :else -1))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("condp should generate Python");
    let root = temporary_directory();
    fs::write(root.join("condp_compile.py"), &generated.source).expect("write generated module");
    let support = generated
        .runtime_support
        .expect("condp should link its private runtime support");
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
        "from condp_compile import choose_handler, choose_operator\nassert choose_operator(1) == 7\nassert choose_operator(4) == -1\nassert choose_handler(1) == 112\nassert choose_handler(4) == -1\nprint('ok')\n",
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
