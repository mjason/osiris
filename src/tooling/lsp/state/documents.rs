impl LspState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_target_python(target_python: PythonVersion) -> Self {
        Self {
            target_python,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn display_locale(&self) -> &str {
        &self.display_locale
    }

    pub fn set_display_locale(&mut self, locale: impl Into<String>) {
        self.display_locale = normalize_locale(locale.into());
    }

    pub fn set_site_roots(&mut self, roots: impl IntoIterator<Item = PathBuf>) {
        self.site_roots = roots.into_iter().collect();
        self.site_roots.sort();
        self.site_roots.dedup();
    }

    #[must_use]
    pub const fn analysis_runs(&self) -> u64 {
        self.analysis_runs
    }

    #[must_use]
    pub const fn shutdown_requested(&self) -> bool {
        self.shutdown_requested
    }

    pub fn request_shutdown(&mut self) {
        self.shutdown_requested = true;
    }

    #[must_use]
    pub fn document(&self, uri: &str) -> Option<&OpenDocument> {
        self.documents.get(uri)
    }

    #[must_use]
    pub fn semantic_document(&self, uri: &str) -> Option<&SemanticDocument> {
        self.document(uri).map(|document| &document.semantic)
    }

    #[must_use]
    pub fn document_version(&self, uri: &str) -> Option<i64> {
        self.document(uri).map(|document| document.version)
    }

    /// Opens or replaces a document and runs the frontend exactly once.
    pub fn did_open(
        &mut self,
        uri: impl Into<String>,
        version: i64,
        text: impl Into<String>,
    ) -> PublishDiagnosticsParams {
        let uri = uri.into();
        let text = text.into();
        let document = self.analyze_document(uri.clone(), version, text);
        self.analysis_runs += 1;
        self.refresh_workspace_symbols(&document);
        self.documents.insert(uri.clone(), document);
        self.diagnostics(&uri)
            .expect("the opened document was just inserted")
    }

    pub fn open_document(
        &mut self,
        uri: impl Into<String>,
        version: i64,
        text: impl Into<String>,
    ) -> PublishDiagnosticsParams {
        self.did_open(uri, version, text)
    }

    /// Applies all changes and runs the frontend once for the resulting text.
    pub fn did_change(
        &mut self,
        uri: &str,
        version: i64,
        changes: &[TextDocumentContentChangeEvent],
    ) -> Result<PublishDiagnosticsParams, LspStateError> {
        let Some(current) = self.documents.get(uri) else {
            return Err(LspStateError::new(
                DOCUMENT_NOT_FOUND,
                format!("document {uri} is not open"),
            ));
        };
        if version <= current.version {
            return Err(LspStateError::new(
                STALE_DOCUMENT_VERSION,
                format!(
                    "document version {version} is not newer than {}",
                    current.version
                ),
            ));
        }
        let mut text = current.text.clone();
        for change in changes {
            apply_content_change(&mut text, change)?;
        }
        let document = self.analyze_document(uri.to_owned(), version, text);
        self.analysis_runs += 1;
        self.refresh_workspace_symbols(&document);
        self.documents.insert(uri.to_owned(), document);
        self.diagnostics(uri)
            .ok_or_else(|| LspStateError::new(DOCUMENT_NOT_FOUND, "changed document disappeared"))
    }

    /// Convenience API for full document synchronization.
    pub fn did_change_full(
        &mut self,
        uri: &str,
        version: i64,
        text: impl Into<String>,
    ) -> Result<PublishDiagnosticsParams, LspStateError> {
        self.did_change(
            uri,
            version,
            &[TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: text.into(),
            }],
        )
    }

    pub fn change_document(
        &mut self,
        uri: &str,
        version: i64,
        text: impl Into<String>,
    ) -> Result<PublishDiagnosticsParams, LspStateError> {
        self.did_change_full(uri, version, text)
    }

    pub fn did_close(&mut self, uri: &str) -> bool {
        self.documents.remove(uri).is_some()
    }

    fn refresh_workspace_symbols(&mut self, updated: &OpenDocument) {
        let index = updated.workspace_symbols.clone();
        for document in self.documents.values_mut() {
            if index.source_uris.contains(&document.uri) {
                document.workspace_symbols = index.clone();
            }
        }
    }

    fn analyze_document(&self, uri: String, version: i64, text: String) -> OpenDocument {
        let snapshot = self.documents.get(&uri).map_or_else(
            || reader::read(&text),
            |previous| reader::read_incremental(&text, &previous.analysis.document),
        );
        let identifier_lints = lint_forms_strict(&snapshot.forms);
        let mut frontend = self
            .analyze_project_document(&uri, &text)
            .unwrap_or_else(|| {
                let fallback = fallback_module_name(&uri);
                let options =
                    CompileOptions::new(fallback, self.target_python).with_source_name(uri.clone());
                let analysis = compiler::analyze(&text, &options);
                let workspace_symbols = build_single_symbol_index(&analysis, &uri, &text);
                ProjectDocumentAnalysis {
                    analysis,
                    function_interfaces: BTreeMap::new(),
                    macro_interfaces: BTreeMap::new(),
                    display_locale: None,
                    workspace_symbols,
                }
            });
        frontend.analysis.document = snapshot;
        OpenDocument::from_analysis(uri, version, text, identifier_lints, frontend)
    }

    fn analyze_project_document(&self, uri: &str, text: &str) -> Option<ProjectDocumentAnalysis> {
        let source_path = file_uri_to_path(uri)?;
        let project = ProjectConfig::discover(&source_path).ok()?;
        let target_path = fs::canonicalize(&source_path).ok()?;
        let target_module = project.module_name_for_source(&source_path).ok()?;

        let open_texts = self
            .documents
            .values()
            .filter_map(|document| {
                let path = file_uri_to_path(&document.uri)?;
                let path = fs::canonicalize(path).ok()?;
                Some((path, document.text.clone()))
            })
            .collect::<BTreeMap<_, _>>();
        let mut paths = Vec::new();
        for root in &project.source_roots {
            collect_workspace_sources(root, &mut paths).ok()?;
        }
        paths.retain(|path| !project.is_excluded(path));
        paths.sort();
        paths.dedup();

        let mut buffers = Vec::with_capacity(paths.len());
        let mut target_index = None;
        for path in paths {
            let canonical = fs::canonicalize(&path).ok()?;
            let module_name = project.module_name_for_source(&path).ok()?;
            let source = if canonical == target_path {
                target_index = Some(buffers.len());
                text.to_owned()
            } else if let Some(open) = open_texts.get(&canonical) {
                open.clone()
            } else {
                fs::read_to_string(&path).ok()?
            };
            buffers.push(WorkspaceBuffer {
                uri: if canonical == target_path {
                    uri.to_owned()
                } else {
                    format!("file://{}", canonical.display())
                },
                options: project_options(&project, &path, module_name),
                source,
            });
        }
        let target_index = target_index?;
        let inputs = buffers
            .iter()
            .map(|buffer| CompileInput::new(&buffer.source, &buffer.options))
            .collect::<Vec<_>>();
        let external_interfaces = load_project_interfaces(&project, &self.site_roots)?;
        let workspace = compiler::compile_workspace(&inputs, &external_interfaces);
        let recovering = workspace.has_errors();
        let (analyses, workspace_diagnostics) = if recovering {
            (
                compiler::analyze_workspace_recovering(&inputs, &external_interfaces),
                workspace.diagnostics,
            )
        } else {
            (
                workspace
                    .units
                    .into_iter()
                    .map(|unit| unit.analysis)
                    .collect(),
                Vec::new(),
            )
        };
        let function_interfaces = collect_function_interfaces(&analyses, &external_interfaces);
        let macro_interfaces = collect_macro_interfaces(&analyses, &external_interfaces);
        let workspace_symbols = build_project_symbol_index(&analyses, &buffers);
        let mut analysis = analyses.into_iter().nth(target_index)?;
        analysis.diagnostics.extend(
            workspace_diagnostics
                .into_iter()
                .filter(|located| located.input_index == target_index)
                .map(|located| located.diagnostic),
        );
        analysis.diagnostics.sort_by(|left, right| {
            (left.span.start, left.span.end, left.code, &left.message).cmp(&(
                right.span.start,
                right.span.end,
                right.code,
                &right.message,
            ))
        });
        analysis.diagnostics.dedup_by(|left, right| {
            left.span == right.span && left.code == right.code && left.message == right.message
        });
        if !recovering {
            debug_assert_eq!(analysis.hir.name, target_module);
        }
        Some(ProjectDocumentAnalysis {
            analysis,
            function_interfaces,
            macro_interfaces,
            display_locale: project.display_locale,
            workspace_symbols,
        })
    }
}
