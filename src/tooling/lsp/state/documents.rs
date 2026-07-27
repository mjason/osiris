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
        let locale = normalize_locale(locale.into());
        self.display_locale = locale.clone();
        self.session_locale = Some(locale);
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
        self.deferred.remove(uri);
        self.documents.remove(uri).is_some()
    }

    /// Applies edits without analyzing them.
    ///
    /// Typing produces one notification per keystroke, and analyzing each one
    /// discards the result before the next keystroke arrives. Deferring lets a
    /// transport coalesce a burst into a single analysis; the edits themselves
    /// are still applied in order and version checks still run immediately.
    /// Call [`Self::flush_analysis`] before answering any request.
    pub fn defer_change(
        &mut self,
        uri: &str,
        version: i64,
        changes: &[TextDocumentContentChangeEvent],
    ) -> Result<(), LspStateError> {
        let Some(document) = self.documents.get_mut(uri) else {
            return Err(LspStateError::new(
                DOCUMENT_NOT_FOUND,
                format!("document {uri} is not open"),
            ));
        };
        let current_version = document
            .pending
            .as_ref()
            .map_or(document.version, |pending| pending.version);
        if version <= current_version {
            return Err(LspStateError::new(
                STALE_DOCUMENT_VERSION,
                format!("document version {version} is not newer than {current_version}"),
            ));
        }
        let mut text = document
            .pending
            .as_ref()
            .map_or_else(|| document.text.clone(), |pending| pending.text.clone());
        for change in changes {
            apply_content_change(&mut text, change)?;
        }
        document.pending = Some(PendingEdit { version, text });
        self.deferred.insert(uri.to_owned());
        Ok(())
    }

    /// Whether any document has edits awaiting analysis.
    #[must_use]
    pub fn has_deferred_changes(&self) -> bool {
        !self.deferred.is_empty()
    }

    /// Analyzes every deferred edit and returns the diagnostics to publish.
    pub fn flush_analysis(&mut self) -> Vec<PublishDiagnosticsParams> {
        let deferred = std::mem::take(&mut self.deferred);
        let mut published = Vec::new();
        for uri in deferred {
            let Some(document) = self.documents.get_mut(&uri) else {
                continue;
            };
            let Some(pending) = document.pending.take() else {
                continue;
            };
            if let Ok(diagnostics) = self.did_change_full(&uri, pending.version, pending.text) {
                published.push(diagnostics);
            }
        }
        published
    }

    fn refresh_workspace_symbols(&mut self, updated: &OpenDocument) {
        let index = Arc::clone(&updated.workspace_symbols);
        for document in self.documents.values_mut() {
            if index.source_uris.contains(&document.uri) {
                document.workspace_symbols = Arc::clone(&index);
            }
        }
    }

    fn analyze_document(&mut self, uri: String, version: i64, text: String) -> OpenDocument {
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
                    function_interfaces: Arc::new(BTreeMap::new()),
                    macro_interfaces: Arc::new(BTreeMap::new()),
                    display_locale: None,
                    workspace_symbols: Arc::new(workspace_symbols),
                }
            });
        frontend.analysis.document = snapshot;
        OpenDocument::from_analysis(uri, version, text, identifier_lints, frontend)
    }

    fn analyze_project_document(&mut self, uri: &str, text: &str) -> Option<ProjectDocumentAnalysis> {
        // Per-stage timings for one reanalysis. `lap` reports the time spent in
        // the stage just finished, so a slow phase is attributable on sight.
        let t0 = std::time::Instant::now();
        let mut mark = t0;
        let mut lap = |label: &str| {
            let elapsed = mark.elapsed().as_secs_f64() * 1000.0;
            mark = std::time::Instant::now();
            lsp_debug!("    {label:<20} {elapsed:>7.1}ms");
        };
        let source_path = file_uri_to_path(uri)?;
        let project = ProjectConfig::discover(&source_path).ok()?;
        lap("discover project");
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
            collect_workspace_sources(root, &project, &mut paths).ok()?;
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
        lap("collect sources");
        let fingerprint = workspace_fingerprint(&project, &buffers, &self.site_roots);
        lap("fingerprint");
        let reusable = self.workspace_cache.as_ref().is_some_and(|cache| {
            cache.project_root == project.root && cache.fingerprint == fingerprint
        });
        lsp_debug!(
            "  workspace {} ({} modules, {})",
            if reusable { "cache hit" } else { "reanalysis" },
            buffers.len(),
            project.root.display()
        );
        if !reusable {
            let inputs = buffers
                .iter()
                .map(|buffer| CompileInput::new(&buffer.source, &buffer.options))
                .collect::<Vec<_>>();
            let external_interfaces = load_project_interfaces(&project, &self.site_roots)?;
            lap("load interfaces");
            let workspace =
                compiler::analyze_workspace_with_memo(&inputs, &external_interfaces, &self.memo);
            lap("strict analysis");
            let reuse = self.memo.stats();
            lsp_debug!(
                "    modules analyzed {}/{} reused, expanded {}/{} reused",
                reuse.reused,
                reuse.reused + reuse.analyzed,
                reuse.expansions_reused,
                reuse.expansions_reused + reuse.expanded
            );
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
            if recovering {
                lap("recovering analysis");
            }
            if !recovering {
                debug_assert_eq!(analyses.get(target_index)?.hir.name, target_module);
            }
            let function_interfaces = collect_function_interfaces(&analyses, &external_interfaces);
            let macro_interfaces = collect_macro_interfaces(&analyses, &external_interfaces);
            lap("collect interfaces");
            let workspace_symbols = build_project_symbol_index(&analyses, &buffers);
            lap("symbol index");
            self.workspace_cache = Some(WorkspaceAnalysisCache {
                project_root: project.root.clone(),
                fingerprint,
                buffers,
                analyses,
                workspace_diagnostics,
                function_interfaces: Arc::new(function_interfaces),
                macro_interfaces: Arc::new(macro_interfaces),
                workspace_symbols: Arc::new(workspace_symbols),
                display_locale: project.display_locale,
            });
        }
        let projected =
            project_document_from_cache(self.workspace_cache.as_ref()?, target_index, uri);
        lap("project document");
        lsp_debug!(
            "  workspace analysis total {:.1}ms",
            t0.elapsed().as_secs_f64() * 1000.0
        );
        projected
    }
}

/// Projects one document out of the shared workspace analysis. Everything the
/// editor queries is reference-counted, so only the target module's diagnostics
/// are materialized per notification.
fn project_document_from_cache(
    cache: &WorkspaceAnalysisCache,
    target_index: usize,
    uri: &str,
) -> Option<ProjectDocumentAnalysis> {
    let mut analysis = cache.analyses.get(target_index)?.clone();
    analysis.diagnostics.extend(
        cache
            .workspace_diagnostics
            .iter()
            .filter(|located| located.input_index == target_index)
            .map(|located| located.diagnostic.clone()),
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
    let workspace_symbols = remap_workspace_uri(
        Arc::clone(&cache.workspace_symbols),
        cache.buffers.get(target_index)?.uri.as_str(),
        uri,
    );
    Some(ProjectDocumentAnalysis {
        analysis,
        function_interfaces: Arc::clone(&cache.function_interfaces),
        macro_interfaces: Arc::clone(&cache.macro_interfaces),
        display_locale: cache.display_locale.clone(),
        workspace_symbols,
    })
}

fn remap_workspace_uri(
    index: Arc<WorkspaceSymbolIndex>,
    from: &str,
    to: &str,
) -> Arc<WorkspaceSymbolIndex> {
    if from == to {
        return index;
    }
    let mut index = (*index).clone();
    if index.source_uris.remove(from) {
        index.source_uris.insert(to.to_owned());
    }
    if let Some(source) = index.sources.remove(from) {
        index.sources.insert(to.to_owned(), source);
    }
    for location in index.definitions.values_mut() {
        if location.uri == from {
            location.uri = to.to_owned();
        }
    }
    for locations in index.references.values_mut() {
        for location in locations {
            if location.uri == from {
                location.uri = to.to_owned();
            }
        }
    }
    for occurrences in index.rename_occurrences.values_mut() {
        for occurrence in occurrences {
            if occurrence.uri == from {
                occurrence.uri = to.to_owned();
            }
        }
    }
    for symbol in &mut index.semantic_symbols {
        if symbol.uri == from {
            symbol.uri = to.to_owned();
        }
    }
    for member in &mut index.pending_import_members {
        if member.uri == from {
            member.uri = to.to_owned();
        }
    }
    for relation in &mut index.relations {
        if relation.uri == from {
            relation.uri = to.to_owned();
        }
    }
    Arc::new(index)
}

fn workspace_fingerprint(
    project: &ProjectConfig,
    buffers: &[WorkspaceBuffer],
    site_roots: &[PathBuf],
) -> String {
    let mut material = String::new();
    material.push_str(&project.root.display().to_string());
    material.push('|');
    material.push_str(&project.distribution);
    material.push('|');
    material.push_str(&project.distribution_version);
    material.push('|');
    material.push_str(&project.output_dir.display().to_string());
    material.push('|');
    material.push_str(project.display_locale.as_deref().unwrap_or(""));
    for root in &project.source_roots {
        material.push('|');
        material.push_str(&root.display().to_string());
    }
    material.push_str(&project.target_python.to_string());
    material.push('|');
    material.push_str(if project.strict { "strict" } else { "permissive" });
    for root in site_roots {
        material.push('|');
        material.push_str(&root.display().to_string());
    }
    for buffer in buffers {
        material.push('|');
        material.push_str(&buffer.options.source_name);
        material.push('|');
        material.push_str(&buffer.source);
    }
    crate::hash::sha256(material.as_bytes())
}
