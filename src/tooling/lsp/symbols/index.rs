pub(super) fn build_single_symbol_index(
    analysis: &Analysis,
    uri: &str,
    source: &str,
) -> WorkspaceSymbolIndex {
    let mut index = WorkspaceSymbolIndex::default();
    let semantic = SemanticDocument::from_analysis(analysis, uri);
    index_analysis_symbols(&mut index, analysis, semantic, uri, source);
    finish_symbol_index(&mut index);
    index
}

pub(super) fn build_project_symbol_index(
    analyses: &[Analysis],
    buffers: &[WorkspaceBuffer],
) -> WorkspaceSymbolIndex {
    // Indexing one module reads only that module, so every module is indexed
    // in parallel into its own partial index. Only the merge below is serial,
    // and it replays the shared index's conflict rules in buffer order, so the
    // result is identical to indexing the modules one at a time.
    //
    // The partials are deliberately not retained across edits: merging moves
    // them into the shared index, and reusing them would mean deep-copying
    // every module's symbols on each merge instead, which costs more than
    // rebuilding the unchanged ones in parallel.
    let partials = analyses
        .par_iter()
        .zip(buffers)
        .map(|(analysis, buffer)| {
            let mut partial = WorkspaceSymbolIndex::default();
            let semantic = SemanticDocument::from_analysis(analysis, &buffer.uri);
            index_analysis_symbols(
                &mut partial,
                analysis,
                semantic,
                &buffer.uri,
                &buffer.source,
            );
            partial
        })
        .collect::<Vec<_>>();
    let mut index = WorkspaceSymbolIndex::default();
    for partial in partials {
        merge_symbol_index(&mut index, partial);
    }
    finish_symbol_index(&mut index);
    index
}

/// Folds one module's partial index into the shared index.
///
/// Ambiguity is resolved exactly as it is when a module is indexed directly
/// into the shared index: a binding defined in two places loses its definition
/// and is recorded as ambiguous, and the first module to claim a provider name
/// or binding kind keeps it.
pub(super) fn merge_symbol_index(
    index: &mut WorkspaceSymbolIndex,
    partial: WorkspaceSymbolIndex,
) {
    index.source_uris.extend(partial.source_uris);
    index.sources.extend(partial.sources);

    // A binding the module itself found ambiguous stays ambiguous globally.
    for binding_id in partial.ambiguous_definitions {
        index.definitions.remove(&binding_id);
        index.ambiguous_definitions.insert(binding_id);
    }
    for (binding_id, definition) in partial.definitions {
        if index.ambiguous_definitions.contains(&binding_id) {
            continue;
        }
        match index.definitions.get(&binding_id) {
            Some(existing) if existing != &definition => {
                index.definitions.remove(&binding_id);
                index.ambiguous_definitions.insert(binding_id);
            }
            Some(_) => {}
            None => {
                index.definitions.insert(binding_id, definition);
            }
        }
    }

    for (binding_id, kind) in partial.binding_kinds {
        index.binding_kinds.entry(binding_id).or_insert(kind);
    }
    for (binding_id, locations) in partial.references {
        index
            .references
            .entry(binding_id)
            .or_default()
            .extend(locations);
    }
    for (binding_id, occurrences) in partial.rename_occurrences {
        index
            .rename_occurrences
            .entry(binding_id)
            .or_default()
            .extend(occurrences);
    }

    for key in partial.ambiguous_provider_names {
        index.provider_names.remove(&key);
        index.ambiguous_provider_names.insert(key);
    }
    for (key, binding_id) in partial.provider_names {
        if index.ambiguous_provider_names.contains(&key) {
            continue;
        }
        match index.provider_names.get(&key) {
            Some(existing) if existing != &binding_id => {
                index.provider_names.remove(&key);
                index.ambiguous_provider_names.insert(key);
            }
            Some(_) => {}
            None => {
                index.provider_names.insert(key, binding_id);
            }
        }
    }

    index
        .pending_import_members
        .extend(partial.pending_import_members);
    index.semantic_symbols.extend(partial.semantic_symbols);
    index.relations.extend(partial.relations);
}

pub(super) fn index_analysis_symbols(
    index: &mut WorkspaceSymbolIndex,
    analysis: &Analysis,
    semantic: SemanticDocument,
    uri: &str,
    source: &str,
) {
    index.source_uris.insert(uri.to_owned());
    index.sources.insert(uri.to_owned(), source.to_owned());
    let lines = LineIndex::new(source);
    index_module_relations(index, analysis, &semantic, uri, source, &lines);
    let local_prefix = format!("{}::", analysis.hir.name);
    for symbol in &semantic.symbols {
        index
            .binding_kinds
            .entry(symbol.binding_id.clone())
            .or_insert(symbol.kind);
        if symbol.binding_id.starts_with(&local_prefix)
            && !index.ambiguous_definitions.contains(&symbol.binding_id)
        {
            let definition = Location {
                uri: uri.to_owned(),
                range: lines.range(source, symbol.definition),
            };
            match index.definitions.get(&symbol.binding_id) {
                Some(existing) if existing != &definition => {
                    index.definitions.remove(&symbol.binding_id);
                    index
                        .ambiguous_definitions
                        .insert(symbol.binding_id.clone());
                }
                Some(_) => {}
                None => {
                    index
                        .definitions
                        .insert(symbol.binding_id.clone(), definition);
                }
            }
        }
        index
            .references
            .entry(symbol.binding_id.clone())
            .or_default()
            .extend(symbol.occurrences.iter().copied().map(|span| Location {
                uri: uri.to_owned(),
                range: lines.range(source, span),
            }));
        index_symbol_rename_occurrences(index, analysis, symbol, uri, source);
        if symbol.public && symbol.binding_id.starts_with(&local_prefix) {
            record_provider_name(
                index,
                &analysis.hir.name,
                &symbol.canonical,
                &symbol.binding_id,
            );
            record_provider_name(
                index,
                &analysis.hir.name,
                &symbol.source_spelling,
                &symbol.binding_id,
            );
            for alias in symbol.aliases.iter().filter(|alias| alias.public) {
                record_provider_name(
                    index,
                    &analysis.hir.name,
                    &alias.spelling,
                    &symbol.binding_id,
                );
            }
        }
    }
    index_declaration_references(index, analysis, &semantic, uri, source);
    index
        .semantic_symbols
        .extend(semantic.symbols.into_iter().map(|symbol| WorkspaceSemanticSymbol {
            uri: uri.to_owned(),
            symbol,
        }));
}

fn index_module_relations(
    index: &mut WorkspaceSymbolIndex,
    analysis: &Analysis,
    semantic: &SemanticDocument,
    uri: &str,
    source: &str,
    lines: &LineIndex,
) {
    let module = format!("module:{}", analysis.hir.name);
    for item in &analysis.hir.items {
        match &item.kind {
            hir::ItemKind::Import(import) if !import.python => {
                index.relations.push(WorkspaceRelation {
                    from: module.clone(),
                    to: format!("module:{}", import.module),
                    kind: "imports".to_owned(),
                    uri: uri.to_owned(),
                    range: lines.range(source, item.span),
                });
            }
            hir::ItemKind::Function(function) => {
                for operation in semantic.operation_graph.nodes.iter().filter(|operation| {
                    operation.kind == "call"
                        && operation.span.start >= item.span.start
                        && operation.span.end <= item.span.end
                }) {
                    if let Some(target) = &operation.binding_id {
                        index.relations.push(WorkspaceRelation {
                            from: function.binding.as_str().to_owned(),
                            to: target.clone(),
                            kind: "calls".to_owned(),
                            uri: uri.to_owned(),
                            range: lines.range(source, operation.span),
                        });
                    }
                }
            }
            _ => {}
        }
    }
    for binding in &analysis.hir.exports {
        index.relations.push(WorkspaceRelation {
            from: module.clone(),
            to: binding.as_str().to_owned(),
            kind: "exports".to_owned(),
            uri: uri.to_owned(),
            range: Range::default(),
        });
    }
    for alias in &analysis.hir.aliases {
        index.relations.push(WorkspaceRelation {
            from: format!("alias:{}:{}", analysis.hir.name, alias.spelling),
            to: alias.target.as_str().to_owned(),
            kind: "alias-of".to_owned(),
            uri: uri.to_owned(),
            range: lines.range(source, alias.span),
        });
    }
    for symbol in &semantic.symbols {
        for reference in &symbol.references {
            let owner = analysis
                .hir
                .items
                .iter()
                .filter(|item| {
                    item.span.start <= reference.start && reference.end <= item.span.end
                })
                .min_by_key(|item| item.span.end.saturating_sub(item.span.start))
                .and_then(|item| match &item.kind {
                    hir::ItemKind::Function(function) => {
                        Some(function.binding.as_str().to_owned())
                    }
                    hir::ItemKind::Value(value) => Some(value.binding.as_str().to_owned()),
                    _ => None,
                })
                .unwrap_or_else(|| module.clone());
            index.relations.push(WorkspaceRelation {
                from: owner,
                to: symbol.binding_id.clone(),
                kind: "references".to_owned(),
                uri: uri.to_owned(),
                range: lines.range(source, *reference),
            });
        }
    }
}

pub(super) fn index_symbol_rename_occurrences(
    index: &mut WorkspaceSymbolIndex,
    analysis: &Analysis,
    symbol: &SemanticSymbol,
    uri: &str,
    source: &str,
) {
    let local_prefix = format!("{}::", analysis.hir.name);
    if symbol.binding_id.starts_with(&local_prefix)
        && let Some(form) = definition_name_form(
            &analysis.document.forms,
            symbol.definition,
            &symbol.source_spelling,
        )
        && let Some((span, spelling)) = rename_member_from_form(source, form)
    {
        push_rename_occurrence(
            index,
            &symbol.binding_id,
            RenameOccurrence {
                uri: uri.to_owned(),
                span,
                spelling,
                declaration: true,
            },
        );
    }
    for reference in &symbol.references {
        let Some(form) = exact_symbol_form(&analysis.document.forms, *reference) else {
            continue;
        };
        let Some((span, spelling)) = rename_member_from_form(source, form) else {
            continue;
        };
        push_rename_occurrence(
            index,
            &symbol.binding_id,
            RenameOccurrence {
                uri: uri.to_owned(),
                span,
                spelling,
                declaration: false,
            },
        );
    }
    for alias in &symbol.aliases {
        let Some(form) = exact_container_form(&analysis.document.forms, alias.span) else {
            continue;
        };
        let Some(local) =
            list_item(form, 1).filter(|form| symbol_form_matches(form, &alias.spelling))
        else {
            continue;
        };
        let Some((span, spelling)) = rename_member_from_form(source, local) else {
            continue;
        };
        push_rename_occurrence(
            index,
            &symbol.binding_id,
            RenameOccurrence {
                uri: uri.to_owned(),
                span,
                spelling,
                declaration: true,
            },
        );
    }
}

pub(super) fn index_declaration_references(
    index: &mut WorkspaceSymbolIndex,
    analysis: &Analysis,
    semantic: &SemanticDocument,
    uri: &str,
    source: &str,
) {
    for item in &analysis.surface.items {
        let Some(form) = exact_container_form(&analysis.document.forms, item.span) else {
            continue;
        };
        match &item.kind {
            crate::ast::ItemKind::Alias(alias) => {
                let Some(resolved) = analysis.hir.aliases.iter().find(|resolved| {
                    resolved.span == alias.span
                        && resolved.spelling.nfc().eq(alias.local.spelling.nfc())
                }) else {
                    continue;
                };
                let Some(target) = list_item(form, 2) else {
                    continue;
                };
                if let Some((span, spelling)) = rename_member_from_form(source, target) {
                    push_rename_occurrence(
                        index,
                        resolved.target.as_str(),
                        RenameOccurrence {
                            uri: uri.to_owned(),
                            span,
                            spelling,
                            declaration: false,
                        },
                    );
                }
            }
            crate::ast::ItemKind::Export(export) => {
                let Some(names) = list_item(form, 1).and_then(collection_items) else {
                    continue;
                };
                for (name, name_form) in export.names.iter().zip(names) {
                    let mut bindings = semantic
                        .symbols
                        .iter()
                        .filter(|symbol| {
                            symbol.public && semantic_symbol_accepts(symbol, &name.spelling)
                        })
                        .map(|symbol| symbol.binding_id.as_str());
                    let Some(binding_id) = bindings.next() else {
                        continue;
                    };
                    if bindings.any(|candidate| candidate != binding_id) {
                        continue;
                    }
                    if let Some((span, spelling)) = rename_member_from_form(source, name_form) {
                        push_rename_occurrence(
                            index,
                            binding_id,
                            RenameOccurrence {
                                uri: uri.to_owned(),
                                span,
                                spelling,
                                declaration: false,
                            },
                        );
                    }
                }
            }
            crate::ast::ItemKind::Import(import) => {
                let members = import_member_forms(form);
                for (_name, name_form) in import.members.iter().zip(members) {
                    let Some((span, spelling)) = rename_member_from_form(source, name_form) else {
                        continue;
                    };
                    index.pending_import_members.push(PendingImportMember {
                        uri: uri.to_owned(),
                        provider: import.module.canonical.clone(),
                        spelling,
                        span,
                    });
                }
            }
            _ => {}
        }
    }
}

pub(super) fn record_provider_name(
    index: &mut WorkspaceSymbolIndex,
    module: &str,
    spelling: &str,
    binding_id: &str,
) {
    let key = (module.to_owned(), spelling.nfc().collect::<String>());
    if index.ambiguous_provider_names.contains(&key) {
        return;
    }
    match index.provider_names.get(&key) {
        Some(existing) if existing != binding_id => {
            index.provider_names.remove(&key);
            index.ambiguous_provider_names.insert(key);
        }
        Some(_) => {}
        None => {
            index.provider_names.insert(key, binding_id.to_owned());
        }
    }
}

pub(super) fn push_rename_occurrence(
    index: &mut WorkspaceSymbolIndex,
    binding_id: &str,
    occurrence: RenameOccurrence,
) {
    index
        .rename_occurrences
        .entry(binding_id.to_owned())
        .or_default()
        .push(occurrence);
}
