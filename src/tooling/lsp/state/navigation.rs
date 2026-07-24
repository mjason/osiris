impl LspState {
    #[must_use]
    pub fn definition(&self, uri: &str, position: Position) -> Option<Location> {
        let document = self.document(uri)?;
        let offset = position_to_offset(&document.text, position)?;
        let symbol = document.semantic.symbol_at_source(offset, &document.text)?;
        document
            .workspace_symbols
            .definitions
            .get(&symbol.binding_id)
            .cloned()
            .or_else(|| {
                let record = crate::stdlib::query_api(&symbol.binding_id, None)
                    .into_iter()
                    .next()?;
                Some(Location {
                    uri: record.api.source.uri,
                    range: Range {
                        start: Position {
                            line: record.api.source.line.saturating_sub(1),
                            character: record.api.source.column.saturating_sub(1),
                        },
                        end: Position {
                            line: record.api.source.line.saturating_sub(1),
                            character: record.api.source.column.saturating_sub(1)
                                + record.api.canonical.chars().count() as u32,
                        },
                    },
                })
            })
    }

    #[must_use]
    pub fn references(&self, uri: &str, position: Position) -> Vec<Location> {
        let Some(document) = self.document(uri) else {
            return Vec::new();
        };
        let Some(offset) = position_to_offset(&document.text, position) else {
            return Vec::new();
        };
        let Some(symbol) = document.semantic.symbol_at_source(offset, &document.text) else {
            return Vec::new();
        };
        document
            .workspace_symbols
            .references
            .get(&symbol.binding_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Returns the exact source spelling which can be renamed at `position`.
    /// Qualified references deliberately expose only their member component.
    #[must_use]
    pub fn prepare_rename(&self, uri: &str, position: Position) -> Option<Range> {
        let document = self.document(uri)?;
        let offset = position_to_offset(&document.text, position)?;
        let (binding_id, occurrence) = rename_target(&document.workspace_symbols, uri, offset)?;
        (document
            .workspace_symbols
            .definitions
            .contains_key(binding_id)
            && rename_kind_supported(&document.workspace_symbols, binding_id)
            && rename_group_has_declaration(
                &document.workspace_symbols,
                binding_id,
                &occurrence.spelling,
            ))
        .then(|| span_to_range(&document.text, occurrence.span))
    }

    /// Builds a deterministic, source-only workspace edit for one spelling of
    /// a stable binding. Aliases are independent spelling groups even though
    /// they resolve to the same `BindingId`.
    pub fn rename(
        &self,
        uri: &str,
        position: Position,
        new_name: &str,
    ) -> Result<Option<WorkspaceEdit>, LspStateError> {
        let Some(document) = self.document(uri) else {
            return Err(document_not_found(uri));
        };
        let Some(offset) = position_to_offset(&document.text, position) else {
            return Err(LspStateError::new(
                INVALID_PARAMS,
                "rename position is outside the document",
            ));
        };
        let Some((binding_id, selected)) = rename_target(&document.workspace_symbols, uri, offset)
        else {
            return Ok(None);
        };
        if !document
            .workspace_symbols
            .definitions
            .contains_key(binding_id)
            || !rename_kind_supported(&document.workspace_symbols, binding_id)
        {
            return Ok(None);
        }

        let new_name = normalize_rename_name(new_name)?;
        if is_reserved_rename_name(&new_name)
            || document
                .macro_interfaces
                .values()
                .any(|macro_| macro_.canonical == new_name)
            || document_declares_phase_name(document, &new_name)
        {
            return Err(LspStateError::new(
                INVALID_PARAMS,
                format!("newName `{new_name}` is reserved by Osiris syntax or a macro"),
            ));
        }
        let selected_spelling = selected.spelling.nfc().collect::<String>();
        reject_rename_collision(
            &document.workspace_symbols,
            binding_id,
            &selected_spelling,
            &new_name,
        )?;

        let mut spans = BTreeSet::<(String, usize, usize)>::new();
        let mut grouped = BTreeMap::<String, Vec<(Span, TextEdit)>>::new();
        for occurrence in document
            .workspace_symbols
            .rename_occurrences
            .get(binding_id)
            .into_iter()
            .flatten()
        {
            if occurrence.spelling.nfc().collect::<String>() != selected_spelling
                || !document
                    .workspace_symbols
                    .source_uris
                    .contains(&occurrence.uri)
            {
                continue;
            }
            let Some(source) = document.workspace_symbols.sources.get(&occurrence.uri) else {
                continue;
            };
            if occurrence.span.end > source.len()
                || !source.is_char_boundary(occurrence.span.start)
                || !source.is_char_boundary(occurrence.span.end)
            {
                continue;
            }
            if !spans.insert((
                occurrence.uri.clone(),
                occurrence.span.start,
                occurrence.span.end,
            )) {
                continue;
            }
            grouped.entry(occurrence.uri.clone()).or_default().push((
                occurrence.span,
                TextEdit {
                    range: span_to_range(source, occurrence.span),
                    new_text: new_name.clone(),
                },
            ));
        }

        let mut changes = BTreeMap::new();
        for (edit_uri, mut edits) in grouped {
            edits.sort_by_key(|(span, _)| (span.start, span.end));
            if edits.windows(2).any(|pair| pair[0].0.end > pair[1].0.start) {
                return Err(LspStateError::new(
                    INTERNAL_ERROR,
                    "rename produced overlapping source edits",
                ));
            }
            changes.insert(edit_uri, edits.into_iter().map(|(_, edit)| edit).collect());
        }
        Ok((!changes.is_empty()).then_some(WorkspaceEdit { changes }))
    }

    #[must_use]
    pub fn expand_preview(&self, uri: &str) -> Option<ExpandPreview> {
        let document = self.document(uri)?;
        Some(ExpandPreview {
            uri: uri.to_owned(),
            version: document.version,
            text: render_document_text(&document.analysis.expanded_document),
            macro_traces: document.semantic.macro_traces.clone(),
            diagnostics: self.diagnostics(uri)?.diagnostics,
        })
    }

    #[must_use]
    pub fn symbols(&self, uri: &str, query: Option<&str>) -> Option<Vec<JsonValue>> {
        let document = self.document(uri)?;
        let mut symbols = document
            .semantic
            .symbols
            .iter()
            .filter(|symbol| {
                let Some(query) = query.filter(|query| !query.is_empty()) else {
                    return true;
                };
                symbol.binding_id == query
                    || symbol.canonical == query
                    || symbol.source_spelling == query
                    || symbol
                        .aliases
                        .iter()
                        .any(|alias| alias.spelling == query || alias.canonical == query)
            })
            .collect::<Vec<_>>();
        symbols.sort_by_key(|symbol| &symbol.binding_id);
        Some(
            symbols
                .into_iter()
                .filter_map(|symbol| serde_json::to_value(symbol).ok())
                .collect(),
        )
    }
}
