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
                // A binding the module never wrote down — an implicit core
                // type, for example — is located at the module form itself.
                // That is a placeholder, not an occurrence: treating it as one
                // lets the binding answer for every position nothing narrower
                // covers, so hovering unrelated syntax reports a core type.
                let whole_source = |span: &Span| *span == analysis.hir.span;
                let occurrences = references
                    .get(&id)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|span| !whole_source(span))
                    .collect::<Vec<_>>();
                let definition = binding.name.span;
                let mut all_occurrences = occurrences.clone();
                all_occurrences.extend(binding_aliases.iter().map(|alias| alias.span));
                if !all_occurrences.contains(&definition) && !whole_source(&definition) {
                    all_occurrences.push(definition);
                }
                all_occurrences.sort_by_key(|span| (span.start, span.end));
                all_occurrences.dedup();
                let localized = localized_names(&binding.metadata);
                let labels = labels_for_name(&binding.name.canonical, &localized);
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
                    names: SemanticNames {
                        canonical: binding.name.canonical.clone(),
                        localized,
                    },
                    documentation: documentation(&binding.metadata),
                    examples: examples(&binding.metadata),
                    content_references: analysis
                        .surface
                        .embedded_content_references
                        .iter()
                        .filter(|reference| {
                            metadata_contains_span(&binding.metadata, reference.reference_span)
                        })
                        .map(|reference| SemanticContentReference {
                            source: source.clone(),
                            field: reference.field.clone(),
                            language: reference.language.clone(),
                            label: reference.label.clone(),
                            content: reference.content.clone(),
                            content_hash: reference.content_hash.clone(),
                            reference_span: reference.reference_span,
                            source_span: reference.source_span,
                            body_span: reference.body_span,
                        })
                        .collect(),
                    span: binding.name.span,
                    definition,
                    references: occurrences,
                    occurrences: all_occurrences,
                }
            })
            .collect::<Vec<_>>();
        symbols.extend(macro_symbols(analysis));
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
        let occurrence = self
            .symbols
            .iter()
            .filter_map(|symbol| {
                let width = symbol
                    .occurrences
                    .iter()
                    .filter(|span| contains(**span, offset))
                    .map(|span| span.end.saturating_sub(span.start))
                    .min()?;
                Some((width, symbol))
            })
            .min_by_key(|(width, _)| *width);
        let operation = self
            .operation_graph
            .nodes
            .iter()
            .filter(|operation| contains(operation.span, offset))
            .filter_map(|operation| {
                let binding = operation.binding_id.as_deref()?;
                let symbol = self
                    .symbols
                    .iter()
                    .find(|symbol| symbol.binding_id == binding)?;
                Some((
                    operation.span.end.saturating_sub(operation.span.start),
                    symbol,
                ))
            })
            .min_by_key(|(width, _)| *width);
        match (occurrence, operation) {
            (Some(left), Some(right)) => Some(if left.0 < right.0 { left.1 } else { right.1 }),
            (Some((_, symbol)), None) | (None, Some((_, symbol))) => Some(symbol),
            (None, None) => None,
        }
    }

    #[must_use]
    pub fn symbol_at_source(&self, offset: usize, source: &str) -> Option<&SemanticSymbol> {
        fn identifier_char(character: char) -> bool {
            // Osiris symbol characters, operators included — `<=` is a macro
            // name, and a tokenizer that cannot spell it can never match it.
            // Kept in step with `identifier_token_at` in the LSP navigation.
            character.is_alphanumeric()
                || matches!(
                    character,
                    '_' | '-' | '?' | '!' | '<' | '>' | '=' | '+' | '*' | '%' | '&' | '$' | '|'
                )
        }
        let offset = offset.min(source.len());
        let left = source
            .get(..offset)
            .and_then(|prefix| {
                prefix
                    .rsplit(|character| !identifier_char(character))
                    .next()
            })
            .unwrap_or_default();
        let right = source
            .get(offset..)
            .and_then(|suffix| suffix.split(|character| !identifier_char(character)).next())
            .unwrap_or_default();
        let token = format!("{left}{right}");
        let qualified_span = {
            // Second tokenization with `/` included: the cursor may sit on the
            // namespace side of a qualified reference, whose symbol records
            // the member. The qualified token's exact span must equal an
            // occurrence — equality, not containment, so this cannot revive
            // the enclosing-symbol noise removed below.
            fn qualified_char(character: char) -> bool {
                identifier_char(character) || character == '/'
            }
            let left = source
                .get(..offset)
                .and_then(|prefix| prefix.rsplit(|c| !qualified_char(c)).next())
                .unwrap_or_default();
            let right = source
                .get(offset..)
                .and_then(|suffix| suffix.split(|c| !qualified_char(c)).next())
                .unwrap_or_default();
            let start = offset - left.len();
            (left.contains('/') || right.contains('/'))
                .then_some((start, start + left.len() + right.len()))
        };
        // A token that matches nothing answers nothing — including the empty
        // token on punctuation. The enclosing-symbol fallback this replaces
        // made every position inside a macro call answer as the expansion's
        // product: its occurrence covers the whole call, so hovering any
        // argument showed a generated binding instead of what the cursor is
        // on, and each caller's own more informed fallback never ran. A
        // qualified reference matches by its segments: the cursor sits on one
        // side of the `/`, and either side names the same binding.
        let exact = self.symbols.iter().find(|symbol| {
            symbol
                .occurrences
                .iter()
                .any(|span| contains(*span, offset))
                && (symbol.canonical == token
                    || symbol.source_spelling == token
                    || symbol
                        .source_spelling
                        .split('/')
                        .any(|segment| segment == token)
                    || symbol.aliases.iter().any(|alias| alias.spelling == token))
        });
        exact.or_else(|| {
            let (start, end) = qualified_span?;
            self.symbols.iter().find(|symbol| {
                symbol
                    .occurrences
                    .iter()
                    .any(|span| span.start == start && span.end == end)
            })
        })
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    pub fn to_pretty_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

fn metadata_contains_span(metadata: &[MetadataEntry], span: Span) -> bool {
    fn contains(form: &Form, span: Span) -> bool {
        if form.span == span {
            return true;
        }
        form.metadata
            .iter()
            .any(|entry| contains(&entry.key, span) || contains(&entry.value, span))
            || match &form.kind {
                FormKind::List(items)
                | FormKind::Vector(items)
                | FormKind::Map(items)
                | FormKind::Set(items) => items.iter().any(|item| contains(item, span)),
                FormKind::ReaderMacro { form, .. } => contains(form, span),
                _ => false,
            }
    }

    metadata
        .iter()
        .any(|entry| contains(&entry.key, span) || contains(&entry.value, span))
}

/// The span of the head symbol of the call form covering `call`.
fn macro_name_span(forms: &[Form], call: Span) -> Option<Span> {
    fn search(form: &Form, call: Span) -> Option<Span> {
        if form.span.start > call.start || form.span.end < call.end {
            return None;
        }
        let children = match &form.kind {
            FormKind::List(items) | FormKind::Vector(items) | FormKind::Set(items) => {
                items.as_slice()
            }
            FormKind::Map(items) => items.as_slice(),
            _ => &[],
        };
        let nested = children
            .iter()
            .filter_map(|child| search(child, call))
            .min_by_key(|span| span.end.saturating_sub(span.start));
        if nested.is_some() {
            return nested;
        }
        if form.span != call {
            return None;
        }
        let FormKind::List(items) = &form.kind else {
            return None;
        };
        let head = items.first()?;
        matches!(head.kind, FormKind::Symbol(_)).then_some(head.span)
    }
    forms
        .iter()
        .filter_map(|form| search(form, call))
        .min_by_key(|span| span.end.saturating_sub(span.start))
}

/// The span of the declared name inside a `defmacro` form covering `item`.
fn macro_declaration_name_span(forms: &[Form], item: Span, canonical: &str) -> Option<Span> {
    fn search(form: &Form, item: Span, canonical: &str) -> Option<Span> {
        if form.span.start > item.start || form.span.end < item.end {
            return None;
        }
        let children = match &form.kind {
            FormKind::List(items) | FormKind::Vector(items) | FormKind::Set(items) => {
                items.as_slice()
            }
            FormKind::Map(items) => items.as_slice(),
            _ => &[],
        };
        if let Some(nested) = children
            .iter()
            .filter_map(|child| search(child, item, canonical))
            .min_by_key(|span| span.end.saturating_sub(span.start))
        {
            return Some(nested);
        }
        let FormKind::List(items) = &form.kind else {
            return None;
        };
        let head = items.first()?;
        let FormKind::Symbol(name) = &head.kind else {
            return None;
        };
        if !matches!(name.canonical.as_str(), "defmacro" | "defn-for-syntax") {
            return None;
        }
        let declared = items.get(1)?;
        let FormKind::Symbol(name) = &declared.kind else {
            return None;
        };
        (name.canonical == canonical).then_some(declared.span)
    }
    forms
        .iter()
        .filter_map(|form| search(form, item, canonical))
        .min_by_key(|span| span.end.saturating_sub(span.start))
}

/// Projects macros as semantic symbols.
///
/// Macros are erased before typed HIR, so they are absent from the binding
/// list every other symbol comes from. Their declarations survive in the
/// lowered surface, and expansion records the span of each call it rewrote, so
/// both halves of a symbol are recoverable. Without this, every surface built
/// on the semantic model — hover, navigation, the workspace symbol index and
/// the graph derived from it — is blind to macros.
///
/// A module that only *calls* a macro still gets a symbol for it, carrying the
/// call sites, so a reference resolves in the file the reader is looking at.
/// Documentation lives on the declaring module's symbol.
fn macro_symbols(analysis: &Analysis) -> Vec<SemanticSymbol> {
    let module = analysis.hir.name.as_str();
    let mut call_sites = BTreeMap::<String, Vec<Span>>::new();
    for trace in &analysis.expansion_traces {
        // Only the outermost rewrite corresponds to source the author wrote;
        // nested expansions describe generated syntax.
        if trace.depth != 0 {
            continue;
        }
        // The trace spans the whole call form. Narrow it to the name the
        // author typed so an occurrence means the same thing it does for every
        // other symbol, and so a wide call does not answer for every position
        // inside it.
        let span =
            macro_name_span(&analysis.document.forms, trace.call_span).unwrap_or(trace.call_span);
        call_sites
            .entry(trace.macro_binding_id.clone())
            .or_default()
            .push(span);
    }

    let mut symbols = Vec::new();
    let mut declared = BTreeSet::new();
    for item in &analysis.surface.items {
        let crate::ast::ItemKind::Defmacro(declaration) = &item.kind else {
            continue;
        };
        let binding_id =
            crate::name::BindingId::new(module, &declaration.name.canonical, BindingKind::Macro);
        let id = binding_id.as_str().to_owned();
        declared.insert(id.clone());
        let references = call_sites.remove(&id).unwrap_or_default();
        let public = analysis
            .hir
            .exports
            .iter()
            .any(|export| export.as_str() == binding_id.as_str());
        // The surface keeps one span for the whole declaration form; narrow it
        // to the declared name so a definition lands where the reader expects.
        let definition = macro_declaration_name_span(
            &analysis.document.forms,
            item.span,
            &declaration.name.canonical,
        )
        .unwrap_or(item.span);
        symbols.push(macro_symbol(
            id,
            &declaration.name.canonical,
            &declaration.name.spelling,
            &declaration.metadata,
            definition,
            references,
            public,
        ));
    }

    // Macros defined elsewhere: the call sites are here, the declaration is not.
    for (id, references) in call_sites {
        if declared.contains(&id) {
            continue;
        }
        let Some(canonical) = id.rsplit("::").next() else {
            continue;
        };
        let Some(first) = references.first().copied() else {
            continue;
        };
        symbols.push(macro_symbol(
            id.clone(),
            canonical,
            canonical,
            &[],
            first,
            references,
            false,
        ));
    }
    symbols
}

fn macro_symbol(
    binding_id: String,
    canonical: &str,
    spelling: &str,
    metadata: &[crate::syntax::MetadataEntry],
    definition: Span,
    references: Vec<Span>,
    public: bool,
) -> SemanticSymbol {
    let summary = SemanticSummary::unknown();
    let layers = layers_for_metadata(metadata, definition, &summary);
    let localized = localized_names(metadata);
    let labels = labels_for_name(canonical, &localized);
    let mut occurrences = references.clone();
    if !occurrences.contains(&definition) {
        occurrences.push(definition);
    }
    occurrences.sort_by_key(|span| (span.start, span.end));
    occurrences.dedup();
    SemanticSymbol {
        binding_id,
        canonical: canonical.to_owned(),
        source: spelling.to_owned(),
        source_spelling: spelling.to_owned(),
        // A macro has no runtime name; it never reaches generated Python.
        python: String::new(),
        kind: BindingKind::Macro,
        aliases: Vec::new(),
        public,
        // Macros run in phase 1 and have no runtime type.
        ty: Type::Unknown,
        metadata: layers,
        summary,
        labels,
        names: SemanticNames {
            canonical: canonical.to_owned(),
            localized,
        },
        documentation: documentation(metadata),
        examples: examples(metadata),
        content_references: Vec::new(),
        span: definition,
        definition,
        references,
        occurrences,
    }
}
