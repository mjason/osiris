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
(import osiris.core :refer [letfn])
(defn ^Int factorial [^Int n]
  (letfn [(^Int fact [^Int value]
           (if (= value 0)
             1
             (* value (fact (- value 1)))))]
    (fact n)))

(defn ^Str parity [^Int n]
  (letfn [(^Bool even? [^Int value]
            (if (= value 0) true (odd? (- value 1))))
          (^Bool odd? [^Int value]
            (if (= value 0) false (even? (- value 1))))]
    (if (even? n) "even" "odd")))
"#;
    let result = compile(source, &options());
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("letfn should generate Python");
    let root = temporary_directory();
    fs::write(root.join("letfn_compile.py"), &generated.source).expect("write generated module");
    if let Some(support) = &generated.runtime_support {
        for (path, source) in osiris::backend::runtime_distribution_files(
            support,
            osiris::project::PythonVersion::default(),
        )
        .expect("link runtime distribution")
        {
            let destination = root.join(path);
            fs::create_dir_all(destination.parent().expect("support parent"))
                .expect("create support directory");
            fs::write(destination, source).expect("write support file");
        }
    }
    let smoke = root.join("smoke.py");
    fs::write(
        &smoke,
        "from letfn_compile import factorial, parity\nassert factorial(6) == 720\nassert parity(10) == 'even'\nassert parity(7) == 'odd'\nprint('ok')\n",
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
