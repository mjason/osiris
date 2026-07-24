use super::*;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[cfg(unix)]
#[test]
fn fmt_all_preserves_permissions_and_reports_paths_in_stable_order() {
    let fixture = SourceFixture::new("none\n");
    let second = fixture.write("src/zeta.osr", "(def   zeta   2)\n");
    let first = fixture.write("src/alpha.osr", "(def   alpha   1)\n");
    fs::write(
        fixture.directory.join("osiris.jsonc"),
        r#"{"source":["src"],"exclude":[]}"#,
    )
    .unwrap();
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"format-project\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&first).unwrap().permissions();
    permissions.set_mode(0o640);
    fs::set_permissions(&first, permissions).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_osr"))
        .args(["fmt", "--all"])
        .current_dir(&fixture.directory)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "src/alpha.osr\nsrc/zeta.osr\n"
    );
    assert_eq!(
        fs::metadata(&first).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert_eq!(fs::read_to_string(first).unwrap(), "(def alpha 1)\n");
    assert_eq!(fs::read_to_string(second).unwrap(), "(def zeta 2)\n");
    assert!(
        fs::read_dir(fixture.directory.join("src"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("osr-fmt"))
    );
}

#[test]
fn fmt_check_and_invalid_syntax_never_rewrite_sources() {
    let fixture = SourceFixture::new("none\n");
    let valid = fixture.write("valid.osr", "(def   value  1)\n");
    let invalid = fixture.write("invalid.osr", "(def broken [1 2)\n");
    let before_valid = fs::read(&valid).unwrap();
    let before_invalid = fs::read(&invalid).unwrap();

    let check = osr(&["fmt", path_argument(&valid), "--check"]);
    assert_eq!(check.status.code(), Some(1));
    assert_eq!(fs::read(&valid).unwrap(), before_valid);

    let failed = osr(&["fmt", path_argument(&valid), path_argument(&invalid)]);
    assert_eq!(failed.status.code(), Some(1));
    assert_eq!(fs::read(&invalid).unwrap(), before_invalid);
    assert_eq!(fs::read_to_string(valid).unwrap(), "(def value 1)\n");
    assert!(String::from_utf8_lossy(&failed.stderr).contains("invalid.osr"));
}

#[test]
fn fmt_stdin_is_byte_identical_to_lsp_document_formatting() {
    let source = "(def   value [1  2])\n";
    let mut cli = Command::new(env!("CARGO_BIN_EXE_osr"))
        .args(["fmt", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    cli.stdin
        .take()
        .unwrap()
        .write_all(source.as_bytes())
        .unwrap();
    let cli = cli.wait_with_output().unwrap();
    assert!(cli.status.success());

    let uri = "file:///tmp/osiris-format-equality.osr";
    let initialize = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
    let open = format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{uri}","languageId":"osiris","version":1,"text":{}}}}}}}"#,
        serde_json::to_string(source).unwrap()
    );
    let format_request = format!(
        r#"{{"jsonrpc":"2.0","id":2,"method":"textDocument/formatting","params":{{"textDocument":{{"uri":"{uri}"}},"options":{{"tabSize":8,"insertSpaces":false}}}}}}"#
    );
    let exit = r#"{"jsonrpc":"2.0","method":"exit"}"#;
    let transcript = [initialize.to_owned(), open, format_request, exit.to_owned()]
        .into_iter()
        .map(|message| format!("Content-Length: {}\r\n\r\n{message}", message.len()))
        .collect::<String>();
    let mut lsp = Command::new(env!("CARGO_BIN_EXE_osr"))
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    lsp.stdin
        .take()
        .unwrap()
        .write_all(transcript.as_bytes())
        .unwrap();
    let lsp = lsp.wait_with_output().unwrap();
    assert!(
        lsp.status.success(),
        "{}",
        String::from_utf8_lossy(&lsp.stderr)
    );
    let output = String::from_utf8(lsp.stdout).unwrap();
    let expected = serde_json::to_string(&String::from_utf8(cli.stdout).unwrap()).unwrap();
    assert!(
        output.contains(&format!(r#""newText":{expected}"#)),
        "{output}"
    );
}
