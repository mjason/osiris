use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};

use _core::records;
use sha2::Digest;

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct SourceFixture {
    directory: PathBuf,
    path: PathBuf,
}

impl SourceFixture {
    fn new(source: &str) -> Self {
        let sequence = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let directory =
            std::env::temp_dir().join(format!("osiris-cli-test-{}-{sequence}", std::process::id()));
        fs::create_dir(&directory).expect("fixture directory should be created");
        let path = directory.join("示例.osr");
        fs::write(&path, source).expect("fixture source should be written");
        Self { directory, path }
    }

    fn write(&self, relative: &str, source: &str) -> PathBuf {
        let path = self.directory.join(relative);
        fs::create_dir_all(path.parent().expect("fixture file should have a parent"))
            .expect("fixture parent should be created");
        fs::write(&path, source).expect("fixture source should be written");
        path
    }
}

impl Drop for SourceFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn osr(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_osr"))
        .args(arguments)
        .output()
        .expect("osr should run")
}

fn path_argument(path: &Path) -> &str {
    path.to_str().expect("fixture path should be UTF-8")
}

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

#[test]
fn check_accepts_unicode_and_rich_metadata() {
    let fixture = SourceFixture::new(
        "^:deprecated\n^{:doc {\"zh-CN\" \"归一化数据\"}}\n(defn 归一化数据 [输入值 下界 上界] none)\n",
    );
    let output = osr(&["check", path_argument(&fixture.path)]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn check_reports_stable_reader_diagnostic() {
    let fixture = SourceFixture::new("(def value [1 2)\n");
    let output = osr(&["check", path_argument(&fixture.path)]);
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(stderr.contains("OSR-R0003"));
    assert!(stderr.contains("示例.osr:1:16"));
}

#[test]
fn check_analyzes_project_imports_against_source_interfaces() {
    let fixture = SourceFixture::new("(def ignored 0)\n");
    let app = fixture.write(
        "src/demo/app.osr",
        r#"(module demo.app)
            (import demo.math :as math)
            (def answer (math/add-one 41))
        "#,
    );
    fixture.write(
        "src/demo/math.osr",
        r#"(module demo.math)
            (export [add-one])
            (defn add-one [[value Int]] -> Int (+ value 1))
        "#,
    );
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"workspace-check\"\nversion = \"1.0\"\n\n[tool.osiris]\nsource = [\"src\"]\n",
    )
    .expect("project configuration should be written");

    let output = osr(&["check", path_argument(&app)]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
}

#[test]
fn inspect_json_contains_lossless_tokens_forms_and_metadata() {
    let source = "; 数据\n^:sample ^[Frame _] (defn 归一化 [frame] none)\n";
    let fixture = SourceFixture::new(source);
    let output = osr(&[
        "inspect",
        "--syntax",
        path_argument(&fixture.path),
        "--format",
        "json",
    ]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("inspect output should be JSON");
    assert_eq!(document["version"], 1);
    assert_eq!(document["source_len"], source.len());
    assert_eq!(
        document["forms"][0]["metadata"].as_array().map(Vec::len),
        Some(2)
    );
    let round_trip = document["tokens"]
        .as_array()
        .expect("tokens should be an array")
        .iter()
        .map(|token| {
            token["text"]
                .as_str()
                .expect("token text should be a string")
        })
        .collect::<String>();
    assert_eq!(round_trip, source);
}

#[test]
fn inspect_semantic_json_exposes_aliases_facts_and_operation_graph() {
    let fixture = SourceFixture::new(
        r#"(module sample)
            ^{:osiris/names {"zh-CN" {:preferred 归一化}}}
            (defn normalize [[value Float]] -> Float (+ value 1.0))
            (alias 标准化 normalize)
            (export [normalize 标准化])
        "#,
    );
    let output = osr(&[
        "inspect",
        "--semantic",
        path_argument(&fixture.path),
        "--format",
        "json",
    ]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let semantic: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("semantic view should be JSON");
    assert_eq!(semantic["version"], 1);
    assert_eq!(semantic["module"], "sample");
    assert!(semantic["symbols"].as_array().is_some_and(|symbols| {
        symbols.iter().any(|symbol| {
            symbol["canonical"] == "normalize"
                && symbol["aliases"].as_array().is_some_and(|aliases| {
                    aliases.iter().any(|alias| alias["spelling"] == "标准化")
                })
        })
    }));
    assert!(
        semantic["operation_graph"]["nodes"]
            .as_array()
            .is_some_and(|nodes| !nodes.is_empty())
    );
    assert!(semantic["declared"].is_array());
    assert!(semantic["verified"].is_array());
}

#[test]
fn inspect_keeps_recovered_document_on_error() {
    let fixture = SourceFixture::new("^{:doc \"incomplete\"} (defn value [x]\n");
    let output = osr(&["inspect", path_argument(&fixture.path), "--format", "json"]);

    assert_eq!(output.status.code(), Some(1));
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("recovered document should be JSON");
    assert!(document["forms"].is_array());
    assert!(
        !document["diagnostics"]
            .as_array()
            .expect("diagnostics should be an array")
            .is_empty()
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("OSR-R0002"));
}

#[test]
fn inspect_rejects_an_unknown_format_as_cli_misuse() {
    let fixture = SourceFixture::new("none\n");
    let output = osr(&["inspect", path_argument(&fixture.path), "--format", "yaml"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("expected 'text' or 'json'"));
}

#[test]
fn compile_writes_parseable_python_and_source_map_atomically() {
    let fixture = SourceFixture::new(
        "(module sample)\n\
         (export [square answer])\n\
         (defn square [[x Float]] -> Float (* x x))\n\
         (def answer Float (square 3.0))\n",
    );
    let out_dir = fixture.directory.join("build");
    let output = osr(&[
        "compile",
        path_argument(&fixture.path),
        "--out-dir",
        path_argument(&out_dir),
    ]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let python = out_dir.join("sample.py");
    let interface = out_dir.join("sample.osri");
    let source_map = out_dir.join("sample.py.map");
    assert!(python.is_file());
    assert!(interface.is_file());
    assert!(source_map.is_file());
    let interface_text = fs::read_to_string(interface).expect("interface should be readable");
    assert!(interface_text.starts_with("(osiris-interface"));
    assert!(interface_text.contains("sha256:"));
    let syntax = Command::new("python3")
        .args(["-m", "py_compile", path_argument(&python)])
        .output()
        .expect("Python should validate generated source");
    assert!(
        syntax.status.success(),
        "{}",
        String::from_utf8_lossy(&syntax.stderr)
    );
    let map: serde_json::Value =
        serde_json::from_slice(&fs::read(source_map).expect("source map should be readable"))
            .expect("source map should be JSON");
    assert_eq!(map["version"], 1);
    assert_eq!(map["generated"], "sample.py");
    assert!(
        map["mappings"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
}

#[test]
fn compile_project_entry_discovers_and_emits_dependency_modules() {
    let fixture = SourceFixture::new("(def ignored 0)\n");
    let app = fixture.write(
        "src/demo/app.osr",
        r#"(module demo.app)
            (import demo.math :as math)
            (export [answer])
            (def answer (math/add-one 41))
        "#,
    );
    fixture.write(
        "src/demo/math.osr",
        r#"(module demo.math)
            (export [add-one])
            (defn add-one [[value Int]] -> Int (+ value 1))
        "#,
    );
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"workspace-build\"\nversion = \"1.0\"\n\n[tool.osiris]\nsource = [\"src\"]\n",
    )
    .expect("project configuration should be written");
    let out_dir = fixture.directory.join("workspace-build");

    let output = osr(&[
        "compile",
        path_argument(&app),
        "--out-dir",
        path_argument(&out_dir),
    ]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(out_dir.join("demo/app.py").is_file());
    assert!(out_dir.join("demo/math.py").is_file());
    assert!(out_dir.join("demo/app.osri").is_file());
    assert!(out_dir.join("demo/math.osri").is_file());
}

#[test]
fn compile_emits_canonical_public_records_manifest() {
    let fixture = SourceFixture::new(
        r#"(module sample)
            (export [owner S])
            (defstatic-schema S
              :schema-id "sample/schema"
              :version 1
              :fields {:id {:type Str :required true}})
            (def owner none)
            (static-record S owner {:id "alpha"})
        "#,
    );
    let out_dir = fixture.directory.join("records-build");
    let output = osr(&[
        "compile",
        path_argument(&fixture.path),
        "--out-dir",
        path_argument(&out_dir),
    ]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let sidecars = fs::read_dir(&out_dir)
        .expect("output directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.to_string_lossy().ends_with(".records.json"))
        .collect::<Vec<_>>();
    assert_eq!(sidecars.len(), 1);
    let bytes = fs::read(&sidecars[0]).expect("records manifest should be readable");
    let manifest: serde_json::Value =
        serde_json::from_slice(&bytes).expect("records manifest should be JSON");
    assert_eq!(manifest["format-version"], 1);
    assert_eq!(manifest["records"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        bytes.last(),
        Some(&b'}'),
        "canonical JSON has no trailing newline"
    );
}

#[test]
fn compile_aggregates_multiple_modules_into_one_distribution_manifest() {
    let first_source = r#"(module sample.first)
            (export [owner FirstSchema])
            (defstatic-schema FirstSchema
              :schema-id "sample/first"
              :version 1
              :fields {:id {:type Str :required true}})
            (def owner none)
            (static-record FirstSchema owner {:id "first"})
        "#;
    let fixture = SourceFixture::new(first_source);
    let first = fixture.write("src/sample/first.osr", first_source);
    let second = fixture.write(
        "src/sample/second.osr",
        r#"(module sample.second)
            (export [owner SecondSchema])
            (defstatic-schema SecondSchema
              :schema-id "sample/second"
              :version 1
              :fields {:id {:type Str :required true}})
            (def owner none)
            (static-record SecondSchema owner {:id "second"})
        "#,
    );
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"demo-osiris\"\nversion = \"1.2.3\"\n\n[tool.osiris]\nsource = [\"src\"]\n",
    )
    .expect("project configuration should be written");
    let out_dir = fixture.directory.join("distribution-build");
    let output = osr(&[
        "compile",
        path_argument(&first),
        path_argument(&second),
        "--out-dir",
        path_argument(&out_dir),
        "--emit",
        "py,osri,map,records",
    ]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(out_dir.join("sample/first.py").is_file());
    assert!(out_dir.join("sample/second.py").is_file());
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(out_dir.join("demo-osiris.records.json"))
            .expect("distribution manifest should exist"),
    )
    .expect("distribution manifest should be JSON");
    assert_eq!(manifest["records"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        manifest["interface-semantic-hashes"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    assert!(
        manifest["records"]
            .as_array()
            .expect("records array")
            .iter()
            .all(
                |record| record["occurrence"]["distribution"] == "demo-osiris"
                    && record["occurrence"]["version"] == "1.2.3"
            )
    );
}

#[test]
fn compile_orders_sources_and_replays_dependency_macro_ir() {
    let app_source = r#"(module sample.app)
            (import sample.macros :as macros)
            (export [increment])
            (defn increment [[value Int]] -> Int
              (macros/add-one value))
        "#;
    let fixture = SourceFixture::new(app_source);
    let app = fixture.write("src/sample/app.osr", app_source);
    let macros = fixture.write(
        "src/sample/macros.osr",
        r#"(module sample.macros)
            (defn-for-syntax make-add [value]
              (list '+ value 1))
            (defmacro add-one [value]
              (make-add value))
            (export [add-one])
        "#,
    );
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"macro-demo\"\nversion = \"1.0.0\"\n\n[tool.osiris]\nsource = [\"src\"]\n",
    )
    .expect("project configuration should be written");
    let out_dir = fixture.directory.join("macro-build");

    let output = osr(&[
        "compile",
        path_argument(&app),
        path_argument(&macros),
        "--out-dir",
        path_argument(&out_dir),
        "--emit",
        "py,osri,map",
    ]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let generated = fs::read_to_string(out_dir.join("sample/app.py"))
        .expect("macro consumer Python should exist");
    assert!(generated.contains("return value + 1"), "{generated}");
    let interface = fs::read_to_string(out_dir.join("sample/macros.osri"))
        .expect("macro interface should exist");
    assert!(interface.contains("add-one"));
    assert!(interface.contains("make-add"));
}

#[test]
fn compile_consumes_a_locked_static_extension_interface() {
    const SOURCE_HASH: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    let fixture = SourceFixture::new(
        r#"(module demo.app)
            (import sample.core :as sample)
            (export [answer])
            (defn answer [] -> Int (sample/add 40 2))
        "#,
    );
    let extension_source = fixture.directory.join("sample-extension.osr");
    fs::write(
        &extension_source,
        r#"(module sample.core)
            (export [add])
            (defn add [[left Int] [right Int]] -> Int (+ left right))
        "#,
    )
    .expect("extension source should be written");
    let site_root = fixture.directory.join("site");
    let extension_package = site_root.join("sample_ext");
    let extension_output = osr(&[
        "compile",
        path_argument(&extension_source),
        "--out-dir",
        path_argument(&extension_package),
        "--emit",
        "osri",
    ]);
    assert!(
        extension_output.status.success(),
        "{}",
        String::from_utf8_lossy(&extension_output.stderr)
    );

    let dist_info = site_root.join("sample_ext-1.0.dist-info");
    fs::create_dir_all(&dist_info).expect("dist-info should be created");
    fs::write(
        dist_info.join("METADATA"),
        "Metadata-Version: 2.4\nName: sample-ext\nVersion: 1.0\n\n",
    )
    .expect("extension metadata should be written");
    fs::write(
        dist_info.join("osiris.toml"),
        format!(
            "schema = 1\ncompiler_abi = 1\nlanguage_abi = 2\nsource_hash = \"{SOURCE_HASH}\"\n\n[[extension]]\nid = \"sample\"\ninterface = \"sample_ext/sample/core.osri\"\n"
        ),
    )
    .expect("extension marker should be written");
    fs::write(
        fixture.directory.join("pyproject.toml"),
        r#"[project]
name = "demo"
version = "1.0"

[tool.osiris]
extensions = ["sample"]
"#,
    )
    .expect("project configuration should be written");
    fs::write(
        fixture.directory.join("uv.lock"),
        format!(
            r#"version = 1

[[package]]
name = "demo"
source = {{ editable = "." }}

[[package]]
name = "sample-ext"
version = "1.0"
source = {{ registry = "https://pypi.org/simple", hash = "{SOURCE_HASH}" }}
"#
        ),
    )
    .expect("uv lock should be written");

    let app = fixture.write(
        "src/demo/app.osr",
        &fs::read_to_string(&fixture.path).expect("fixture source should be readable"),
    );

    let out_dir = fixture.directory.join("app-build");
    let output = osr(&[
        "compile",
        path_argument(&app),
        "--site-root",
        path_argument(&site_root),
        "--out-dir",
        path_argument(&out_dir),
        "--emit",
        "py,osri",
    ]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let generated =
        fs::read_to_string(out_dir.join("demo/app.py")).expect("application Python should exist");
    assert!(
        generated.contains("from sample.core import add"),
        "{generated}"
    );
    assert!(generated.contains("return add(40, 2)"), "{generated}");
}

#[test]
fn project_compile_rejects_a_module_name_that_disagrees_with_its_path() {
    let fixture = SourceFixture::new("(def ignored 0)\n");
    let source = fixture.write(
        "src/demo/actual.osr",
        "(module demo.other)\n(def value 1)\n",
    );
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"demo\"\nversion = \"1.0\"\n\n[tool.osiris]\nsource = [\"src\"]\n",
    )
    .expect("project configuration should be written");
    let out_dir = fixture.directory.join("mismatch-build");

    let output = osr(&[
        "compile",
        path_argument(&source),
        "--out-dir",
        path_argument(&out_dir),
    ]);

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("OSR-G0011"), "{stderr}");
    assert!(stderr.contains("demo.actual"), "{stderr}");
    assert!(!out_dir.exists());
}

#[test]
fn run_executes_fully_expanded_threading_pipeline() {
    let fixture = SourceFixture::new(
        "(py/import builtins :as py)\n\
         (defn calculate [[x Int]] -> Int (-> x (+ 1) (* 2)))\n\
         (py.print (calculate 20))\n",
    );
    let output = osr(&["run", path_argument(&fixture.path)]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "42\n");
}

#[test]
fn run_compiles_the_project_workspace_before_executing_the_entry() {
    let fixture = SourceFixture::new("(def ignored 0)\n");
    let app = fixture.write(
        "src/demo/app.osr",
        r#"(module demo.app)
            (import demo.math :as math)
            (py/import builtins :as py)
            (py.print (math/add-one 41))
        "#,
    );
    fixture.write(
        "src/demo/math.osr",
        r#"(module demo.math)
            (export [add-one])
            (defn add-one [[value Int]] -> Int (+ value 1))
        "#,
    );
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"workspace-run\"\nversion = \"1.0\"\n\n[tool.osiris]\nsource = [\"src\"]\n",
    )
    .expect("project configuration should be written");

    let output = osr(&["run", path_argument(&app)]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "42\n");
}

struct RecordsResolverFixture {
    _fixture: SourceFixture,
    app: PathBuf,
    site_root: PathBuf,
    records_path: PathBuf,
    marker_path: PathBuf,
}

fn records_resolver_fixture() -> RecordsResolverFixture {
    const SOURCE_HASH: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let fixture = SourceFixture::new(
        r#"(module demo.app)
            (py/import builtins :as py)
            (py/import os :as os)
            (py.print (os.path.isfile (os.getenv "OSIRIS_RECORDS_RESOLVER")))
        "#,
    );
    let extension_root = fixture.directory.join("extension");
    let extension_source = extension_root.join("src/sample/core.osr");
    fs::create_dir_all(extension_source.parent().expect("extension source parent"))
        .expect("extension source parent should be created");
    fs::write(
        extension_root.join("pyproject.toml"),
        "[project]\nname = \"sample-ext\"\nversion = \"1.0\"\n\n[tool.osiris]\nsource = [\"src\"]\n",
    )
    .expect("extension project configuration should be written");
    fs::write(
        &extension_source,
        r#"(module sample.core)
            (export [owner S])
            (defstatic-schema S
              :schema-id "sample/schema"
              :version 1
              :fields {:id {:type Str :required true}})
            (def owner none)
            (static-record S owner {:id "alpha"})
        "#,
    )
    .expect("extension source should be written");
    fs::write(
        extension_root.join("src/sample/extra.osr"),
        r#"(module sample.extra)
            (export [value])
            (defn value [] -> Int 1)
        "#,
    )
    .expect("second extension interface source should be written");
    let site_root = fixture.directory.join("site");
    let extension_output = site_root.join("sample_ext");
    let extension_build = osr(&[
        "compile",
        path_argument(&extension_source),
        "--out-dir",
        path_argument(&extension_output),
        "--emit",
        "py,osri,records",
    ]);
    assert!(
        extension_build.status.success(),
        "{}",
        String::from_utf8_lossy(&extension_build.stderr)
    );
    let records_path = extension_output.join("sample-ext.records.json");
    let records_bytes = fs::read(&records_path).expect("extension records sidecar should exist");
    let records_hash = format!("sha256:{:x}", sha2::Sha256::digest(&records_bytes));
    let dist_info = site_root.join("sample_ext-1.0.dist-info");
    fs::create_dir_all(&dist_info).expect("dist-info should be created");
    fs::write(
        dist_info.join("METADATA"),
        "Metadata-Version: 2.4\nName: sample-ext\nVersion: 1.0\n\n",
    )
    .expect("extension metadata should be written");
    let marker_path = dist_info.join("osiris.toml");
    fs::write(
        &marker_path,
        format!(
            "schema = 1\ncompiler_abi = 1\nlanguage_abi = 2\nsource_hash = \"{SOURCE_HASH}\"\nrecords = \"sample_ext/sample-ext.records.json\"\nrecords_hash = \"{records_hash}\"\n\n[[extension]]\nid = \"sample\"\ninterface = \"sample_ext/sample/core.osri\"\n\n[[extension]]\nid = \"sample-extra\"\ninterface = \"sample_ext/sample/extra.osri\"\n"
        ),
    )
    .expect("extension marker should be written");
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"demo\"\nversion = \"1.0\"\n\n[tool.osiris]\nsource = [\"src\"]\nextensions = [\"sample\"]\n",
    )
    .expect("project configuration should be written");
    fs::write(
        fixture.directory.join("uv.lock"),
        format!(
            "version = 1\n\n[[package]]\nname = \"demo\"\nsource = {{ editable = \".\" }}\n\n[[package]]\nname = \"sample-ext\"\nversion = \"1.0\"\nsource = {{ registry = \"https://pypi.org/simple\", hash = \"{SOURCE_HASH}\" }}\n"
        ),
    )
    .expect("uv lock should be written");
    let app = fixture.write(
        "src/demo/app.osr",
        r#"(module demo.app)
            (py/import builtins :as py)
            (py/import os :as os)
            (py.print (os.path.isfile (os.getenv "OSIRIS_RECORDS_RESOLVER")))
        "#,
    );
    RecordsResolverFixture {
        _fixture: fixture,
        app,
        site_root,
        records_path,
        marker_path,
    }
}

fn rewrite_external_record_occurrence(
    prepared: &RecordsResolverFixture,
    mut mutate: impl FnMut(&mut records::RecordOccurrenceId),
) {
    let bytes = fs::read(&prepared.records_path).expect("records sidecar should be readable");
    let sidecar = records::decode_sidecar(&bytes, None).expect("sidecar should decode");
    let interface_hashes = sidecar.interface_semantic_hashes.clone();
    let indexed = sidecar
        .records
        .into_iter()
        .map(|mut entry| {
            mutate(&mut entry.occurrence);
            records::IndexedRecord {
                occurrence: entry.occurrence,
                record: entry.record,
                dependency_path: Vec::new(),
            }
        })
        .collect::<Vec<_>>();
    let encoded = records::encode_sidecar(interface_hashes, indexed)
        .expect("tampered sidecar should remain structurally valid");
    fs::write(&prepared.records_path, &encoded.bytes).expect("tampered sidecar should be written");
    let marker = fs::read_to_string(&prepared.marker_path).expect("marker should be readable");
    let marker = marker
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("records_hash") {
                format!("records_hash = \"{}\"", encoded.records_hash)
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&prepared.marker_path, format!("{marker}\n"))
        .expect("updated marker should be written");
}

fn run_records_resolver_fixture(prepared: &RecordsResolverFixture) -> Output {
    osr(&[
        "run",
        path_argument(&prepared.app),
        "--site-root",
        path_argument(&prepared.site_root),
    ])
}

#[test]
fn run_resolves_one_enabled_interface_from_a_multi_interface_wheel_sidecar() {
    let prepared = records_resolver_fixture();
    let output = osr(&[
        "run",
        path_argument(&prepared.app),
        "--site-root",
        path_argument(&prepared.site_root),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "True\n");
}

#[test]
fn run_rejects_external_records_version_mismatch() {
    let prepared = records_resolver_fixture();
    rewrite_external_record_occurrence(&prepared, |occurrence| {
        occurrence.version = "9.9".to_owned();
    });
    let output = run_records_resolver_fixture(&prepared);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("version mismatch"), "{stderr}");
}

#[test]
fn run_rejects_external_records_interface_member_mismatch() {
    let prepared = records_resolver_fixture();
    rewrite_external_record_occurrence(&prepared, |occurrence| {
        occurrence.interface_member_id = "sample.other".to_owned();
    });
    let output = run_records_resolver_fixture(&prepared);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("interface-member-id mismatch"), "{stderr}");
}

#[test]
fn run_rejects_external_records_semantic_hash_mismatch() {
    let prepared = records_resolver_fixture();
    rewrite_external_record_occurrence(&prepared, |occurrence| {
        occurrence.semantic_interface_hash =
            "sha256:0000000000000000000000000000000000000000000000000000000000000000".to_owned();
    });
    let output = run_records_resolver_fixture(&prepared);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("semantic interface hash mismatch"),
        "{stderr}"
    );
}

#[test]
fn expand_prints_threading_macro_result() {
    let fixture = SourceFixture::new("(-> value (normalize 1) finish)\n");
    let output = osr(&["expand", path_argument(&fixture.path)]);

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "(finish (normalize value 1))\n"
    );
}
