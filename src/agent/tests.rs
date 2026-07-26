use super::client::{chat_completions_body, extract_chat_completions_text, extract_responses_text};
use super::*;
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn locale_detection_prefers_chinese_input() {
    assert_eq!(detect_locale("解释 reduce"), "zh-CN");
    assert_eq!(detect_locale("Explain reduce"), "en");
    assert_eq!(detect_locale("reduce の例"), "ja");
}

#[test]
fn session_ids_reject_path_traversal() {
    assert!(validate_session_id("../secret").is_err());
    assert!(validate_session_id("..").is_err());
    assert!(validate_session_id("review-1").is_ok());
}

#[test]
fn locales_are_normalized_as_bcp47_tags() {
    assert_eq!(normalize_locale("zh-cn").unwrap(), "zh-CN");
    assert!(normalize_locale("zh_CN").is_err());
}

#[test]
fn parses_both_responses_api_text_shapes() {
    let top_level = serde_json::json!({"output_text": "hello"});
    assert_eq!(extract_responses_text(&top_level).unwrap(), "hello");
    let nested = serde_json::json!({
        "output": [{"content": [{"type": "output_text", "text": "hello"}]}]
    });
    assert_eq!(extract_responses_text(&nested).unwrap(), "hello");

    let chat = serde_json::json!({
        "choices": [{"message": {"content": "hello"}}]
    });
    assert_eq!(extract_chat_completions_text(&chat).unwrap(), "hello");
}

#[test]
fn disables_thinking_for_deepseek_chat_completions() {
    let deepseek = AgentConfig {
        model: "deepseek-v4-flash".to_owned(),
        ..AgentConfig::default()
    };
    let body = chat_completions_body(&deepseek, "hello");
    assert_eq!(body["thinking"]["type"], "disabled");
    assert!(body.get("reasoning_effort").is_none());

    let other = AgentConfig {
        model: "gpt-compatible".to_owned(),
        ..AgentConfig::default()
    };
    let body = chat_completions_body(&other, "hello");
    assert!(body.get("thinking").is_none());
    assert!(body.get("reasoning_effort").is_none());
}

#[test]
fn parses_model_json_without_compiler_owned_fields() {
    let response = parse_model_response(
        r#"{"answer":"Use reduce.","examples":[],"references":[]}"#,
        "session-1",
    )
    .unwrap();
    assert_eq!(response.session_id, "session-1");
    assert_eq!(response.answer, "Use reduce.");
}

#[test]
fn extracts_model_json_from_reasoning_and_markdown_wrappers() {
    let wrapped = "<think>consider {not json}</think>\n```json\n{\"answer\":\"ok\",\"examples\":[],\"references\":[]}\n```\ntrailing";
    let response = parse_model_response(wrapped, "session-1").unwrap();
    assert_eq!(response.answer, "ok");

    let calls = parse_tool_calls(
        "I will inspect it.\n{\"toolCalls\":[{\"id\":\"one\",\"operation\":\"workspace-search\",\"arguments\":{\"query\":\"Point\"}}]}",
    )
    .unwrap()
    .unwrap();
    assert_eq!(calls[0].id, "one");
    assert!(model_json_text("reasoning only").is_err());
}

#[test]
fn parses_bounded_language_service_tool_calls() {
    let calls = parse_tool_calls(
        r#"{"toolCalls":[{"id":"search-1","operation":"workspace-search","arguments":{"query":"format message"}}]}"#,
    )
    .unwrap()
    .expect("tool calls");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "search-1");
    assert_eq!(calls[0].operation, "workspace-search");
    assert_eq!(calls[0].arguments["query"], "format message");
    assert!(
        parse_tool_calls(r#"{"answer":"done","examples":[],"references":[]}"#)
            .unwrap()
            .is_none()
    );
}

#[test]
fn model_cannot_supply_compiler_owned_language_service_evidence() {
    let response = parse_model_response(
        r#"{"answer":"claim","examples":[],"references":["invented"],"languageService":[{"callId":"fake","operation":"hover","status":"ok","result":{"invented":true}}]}"#,
        "session-1",
    )
    .unwrap();
    assert!(response.language_service.is_empty());
}

#[test]
fn exact_standard_api_requests_outrank_namespace_matches() {
    let material = collect_material(
        Path::new("."),
        None,
        "Explain osiris.core/reduce with an example",
        None,
    )
    .unwrap();
    let reduce = material
        .references
        .iter()
        .position(|reference| reference == "osiris.core::function::reduce")
        .expect("reduce should be retrieved");
    assert!(material.text.contains("Type: Fn["));
    assert!(
        material
            .text
            .contains("(reduce function initial collection)")
    );
    let map = material
        .references
        .iter()
        .position(|reference| reference == "osiris.core::function::map");
    assert!(map.is_none_or(|map| reduce < map));
}

#[test]
fn standard_api_retrieval_does_not_use_substring_name_matches() {
    let material = collect_material(
        Path::new("."),
        None,
        "Explain defstruct and provide one complete example",
        None,
    )
    .unwrap();
    for unrelated in [
        "osiris.core::macro::and",
        "osiris.core::function::comp",
        "osiris.math::value::e",
        "osiris.math::function::exp",
    ] {
        assert!(!material.references.iter().any(|item| item == unrelated));
    }
}

#[test]
fn an_explicit_example_request_rejects_an_empty_example_list() {
    let response = LsaResponse {
        schema: RESPONSE_SCHEMA.to_owned(),
        session_id: "session-1".to_owned(),
        answer: "Explanation".to_owned(),
        examples: Vec::new(),
        references: Vec::new(),
        language_service: Vec::new(),
    };
    assert_eq!(response_issue_count(&response, "Provide an example"), 1);
    assert_eq!(response_issue_count(&response, "Only explain this"), 0);
}

#[test]
fn syntax_retrieval_finds_later_structures_section() {
    let material = collect_material(
        Path::new("."),
        None,
        "Explain defstruct with a Point example",
        None,
    )
    .unwrap();
    assert!(material.text.contains("## Structures"));
    assert!(material.text.contains("(defstruct Threshold"));
}

#[test]
fn project_questions_retrieve_configuration_and_publication_guidance() {
    let material = collect_material(
        Path::new("."),
        None,
        "如何设置输出目录并把 Osiris 库发布到 PyPI？",
        None,
    )
    .unwrap();
    assert!(material.references.iter().any(|item| item == "tooling/cli"));
    assert!(
        !material
            .references
            .iter()
            .any(|item| item == "language/syntax")
    );
    assert!(material.text.contains("## Project Configuration"));
    assert!(material.text.contains("## Publishing a Package"));
    assert!(material.text.contains("uv publish dist/*"));
}

#[test]
fn explicitly_requested_project_config_is_redacted() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = env::temp_dir().join(format!("osiris-lsa-config-{}-{nonce}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("pyproject.toml"),
        "[project]\nname = \"lsa-config\"\nversion = \"0\"\n",
    )
    .unwrap();
    fs::write(
        root.join("osiris.jsonc"),
        r#"{
          "source": ["src"],
          "strict": false,
          "agent": {"baseUrl": "https://private.invalid/v1"},
          "apiToken": "must-not-leak"
        }"#,
    )
    .unwrap();
    let project = ProjectConfig::load(&root.join("pyproject.toml")).unwrap();
    let material = collect_material(
        &root,
        Some(Path::new("osiris.jsonc")),
        "Explain my configuration",
        Some(&project),
    )
    .unwrap();
    let _ = fs::remove_dir_all(&root);

    assert!(
        material
            .text
            .contains("Explicitly requested project configuration")
    );
    assert!(!material.text.contains("private.invalid"));
    assert!(!material.text.contains("must-not-leak"));
    assert!(material.text.contains("<redacted by osr lsa>"));
}

#[test]
fn credential_files_cannot_be_added_as_lsa_context() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = env::temp_dir().join(format!("osiris-lsa-secret-{}-{nonce}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join(".env"), "OSR_API_KEY=secret\n").unwrap();
    let error =
        collect_material(&root, Some(Path::new(".env")), "Explain this file", None).unwrap_err();
    let _ = fs::remove_dir_all(&root);

    assert!(error.contains("credential-bearing files"));
}

#[test]
fn prompts_require_a_standalone_module_form() {
    let material = ContextMaterial {
        text: "## Structures".to_owned(),
        references: Vec::new(),
    };
    let prompt = build_prompt("Show Point", "en", &material, &SessionFile::default(), &[]);
    assert!(prompt.contains("`(module example.point)`"));
    assert!(prompt.contains("never put a body inside it"));
    assert!(prompt.contains("Answer directly when retrieved syntax"));
    assert!(prompt.contains("Never search generic concepts"));
    assert!(prompt.contains("MUST request workspace tools"));
    assert!(prompt.contains("OUTPUT GATE"));

    let response = LsaResponse {
        schema: RESPONSE_SCHEMA.to_owned(),
        session_id: "session-1".to_owned(),
        answer: "Point".to_owned(),
        examples: Vec::new(),
        references: Vec::new(),
        language_service: Vec::new(),
    };
    let repair = build_repair_prompt(&response, "Show Point", "en", &material, &[]).unwrap();
    assert!(repair.contains("Original user request:\nShow Point"));
    assert!(repair.contains("closing parenthesis immediately after the one module name"));
}

#[test]
fn validates_and_formats_complete_examples() {
    let example = validate_example(LsaExample {
        code: "(module lsa.example)\n\n(py/import builtins :as py)\n(py.print \"hello\")\n"
            .to_owned(),
        result: None,
        compiled: false,
        evaluated: true,
        diagnostics: Vec::new(),
    });
    assert!(example.compiled, "{:?}", example.diagnostics);
    assert!(example.code.ends_with('\n'));
    assert!(example.evaluated, "{:?}", example.diagnostics);
    assert!(example.result.is_none());
}

#[test]
fn validates_typed_reduce_examples_with_automatic_core_refer() {
    let example = validate_example(LsaExample {
        code: "(module lsa.example)\n\n(reduce (fn [^Int total ^Int value] (+ total value)) 0 [1 2 3 4])\n"
            .to_owned(),
        result: Some(serde_json::json!("model prediction")),
        compiled: false,
        evaluated: false,
        diagnostics: Vec::new(),
    });
    assert!(example.compiled, "{:?}", example.diagnostics);
    assert!(example.evaluated, "{:?}", example.diagnostics);
    assert_eq!(example.result, Some(serde_json::json!(10)));
}

#[test]
fn evaluates_structure_modules_without_inventing_a_result() {
    let example = validate_example(LsaExample {
        code: "(module lsa.point)\n\n(defstruct Point [x Int] [y Int])\n".to_owned(),
        result: Some(serde_json::json!("invented")),
        compiled: false,
        evaluated: false,
        diagnostics: Vec::new(),
    });
    assert!(example.compiled, "{:?}", example.diagnostics);
    assert!(example.evaluated, "{:?}", example.diagnostics);
    assert!(example.result.is_none());
}

#[test]
fn evaluates_osiris_modules_with_embedded_python() {
    let example = validate_example(LsaExample {
        code: r#"(module lsa.embedded)

~python<text-backend>
def normalize(value: str) -> str:
    return value.strip().casefold()
</text-backend>

(extern python text-backend
  (defn ^Str normalize [^Str value]))

(normalize "  Hello  ")
"#
        .to_owned(),
        result: None,
        compiled: false,
        evaluated: false,
        diagnostics: Vec::new(),
    });
    assert!(example.compiled, "{:?}", example.diagnostics);
    assert!(example.evaluated, "{:?}", example.diagnostics);
    assert_eq!(example.result, Some(serde_json::json!("hello")));
}

#[test]
fn evaluates_osiris_modules_with_python_imports() {
    let example = validate_example(LsaExample {
        code: "(module lsa.math)\n\n(py/import math :as math)\n\n(math.sqrt 9.0)\n".to_owned(),
        result: None,
        compiled: false,
        evaluated: false,
        diagnostics: Vec::new(),
    });
    assert!(example.compiled, "{:?}", example.diagnostics);
    assert!(example.evaluated, "{:?}", example.diagnostics);
    assert_eq!(example.result, Some(serde_json::json!(3.0)));
}

#[test]
fn evaluation_stages_the_workspace_records_resolver() {
    let example = validate_example(LsaExample {
        code: "(module lsa.records)\n\n(py/import os :as os)\n\n(os.path.isfile (os.getenv \"OSIRIS_RECORDS_RESOLVER\"))\n"
            .to_owned(),
        result: None,
        compiled: false,
        evaluated: false,
        diagnostics: Vec::new(),
    });
    assert!(example.compiled, "{:?}", example.diagnostics);
    assert!(example.evaluated, "{:?}", example.diagnostics);
    assert_eq!(example.result, Some(serde_json::json!(true)));
}

#[test]
fn evaluates_osiris_modules_with_standard_library_imports() {
    let example = validate_example(LsaExample {
        code: "(module lsa.text)\n\n(import osiris.string :refer [upper])\n\n(upper \"hello\")\n"
            .to_owned(),
        result: None,
        compiled: false,
        evaluated: false,
        diagnostics: Vec::new(),
    });
    assert!(example.compiled, "{:?}", example.diagnostics);
    assert!(example.evaluated, "{:?}", example.diagnostics);
    assert_eq!(example.result, Some(serde_json::json!("HELLO")));
}

#[test]
fn evaluates_imports_from_the_current_project_workspace() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = env::temp_dir().join(format!(
        "osiris-lsa-workspace-{}-{nonce}",
        std::process::id()
    ));
    let source_root = root.join("src/app");
    fs::create_dir_all(&source_root).unwrap();
    fs::write(
        root.join("osiris.jsonc"),
        r#"{"source":["src"],"targetPython":"3.11","strict":true}"#,
    )
    .unwrap();
    fs::write(
        root.join("pyproject.toml"),
        "[project]\nname = \"lsa-workspace\"\nversion = \"0\"\n",
    )
    .unwrap();
    fs::write(
        source_root.join("value.osr"),
        "(module app.value)\n\n(export [answer])\n\n^{:doc {:default \"The answer.\"}}\n(def ^Int answer 41)\n",
    )
    .unwrap();
    fs::write(
        source_root.join("broken.osr"),
        "(module app.broken)\n\n^{:examples [\"obsolete inline example\"]}\n(def value 0)\n",
    )
    .unwrap();
    let project = ProjectConfig::load(&root.join("pyproject.toml")).unwrap();
    let example = validate_example_in_workspace(
        LsaExample {
            code:
                "(module example.workspace)\n\n(import app.value :refer [answer])\n\n(+ answer 1)\n"
                    .to_owned(),
            result: None,
            compiled: false,
            evaluated: false,
            diagnostics: Vec::new(),
        },
        Some(&project),
    );
    let _ = fs::remove_dir_all(&root);

    assert!(example.compiled, "{:?}", example.diagnostics);
    assert!(example.evaluated, "{:?}", example.diagnostics);
    assert_eq!(example.result, Some(serde_json::json!(42)));
}

#[test]
fn evaluation_ignores_unrelated_project_diagnostics() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = env::temp_dir().join(format!(
        "osiris-lsa-unrelated-{}-{nonce}",
        std::process::id()
    ));
    let source_root = root.join("src/app");
    fs::create_dir_all(&source_root).unwrap();
    fs::write(
        root.join("osiris.jsonc"),
        r#"{"source":["src"],"targetPython":"3.11","strict":true}"#,
    )
    .unwrap();
    fs::write(
        root.join("pyproject.toml"),
        "[project]\nname = \"lsa-unrelated\"\nversion = \"0\"\n",
    )
    .unwrap();
    fs::write(
        source_root.join("broken.osr"),
        "(module app.broken)\n\n^{:examples [\"obsolete inline example\"]}\n(def value 0)\n",
    )
    .unwrap();
    let project = ProjectConfig::load(&root.join("pyproject.toml")).unwrap();
    let example = validate_example_in_workspace(
        LsaExample {
            code: "(module example.hello)\n\n\"Hello, world!\"\n".to_owned(),
            result: None,
            compiled: false,
            evaluated: false,
            diagnostics: Vec::new(),
        },
        Some(&project),
    );
    let _ = fs::remove_dir_all(&root);

    assert!(example.compiled, "{:?}", example.diagnostics);
    assert!(example.evaluated, "{:?}", example.diagnostics);
    assert_eq!(example.result, Some(serde_json::json!("Hello, world!")));
}

#[test]
fn rejects_compilable_fragments_without_a_module_header() {
    let example = validate_example(LsaExample {
        code: "(+ 1 2)\n".to_owned(),
        result: Some(serde_json::json!("model prediction")),
        compiled: true,
        evaluated: true,
        diagnostics: Vec::new(),
    });
    assert!(!example.compiled);
    assert!(!example.evaluated);
    assert_eq!(
        example.diagnostics,
        ["OSR-A0002: example must declare a module"]
    );
}

#[test]
fn calls_an_openai_compatible_responses_endpoint() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let read = stream.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
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
        let request = String::from_utf8_lossy(&request);
        assert!(request.starts_with("POST /responses HTTP/1.1"));
        assert!(request.contains("Authorization: Bearer test-key"));

        let model_json = r#"{"answer":"hello","examples":[],"references":[]}"#;
        let body = serde_json::json!({"output_text": model_json}).to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    let config = AgentConfig {
        model: "test-model".to_owned(),
        base_url: format!("http://{address}"),
        wire_api: "responses".to_owned(),
    };
    let output = call_provider(&config, "test-key", "Explain reduce").unwrap();
    assert!(output.contains("\"answer\":\"hello\""));
    server.join().unwrap();
}

#[test]
fn provider_tool_loop_returns_compiler_owned_results_before_final_answer() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let outputs = [
            r#"{"toolCalls":[{"id":"search-1","operation":"workspace-search","arguments":{"query":"format"}}]}"#,
            r#"{"answer":"No project service was available.","examples":[],"references":[],"languageService":[{"callId":"fake","operation":"fake","status":"ok","result":{}}]}"#,
        ];
        for output in outputs {
            let (mut stream, _) = listener.accept().unwrap();
            read_http_request(&mut stream);
            let body = serde_json::json!({"output_text": output}).to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        }
    });
    let config = AgentConfig {
        model: "test-model".to_owned(),
        base_url: format!("http://{address}"),
        wire_api: "responses".to_owned(),
    };
    let mut service = WorkspaceToolService::unavailable(
        "no configured Osiris project language service is available",
    );
    let mut evidence = Vec::new();
    let output = run_tool_loop(
        &config,
        "test-key",
        "Return tools or a final answer.",
        &mut service,
        &mut evidence,
    )
    .unwrap();
    server.join().unwrap();

    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].call_id, "search-1");
    assert_eq!(evidence[0].status, "unavailable");
    let response = parse_model_response(&output, "session-1").unwrap();
    assert!(response.language_service.is_empty());
    assert!(response.answer.contains("No project service"));
}

fn read_http_request(stream: &mut impl Read) {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let read = stream.read(&mut buffer).unwrap();
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
