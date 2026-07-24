#[test]
fn project_document_uses_path_identity_and_dependency_interfaces() {
    let sequence = NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "osiris-lsp-workspace-{}-{sequence}",
        std::process::id()
    ));
    let source_root = root.join("src/demo");
    fs::create_dir_all(&source_root).expect("source root");
    fs::write(
            root.join("pyproject.toml"),
            "[project]\nname = \"lsp-workspace\"\nversion = \"1.0\"\n",
        )
        .expect("project configuration");
    fs::write(
        root.join("osiris.jsonc"),
        r#"{"source":["src"],"displayLocale":"zh-CN"}"#,
    )
        .expect("Osiris configuration");
    fs::write(
        source_root.join("math.osr"),
        "(module demo.math)\n(export [add-one])\n^{:doc \"Increment an integer.\"} (defn ^Int add-one [^Int x] (+ x 1))\n",
    )
    .expect("dependency source");
    let app_source =
        "(module demo.app)\n(import demo.math :as math)\n(def answer (math/add-one 41))\n";
    let app = source_root.join("app.osr");
    fs::write(&app, app_source).expect("application source");
    let uri = format!("file://{}", app.display());
    let mut state = LspState::new();

    let diagnostics = state.did_open(&uri, 1, app_source);

    assert!(diagnostics.diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(
        state
            .document(&uri)
            .expect("open document")
            .analysis
            .hir
            .name,
        "demo.app"
    );
    drop(state);
    fs::remove_dir_all(root).expect("workspace cleanup");
}

#[test]
fn workspace_navigation_uses_provider_locations_and_stable_binding_identity() {
    let sequence = NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "osiris-lsp-navigation-workspace-{}-{sequence}",
        std::process::id()
    ));
    let source_root = root.join("src/demo");
    fs::create_dir_all(&source_root).expect("source root");
    fs::write(
            root.join("pyproject.toml"),
            "[project]\nname = \"lsp-navigation\"\nversion = \"1.0\"\n",
        )
        .expect("project configuration");
    fs::write(
        root.join("osiris.jsonc"),
        r#"{"source":["src"],"displayLocale":"zh-CN"}"#,
    )
        .expect("Osiris configuration");
    let alpha_source = r#"(module demo.alpha)
(export [score 得分])
^{:doc "Return the alpha score."} (defn ^Int score [^Int value] value)
(alias 得分 score)
"#;
    let beta_source = r#"(module demo.beta)
(export [score])
^{:doc "Return the beta score."} (defn ^Int score [^Int value] value)
"#;
    let app_source = r#"(module demo.app)
(import demo.alpha :as alpha :refer [得分])
(import demo.beta :as beta)
(def alpha-result (alpha/score 1))
(def alias-result (得分 2))
(def beta-result (beta/score 3))
"#;
    let broken_source = r#"(module demo.broken)
(import demo.alpha :as alpha)
(def broken-result (alpha/score 4))
(defn ^Int invalid [^Int x])
"#;
    let alpha_path = source_root.join("alpha.osr");
    let beta_path = source_root.join("beta.osr");
    let app_path = source_root.join("app.osr");
    let broken_path = source_root.join("broken.osr");
    fs::write(&alpha_path, alpha_source).expect("alpha source");
    fs::write(&beta_path, beta_source).expect("beta source");
    fs::write(&app_path, app_source).expect("app source");
    fs::write(&broken_path, broken_source).expect("broken source");
    let alpha_uri = format!("file://{}", alpha_path.display());
    let beta_uri = format!("file://{}", beta_path.display());
    let app_uri = format!("file://{}", app_path.display());
    let broken_uri = format!("file://{}", broken_path.display());
    let mut state = LspState::new();

    let app_diagnostics = state.did_open(&app_uri, 1, app_source);
    assert!(
        app_diagnostics.diagnostics.is_empty(),
        "{app_diagnostics:?}"
    );
    let alpha_call = offset_to_position(
        app_source,
        app_source.find("alpha/score 1").expect("alpha call"),
    );
    let alias_call = offset_to_position(
        app_source,
        app_source.find("得分 2").expect("referred alias call"),
    );
    let beta_call = offset_to_position(
        app_source,
        app_source.find("beta/score 3").expect("beta call"),
    );

    let alpha_definition = state
        .definition(&app_uri, alpha_call)
        .expect("qualified alpha definition");
    let alias_definition = state
        .definition(&app_uri, alias_call)
        .expect("Chinese alias definition");
    let beta_definition = state
        .definition(&app_uri, beta_call)
        .expect("qualified beta definition");
    assert_eq!(alpha_definition.uri, alpha_uri);
    assert_eq!(alias_definition, alpha_definition);
    assert_eq!(beta_definition.uri, beta_uri);
    assert_ne!(beta_definition, alpha_definition);
    let localized = state.completion(
        &app_uri,
        offset_to_position(app_source, app_source.len()),
        None,
    );
    assert!(
        localized
            .iter()
            .any(|item| item.insert_text == "得分" && item.label == "得分"),
        "{localized:?}"
    );

    let alpha_references = state.references(&app_uri, alpha_call);
    assert!(
        alpha_references
            .iter()
            .any(|location| location.uri == alpha_uri)
    );
    assert!(
        alpha_references
            .iter()
            .any(|location| location.uri == app_uri)
    );
    assert!(
        alpha_references
            .iter()
            .any(|location| location.uri == broken_uri)
    );
    assert!(
        alpha_references
            .iter()
            .all(|location| location.uri != beta_uri)
    );

    let alpha_diagnostics = state.did_open(&alpha_uri, 1, alpha_source);
    assert!(
        alpha_diagnostics.diagnostics.is_empty(),
        "{alpha_diagnostics:?}"
    );
    let alpha_declaration = offset_to_position(
        alpha_source,
        alpha_source
            .find("score [^Int value")
            .expect("alpha declaration"),
    );
    let provider_references = state.references(&alpha_uri, alpha_declaration);
    assert!(
        provider_references
            .iter()
            .any(|location| location.uri == app_uri)
    );
    assert!(
        provider_references
            .iter()
            .any(|location| location.uri == broken_uri)
    );

    let broken_diagnostics = state.did_open(&broken_uri, 1, broken_source);
    assert!(!broken_diagnostics.diagnostics.is_empty());
    let recovered_call = offset_to_position(
        broken_source,
        broken_source
            .find("alpha/score 4")
            .expect("recovered alpha call"),
    );
    assert_eq!(
        state
            .definition(&broken_uri, recovered_call)
            .expect("definition survives recovery"),
        alpha_definition
    );

    let alpha_member_call = offset_to_position(
        app_source,
        app_source.find("alpha/score 1").expect("alpha call") + "alpha/".len(),
    );
    assert_eq!(state.prepare_rename(&app_uri, alpha_call), None);
    let prepared = state
        .prepare_rename(&app_uri, alpha_member_call)
        .expect("qualified member prepare range");
    let prepared_start = position_to_offset(app_source, prepared.start).expect("range start");
    let prepared_end = position_to_offset(app_source, prepared.end).expect("range end");
    assert_eq!(&app_source[prepared_start..prepared_end], "score");

    let renamed = state
        .rename(&app_uri, alpha_member_call, "rank")
        .expect("workspace rename")
        .expect("workspace edits");
    assert_eq!(renamed.changes.get(&alpha_uri).map(Vec::len), Some(3));
    assert_eq!(renamed.changes.get(&app_uri).map(Vec::len), Some(1));
    assert_eq!(renamed.changes.get(&broken_uri).map(Vec::len), Some(1));
    assert!(!renamed.changes.contains_key(&beta_uri));
    for (edit_uri, edit_source) in [
        (&alpha_uri, alpha_source),
        (&app_uri, app_source),
        (&broken_uri, broken_source),
    ] {
        for edit in renamed
            .changes
            .get(edit_uri)
            .expect("expected source edits")
        {
            let start = position_to_offset(edit_source, edit.range.start).expect("edit start");
            let end = position_to_offset(edit_source, edit.range.end).expect("edit end");
            assert_eq!(&edit_source[start..end], "score");
            assert_eq!(edit.new_text, "rank");
        }
    }

    let alias_renamed = state
        .rename(&app_uri, alias_call, "分数")
        .expect("workspace alias rename")
        .expect("alias edits");
    assert_eq!(alias_renamed.changes.get(&alpha_uri).map(Vec::len), Some(2));
    assert_eq!(alias_renamed.changes.get(&app_uri).map(Vec::len), Some(2));
    assert!(!alias_renamed.changes.contains_key(&beta_uri));
    assert!(!alias_renamed.changes.contains_key(&broken_uri));
    for (edit_uri, edit_source) in [(&alpha_uri, alpha_source), (&app_uri, app_source)] {
        for edit in alias_renamed
            .changes
            .get(edit_uri)
            .expect("expected alias edits")
        {
            let start = position_to_offset(edit_source, edit.range.start).expect("edit start");
            let end = position_to_offset(edit_source, edit.range.end).expect("edit end");
            assert_eq!(&edit_source[start..end], "得分");
            assert_eq!(edit.new_text, "分数");
        }
    }

    drop(state);
    fs::remove_dir_all(root).expect("workspace cleanup");
}

#[test]
fn external_interface_without_source_has_no_definition_location() {
    let provider_source = r#"(module vendor.math)
(export [score])
^{:doc "Return the vendor score."} (defn ^Int score [^Int value] value)
"#;
    let provider_options = CompileOptions::new("vendor.math", PythonVersion::MINIMUM);
    let provider = compiler::analyze(provider_source, &provider_options);
    assert!(
        provider.diagnostics.is_empty(),
        "{:?}",
        provider.diagnostics
    );
    let provider_interface =
        interface::build_provisional(&provider.surface).expect("provider interface");
    let external_interfaces = BTreeMap::from([("vendor.math".to_owned(), provider_interface)]);
    let consumer_source = r#"(module demo.app)
(import vendor.math :as math)
(def result (math/score 1))
"#;
    let consumer_options = CompileOptions::new("demo.app", PythonVersion::MINIMUM);
    let inputs = [CompileInput::new(consumer_source, &consumer_options)];
    let mut analyses = compiler::analyze_workspace_recovering(&inputs, &external_interfaces);
    assert_eq!(analyses.len(), 1);
    assert!(
        analyses[0].diagnostics.is_empty(),
        "{:?}",
        analyses[0].diagnostics
    );
    let function_interfaces = collect_function_interfaces(&analyses, &external_interfaces);
    let macro_interfaces = collect_macro_interfaces(&analyses, &external_interfaces);
    let analysis = analyses.remove(0);
    let uri = "file:///workspace/external-consumer.osr";
    let workspace_symbols = build_single_symbol_index(&analysis, uri, consumer_source);
    let document = OpenDocument::from_analysis(
        uri.to_owned(),
        1,
        consumer_source.to_owned(),
        Vec::new(),
        ProjectDocumentAnalysis {
            analysis,
            function_interfaces,
            macro_interfaces,
            display_locale: None,
            workspace_symbols,
        },
    );
    let mut state = LspState::new();
    state.documents.insert(uri.to_owned(), document);
    let call = offset_to_position(
        consumer_source,
        consumer_source.find("math/score 1").expect("external call"),
    );

    assert_eq!(state.definition(uri, call), None);
    assert!(
        state
            .references(uri, call)
            .iter()
            .all(|location| location.uri == uri)
    );
    let member = offset_to_position(
        consumer_source,
        consumer_source.find("math/score 1").expect("external call") + "math/".len(),
    );
    assert_eq!(state.prepare_rename(uri, member), None);
    assert_eq!(
        state.rename(uri, member, "rank").expect("rename result"),
        None
    );
}

#[test]
fn project_errors_preserve_workspace_identity_imports_and_completion() {
    let sequence = NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "osiris-lsp-recovering-workspace-{}-{sequence}",
        std::process::id()
    ));
    let source_root = root.join("src/demo");
    fs::create_dir_all(&source_root).expect("source root");
    fs::write(
            root.join("pyproject.toml"),
            "[project]\nname = \"lsp-workspace\"\nversion = \"1.0\"\n",
        )
        .expect("project configuration");
    fs::write(root.join("osiris.jsonc"), r#"{"source":["src"]}"#)
        .expect("Osiris configuration");
    fs::write(
        source_root.join("math.osr"),
        "(module demo.math)\n(export [add-one])\n^{:doc \"Increment an integer.\"} (defn ^Int add-one [^Int x] (+ x 1))\n",
    )
    .expect("dependency source");
    let app_source =
        "(module demo.app)\n(import demo.math :as math)\n(def answer (math/add-one 41))\n";
    let app = source_root.join("app.osr");
    fs::write(&app, app_source).expect("application source");
    let broken_source =
        "(module demo.broken)\n(import demo.math :as math)\n(defn ^Int invalid [^Int x])\n";
    let broken = source_root.join("broken.osr");
    fs::write(&broken, broken_source).expect("broken source");
    let app_uri = format!("file://{}", app.display());
    let broken_uri = format!("file://{}", broken.display());
    let mut state = LspState::new();

    let app_diagnostics = state.did_open(&app_uri, 1, app_source);

    assert!(
        app_diagnostics.diagnostics.is_empty(),
        "{app_diagnostics:?}"
    );
    let app_document = state.document(&app_uri).expect("open app document");
    assert_eq!(app_document.analysis.hir.name, "demo.app");
    let imported = app_document
        .semantic
        .symbols
        .iter()
        .find(|symbol| symbol.binding_id == "demo.math::function::add-one")
        .expect("imported function should remain in app semantics");
    assert_eq!(imported.kind, BindingKind::Function);
    assert!(matches!(imported.ty, Type::Fn(_)));
    assert!(
        state
            .completion(
                &app_uri,
                Position {
                    line: 3,
                    character: 0,
                },
                None,
            )
            .iter()
            .any(|item| item.data["bindingId"] == "demo.math::function::add-one")
    );

    let broken_diagnostics = state.did_open(&broken_uri, 1, broken_source);

    assert!(!broken_diagnostics.diagnostics.is_empty());
    let broken_document = state.document(&broken_uri).expect("open broken document");
    assert_eq!(broken_document.analysis.hir.name, "demo.broken");
    assert!(broken_document.semantic.symbols.iter().any(|symbol| {
        symbol.binding_id == "demo.math::function::add-one"
            && symbol.kind == BindingKind::Function
            && matches!(symbol.ty, Type::Fn(_))
    }));
    assert!(
        state
            .completion(
                &broken_uri,
                Position {
                    line: 3,
                    character: 0,
                },
                None,
            )
            .iter()
            .any(|item| item.data["bindingId"] == "demo.math::function::add-one")
    );
    drop(state);
    fs::remove_dir_all(root).expect("workspace cleanup");
}
