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
(defn bump [[value Int]] -> Int (+ value 1))
(defn match-plus-ten [[test Int] [value Int]] -> (Option Int)
  (if (= test value) (+ value 10) none))
(defn render [[matched Int]] -> Int (+ matched 100))
(defn choose-operator [[value Int]] -> Int
  (condp = (bump value)
    2 7
    :else -1))
(defn choose-handler [[value Int]] -> Int
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
    let generated = result.python.expect("condp should generate Python").source;
    let root = temporary_directory();
    fs::write(root.join("condp_compile.py"), &generated).expect("write generated module");
    let smoke = root.join("smoke.py");
    fs::write(
        &smoke,
        "from condp_compile import choose_handler, choose_operator\nassert choose_operator(1) == 7\nassert choose_operator(4) == -1\nassert choose_handler(1) == 112\nassert choose_handler(4) == -1\nprint('ok')\n",
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
