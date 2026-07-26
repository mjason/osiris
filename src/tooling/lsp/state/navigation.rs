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
        let query = query.filter(|query| !query.is_empty());
        let mut symbols = BTreeMap::<&str, (&SemanticSymbol, bool)>::new();
        for entry in &document.workspace_symbols.semantic_symbols {
            let symbol = &entry.symbol;
            if query.is_some_and(|query| !semantic_symbol_accepts(symbol, query)) {
                continue;
            }
            let provider = document
                .workspace_symbols
                .definitions
                .get(&symbol.binding_id)
                .is_some_and(|definition| definition.uri == entry.uri);
            match symbols.entry(&symbol.binding_id) {
                std::collections::btree_map::Entry::Vacant(slot) => {
                    slot.insert((symbol, provider));
                }
                std::collections::btree_map::Entry::Occupied(mut slot)
                    if provider && !slot.get().1 =>
                {
                    slot.insert((symbol, true));
                }
                _ => {}
            }
        }
        Some(
            symbols
                .into_iter()
                .filter_map(|(_, (symbol, _))| serde_json::to_value(symbol).ok())
                .collect(),
        )
    }

    /// Standard LSP workspace symbols enriched with compiler-owned semantic data.
    #[must_use]
    pub fn workspace_symbols(&self, query: &str) -> Vec<JsonValue> {
        let Some(document) = self.documents.values().next() else {
            return Vec::new();
        };
        let mut symbols = BTreeMap::<&str, (&WorkspaceSemanticSymbol, bool)>::new();
        for entry in &document.workspace_symbols.semantic_symbols {
            let provider = document
                .workspace_symbols
                .definitions
                .get(&entry.symbol.binding_id)
                .is_some_and(|definition| definition.uri == entry.uri);
            match symbols.entry(&entry.symbol.binding_id) {
                std::collections::btree_map::Entry::Vacant(slot) => {
                    slot.insert((entry, provider));
                }
                std::collections::btree_map::Entry::Occupied(mut slot)
                    if provider && !slot.get().1 =>
                {
                    slot.insert((entry, true));
                }
                _ => {}
            }
        }
        let mut matches = symbols
            .into_values()
            .filter_map(|(entry, _)| {
                let (score, reasons) = workspace_symbol_score(&entry.symbol, query)?;
                let location = if let Some(location) = document
                    .workspace_symbols
                    .definitions
                    .get(&entry.symbol.binding_id)
                {
                    location.clone()
                } else {
                    Location {
                        uri: entry.uri.clone(),
                        range: span_to_range(
                            document.workspace_symbols.sources.get(&entry.uri)?,
                            entry.symbol.definition,
                        ),
                    }
                };
                Some((
                    score,
                    entry.symbol.binding_id.clone(),
                    json!({
                        "name": entry.symbol.source_spelling,
                        "kind": lsp_symbol_kind(entry.symbol.kind),
                        "location": location,
                        "containerName": entry.symbol.binding_id.split("::").next().unwrap_or_default(),
                        "data": {
                            "bindingId": entry.symbol.binding_id,
                            "canonical": entry.symbol.canonical,
                            "type": entry.symbol.ty,
                            "documentation": entry.symbol.documentation,
                            "examples": entry.symbol.examples,
                            "names": entry.symbol.names,
                            "aliases": entry.symbol.aliases,
                            "summary": entry.symbol.summary,
                            "score": score,
                            "matchReasons": reasons,
                        },
                    }),
                ))
            })
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        matches.into_iter().map(|(_, _, value)| value).collect()
    }

    /// Standard LSP document symbols for definitions owned by one source URI.
    #[must_use]
    pub fn document_symbols(&self, uri: &str) -> Option<Vec<JsonValue>> {
        let document = self.document(uri)?;
        let source = document.workspace_symbols.sources.get(uri)?;
        let mut seen = BTreeSet::new();
        let mut symbols = document
            .workspace_symbols
            .semantic_symbols
            .iter()
            .filter(|entry| entry.uri == uri)
            .filter(|entry| seen.insert(entry.symbol.binding_id.clone()))
            .filter(|entry| {
                document
                    .workspace_symbols
                    .definitions
                    .get(&entry.symbol.binding_id)
                    .is_some_and(|location| location.uri == uri)
            })
            .map(|entry| {
                let range = span_to_range(source, entry.symbol.span);
                let selection_range = span_to_range(source, entry.symbol.definition);
                json!({
                    "name": entry.symbol.source_spelling,
                    "detail": entry.symbol.ty.to_string(),
                    "kind": lsp_symbol_kind(entry.symbol.kind),
                    "range": range,
                    "selectionRange": selection_range,
                    "data": {"bindingId": entry.symbol.binding_id},
                })
            })
            .collect::<Vec<_>>();
        symbols.sort_by_key(|symbol| {
            (
                symbol["range"]["start"]["line"].as_u64().unwrap_or_default(),
                symbol["range"]["start"]["character"]
                    .as_u64()
                    .unwrap_or_default(),
            )
        });
        Some(symbols)
    }

    /// Compiler-owned graph snapshot used by LSC's persistent project index.
    #[must_use]
    pub fn workspace_graph(&self) -> JsonValue {
        let Some(document) = self.documents.values().next() else {
            return json!({"nodes": [], "edges": []});
        };
        let symbol_nodes = self
            .workspace_symbols("")
            .into_iter()
            .map(|symbol| {
                json!({
                    "id": symbol["data"]["bindingId"],
                    "kind": "symbol",
                    "name": symbol["name"],
                    "module": symbol["containerName"],
                    "location": symbol["location"],
                    "type": symbol["data"]["type"],
                    "documentation": symbol["data"]["documentation"],
                    "examples": symbol["data"]["examples"],
                    "names": symbol["data"]["names"],
                    "aliases": symbol["data"]["aliases"],
                    "summary": symbol["data"]["summary"],
                })
            })
            .collect::<Vec<_>>();
        let mut synthetic = BTreeMap::<String, JsonValue>::new();
        for relation in &document.workspace_symbols.relations {
            for id in [&relation.from, &relation.to] {
                if let Some(module) = id.strip_prefix("module:") {
                    synthetic.entry(id.clone()).or_insert_with(|| {
                        json!({
                            "id": id,
                            "kind": "module",
                            "name": module,
                            "module": module,
                            "location": null,
                        })
                    });
                } else if let Some(alias) = id.strip_prefix("alias:") {
                    let (module, name) = alias.rsplit_once(':').unwrap_or(("", alias));
                    synthetic.entry(id.clone()).or_insert_with(|| {
                        json!({
                            "id": id,
                            "kind": "alias",
                            "name": name,
                            "module": module,
                            "location": null,
                        })
                    });
                }
            }
        }
        let mut nodes = synthetic.into_values().collect::<Vec<_>>();
        nodes.extend(symbol_nodes);
        nodes.sort_by(|left, right| left["id"].as_str().cmp(&right["id"].as_str()));
        json!({
            "schema": "osiris.workspace-graph/v1",
            "nodes": nodes,
            "edges": document.workspace_symbols.relations,
        })
    }
}

fn workspace_symbol_score(symbol: &SemanticSymbol, query: &str) -> Option<(u16, Vec<&'static str>)> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some((1, vec!["all"]));
    }
    let mut score = 0;
    let mut reasons = Vec::new();
    for (value, reason, weight) in [
        (symbol.binding_id.as_str(), "binding-id", 120),
        (symbol.canonical.as_str(), "canonical-name", 110),
        (symbol.source_spelling.as_str(), "source-name", 105),
    ] {
        update_match_score(value, &query, weight, reason, &mut score, &mut reasons);
    }
    for alias in &symbol.aliases {
        update_match_score(
            &alias.spelling,
            &query,
            100,
            "alias",
            &mut score,
            &mut reasons,
        );
    }
    for names in symbol.names.localized.values() {
        update_match_score(
            &names.preferred,
            &query,
            100,
            "localized-name",
            &mut score,
            &mut reasons,
        );
        for alias in &names.aliases {
            update_match_score(
                alias,
                &query,
                90,
                "localized-alias",
                &mut score,
                &mut reasons,
            );
        }
    }
    if symbol
        .documentation
        .default
        .iter()
        .chain(symbol.documentation.translations.values())
        .any(|value| value.to_lowercase().contains(&query))
    {
        score = score.max(45);
        reasons.push("documentation");
    }
    if symbol
        .examples
        .iter()
        .flatten()
        .any(|value| value.to_lowercase().contains(&query))
    {
        score = score.max(35);
        reasons.push("example");
    }
    (score > 0).then_some((score, reasons))
}

fn update_match_score(
    value: &str,
    query: &str,
    weight: u16,
    reason: &'static str,
    score: &mut u16,
    reasons: &mut Vec<&'static str>,
) {
    let value = value.to_lowercase();
    let candidate = if value == query {
        weight
    } else if value.starts_with(query) {
        weight.saturating_sub(15)
    } else if value.contains(query) {
        weight.saturating_sub(30)
    } else {
        return;
    };
    *score = (*score).max(candidate);
    reasons.push(reason);
}

const fn lsp_symbol_kind(kind: BindingKind) -> u8 {
    match kind {
        BindingKind::Function => 12,
        BindingKind::Macro => 12,
        BindingKind::Type => 23,
        BindingKind::Field => 8,
        BindingKind::Module | BindingKind::PythonModule => 2,
        BindingKind::Parameter | BindingKind::Value => 13,
    }
}

fn semantic_symbol_accepts(symbol: &SemanticSymbol, query: &str) -> bool {
    if symbol.binding_id == query || semantic_symbol_accepts_spelling(symbol, query) {
        return true;
    }
    let module = symbol.binding_id.split("::").next().unwrap_or_default();
    [format!("{module}/"), format!("{module}.")]
        .iter()
        .find_map(|prefix| query.strip_prefix(prefix))
        .is_some_and(|spelling| semantic_symbol_accepts_spelling(symbol, spelling))
}

fn semantic_symbol_accepts_spelling(symbol: &SemanticSymbol, spelling: &str) -> bool {
    symbol.canonical == spelling
        || symbol.source_spelling == spelling
        || symbol
            .aliases
            .iter()
            .any(|alias| alias.spelling == spelling || alias.canonical == spelling)
        || symbol.names.localized.values().any(|entry| {
            entry.preferred == spelling || entry.aliases.iter().any(|alias| alias == spelling)
        })
}
