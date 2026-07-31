impl LspState {
    pub fn hover(&self, uri: &str, position: Position, locale: Option<&str>) -> Option<Hover> {
        let document = self.document(uri)?;
        let offset = position_to_offset(&document.text, position)?;
        let locale = effective_display_locale(
            document,
            locale,
            self.session_locale.as_deref(),
            &self.display_locale,
        );
        let Some(symbol) = document.semantic.symbol_at_source(offset, &document.text) else {
            // A name the file referred through `import-for-syntax`, used
            // inside another macro's arguments: the expansion consumed its
            // occurrence, so it answers by its written name.
            if let Some((module, name)) = self.referred_member_at(document, offset) {
                for kind in ["macro", "function", "value", "type"] {
                    let id = format!("{module}::{kind}::{name}");
                    if let Some((hover, _)) = self.hover_for_binding(uri, &id, Some(locale))
                    {
                        return Some(hover);
                    }
                }
            }
            // Nothing here is a binding of its own. Inside a macro call the
            // useful answer is the macro: its clause keywords and shape are
            // documented there, and they exist nowhere else to point at.
            // Occurrences stay narrow so navigation and rename are unaffected;
            // only this fallback widens.
            return self.enclosing_macro_hover(document, offset, locale);
        };
        if let Some(standard) = crate::stdlib::query_api(&symbol.binding_id, Some(locale))
            .into_iter()
            .next()
        {
            let call_syntax = if standard.api.call_shapes.is_empty() {
                standard.api.signature.clone()
            } else {
                standard.api.call_shapes.join("\n")
            };
            let mut value = format!(
                "**{}** · {}\n\n{}\n\n**{}**\n\n```osiris\n{}\n```",
                escape_markdown(&standard.label),
                binding_kind_label(standard.api.kind, locale),
                escape_markdown(&standard.selected_documentation),
                localized_heading(locale, "Usage", "用法"),
                call_syntax,
            );
            if !standard.api.examples.is_empty() {
                value.push_str(&format!(
                    "\n\n**{}**",
                    localized_heading(locale, "Examples", "示例")
                ));
                for example in &standard.api.examples {
                    value.push_str(&format!(
                        "\n\n```osiris\n{}\n```",
                        example.join("\n")
                    ));
                }
            }
            if !standard.api.call_shapes.is_empty() {
                value.push_str(&format!(
                    "\n\n**{}**  `{}`",
                    localized_heading(locale, "Type", "类型"),
                    escape_markdown(&standard.api.signature)
                ));
            }
            if let Some(behavior) = evaluation_behavior(standard.api.evaluation, locale) {
                value.push_str(&format!(
                    "\n\n**{}**  {}",
                    localized_heading(locale, "Behavior", "行为"),
                    behavior
                ));
            }
            value.push_str(&format!(
                "\n\n`{}/{}`",
                standard.api.namespace,
                escape_markdown(standard.api.canonical)
            ));
            return Some(Hover {
                contents: MarkupContent {
                    kind: "markdown".to_owned(),
                    value,
                },
                range: occurrence_at(symbol, offset)
                    .map(|span| span_to_range(&document.text, span)),
            });
        }
        if symbol.kind == crate::name::BindingKind::PythonModule {
            let module = document
                .analysis
                .hir
                .bindings
                .iter()
                .find(|binding| binding.name.id.as_str() == symbol.binding_id)
                .and_then(|binding| binding.runtime.as_ref())
                .map_or(symbol.canonical.as_str(), |runtime| runtime.module.as_str());
            let explanation = if locale == "zh" || locale.starts_with("zh-") {
                format!(
                    "Python 模块 `{module}` 以 `{}` 引入。属性读取和调用保持 `Any`；需要静态类型时，请声明 typed `extern` 或安装提供 `.osri` 接口的扩展包。",
                    symbol.canonical
                )
            } else {
                format!(
                    "Python module `{module}` imported as `{}`. Attribute reads and calls remain `Any`; declare a typed `extern` or install an extension with a `.osri` interface when static types are required.",
                    symbol.canonical
                )
            };
            let value = format!(
                "**{}** · {}\n\n{}\n\n**{}**\n\n```osiris\n({}.attribute arguments...)\n```",
                escape_markdown(label_for_symbol(symbol, locale)),
                binding_kind_label(symbol.kind, locale),
                explanation,
                localized_heading(locale, "Example shape", "示例形式"),
                symbol.canonical,
            );
            return Some(Hover {
                contents: MarkupContent {
                    kind: "markdown".to_owned(),
                    value,
                },
                range: occurrence_at(symbol, offset)
                    .map(|span| span_to_range(&document.text, span)),
            });
        }
        let authored = authored_spelling_at(document, symbol, offset, locale);
        Some(render_symbol_hover(
            document,
            symbol,
            locale,
            authored.as_ref(),
            occurrence_at(symbol, offset).map(|span| span_to_range(&document.text, span)),
        ))
    }

    /// Describes the innermost macro call covering `offset`.
    fn enclosing_macro_hover(
        &self,
        document: &OpenDocument,
        offset: usize,
        locale: &str,
    ) -> Option<Hover> {
        let trace = document
            .analysis
            .expansion_traces
            .iter()
            .filter(|trace| span_contains(trace.call_span, offset))
            .min_by_key(|trace| trace.call_span.end.saturating_sub(trace.call_span.start))?;
        let entry = workspace_symbol_for_binding(document, &trace.macro_binding_id)?;
        Some(render_symbol_hover(
            document,
            &entry.symbol,
            locale,
            None,
            Some(span_to_range(&document.text, trace.call_span)),
        ))
    }

    /// Project a workspace binding without requiring the caller to know a
    /// source position. LSC name queries use the same hover record as LSP.
    #[must_use]
    pub fn hover_for_binding(
        &self,
        uri: &str,
        binding_id: &str,
        locale: Option<&str>,
    ) -> Option<(Hover, JsonValue)> {
        let document = self.document(uri)?;
        let entry = workspace_symbol_for_binding(document, binding_id)?;
        let symbol = &entry.symbol;
        let effective_locale = effective_display_locale(
            document,
            locale,
            self.session_locale.as_deref(),
            &self.display_locale,
        );
        let definition = document.workspace_symbols.definitions.get(binding_id);
        let range = definition.as_ref().map(|location| location.range);
        let source_uri = definition.map(|location| location.uri.as_str());
        let provenance = if definition.is_some() {
            "workspace-source"
        } else {
            "validated-interface"
        };
        let hover = render_symbol_hover(document, symbol, effective_locale, None, range);
        let machine = symbol_hover_machine_projection(
            document,
            symbol,
            locale,
            effective_locale,
            source_uri,
            range,
            provenance,
            None,
        );
        Some((hover, machine))
    }

    /// Return the full semantic symbol behind a position for non-LSP tooling
    /// projections such as `osr lsc hover --format json`.
    #[must_use]
    pub fn semantic_symbol_at(
        &self,
        uri: &str,
        position: Position,
    ) -> Option<&crate::semantic::SemanticSymbol> {
        let document = self.document(uri)?;
        let offset = position_to_offset(&document.text, position)?;
        document.semantic.symbol_at_source(offset, &document.text)
    }

    /// Return the operation-scoped machine projection used by LSC hover.
    #[must_use]
    pub fn hover_machine_projection(
        &self,
        uri: &str,
        position: Position,
        locale: Option<&str>,
    ) -> Option<JsonValue> {
        let document = self.document(uri)?;
        let offset = position_to_offset(&document.text, position)?;
        // Same fallback as `hover`: inside a macro call with no binding of its
        // own, the macro is the answer, and both surfaces must agree.
        let (symbol, range) = match document.semantic.symbol_at_source(offset, &document.text) {
            Some(symbol) => (
                symbol,
                occurrence_at(symbol, offset).map(|span| span_to_range(&document.text, span)),
            ),
            None => {
                let trace = document
                    .analysis
                    .expansion_traces
                    .iter()
                    .filter(|trace| span_contains(trace.call_span, offset))
                    .min_by_key(|trace| {
                        trace.call_span.end.saturating_sub(trace.call_span.start)
                    })?;
                let entry = workspace_symbol_for_binding(document, &trace.macro_binding_id)?;
                (
                    &entry.symbol,
                    Some(span_to_range(&document.text, trace.call_span)),
                )
            }
        };
        let effective_locale = effective_display_locale(
            document,
            locale,
            self.session_locale.as_deref(),
            &self.display_locale,
        );
        if let Some(standard) = crate::stdlib::query_api(&symbol.binding_id, Some(effective_locale))
            .into_iter()
            .next()
        {
            return Some(json!({
                "schema": "osiris.hover/v1",
                "bindingId": standard.api.binding_id,
                "documentVersion": document.version,
                "kind": standard.api.kind,
                "label": standard.label,
                "canonical": {
                    "name": standard.api.canonical,
                    "qualified": format!("{}/{}", standard.api.namespace, standard.api.canonical),
                },
                "documentation": {
                    "default": standard.api.documentation.default,
                    "translations": standard.api.documentation.translations,
                    "selection": {
                        "requestedLocale": locale,
                        "resolvedLocale": standard.resolved_locale,
                        "text": standard.selected_documentation,
                    },
                },
                "usage": standard.api.call_shapes,
                "examples": standard.api.examples,
                "type": standard.api.signature,
                "source": {
                    "uri": standard.api.source.uri,
                    "line": standard.api.source.line,
                    "column": standard.api.source.column,
                    "provenance": standard.provenance,
                    "range": range,
                },
                "semantic": {
                    "effects": standard.api.effects,
                    "evaluation": standard.api.evaluation,
                    "exceptions": standard.api.exceptions,
                },
                "authoredSpelling": authored_spelling_json(
                    authored_spelling_at(document, symbol, offset, effective_locale).as_ref()
                ),
            }));
        }

        let authored = authored_spelling_at(document, symbol, offset, effective_locale);
        Some(symbol_hover_machine_projection(
            document,
            symbol,
            locale,
            effective_locale,
            Some(uri),
            range,
            "workspace-source",
            authored.as_ref(),
        ))
    }


}
