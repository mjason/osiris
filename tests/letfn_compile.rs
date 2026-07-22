use std::{
    env, fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use _core::{
    compiler::{CompileOptions, compile},
    project::PythonVersion,
};

fn options() -> CompileOptions {
    CompileOptions::new("letfn_compile", PythonVersion::default())
}

fn temporary_directory() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let path = env::temp_dir().join(format!("osiris-letfn-{nonce}"));
    fs::create_dir_all(&path).expect("create temporary directory");
    path
}

#[test]
fn letfn_supports_self_and_mutual_recursion() {
    let source = r#"
(module letfn_compile)
(defn factorial [[n Int]] -> Int
  (letfn [(fact [[value Int]] -> Int
           (if (= value 0)
             1
             (* value (fact (- value 1)))))]
    (fact n)))

(defn parity [[n Int]] -> Str
  (letfn [(even? [[value Int]] -> Bool
            (if (= value 0) true (odd? (- value 1))))
          (odd? [[value Int]] -> Bool
            (if (= value 0) false (even? (- value 1))))]
    (if (even? n) "even" "odd")))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("letfn should generate Python").source;
    let root = temporary_directory();
    fs::write(root.join("letfn_compile.py"), &generated).expect("write generated module");
    let smoke = root.join("smoke.py");
    fs::write(
        &smoke,
        "from letfn_compile import factorial, parity\nassert factorial(6) == 720\nassert parity(10) == 'even'\nassert parity(7) == 'odd'\nprint('ok')\n",
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
