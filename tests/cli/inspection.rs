use super::*;

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
