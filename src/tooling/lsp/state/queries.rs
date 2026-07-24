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
        let locale = effective_display_locale(
            document,
            None,
            self.session_locale.as_deref(),
            &self.display_locale,
        );
        diagnostics.extend(alias_migration_diagnostics(document, locale));
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
    pub fn code_actions(&self, uri: &str, requested: Range) -> Vec<JsonValue> {
        let Some(document) = self.document(uri) else {
            return Vec::new();
        };
        self.diagnostics(uri)
            .into_iter()
            .flat_map(|published| published.diagnostics)
            .filter(|diagnostic| {
                diagnostic.code == "OSR-L0002" && ranges_overlap(diagnostic.range, requested)
            })
            .filter_map(|diagnostic| {
                let replacement = diagnostic.data["replacement"].as_str()?;
                let span = diagnostic.data["span"].clone();
                let source = document.text.get(
                    position_to_offset(&document.text, diagnostic.range.start)?
                        ..position_to_offset(&document.text, diagnostic.range.end)?,
                )?;
                let new_text = if source.starts_with(':') {
                    format!(":{replacement}")
                } else {
                    replacement.to_owned()
                };
                Some(json!({
                    "title": format!("Replace with `{replacement}`"),
                    "kind": "quickfix",
                    "diagnostics": [diagnostic],
                    "isPreferred": true,
                    "edit": {
                        "changes": {
                            (uri): [{
                                "range": diagnostic.range,
                                "newText": new_text,
                            }],
                        },
                    },
                    "data": {
                        "kind": "migration-alias",
                        "span": span,
                        "documentVersion": document.version,
                    },
                }))
            })
            .collect()
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
        let locale = effective_display_locale(
            document,
            locale,
            self.session_locale.as_deref(),
            &self.display_locale,
        );
        if let Some(call_form) = find_source_call_at(&document.analysis.document.forms, offset)
            && let FormKind::List(items) = &call_form.kind
            && let Some(callee) = items.first()
            && let Some(symbol) = document
                .semantic
                .symbol_at_source(callee.span.start, &document.text)
            && let Some(signature) = callable_signature(document, &symbol.binding_id)
        {
            return Some(callable_signature_help(
                &signature, items, offset, locale,
            ));
        }
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
                locale,
            );
        }
        let call = runtime_call?;
        let binding_id = call.binding_id.as_deref()?;
        let signature = callable_signature(document, binding_id)?;
        let call_form = find_call_form(&document.analysis.document.forms, call.span)?;
        let FormKind::List(items) = &call_form.kind else {
            return None;
        };
        Some(callable_signature_help(&signature, items, offset, locale))
    }
}

fn ranges_overlap(left: Range, right: Range) -> bool {
    position_leq(left.start, right.end) && position_leq(right.start, left.end)
}

fn position_leq(left: Position, right: Position) -> bool {
    (left.line, left.character) <= (right.line, right.character)
}

fn alias_migration_diagnostics(
    document: &OpenDocument,
    locale: &str,
) -> Vec<LspDiagnostic> {
    let chinese = locale == "zh" || locale.starts_with("zh-");
    let mut diagnostics = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for advisory in &document.analysis.migration_advisories {
        if !seen.insert((advisory.span.start, advisory.span.end, &advisory.canonical)) {
            continue;
        }
        let replacement = advisory.replacement(Some(locale));
        let message = if chinese {
            format!(
                "`{}` 是兼容旧源码的别名；请改用 `{replacement}`",
                advisory.alias
            )
        } else {
            format!(
                "`{}` is a source-compatibility alias; use `{replacement}`",
                advisory.alias
            )
        };
        diagnostics.push(LspDiagnostic {
            range: span_to_range(&document.text, advisory.span),
            severity: 2,
            code: "OSR-L0002".to_owned(),
            source: LSP_SERVER_NAME.to_owned(),
            message,
            data: json!({
                "span": advisory.span,
                "nodeId": node_id_for_span(document, advisory.span),
                "documentVersion": document.version,
                "lintKind": "migration-alias",
                "alias": advisory.alias,
                "canonical": advisory.canonical,
                "replacement": replacement,
            }),
        });
    }
    diagnostics
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
