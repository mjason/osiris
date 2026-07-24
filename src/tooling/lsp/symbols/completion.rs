pub(super) fn completion_items(
    symbol: &SemanticSymbol,
    locale: Option<&str>,
) -> Vec<CompletionItem> {
    let localized_name = localized_name_for(&symbol.names.localized, locale);
    let label = localized_name.map_or_else(
        || symbol.source_spelling.clone(),
        |name| name.preferred.clone(),
    );
    let insert_text = label.clone();
    let localized = localized_name.is_some();
    let primary = CompletionItem {
        label,
        kind: completion_kind(symbol.kind),
        detail: format!("{} : {}", symbol.canonical, symbol.ty),
        insert_text: insert_text.clone(),
        sort_text: format!("{}:{}", if localized { 0 } else { 1 }, symbol.canonical),
        filter_text: format!(
            "{} {} {}",
            symbol.canonical,
            symbol.source_spelling,
            symbol
                .aliases
                .iter()
                .map(|alias| alias.spelling.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        ),
        data: json!({
            "bindingId": symbol.binding_id,
            "canonical": symbol.canonical,
            "insertedAlias": localized_name.map(|name| name.preferred.as_str()),
        }),
    };
    let mut items = vec![primary];
    for alias in &symbol.aliases {
        if items
            .iter()
            .any(|item| item.insert_text == alias.spelling)
        {
            continue;
        }
        items.push(CompletionItem {
            label: alias.spelling.clone(),
            kind: completion_kind(symbol.kind),
            detail: format!("{} : {}", symbol.canonical, symbol.ty),
            insert_text: alias.spelling.clone(),
            sort_text: format!(
                "{}:{}:{}",
                if alias.preferred { 0 } else { 1 },
                symbol.canonical,
                alias.canonical
            ),
            filter_text: format!(
                "{} {} {}",
                symbol.canonical, symbol.source_spelling, alias.spelling
            ),
            data: json!({
                "bindingId": symbol.binding_id,
                "canonical": symbol.canonical,
                "insertedAlias": alias.spelling,
            }),
        });
    }
    items
}

pub(super) const fn completion_kind(kind: BindingKind) -> u8 {
    match kind {
        BindingKind::Function | BindingKind::Macro => 3,
        BindingKind::Type => 7,
        BindingKind::Module | BindingKind::PythonModule => 9,
        BindingKind::Field => 5,
        BindingKind::Parameter | BindingKind::Value => 6,
    }
}

pub(super) fn symbol_matches_prefix(symbol: &SemanticSymbol, prefix: &str) -> bool {
    prefix.is_empty()
        || symbol.canonical.starts_with(prefix)
        || symbol.source_spelling.starts_with(prefix)
        || symbol
            .aliases
            .iter()
            .any(|alias| alias.spelling.starts_with(prefix))
}

pub(super) fn completion_prefix(source: &str, offset: usize) -> String {
    let offset = offset.min(source.len());
    source[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, character)| {
            !character.is_whitespace()
                && !matches!(character, '(' | ')' | '[' | ']' | '{' | '}' | '"' | ',')
        })
        .last()
        .map_or_else(String::new, |(start, _)| source[start..offset].to_owned())
}
