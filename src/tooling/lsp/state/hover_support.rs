fn workspace_symbol_for_binding<'a>(
    document: &'a OpenDocument,
    binding_id: &str,
) -> Option<&'a WorkspaceSemanticSymbol> {
    let definition_uri = document
        .workspace_symbols
        .definitions
        .get(binding_id)
        .map(|location| location.uri.as_str());
    document
        .workspace_symbols
        .semantic_symbols
        .iter()
        .filter(|entry| entry.symbol.binding_id == binding_id)
        .min_by_key(|entry| (Some(entry.uri.as_str()) != definition_uri, entry.uri.as_str()))
}

fn render_symbol_hover(
    document: &OpenDocument,
    symbol: &SemanticSymbol,
    locale: &str,
    authored: Option<&AuthoredSpelling>,
    range: Option<Range>,
) -> Hover {
    let label = symbol.labels.for_locale(locale);
    let (documentation, _) = symbol.documentation.for_locale(Some(locale));
    let legacy_aliases = symbol
        .aliases
        .iter()
        .filter(|alias| alias.role == crate::semantic::SemanticAliasRole::Migration)
        .map(|alias| alias.spelling.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let mut value = format!(
        "**{}** · {}",
        escape_markdown(label),
        binding_kind_label(symbol.kind, locale),
    );
    if !documentation.is_empty() {
        value.push_str(&format!("\n\n{}", escape_markdown(documentation)));
    }
    if let Some(signature) = callable_signature(document, &symbol.binding_id) {
        let parameters = signature
            .parameters
            .iter()
            .map(|parameter| signature_parameter_label(parameter, Some(locale)))
            .collect::<Vec<_>>()
            .join(" ");
        value.push_str(&format!(
            "\n\n**{}**\n\n```osiris\n({} {})\n```",
            localized_heading(locale, "Usage", "用法"),
            escape_markdown(label),
            escape_markdown(&parameters),
        ));
    }
    if !symbol.examples.is_empty() {
        value.push_str(&format!(
            "\n\n**{}**",
            localized_heading(locale, "Examples", "示例")
        ));
        for example in &symbol.examples {
            value.push_str(&format!(
                "\n\n```osiris\n{}\n```",
                example.join("\n")
            ));
        }
    }
    value.push_str(&format!(
        "\n\n**{}**  `{}`",
        localized_heading(locale, "Type", "类型"),
        escape_markdown(&symbol.ty.to_string())
    ));
    if let Some(authored) = authored
        && authored.role == AuthoredSpellingRole::Migration
    {
        let replacement = authored.replacement.as_deref().unwrap_or(&symbol.canonical);
        value.push_str(&format!(
            "\n\n**{}**  `{}` -> `{}`",
            localized_heading(locale, "Migration", "迁移提示"),
            escape_markdown(&authored.spelling),
            escape_markdown(replacement),
        ));
    }
    if !legacy_aliases.is_empty() {
        value.push_str(&format!(
            "\n\n**{}**  `{}`",
            localized_heading(locale, "Legacy aliases", "旧别名"),
            escape_markdown(&legacy_aliases)
        ));
    }
    value.push_str(&format!(
        "\n\n`{}`",
        escape_markdown(&qualified_canonical(symbol))
    ));
    Hover {
        contents: MarkupContent {
            kind: "markdown".to_owned(),
            value,
        },
        range,
    }
}

#[allow(clippy::too_many_arguments)]
fn symbol_hover_machine_projection(
    document: &OpenDocument,
    symbol: &SemanticSymbol,
    requested_locale: Option<&str>,
    effective_locale: &str,
    source_uri: Option<&str>,
    range: Option<Range>,
    provenance: &str,
    authored: Option<&AuthoredSpelling>,
) -> JsonValue {
    let (selected_doc, resolved_doc_locale) =
        symbol.documentation.for_locale(Some(effective_locale));
    let (selected_name, resolved_name_locale) = symbol.names.for_locale(Some(effective_locale));
    json!({
        "schema": "osiris.hover/v1",
        "bindingId": symbol.binding_id,
        "documentVersion": document.version,
        "kind": symbol.kind,
        "label": selected_name,
        "canonical": {
            "name": symbol.canonical,
            "qualified": qualified_canonical(symbol),
        },
        "documentation": {
            "default": symbol.documentation.default,
            "translations": symbol.documentation.translations,
            "selection": {
                "requestedLocale": requested_locale,
                "resolvedLocale": resolved_doc_locale,
                "text": selected_doc,
            },
        },
        "names": {
            "canonical": symbol.names.canonical,
            "localized": symbol.names.localized,
            "selection": {
                "requestedLocale": requested_locale,
                "resolvedLocale": resolved_name_locale,
                "label": selected_name,
            },
        },
        "aliases": symbol.aliases,
        "examples": symbol.examples,
        "contentReferences": symbol.content_references,
        "type": symbol.ty,
        "source": {
            "uri": source_uri,
            "range": range,
            "provenance": provenance,
        },
        "semantic": symbol.summary,
        "authoredSpelling": authored_spelling_json(authored),
    })
}


#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AuthoredSpellingRole {
    Canonical,
    Preferred,
    Migration,
    LocalRename,
}

impl AuthoredSpellingRole {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Canonical => "canonical",
            Self::Preferred => "preferred",
            Self::Migration => "migration",
            Self::LocalRename => "local-rename",
        }
    }
}

struct AuthoredSpelling {
    spelling: String,
    role: AuthoredSpellingRole,
    replacement: Option<String>,
    span: Span,
}

fn authored_spelling_at(
    document: &OpenDocument,
    symbol: &crate::semantic::SemanticSymbol,
    offset: usize,
    locale: &str,
) -> Option<AuthoredSpelling> {
    authored_spelling_for_span(document, symbol, occurrence_at(symbol, offset)?, locale)
}

fn authored_spelling_for_span(
    document: &OpenDocument,
    symbol: &crate::semantic::SemanticSymbol,
    span: Span,
    locale: &str,
) -> Option<AuthoredSpelling> {
    let source = document.text.get(span.start..span.end)?;
    let terminal_start = source
        .rmatch_indices(['/', '.'])
        .next()
        .map_or(0, |(index, separator)| index + separator.len());
    let spelling = source.get(terminal_start..)?.to_owned();
    if spelling.is_empty() {
        return None;
    }
    let role = symbol
        .names
        .localized
        .values()
        .find_map(|entry| {
            if entry.preferred == spelling {
                Some(AuthoredSpellingRole::Preferred)
            } else if entry.aliases.iter().any(|alias| alias == &spelling) {
                Some(AuthoredSpellingRole::Migration)
            } else {
                None
            }
        })
        .or_else(|| {
            symbol
                .aliases
                .iter()
                .find(|alias| alias.spelling == spelling || alias.canonical == spelling)
                .map(|alias| match alias.role {
                    crate::semantic::SemanticAliasRole::Preferred => {
                        AuthoredSpellingRole::Preferred
                    }
                    crate::semantic::SemanticAliasRole::Migration => {
                        AuthoredSpellingRole::Migration
                    }
                    crate::semantic::SemanticAliasRole::LocalRename => {
                        AuthoredSpellingRole::LocalRename
                    }
                })
        })
        .unwrap_or(AuthoredSpellingRole::Canonical);
    let replacement = (role == AuthoredSpellingRole::Migration).then(|| {
        let (preferred, _) = symbol.names.for_locale(Some(locale));
        preferred.to_owned()
    });
    Some(AuthoredSpelling {
        spelling,
        role,
        replacement,
        span: Span::new(span.start + terminal_start, span.end),
    })
}

fn authored_spelling_json(authored: Option<&AuthoredSpelling>) -> JsonValue {
    authored.map_or(JsonValue::Null, |authored| {
        json!({
            "text": authored.spelling,
            "role": authored.role.as_str(),
            "replacement": authored.replacement,
            "span": authored.span,
        })
    })
}

fn qualified_canonical(symbol: &crate::semantic::SemanticSymbol) -> String {
    let module = symbol.binding_id.split("::").next().unwrap_or_default();
    if module.is_empty() {
        symbol.canonical.clone()
    } else {
        format!("{module}/{}", symbol.canonical)
    }
}

fn binding_kind_label(kind: crate::name::BindingKind, locale: &str) -> &'static str {
    let chinese = locale == "zh" || locale.starts_with("zh-");
    match (kind, chinese) {
        (crate::name::BindingKind::Module, true) => "模块",
        (crate::name::BindingKind::Value, true) => "值",
        (crate::name::BindingKind::Function, true) => "函数",
        (crate::name::BindingKind::Type, true) => "类型",
        (crate::name::BindingKind::Field, true) => "字段",
        (crate::name::BindingKind::Parameter, true) => "参数",
        (crate::name::BindingKind::Macro, true) => "宏",
        (crate::name::BindingKind::PythonModule, true) => "Python 模块",
        (crate::name::BindingKind::Module, false) => "Module",
        (crate::name::BindingKind::Value, false) => "Value",
        (crate::name::BindingKind::Function, false) => "Function",
        (crate::name::BindingKind::Type, false) => "Type",
        (crate::name::BindingKind::Field, false) => "Field",
        (crate::name::BindingKind::Parameter, false) => "Parameter",
        (crate::name::BindingKind::Macro, false) => "Macro",
        (crate::name::BindingKind::PythonModule, false) => "Python module",
    }
}

fn localized_heading<'a>(locale: &str, english: &'a str, chinese: &'a str) -> &'a str {
    if locale == "zh" || locale.starts_with("zh-") {
        chinese
    } else {
        english
    }
}

fn evaluation_behavior(evaluation: &str, locale: &str) -> Option<&'static str> {
    let chinese = locale == "zh" || locale.starts_with("zh-");
    match (evaluation, chinese) {
        ("consumer", true) => Some("立即消费输入集合。"),
        ("consumer", false) => Some("Consumes its input eagerly."),
        ("lazy", true) => Some("按需生成结果。"),
        ("lazy", false) => Some("Produces results lazily."),
        _ => None,
    }
}

fn label_for_symbol<'a>(symbol: &'a crate::semantic::SemanticSymbol, locale: &str) -> &'a str {
    symbol.labels.for_locale(locale)
}

