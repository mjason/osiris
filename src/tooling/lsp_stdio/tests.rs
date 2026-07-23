use std::io::Cursor;

use super::serve;

fn frame(payload: &str) -> Vec<u8> {
    format!("Content-Length: {}\r\n\r\n{payload}", payload.len()).into_bytes()
}

#[test]
fn serves_multiple_framed_messages_and_stops_on_exit() {
    let initialize = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
    let exit = r#"{"jsonrpc":"2.0","method":"exit","params":{}}"#;
    let ignored = r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{}}"#;
    let mut input = frame(initialize);
    input.extend(frame(exit));
    input.extend(frame(ignored));
    let mut reader = Cursor::new(input);
    let mut output = Vec::new();

    serve(&mut reader, &mut output).expect("transport should serve transcript");

    let output = String::from_utf8(output).expect("server output should be UTF-8");
    assert!(output.starts_with("Content-Length: "));
    assert!(output.contains(r#""id":1"#));
    assert!(!output.contains(r#""id":2"#));
}

#[test]
fn content_length_counts_utf8_bytes() {
    let open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///示例.osr","languageId":"osiris","version":1,"text":"(def 值 1)"}}}"#;
    let exit = r#"{"jsonrpc":"2.0","method":"exit"}"#;
    let mut input = frame(open);
    input.extend(frame(exit));
    let mut reader = Cursor::new(input);
    let mut output = Vec::new();

    serve(&mut reader, &mut output).expect("Unicode transcript should be framed by bytes");
    assert!(
        String::from_utf8(output)
            .expect("UTF-8 output")
            .contains("publishDiagnostics")
    );
}
