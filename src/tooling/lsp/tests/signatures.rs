#[test]
fn signature_help_uses_cross_module_types_aliases_and_default_presence() {
    let sequence = NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "osiris-lsp-signature-workspace-{}-{sequence}",
        std::process::id()
    ));
    let source_root = root.join("src/demo");
    fs::create_dir_all(&source_root).expect("source root");
    fs::write(
            root.join("pyproject.toml"),
            "[project]\nname = \"lsp-signature\"\nversion = \"1.0\"\n\n[tool.osiris]\nsource = [\"src\"]\n",
        )
        .expect("project configuration");
    fs::write(
        source_root.join("math.osr"),
        r#"(module demo.math)
(export [rolling])
(defn rolling
  [[values Float]
   ^{:osiris/names {"zh-CN" {:preferred 周期}}} [window Int = 14]]
  -> Float
  values)
"#,
    )
    .expect("dependency source");
    let app_source = r#"(module demo.app)
(import demo.math :as math)
(def answer (math/rolling 1.0 ))
"#;
    let app = source_root.join("app.osr");
    fs::write(&app, app_source).expect("application source");
    let uri = format!("file://{}", app.display());
    let mut state = LspState::new();
    let diagnostics = state.did_open(&uri, 1, app_source);
    assert!(diagnostics.diagnostics.is_empty(), "{diagnostics:?}");
    let cursor = app_source.find("(math/rolling 1.0 )").expect("call") + "(math/rolling 1.0 ".len();

    let signature = state
        .signature_help(&uri, offset_to_position(app_source, cursor), Some("zh-CN"))
        .expect("cross-module signature help");

    assert_eq!(signature.active_parameter, Some(1));
    assert_eq!(
        signature.signatures[0].label,
        "math/rolling(values: Float, 周期: Int = ...) -> Float"
    );
    assert_eq!(
        signature.signatures[0].parameters[1].label,
        "周期: Int = ..."
    );
    drop(state);
    fs::remove_dir_all(root).expect("workspace cleanup");
}

#[test]
fn macro_signature_help_uses_stable_identity_for_qualified_referred_and_local_macros() {
    let sequence = NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "osiris-lsp-macro-signature-workspace-{}-{sequence}",
        std::process::id()
    ));
    let source_root = root.join("src/demo");
    fs::create_dir_all(&source_root).expect("source root");
    fs::write(
            root.join("pyproject.toml"),
            "[project]\nname = \"lsp-macro-signature\"\nversion = \"1.0\"\n\n[tool.osiris]\nsource = [\"src\"]\n",
        )
        .expect("project configuration");
    fs::write(
        source_root.join("first.osr"),
        "(module demo.first)\n(export [wrap])\n(defmacro wrap [第一值 & 其余] 第一值)\n",
    )
    .expect("first macro dependency");
    fs::write(
        source_root.join("second.osr"),
        "(module demo.second)\n(export [wrap])\n(defmacro wrap [第二值] 第二值)\n",
    )
    .expect("second macro dependency");
    let app_source = r#"(module demo.app)
(import-for-syntax demo.first :as first)
(import-for-syntax demo.second :refer [wrap])
(defmacro local-wrap [本地值] 本地值)
(def first-result (first/wrap 1 2))
(def second-result (wrap 2))
(def local-result (local-wrap 3))
"#;
    let app = source_root.join("app.osr");
    fs::write(&app, app_source).expect("application source");
    let uri = format!("file://{}", app.display());
    let mut state = LspState::new();
    let diagnostics = state.did_open(&uri, 1, app_source);
    assert!(diagnostics.diagnostics.is_empty(), "{diagnostics:?}");

    let qualified_position = offset_to_position(
        app_source,
        app_source.find("(first/wrap 1 2)").expect("qualified call") + "(first/wrap 1 ".len(),
    );
    let referred_position = offset_to_position(
        app_source,
        app_source.find("(wrap 2)").expect("referred call") + "(wrap ".len(),
    );
    let local_position = offset_to_position(
        app_source,
        app_source.find("(local-wrap 3)").expect("local call") + "(local-wrap ".len(),
    );

    let qualified = state
        .signature_help(&uri, qualified_position, Some("zh-CN"))
        .expect("qualified macro signature");
    let referred = state
        .signature_help(&uri, referred_position, Some("zh-CN"))
        .expect("referred macro signature");
    let local = state
        .signature_help(&uri, local_position, Some("zh-CN"))
        .expect("local macro signature");

    assert_eq!(qualified.signatures[0].label, "first/wrap(第一值, & 其余)");
    assert_eq!(qualified.active_parameter, Some(1));
    assert_eq!(referred.signatures[0].label, "wrap(第二值)");
    assert_eq!(local.signatures[0].label, "local-wrap(本地值)");
    let trace_ids = state
        .document(&uri)
        .expect("open app")
        .semantic
        .macro_traces
        .iter()
        .map(|trace| trace.macro_binding_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        trace_ids,
        [
            "demo.first::macro::wrap",
            "demo.second::macro::wrap",
            "demo.app::macro::local-wrap"
        ]
    );
    drop(state);
    fs::remove_dir_all(root).expect("workspace cleanup");
}
