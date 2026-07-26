use super::*;

#[test]
fn lsa_reports_a_missing_api_key_without_starting_a_request() {
    let fixture = SourceFixture::new("(module sample)\n");
    fixture.write(
        "pyproject.toml",
        "[project]\nname = \"lsa-key\"\nversion = \"0\"\n",
    );
    fixture.write("osiris.jsonc", r#"{"source":["src"]}"#);
    fixture.write("src/sample.osr", "(module sample)\n");
    let output = Command::new(env!("CARGO_BIN_EXE_osr"))
        .args(["lsa", "Explain reduce"])
        .current_dir(&fixture.directory)
        .env_remove("OSR_API_KEY")
        .env_remove("OSR_BASE_URL")
        .env_remove("OSR_MODEL")
        .env_remove("OSR_WIRE_API")
        .output()
        .expect("osr lsa should run");

    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("OSR_API_KEY is not set"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn lsc_composite_queries_use_the_project_libsql_graph() {
    let fixture = SourceFixture::new("(module ignored)\n");
    fixture.write(
        "pyproject.toml",
        "[project]\nname = \"lsc-graph\"\nversion = \"0\"\n",
    );
    fixture.write(
        "osiris.jsonc",
        r#"{"source":["src"],"targetPython":"3.11","strict":true}"#,
    );
    fixture.write(
        "src/demo/text.osr",
        r#"(module demo.text)

(export [format-message])

^{:doc {:default "Format a message for display."}}
(defn ^Str format-message [^Str value]
  value)
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_osr"))
        .args([
            "lsc",
            "workspace-search",
            "message for display",
            "--format",
            "json",
        ])
        .current_dir(&fixture.directory)
        .output()
        .expect("lsc graph search");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("LSC JSON");
    assert_eq!(value["result"]["status"], "ok");
    assert_eq!(
        value["result"]["result"][0]["data"]["bindingId"],
        "demo.text::function::format-message"
    );
    assert!(
        value["result"]["result"][0]["location"]["uri"]
            .as_str()
            .is_some_and(|uri| uri.starts_with("osiris-workspace:///"))
    );
    assert!(
        fixture
            .directory
            .join(".osiris/cache/language-graph.sqlite3")
            .is_file()
    );

    let status = Command::new(env!("CARGO_BIN_EXE_osr"))
        .args(["lsc", "cache", "status", "--format", "json"])
        .current_dir(&fixture.directory)
        .output()
        .expect("lsc cache status");
    assert!(status.status.success());
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).expect("status JSON");
    assert_eq!(status["result"]["status"], "fresh");
    assert_eq!(status["result"]["hashedInputs"], 0);
    assert_eq!(
        status["result"]["inputCount"],
        status["result"]["reusedHashes"]
    );

    let rebuild = Command::new(env!("CARGO_BIN_EXE_osr"))
        .args(["lsc", "cache", "rebuild", "--format", "json"])
        .current_dir(&fixture.directory)
        .output()
        .expect("lsc cache rebuild");
    assert!(
        rebuild.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuild.stderr)
    );
    let rebuild: serde_json::Value = serde_json::from_slice(&rebuild.stdout).expect("rebuild JSON");
    assert_eq!(rebuild["result"]["status"], "rebuilt");
    assert_eq!(rebuild["result"]["reusedHashes"], 0);
    assert_eq!(
        rebuild["result"]["inputCount"],
        rebuild["result"]["hashedInputs"]
    );
}
