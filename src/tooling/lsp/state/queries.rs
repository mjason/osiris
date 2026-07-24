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
        let symbol = document.semantic.symbol_at(offset)?;
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
            let value = format!(
                "**{}** ({})\n\n{}\n\nType: {}  \nBinding: {}  \nEvaluation: {}  \nSource: {}:{}:{}",
                escape_markdown(&standard.label),
                escape_markdown(standard.api.canonical),
                escape_markdown(&standard.selected_documentation),
                escape_markdown(&standard.api.signature),
                escape_markdown(&standard.api.binding_id),
                standard.api.evaluation,
                standard.api.source.uri,
                standard.api.source.line,
                standard.api.source.column,
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
        if !documentation.is_empty() {
            value.push_str(&format!("\n\n{}", escape_markdown(documentation)));
        }
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
        document.semantic.symbol_at(offset)
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
