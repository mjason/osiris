use super::*;

#[test]
fn check_accepts_unicode_and_rich_metadata() {
    let fixture = SourceFixture::new(
        "^:deprecated\n^{:doc {:default \"Normalize data.\" \"zh-CN\" \"归一化数据\"}}\n(defn 归一化数据 [输入值 下界 上界] none)\n",
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
fn check_reports_migration_aliases_without_failing() {
    let fixture = SourceFixture::new("(def outside 0)\n");
    let source = fixture.write(
        "src/app.osr",
        r#"(module app)
^{:osiris/names {"zh-CN" {:preferred 格式化文本 :aliases [渲染文本]}}}
(defn ^Str format-message [^Str value] value)
(def result (渲染文本 "hello"))
"#,
    );
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"alias-check\"\nversion = \"1.0\"\n",
    )
    .unwrap();
    fs::write(
        fixture.directory.join("osiris.jsonc"),
        r#"{"source":["src"],"displayLocale":"zh-CN"}"#,
    )
    .unwrap();

    let output = osr(&["check", path_argument(&source)]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("warning[OSR-L0002]"), "{stderr}");
    assert!(stderr.contains("`渲染文本`"), "{stderr}");
    assert!(stderr.contains("`格式化文本`"), "{stderr}");

    let lsc = osr(&[
        "lsc",
        "diagnostics",
        path_argument(&source),
        "--locale",
        "zh-CN",
        "--format",
        "json",
    ]);
    assert!(
        lsc.status.success(),
        "{}",
        String::from_utf8_lossy(&lsc.stderr)
    );
    let lsc: serde_json::Value = serde_json::from_slice(&lsc.stdout).unwrap();
    let advisory = lsc["result"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|diagnostic| diagnostic["code"] == "OSR-L0002")
        .expect("LSC migration advisory");
    assert_eq!(advisory["data"]["replacement"], "格式化文本");

    let hover = Command::new(env!("CARGO_BIN_EXE_osr"))
        .args([
            "lsc",
            "hover",
            "渲染文本",
            "--locale",
            "zh-CN",
            "--format",
            "json",
        ])
        .current_dir(&fixture.directory)
        .output()
        .unwrap();
    assert!(
        hover.status.success(),
        "{}",
        String::from_utf8_lossy(&hover.stderr)
    );
    let hover: serde_json::Value = serde_json::from_slice(&hover.stdout).unwrap();
    assert_eq!(
        hover["result"]["bindingId"],
        "app::function::format-message"
    );
    assert_eq!(hover["result"]["queriedSpelling"]["role"], "migration");
    assert_eq!(
        hover["result"]["queriedSpelling"]["replacement"],
        "格式化文本"
    );
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
fn check_renders_the_macro_chain_behind_a_diagnostic() {
    // OEP-0001-R032A/R032C: the only authored call is `(outer n)`, so without
    // the chain the reported line belongs to a macro the author never called.
    let fixture = SourceFixture::new(
        "(defmacro inner [value] `(no-such-fn ~value))\n\
         (defmacro outer [value] `(inner ~value))\n\
         (defn ^Int demo [^Int n] (outer n))\n",
    );
    let output = osr(&["check", path_argument(&fixture.path)]);
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert_eq!(output.status.code(), Some(1));
    assert!(stderr.contains("OSR-N0012"), "{stderr}");
    assert!(
        stderr.contains("= note: expanded from macro `outer` called here"),
        "{stderr}"
    );
    assert!(
        stderr.contains("= note: expanded from macro `inner` called here"),
        "{stderr}"
    );
    assert!(
        stderr.contains("= note: macro `inner` is defined here"),
        "{stderr}"
    );
    // The call-site note must point at the authored call on line 3.
    assert!(stderr.contains("示例.osr:3:26"), "{stderr}");
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
            ^{:doc "Increment an integer."}
            (defn ^Int add-one [^Int value] (+ value 1))
        "#,
    );
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"workspace-check\"\nversion = \"1.0\"\n",
    )
    .expect("project configuration should be written");
    fs::write(
        fixture.directory.join("osiris.jsonc"),
        r#"{"source":["src"]}"#,
    )
    .expect("Osiris configuration should be written");

    let output = osr(&["check", path_argument(&app)]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
}

#[test]
fn check_defaults_to_the_current_project_and_accepts_a_project_directory() {
    let fixture = SourceFixture::new("(def outside-source-root 0)\n");
    fixture.write("src/main.osr", "(module main)\n(def answer 42)\n");
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"project-check\"\nversion = \"1.0\"\n",
    )
    .unwrap();
    fs::write(
        fixture.directory.join("osiris.jsonc"),
        r#"{"source":["src"]}"#,
    )
    .unwrap();

    let directory = osr(&["check", path_argument(&fixture.directory)]);
    assert!(
        directory.status.success(),
        "{}",
        String::from_utf8_lossy(&directory.stderr)
    );

    let current = Command::new(env!("CARGO_BIN_EXE_osr"))
        .arg("check")
        .current_dir(&fixture.directory)
        .output()
        .unwrap();
    assert!(
        current.status.success(),
        "{}",
        String::from_utf8_lossy(&current.stderr)
    );
    assert!(!fixture.directory.join("dist").exists());
}

#[test]
fn lsc_syntax_json_contains_lossless_tokens_forms_and_metadata() {
    let source = "; 数据\n^:sample ^[Frame _] (defn 归一化 [frame] none)\n";
    let fixture = SourceFixture::new(source);
    let output = osr(&[
        "lsc",
        "syntax",
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
    let document = &document["result"];
    assert_eq!(document["version"], 1);
    assert_eq!(
        document["source"].as_str().map(str::len),
        Some(source.len())
    );
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
fn lsc_semantic_json_exposes_aliases_facts_and_operation_graph() {
    let fixture = SourceFixture::new(
        r#"(module sample)
            ^{:doc "Normalize a value."
              :osiris/names {"zh-CN" {:preferred 归一化}}}
            (defn ^Float normalize [^Float value] (+ value 1.0))
            (alias 标准化 normalize)
            (export [normalize 标准化])
        "#,
    );
    let output = osr(&[
        "lsc",
        "semantic",
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
    let semantic = &semantic["result"];
    assert_eq!(
        semantic["version"],
        osiris::semantic::SEMANTIC_DOCUMENT_VERSION
    );
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
fn lsc_position_hover_is_operation_scoped_and_preserves_default_language() {
    let fixture = SourceFixture::new(
        r#"(module sample)
^{:doc {:default "默认中文。" "en" "English documentation."}
  :osiris/names {"en" {:preferred calculate-value :aliases [compute-value]}}}
(defn calculate [value] value)
"#,
    );
    let at = format!("{}:4:7", fixture.path.display());
    let default = osr(&["lsc", "hover", "--at", &at, "--format", "json"]);
    assert!(
        default.status.success(),
        "{}",
        String::from_utf8_lossy(&default.stderr)
    );
    let default: serde_json::Value = serde_json::from_slice(&default.stdout).unwrap();
    let result = &default["result"];
    assert_eq!(result["schema"], "osiris.hover/v1");
    assert_eq!(result["bindingId"], "sample::function::calculate");
    assert_eq!(result["canonical"]["qualified"], "sample/calculate");
    assert_eq!(result["documentation"]["default"], "默认中文。");
    assert_eq!(
        result["documentation"]["translations"]["en"],
        "English documentation."
    );
    assert!(result["documentation"]["selection"]["requestedLocale"].is_null());
    assert!(result["documentation"]["selection"]["resolvedLocale"].is_null());
    assert_eq!(result["documentation"]["selection"]["text"], "默认中文。");
    assert_eq!(result["names"]["canonical"], "calculate");
    assert_eq!(result["names"]["selection"]["label"], "calculate");
    assert!(result.get("metadata").is_none());
    assert!(result["semantic"].is_object());

    let localized = osr(&[
        "lsc", "hover", "--at", &at, "--locale", "en-US", "--format", "json",
    ]);
    assert!(
        localized.status.success(),
        "{}",
        String::from_utf8_lossy(&localized.stderr)
    );
    let localized: serde_json::Value = serde_json::from_slice(&localized.stdout).unwrap();
    let result = &localized["result"];
    assert_eq!(
        result["documentation"]["selection"]["requestedLocale"],
        "en-US"
    );
    assert_eq!(result["documentation"]["selection"]["resolvedLocale"], "en");
    assert_eq!(result["names"]["selection"]["resolvedLocale"], "en");
    assert_eq!(
        result["documentation"]["selection"]["text"],
        "English documentation."
    );
    assert_eq!(result["label"], "calculate-value");
}

#[test]
fn lsc_standard_hover_uses_progressive_human_and_machine_projections() {
    let text = osr(&["lsc", "hover", "osiris.core/reduce", "--locale", "zh-CN"]);
    assert!(
        text.status.success(),
        "{}",
        String::from_utf8_lossy(&text.stderr)
    );
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.contains("reduce · 函数"), "{text}");
    assert!(text.contains("(reduce + 0 [1 2 3 4])"), "{text}");
    assert!(text.contains(";; => 10"), "{text}");
    assert!(text.contains("立即消费输入集合。"), "{text}");
    assert!(text.contains("osiris.core/reduce"), "{text}");
    for internal in ["Binding:", "Source:", "Evaluation:", "Any"] {
        assert!(!text.contains(internal), "{text}");
    }

    let json = osr(&["lsc", "hover", "osiris.core/reduce", "--format", "json"]);
    assert!(
        json.status.success(),
        "{}",
        String::from_utf8_lossy(&json.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    let reduce = &json["result"];
    assert_eq!(reduce["schema"], "osiris.hover/v1");
    assert_eq!(reduce["examples"][0][0], "(reduce + 0 [1 2 3 4])");
    assert_eq!(reduce["examples"][0][1], ";; => 10");
    assert_eq!(reduce["semantic"]["evaluation"], "consumer");
    assert_eq!(reduce["bindingId"], "osiris.core::function::reduce");
    assert_eq!(
        reduce["source"]["uri"],
        "osiris-stdlib:///osiris/core/transform.osr"
    );
    assert!(reduce["semantic"]["effects"].is_object());
}

#[test]
fn lsc_syntax_keeps_recovered_document_on_error() {
    let fixture = SourceFixture::new("^{:doc \"incomplete\"} (defn value [x]\n");
    let output = osr(&[
        "lsc",
        "syntax",
        path_argument(&fixture.path),
        "--format",
        "json",
    ]);

    assert_eq!(output.status.code(), Some(1));
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("recovered document should be JSON");
    let document = &document["result"];
    assert!(document["forms"].is_array());
    assert!(
        !document["diagnostics"]
            .as_array()
            .expect("diagnostics should be an array")
            .is_empty()
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn lsc_rejects_an_unknown_format_as_cli_misuse() {
    let fixture = SourceFixture::new("none\n");
    let output = osr(&[
        "lsc",
        "syntax",
        path_argument(&fixture.path),
        "--format",
        "yaml",
    ]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("must be 'text' or 'json'"));
}
