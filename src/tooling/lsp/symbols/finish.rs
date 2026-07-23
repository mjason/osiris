pub(super) fn finish_symbol_index(index: &mut WorkspaceSymbolIndex) {
    let pending = std::mem::take(&mut index.pending_import_members);
    for member in pending {
        let key = (
            member.provider.clone(),
            member.spelling.nfc().collect::<String>(),
        );
        if index.ambiguous_provider_names.contains(&key) {
            continue;
        }
        let Some(binding_id) = index.provider_names.get(&key).cloned() else {
            continue;
        };
        push_rename_occurrence(
            index,
            &binding_id,
            RenameOccurrence {
                uri: member.uri,
                span: member.span,
                spelling: member.spelling,
                declaration: false,
            },
        );
    }
    for references in index.references.values_mut() {
        references.sort_by(|left, right| {
            (
                &left.uri,
                left.range.start.line,
                left.range.start.character,
                left.range.end.line,
                left.range.end.character,
            )
                .cmp(&(
                    &right.uri,
                    right.range.start.line,
                    right.range.start.character,
                    right.range.end.line,
                    right.range.end.character,
                ))
        });
        references.dedup();
    }
    for occurrences in index.rename_occurrences.values_mut() {
        occurrences.sort_by(|left, right| {
            (
                &left.uri,
                left.span.start,
                left.span.end,
                &left.spelling,
                !left.declaration,
            )
                .cmp(&(
                    &right.uri,
                    right.span.start,
                    right.span.end,
                    &right.spelling,
                    !right.declaration,
                ))
        });
        occurrences.dedup_by(|left, right| {
            left.uri == right.uri
                && left.span == right.span
                && left.spelling == right.spelling
                && left.declaration == right.declaration
        });
    }
}

pub(super) fn collect_function_interfaces(
    analyses: &[Analysis],
    external_interfaces: &BTreeMap<String, Interface>,
) -> BTreeMap<String, interface::FunctionInterface> {
    let mut functions = external_interfaces
        .values()
        .flat_map(|interface| interface.functions.iter())
        .map(|function| (function.binding.clone(), function.clone()))
        .collect::<BTreeMap<_, _>>();
    for analysis in analyses {
        let Ok(interface) = interface::build_provisional(&analysis.surface) else {
            continue;
        };
        functions.extend(
            interface
                .functions
                .into_iter()
                .map(|function| (function.binding.clone(), function)),
        );
    }
    functions
}

pub(super) fn collect_macro_interfaces(
    analyses: &[Analysis],
    external_interfaces: &BTreeMap<String, Interface>,
) -> BTreeMap<String, interface::MacroInterface> {
    let mut macros = external_interfaces
        .values()
        .flat_map(|interface| interface.macros.iter())
        .map(|macro_| (macro_.id.clone(), macro_.clone()))
        .collect::<BTreeMap<_, _>>();
    for analysis in analyses {
        let Ok(interface) = interface::build_provisional(&analysis.surface) else {
            continue;
        };
        macros.extend(
            interface
                .macros
                .into_iter()
                .map(|macro_| (macro_.id.clone(), macro_)),
        );
    }
    macros
}
