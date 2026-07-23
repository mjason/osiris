impl LspState {
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
        let symbol = document.semantic.symbol_at(offset)?;
        let locale = effective_display_locale(document, locale, &self.display_locale);
        let label = symbol.labels.for_locale(locale);
        let aliases = symbol
            .aliases
            .iter()
            .map(|alias| alias.spelling.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let effects = serde_json::to_string(&symbol.summary.effects).unwrap_or_default();
        let temporal = serde_json::to_string(&symbol.summary.temporal).unwrap_or_default();
        let data = serde_json::to_string(&symbol.summary.data).unwrap_or_default();
        let mut value = format!(
            "**{}** ({})\n\nType: {}  \nPython: {}  \nBinding: {}",
            escape_markdown(label),
            escape_markdown(&symbol.canonical),
            escape_markdown(&symbol.ty.to_string()),
            escape_markdown(&symbol.python),
            escape_markdown(&symbol.binding_id),
        );
        if !aliases.is_empty() {
            value.push_str(&format!("\n\nAliases: {}", escape_markdown(&aliases)));
        }
        value.push_str(&format!(
            "\n\nEffects: {}  \nTemporal: {}  \nData: {}",
            escape_markdown(&effects),
            escape_markdown(&temporal),
            escape_markdown(&data),
        ));
        Some(Hover {
            contents: MarkupContent {
                kind: "markdown".to_owned(),
                value,
            },
            range: occurrence_at(symbol, offset).map(|span| span_to_range(&document.text, span)),
        })
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
        let locale = effective_display_locale(document, locale, &self.display_locale);
        let chinese = is_chinese_locale(locale);
        let mut items = document
            .semantic
            .symbols
            .iter()
            .filter(|symbol| symbol_matches_prefix(symbol, &prefix))
            .map(|symbol| completion_item(symbol, chinese))
            .collect::<Vec<_>>();
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
                effective_display_locale(document, locale, &self.display_locale),
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
        let chinese = is_chinese_locale(effective_display_locale(
            document,
            locale,
            &self.display_locale,
        ));
        let parameter_labels = signature
            .parameters
            .iter()
            .map(|parameter| signature_parameter_label(parameter, chinese))
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

fn effective_display_locale<'a>(
    document: &'a OpenDocument,
    requested: Option<&'a str>,
    fallback: &'a str,
) -> &'a str {
    requested
        .or(document.display_locale.as_deref())
        .unwrap_or(fallback)
}
