pub(super) fn build_single_symbol_index(
    analysis: &Analysis,
    uri: &str,
    source: &str,
) -> WorkspaceSymbolIndex {
    let mut index = WorkspaceSymbolIndex::default();
    index_analysis_symbols(&mut index, analysis, uri, source);
    finish_symbol_index(&mut index);
    index
}

pub(super) fn build_project_symbol_index(
    analyses: &[Analysis],
    buffers: &[WorkspaceBuffer],
) -> WorkspaceSymbolIndex {
    let mut index = WorkspaceSymbolIndex::default();
    for (analysis, buffer) in analyses.iter().zip(buffers) {
        index_analysis_symbols(&mut index, analysis, &buffer.uri, &buffer.source);
    }
    finish_symbol_index(&mut index);
    index
}

pub(super) fn index_analysis_symbols(
    index: &mut WorkspaceSymbolIndex,
    analysis: &Analysis,
    uri: &str,
    source: &str,
) {
    index.source_uris.insert(uri.to_owned());
    index.sources.insert(uri.to_owned(), source.to_owned());
    let semantic = SemanticDocument::from_analysis(analysis, uri);
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
                range: span_to_range(source, symbol.definition),
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
                range: span_to_range(source, span),
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
