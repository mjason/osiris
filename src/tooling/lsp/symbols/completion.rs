pub(super) fn completion_item(symbol: &SemanticSymbol, chinese: bool) -> CompletionItem {
    let actual_alias = chinese
        .then(|| {
            symbol
                .aliases
                .iter()
                .filter(|alias| contains_cjk(&alias.spelling))
                .min_by_key(|alias| (!alias.public, !alias.preferred, &alias.spelling))
        })
        .flatten();
    let insert_text = actual_alias.map_or_else(
        || symbol.source_spelling.clone(),
        |alias| alias.spelling.clone(),
    );
    let label = if chinese {
        actual_alias.map_or_else(
            || symbol.labels.zh_cn.clone(),
            |alias| alias.spelling.clone(),
        )
    } else {
        symbol.labels.en.clone()
    };
    let localized = chinese && (contains_cjk(&label) || contains_cjk(&insert_text));
    CompletionItem {
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
            "insertedAlias": actual_alias.map(|alias| alias.spelling.as_str()),
        }),
    }
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
