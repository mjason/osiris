impl LspState {
    pub fn formatting(&self, uri: &str) -> Result<Vec<TextEdit>, LspStateError> {
        let document = self.document(uri).ok_or_else(|| document_not_found(uri))?;
        let formatted = crate::formatter::format_source(&document.text).map_err(|error| {
            let message = error
                .diagnostics
                .first()
                .map_or("source cannot be formatted", |diagnostic| diagnostic.message.as_str());
            LspStateError::new(INVALID_PARAMS, message)
        })?;
        if formatted == document.text {
            return Ok(Vec::new());
        }
        Ok(vec![TextEdit {
            range: Range {
                start: Position::default(),
                end: offset_to_position(&document.text, document.text.len()),
            },
            new_text: formatted,
        }])
    }

    #[must_use]
    pub fn diagnostics(&self, uri: &str) -> Option<PublishDiagnosticsParams> {
        let document = self.document(uri)?;
        let mut diagnostics = document
            .analysis
            .diagnostics
            .iter()
            .map(|diagnostic| LspDiagnostic {
                range: span_to_range(&document.text, diagnostic.span),
                severity: 1,
                code: diagnostic.code.to_owned(),
                source: LSP_SERVER_NAME.to_owned(),
                message: diagnostic.message.clone(),
                data: json!({
                    "span": diagnostic.span,
                    "nodeId": node_id_for_span(document, diagnostic.span),
                    "documentVersion": document.version,
                }),
            })
            .collect::<Vec<_>>();
        diagnostics.extend(document.identifier_lints.iter().map(|lint| LspDiagnostic {
            range: span_to_range(&document.text, lint.span),
            severity: 2,
            code: lint.code.to_owned(),
            source: LSP_SERVER_NAME.to_owned(),
            message: lint.message.clone(),
            data: json!({
                "span": lint.span,
                "nodeId": node_id_for_span(document, lint.span),
                "documentVersion": document.version,
                "lintKind": lint.kind,
                "strictUnicode": true,
            }),
        }));
        diagnostics.sort_by(|left, right| {
            (
                left.range.start.line,
                left.range.start.character,
                left.range.end.line,
                left.range.end.character,
                left.severity,
                &left.code,
            )
                .cmp(&(
                    right.range.start.line,
                    right.range.start.character,
                    right.range.end.line,
                    right.range.end.character,
                    right.severity,
                    &right.code,
                ))
        });
        Some(PublishDiagnosticsParams {
            uri: uri.to_owned(),
            version: document.version,
            diagnostics,
        })
    }

    #[must_use]
    pub fn hover(&self, uri: &str, position: Position, locale: Option<&str>) -> Option<Hover> {
        let document = self.document(uri)?;
        let offset = position_to_offset(&document.text, position)?;
        let symbol = document.semantic.symbol_at_source(offset, &document.text)?;
        let locale = effective_display_locale(
            document,
            locale,
            self.session_locale.as_deref(),
            &self.display_locale,
        );
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
        let label = symbol.labels.for_locale(locale);
        let (documentation, _) = symbol.documentation.for_locale(Some(locale));
        let aliases = symbol
            .aliases
            .iter()
            .map(|alias| alias.spelling.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let mut value = format!(
            "**{}** · {}\n\n```osiris\n{} : {}\n```",
            escape_markdown(label),
            binding_kind_label(symbol.kind, locale),
            escape_markdown(&symbol.canonical),
            escape_markdown(&symbol.ty.to_string()),
        );
        if !documentation.is_empty() {
            value.push_str(&format!("\n\n{}", escape_markdown(documentation)));
        }
        if !aliases.is_empty() {
            value.push_str(&format!("\n\nAliases: `{}`", escape_markdown(&aliases)));
        }
        if symbol.python != symbol.canonical {
            value.push_str(&format!(
                "\n\nPython: `{}`",
                escape_markdown(&symbol.python)
            ));
        }
        Some(Hover {
            contents: MarkupContent {
                kind: "markdown".to_owned(),
                value,
            },
            range: occurrence_at(symbol, offset).map(|span| span_to_range(&document.text, span)),
        })
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

    #[must_use]
    pub fn completion(
        &self,
        uri: &str,
        position: Position,
        locale: Option<&str>,
    ) -> Vec<CompletionItem> {
        let Some(document) = self.document(uri) else {
            return Vec::new();
        };
        let offset = position_to_offset(&document.text, position).unwrap_or(document.text.len());
        let prefix = completion_prefix(&document.text, offset);
        let locale = effective_display_locale(
            document,
            locale,
            self.session_locale.as_deref(),
            &self.display_locale,
        );
        let mut items = document
            .semantic
            .symbols
            .iter()
            .filter(|symbol| symbol_matches_prefix(symbol, &prefix))
            .flat_map(|symbol| completion_items(symbol, Some(locale)))
            .collect::<Vec<_>>();
        let surface = crate::ast::lower_document(&document.analysis.document);
        if crate::stdlib::uses_implicit_core(&surface.module) {
            let existing = items
                .iter()
                .map(|item| item.insert_text.as_str())
                .collect::<BTreeSet<_>>();
            let implicit = crate::stdlib::exports(crate::stdlib::CORE_NAMESPACE)
                .filter(|binding| prefix.is_empty() || binding.canonical.starts_with(&prefix))
                .filter(|binding| !existing.contains(binding.canonical))
                .map(|binding| {
                    CompletionItem {
                        label: binding.canonical.to_owned(),
                        kind: completion_kind(binding.kind),
                        detail: format!(
                            "{} · {}",
                            crate::stdlib::CORE_NAMESPACE,
                            binding_kind_label(binding.kind, locale)
                        ),
                        insert_text: binding.canonical.to_owned(),
                        sort_text: format!("1:{}", binding.canonical),
                        filter_text: binding.canonical.to_owned(),
                        data: json!({
                            "bindingId": binding.id().as_str(),
                            "canonical": binding.canonical,
                            "implicitCore": true,
                        }),
                    }
                })
                .collect::<Vec<_>>();
            items.extend(implicit);
        }
        items.sort_by(|left, right| {
            (&left.sort_text, &left.label, &left.insert_text).cmp(&(
                &right.sort_text,
                &right.label,
                &right.insert_text,
            ))
        });
        items
    }

    #[must_use]
    pub fn signature_help(
        &self,
        uri: &str,
        position: Position,
        locale: Option<&str>,
    ) -> Option<SignatureHelp> {
        let document = self.document(uri)?;
        let offset = position_to_offset(&document.text, position)?;
        let macro_trace = document
            .analysis
            .expansion_traces
            .iter()
            .filter(|trace| span_contains(trace.call_span, offset))
            .min_by_key(|trace| trace.call_span.end.saturating_sub(trace.call_span.start));
        let runtime_call = document
            .semantic
            .operation_graph
            .nodes
            .iter()
            .filter(|operation| {
                operation.kind == "call"
                    && operation.binding_id.is_some()
                    && span_contains(operation.span, offset)
            })
            .min_by_key(|operation| operation.span.end.saturating_sub(operation.span.start));
        if let Some(trace) = macro_trace
            && runtime_call.is_none_or(|call| {
                trace.call_span.end.saturating_sub(trace.call_span.start)
                    <= call.span.end.saturating_sub(call.span.start)
            })
        {
            return macro_signature_help(
                document,
                trace,
                offset,
                effective_display_locale(
                    document,
                    locale,
                    self.session_locale.as_deref(),
                    &self.display_locale,
                ),
            );
        }
        let call = runtime_call?;
        let binding_id = call.binding_id.as_deref()?;
        let signature = callable_signature(document, binding_id)?;
        let call_form = find_call_form(&document.analysis.document.forms, call.span)?;
        let FormKind::List(items) = &call_form.kind else {
            return None;
        };
        let invoked_name = items
            .first()
            .and_then(form_name)
            .unwrap_or(&signature.canonical);
        let arguments = source_arguments(items);
        let source_argument = active_source_argument(items, &arguments, offset);
        let active_parameter = active_parameter(&signature.parameters, &arguments, source_argument)
            .map(|index| index as u32);
        let locale = effective_display_locale(
            document,
            locale,
            self.session_locale.as_deref(),
            &self.display_locale,
        );
        let parameter_labels = signature
            .parameters
            .iter()
            .map(|parameter| signature_parameter_label(parameter, Some(locale)))
            .collect::<Vec<_>>();
        let label = format!(
            "{}({}) -> {}",
            invoked_name,
            parameter_labels.join(", "),
            signature.return_type
        );
        Some(SignatureHelp {
            signatures: vec![SignatureInformation {
                label,
                parameters: parameter_labels
                    .into_iter()
                    .map(|label| ParameterInformation { label })
                    .collect(),
                active_parameter,
            }],
            active_signature: 0,
            active_parameter,
        })
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

fn effective_display_locale<'a>(
    document: &'a OpenDocument,
    requested: Option<&'a str>,
    session: Option<&'a str>,
    fallback: &'a str,
) -> &'a str {
    requested
        .or(session)
        .or(document.display_locale.as_deref())
        .unwrap_or(fallback)
}
