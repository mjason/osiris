use super::*;

/// Projects an analysis into the versioned semantic model.
#[must_use]
pub fn project(analysis: &Analysis, source_name: impl Into<String>) -> SemanticDocument {
    SemanticDocument::from_analysis(analysis, source_name)
}

impl SemanticDocument {
    /// Projects one analysis without running another compiler pass.
    #[must_use]
    pub fn from_analysis(analysis: &Analysis, source_name: impl Into<String>) -> Self {
        Self::from_analysis_at_version(analysis, source_name, 0)
    }

    /// Projects one analysis and associates it with an editor version.
    #[must_use]
    pub fn from_analysis_at_version(
        analysis: &Analysis,
        source_name: impl Into<String>,
        document_version: i64,
    ) -> Self {
        let source = source_name.into();
        let aliases_by_target = aliases_by_target(&analysis.hir);
        let references = collect_references(analysis);
        let symbol_summaries = collect_symbol_summaries(&analysis.hir);
        let records = collect_records(&analysis.hir);
        let mut symbols = analysis
            .hir
            .bindings
            .iter()
            .map(|binding| {
                let id = binding.name.id.as_str().to_owned();
                let binding_aliases = aliases_by_target.get(&id).cloned().unwrap_or_default();
                let summary = symbol_summaries
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(SemanticSummary::unknown);
                let mut layers =
                    layers_for_metadata(&binding.metadata, binding.name.span, &summary);
                layers
                    .records
                    .extend(records_for_binding(&records, &binding.name.canonical));
                let occurrences = references.get(&id).cloned().unwrap_or_default();
                let definition = binding.name.span;
                let mut all_occurrences = occurrences.clone();
                all_occurrences.extend(binding_aliases.iter().map(|alias| alias.span));
                if !all_occurrences.contains(&definition) {
                    all_occurrences.push(definition);
                }
                all_occurrences.sort_by_key(|span| (span.start, span.end));
                all_occurrences.dedup();
                let preferred = preferred_alias(&binding_aliases, &binding.metadata);
                let labels = labels_for_name(&binding.name.canonical, preferred);
                SemanticSymbol {
                    binding_id: id,
                    canonical: binding.name.canonical.clone(),
                    source: binding.source_spelling.clone(),
                    source_spelling: binding.source_spelling.clone(),
                    python: binding.name.python.clone(),
                    kind: binding.name.kind,
                    aliases: binding_aliases,
                    public: binding.public,
                    ty: binding.ty.clone(),
                    metadata: layers,
                    summary,
                    labels,
                    span: binding.name.span,
                    definition,
                    references: occurrences,
                    occurrences: all_occurrences,
                }
            })
            .collect::<Vec<_>>();
        symbols.sort_by(|left, right| {
            (left.span.start, left.span.end, &left.binding_id).cmp(&(
                right.span.start,
                right.span.end,
                &right.binding_id,
            ))
        });

        let authored = collect_authored(analysis);
        let module_summary = module_summary(&analysis.hir);
        let mut declared = declared_facts(&analysis.hir.metadata, analysis.hir.span);
        let mut verified = vec![verified_module_fact(&analysis.hir, &module_summary)];
        for symbol in &symbols {
            declared.extend(symbol.metadata.declared.clone());
            verified.extend(symbol.metadata.verified.clone());
        }
        let macro_traces = analysis
            .expansion_traces
            .iter()
            .map(MacroTraceView::from)
            .collect::<Vec<_>>();
        let operation_graph = build_operation_graph(&analysis.hir, &analysis.expansion_traces);
        let operations = operation_graph.nodes.clone();
        let operation_edges = operation_graph.edges.clone();

        Self {
            version: SEMANTIC_DOCUMENT_VERSION,
            document_version,
            source,
            source_len: analysis.document.source_len,
            module: analysis.hir.name.clone(),
            symbols,
            authored,
            records,
            declared,
            verified,
            macro_traces,
            operation_graph,
            operations,
            operation_edges,
            diagnostics: analysis
                .diagnostics
                .iter()
                .map(SemanticDiagnostic::from)
                .collect(),
        }
    }

    #[must_use]
    pub fn new(analysis: &Analysis, source_name: impl Into<String>, document_version: i64) -> Self {
        Self::from_analysis_at_version(analysis, source_name, document_version)
    }

    #[must_use]
    pub fn symbol(&self, binding_id: &str) -> Option<&SemanticSymbol> {
        self.symbols
            .iter()
            .find(|symbol| symbol.binding_id == binding_id)
    }

    #[must_use]
    pub fn symbol_at(&self, offset: usize) -> Option<&SemanticSymbol> {
        self.symbols
            .iter()
            .filter(|symbol| {
                symbol
                    .occurrences
                    .iter()
                    .any(|span| contains(*span, offset))
            })
            .min_by_key(|symbol| {
                symbol
                    .occurrences
                    .iter()
                    .filter(|span| contains(**span, offset))
                    .map(|span| span.end.saturating_sub(span.start))
                    .min()
                    .unwrap_or(usize::MAX)
            })
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    pub fn to_pretty_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}
