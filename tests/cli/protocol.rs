use super::*;

#[test]
fn version_flag_reports_package_version() {
    let output = osr(&["--version"]);

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout should be UTF-8"),
        format!("osr {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn unknown_arguments_fail() {
    let output = osr(&["source.osr"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unexpected arguments"));
}

#[test]
fn lsp_command_uses_standard_content_length_framing() {
    let initialize = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
    let exit = r#"{"jsonrpc":"2.0","method":"exit"}"#;
    let transcript = format!(
        "Content-Length: {}\r\n\r\n{}Content-Length: {}\r\n\r\n{}",
        initialize.len(),
        initialize,
        exit.len(),
        exit
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_osr"))
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("LSP server should start");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(transcript.as_bytes())
        .expect("transcript should be written");
    let output = child.wait_with_output().expect("LSP server should exit");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("LSP output should be UTF-8");
    assert!(stdout.starts_with("Content-Length: "));
    assert!(stdout.contains(r#""id":1"#));
    assert!(stdout.contains("serverInfo"));
}

#[test]
fn lsp_resolves_workspace_module_identity_and_source_interfaces() {
    let fixture = SourceFixture::new("(def ignored 0)\n");
    let app_source = r#"(module demo.app)
        (import demo.math :as math)
        (def answer (math/add-one 41))
    "#;
    let app = fixture.write("src/demo/app.osr", app_source);
    fixture.write(
        "src/demo/math.osr",
        r#"(module demo.math)
            (export [add-one])
            (defn add-one [[value Int]] -> Int (+ value 1))
        "#,
    );
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"workspace-lsp\"\nversion = \"1.0\"\n\n[tool.osiris]\nsource = [\"src\"]\n",
    )
    .expect("project configuration should be written");
    let uri = format!("file://{}", app.display());
    let messages = [
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {},
        })
        .to_string(),
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": uri.clone(),
                    "languageId": "osiris",
                    "version": 1,
                    "text": app_source,
                }
            },
        })
        .to_string(),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "osiris/semanticView",
            "params": { "textDocument": { "uri": uri } },
        })
        .to_string(),
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "exit",
        })
        .to_string(),
    ];
    let transcript = messages
        .iter()
        .map(|message| format!("Content-Length: {}\r\n\r\n{message}", message.len()))
        .collect::<String>();
    let mut child = Command::new(env!("CARGO_BIN_EXE_osr"))
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("LSP server should start");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(transcript.as_bytes())
        .expect("transcript should be written");
    let output = child.wait_with_output().expect("LSP server should exit");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("LSP output should be UTF-8");
    assert!(stdout.contains(r#""diagnostics":[]"#), "{stdout}");
    assert!(stdout.contains(r#""module":"demo.app""#), "{stdout}");
}
