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
    let first_fingerprint = state
        .workspace_cache
        .as_ref()
        .expect("workspace analysis cache")
        .fingerprint
        .clone();
    let provider_path = source_root.join("math.osr");
    let provider_uri = format!("file://{}", provider_path.display());
    let provider_source = fs::read_to_string(provider_path).expect("provider source");
    state.did_open(&provider_uri, 1, provider_source);
    assert_eq!(
        state
            .workspace_cache
            .as_ref()
            .expect("workspace analysis cache")
            .fingerprint,
        first_fingerprint,
        "opening another file in an unchanged workspace should reuse analysis"
    );
    drop(state);
    fs::remove_dir_all(root).expect("workspace cleanup");
}

#[test]
fn workspace_symbol_queries_find_localized_names_outside_the_open_module() {
    let sequence = NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "osiris-lsp-symbol-workspace-{}-{sequence}",
        std::process::id()
    ));
    let source_root = root.join("src/demo");
    fs::create_dir_all(&source_root).expect("source root");
    fs::write(
        root.join("pyproject.toml"),
        "[project]\nname = \"lsp-symbol-workspace\"\nversion = \"1.0\"\n",
    )
    .expect("project configuration");
    fs::write(
        root.join("osiris.jsonc"),
        r#"{"source":["src"],"displayLocale":"zh-CN"}"#,
    )
    .expect("Osiris configuration");
    let provider_source = r#"(module demo.text)
(export [format-message])
^{:doc {:default "Format text for display." "zh-CN" "格式化用于显示的文本。"}
  :osiris/names {"zh-CN" {:preferred 格式化文本 :aliases [渲染文本]}}}
(defn ^Str format-message [^Str value] value)
"#;
    let provider = source_root.join("text.osr");
    fs::write(&provider, provider_source).expect("provider source");
    let app_source = "(module demo.app)\n(def answer 42)\n";
    let app = source_root.join("app.osr");
    fs::write(&app, app_source).expect("application source");
    let app_uri = format!("file://{}", app.display());
    let provider_uri = format!("file://{}", provider.display());
    let mut state = LspState::new();

    let diagnostics = state.did_open(&app_uri, 1, app_source);
    assert!(diagnostics.diagnostics.is_empty(), "{diagnostics:?}");
    let symbols = state
        .symbols(&app_uri, Some("渲染文本"))
        .expect("workspace symbols");
    assert_eq!(symbols.len(), 1, "{symbols:?}");
    assert_eq!(
        symbols[0]["binding_id"],
        "demo.text::function::format-message"
    );
    assert_eq!(
        state
            .symbols(&app_uri, Some("demo.text/渲染文本"))
            .expect("qualified workspace symbols")
            .len(),
        1
    );
    let (hover, machine) = state
        .hover_for_binding(
            &app_uri,
            "demo.text::function::format-message",
            Some("zh-CN"),
        )
        .expect("provider hover");
    assert!(hover.contents.value.starts_with("**格式化文本** · 函数"));
    assert_eq!(machine["source"]["uri"], provider_uri);
    assert_eq!(machine["source"]["provenance"], "workspace-source");

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
        !app_diagnostics
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == 1),
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
            function_interfaces: std::sync::Arc::new(function_interfaces),
            macro_interfaces: std::sync::Arc::new(macro_interfaces),
            display_locale: None,
            workspace_symbols: std::sync::Arc::new(workspace_symbols),
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

/// The project symbol index is built per module in parallel and merged. The
/// merge has to resolve ambiguity exactly as indexing the modules one at a
/// time into a shared index does, so this compares it against that reference.
#[test]
fn the_merged_project_symbol_index_matches_indexing_modules_in_order() {
    // `demo.alpha` defines `score` twice, so one module's own index is already
    // ambiguous and the merge has to carry that into the shared index.
    let sources = [
        (
            "demo.alpha",
            "(module demo.alpha)\n(export [score])\n^{:doc \"Alpha.\"} (defn ^Int score [^Int x] x)\n             ^{:doc \"Alpha again.\"} (defn ^Int score [^Int x] (+ x 1))\n",
        ),
        (
            "demo.beta",
            "(module demo.beta)\n(export [score])\n^{:doc \"Beta.\"} (defn ^Int score [^Int x] x)\n",
        ),
        (
            "demo.app",
            "(module demo.app)\n(import demo.alpha :as alpha)\n(import demo.beta :as beta)\n\
             (def first-result (alpha/score 1))\n(def second-result (beta/score 2))\n",
        ),
    ];
    let all_options = sources
        .iter()
        .map(|(name, _)| {
            CompileOptions::new(*name, PythonVersion::MINIMUM).with_expected_module_name(*name)
        })
        .collect::<Vec<_>>();
    let inputs = sources
        .iter()
        .zip(&all_options)
        .map(|((_, source), options)| CompileInput::new(source, options))
        .collect::<Vec<_>>();
    let analyses = compiler::analyze_workspace_recovering(&inputs, &BTreeMap::new());
    let buffers = sources
        .iter()
        .zip(&all_options)
        .map(|((name, source), options)| WorkspaceBuffer {
            uri: format!("file:///workspace/{name}.osr"),
            source: (*source).to_owned(),
            options: options.clone(),
        })
        .collect::<Vec<_>>();

    let merged = build_project_symbol_index(&analyses, &buffers);

    let mut reference = WorkspaceSymbolIndex::default();
    for (analysis, buffer) in analyses.iter().zip(&buffers) {
        let semantic = SemanticDocument::from_analysis(analysis, &buffer.uri);
        index_analysis_symbols(
            &mut reference,
            analysis,
            semantic,
            &buffer.uri,
            &buffer.source,
        );
    }
    finish_symbol_index(&mut reference);

    assert_eq!(merged.source_uris, reference.source_uris);
    assert_eq!(merged.sources, reference.sources);
    assert_eq!(merged.definitions, reference.definitions);
    assert_eq!(merged.ambiguous_definitions, reference.ambiguous_definitions);
    assert_eq!(merged.references, reference.references);
    assert_eq!(merged.binding_kinds, reference.binding_kinds);
    assert_eq!(merged.provider_names, reference.provider_names);
    assert_eq!(
        merged.ambiguous_provider_names,
        reference.ambiguous_provider_names
    );
    assert_eq!(merged.relations, reference.relations);
    assert_eq!(
        merged.rename_occurrences.keys().collect::<Vec<_>>(),
        reference.rename_occurrences.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        merged.pending_import_members.len(),
        reference.pending_import_members.len()
    );
    assert_eq!(
        merged.semantic_symbols.len(),
        reference.semantic_symbols.len()
    );
    assert!(
        !merged.provider_names.is_empty(),
        "the fixture must actually populate provider names"
    );
    assert!(
        merged.relations.len() > 3 && merged.references.len() > 1,
        "the fixture must actually merge several modules' contributions"
    );
}

/// Conflict resolution in the merge, exercised directly.
///
/// A binding defined at two different locations must lose its definition and
/// be recorded as ambiguous, and the first module to claim a provider name or
/// binding kind must keep it. Valid source cannot produce these collisions —
/// binding identifiers are module-qualified — so the partials are built here.
#[test]
fn merging_partial_symbol_indexes_resolves_conflicts() {
    let location = |uri: &str, line: u32| Location {
        uri: uri.to_owned(),
        range: Range {
            start: Position { line, character: 0 },
            end: Position { line, character: 4 },
        },
    };
    let partial = |uri: &str, line: u32, kind: BindingKind| {
        let mut index = WorkspaceSymbolIndex::default();
        index
            .definitions
            .insert("shared::value".to_owned(), location(uri, line));
        index.binding_kinds.insert("shared::value".to_owned(), kind);
        index.provider_names.insert(
            ("demo".to_owned(), "name".to_owned()),
            format!("{uri}::binding"),
        );
        index
            .references
            .insert("shared::value".to_owned(), vec![location(uri, line)]);
        index
    };

    let mut merged = WorkspaceSymbolIndex::default();
    merge_symbol_index(&mut merged, partial("first", 1, BindingKind::Function));
    // A second, different definition of the same binding makes it ambiguous.
    merge_symbol_index(&mut merged, partial("second", 2, BindingKind::Value));

    assert!(!merged.definitions.contains_key("shared::value"));
    assert!(merged.ambiguous_definitions.contains("shared::value"));
    assert_eq!(
        merged.binding_kinds.get("shared::value"),
        Some(&BindingKind::Function),
        "the first module's kind wins"
    );
    assert!(
        merged
            .ambiguous_provider_names
            .contains(&("demo".to_owned(), "name".to_owned())),
        "two providers claiming one name is ambiguous"
    );
    assert!(!merged.provider_names.contains_key(&("demo".to_owned(), "name".to_owned())));
    assert_eq!(
        merged.references["shared::value"].len(),
        2,
        "references accumulate across modules"
    );

    // A third partial repeating the first definition must not resurrect it.
    merge_symbol_index(&mut merged, partial("first", 1, BindingKind::Function));
    assert!(!merged.definitions.contains_key("shared::value"));
    assert!(merged.ambiguous_definitions.contains("shared::value"));
}

/// The semantic projection is the single kernel both LSP and LSC read, so a
/// macro reference must resolve through the ordinary hover path rather than a
/// surface-specific special case.
#[test]
fn hovering_a_macro_call_reports_the_macro_and_its_documentation() {
    let sequence = NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "osiris-lsp-macro-hover-{}-{sequence}",
        std::process::id()
    ));
    let source_root = root.join("src/demo");
    fs::create_dir_all(&source_root).expect("source root");
    fs::write(
        root.join("pyproject.toml"),
        "[project]\nname = \"lsp-macro-hover\"\nversion = \"1.0\"\n",
    )
    .expect("project configuration");
    fs::write(root.join("osiris.jsonc"), r#"{"source":["src"]}"#)
        .expect("Osiris configuration");
    fs::write(
        source_root.join("lib.osr"),
        "(module demo.lib)\n(export [twice])\n\
         ^{:doc \"Double the expression.\"} (defmacro twice [x] `(+ ~x ~x))\n",
    )
    .expect("provider source");
    let app_source = "(module demo.app)\n(import demo.lib :refer [twice])\n(def a (twice 3))\n";
    let app = source_root.join("app.osr");
    fs::write(&app, app_source).expect("application source");
    let uri = format!("file://{}", app.display());
    let mut state = LspState::new();
    state.did_open(&uri, 1, app_source);

    let call = offset_to_position(app_source, app_source.rfind("twice").expect("call site"));
    let hover = state.hover(&uri, call, Some("en")).expect("macro hover");

    assert!(
        hover.contents.value.contains("Macro"),
        "{}",
        hover.contents.value
    );
    // Documentation lives on the declaring module's symbol; a call site in
    // another module must still show it.
    assert!(
        hover.contents.value.contains("Double the expression."),
        "{}",
        hover.contents.value
    );
    // A macro never reaches typed HIR, so claiming a runtime type would assert
    // something the compiler never determined.
    assert!(
        !hover.contents.value.contains("Unknown"),
        "{}",
        hover.contents.value
    );

    let definition = state.definition(&uri, call).expect("macro definition");
    assert!(definition.uri.ends_with("lib.osr"), "{definition:?}");

    drop(state);
    fs::remove_dir_all(root).expect("workspace cleanup");
}
