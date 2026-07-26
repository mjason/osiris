use super::*;
use std::{
    io::Read,
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

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

#[test]
fn lsa_validates_only_reachable_project_sources_in_one_provider_request() {
    let fixture = SourceFixture::new("(module ignored)\n");
    fixture.write(
        "pyproject.toml",
        "[project]\nname = \"lsa-reachable\"\nversion = \"0\"\n",
    );
    fixture.write(
        "osiris.jsonc",
        r#"{"source":["src"],"targetPython":"3.11","strict":true}"#,
    );
    fixture.write(
        "src/app/broken.osr",
        "(module app.broken)\n\n^{:examples [\"obsolete inline example\"]}\n(def value 0)\n",
    );

    let listener = TcpListener::bind("127.0.0.1:0").expect("mock provider should bind");
    listener
        .set_nonblocking(true)
        .expect("mock provider should become nonblocking");
    let address = listener.local_addr().expect("mock provider address");
    let requests = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let server_requests = Arc::clone(&requests);
    let server_stop = Arc::clone(&stop);
    let server = thread::spawn(move || {
        while !server_stop.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    server_requests.fetch_add(1, Ordering::AcqRel);
                    read_provider_request(&mut stream);
                    let model_output = serde_json::json!({
                        "answer": "这是一个最小的 Hello World 示例。",
                        "examples": [{
                            "code": "(module example.hello)\n\n\"Hello, world!\"\n"
                        }],
                        "references": []
                    })
                    .to_string();
                    let body = serde_json::json!({
                        "choices": [{"message": {"content": model_output}}]
                    })
                    .to_string();
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                    .expect("mock response should be written");
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("mock provider failed: {error}"),
            }
        }
    });

    let output = Command::new(env!("CARGO_BIN_EXE_osr"))
        .args(["lsa", "怎么写一个 hello world 示例？"])
        .current_dir(&fixture.directory)
        .env("OSR_API_KEY", "test-key")
        .env("OSR_BASE_URL", format!("http://{address}"))
        .env("OSR_MODEL", "test-model")
        .env("OSR_WIRE_API", "chatCompletions")
        .output()
        .expect("osr lsa should run");
    stop.store(true, Ordering::Release);
    server.join().expect("mock provider should stop");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(requests.load(Ordering::Acquire), 1);
    let response: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("LSA JSON response");
    assert_eq!(response["examples"][0]["compiled"], true);
    assert_eq!(response["examples"][0]["evaluated"], true);
    assert_eq!(response["examples"][0]["result"], "Hello, world!");
    assert_eq!(
        response["examples"][0]["diagnostics"],
        serde_json::json!([])
    );
    assert!(
        !fixture
            .directory
            .join(".osiris/cache/language-graph.sqlite3")
            .exists()
    );
}

fn read_provider_request(stream: &mut impl Read) {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let read = stream
            .read(&mut buffer)
            .expect("request should be readable");
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        if request.len() >= header_end + 4 + content_length {
            break;
        }
    }
}
