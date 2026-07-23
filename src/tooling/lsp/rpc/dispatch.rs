#[derive(Default)]
struct DispatchOutcome {
    result: Option<JsonValue>,
    notifications: Vec<JsonValue>,
}

fn dispatch(
    state: &mut LspState,
    method: &str,
    params: &JsonValue,
) -> Result<DispatchOutcome, LspStateError> {
    match method {
        "initialize" => {
            if let Some(locale) = find_string(params, &["initializationOptions", "displayLocale"])
                .or_else(|| find_string(params, &["locale"]))
            {
                state.set_display_locale(locale);
            }
            if let Some(site_roots) = find_value(params, &["initializationOptions", "siteRoots"]) {
                let roots = site_roots.as_array().ok_or_else(|| {
                    LspStateError::new(
                        INVALID_PARAMS,
                        "initializationOptions.siteRoots must be an array of paths",
                    )
                })?;
                let roots = roots
                    .iter()
                    .map(|root| {
                        root.as_str().map(PathBuf::from).ok_or_else(|| {
                            LspStateError::new(
                                INVALID_PARAMS,
                                "initializationOptions.siteRoots must contain only paths",
                            )
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                state.set_site_roots(roots);
            }
            Ok(DispatchOutcome {
                result: Some(initialize_result()),
                notifications: Vec::new(),
            })
        }
        "initialized" | "$/cancelRequest" | "exit" => Ok(DispatchOutcome::default()),
        "shutdown" => {
            state.request_shutdown();
            Ok(DispatchOutcome {
                result: Some(JsonValue::Null),
                notifications: Vec::new(),
            })
        }
        "textDocument/didOpen" => {
            let params: DidOpenParams = decode_params(params)?;
            let diagnostics = state.did_open(
                params.text_document.uri,
                params.text_document.version,
                params.text_document.text,
            );
            Ok(DispatchOutcome {
                result: None,
                notifications: vec![publish_diagnostics_notification(diagnostics)],
            })
        }
        "textDocument/didChange" => {
            let params: DidChangeParams = decode_params(params)?;
            let diagnostics = state.did_change(
                &params.text_document.uri,
                params.text_document.version,
                &params.content_changes,
            )?;
            Ok(DispatchOutcome {
                result: None,
                notifications: vec![publish_diagnostics_notification(diagnostics)],
            })
        }
        "textDocument/didClose" => {
            let params: DidCloseParams = decode_params(params)?;
            state.did_close(&params.text_document.uri);
            Ok(DispatchOutcome {
                result: None,
                notifications: vec![json!({
                    "jsonrpc": JSON_RPC_VERSION,
                    "method": "textDocument/publishDiagnostics",
                    "params": {
                        "uri": params.text_document.uri,
                        "diagnostics": [],
                    },
                })],
            })
        }
        "textDocument/hover" => {
            let params: PositionParams = decode_params(params)?;
            ensure_document(state, &params.text_document.uri)?;
            let result = match state.hover(
                &params.text_document.uri,
                params.position,
                params.locale.as_deref(),
            ) {
                Some(hover) => serialize_value(hover)?,
                None => JsonValue::Null,
            };
            Ok(result_outcome(result))
        }
        "textDocument/completion" => {
            let params: PositionParams = decode_params(params)?;
            ensure_document(state, &params.text_document.uri)?;
            let result = serialize_value(state.completion(
                &params.text_document.uri,
                params.position,
                params.locale.as_deref(),
            ))?;
            Ok(result_outcome(result))
        }
        "textDocument/signatureHelp" => {
            let params: PositionParams = decode_params(params)?;
            ensure_document(state, &params.text_document.uri)?;
            let result = state
                .signature_help(
                    &params.text_document.uri,
                    params.position,
                    params.locale.as_deref(),
                )
                .map_or(Ok(JsonValue::Null), serialize_value)?;
            Ok(result_outcome(result))
        }
        "textDocument/definition" => {
            let params: PositionParams = decode_params(params)?;
            ensure_document(state, &params.text_document.uri)?;
            let result = match state.definition(&params.text_document.uri, params.position) {
                Some(location) => serialize_value(location)?,
                None => JsonValue::Null,
            };
            Ok(result_outcome(result))
        }
        "textDocument/references" => {
            let params: PositionParams = decode_params(params)?;
            ensure_document(state, &params.text_document.uri)?;
            let result =
                serialize_value(state.references(&params.text_document.uri, params.position))?;
            Ok(result_outcome(result))
        }
        "textDocument/prepareRename" => {
            let params: PositionParams = decode_params(params)?;
            ensure_document(state, &params.text_document.uri)?;
            let result = state
                .prepare_rename(&params.text_document.uri, params.position)
                .map_or(Ok(JsonValue::Null), serialize_value)?;
            Ok(result_outcome(result))
        }
        "textDocument/rename" => {
            let params: RenameParams = decode_params(params)?;
            ensure_document(state, &params.text_document.uri)?;
            let result = state
                .rename(&params.text_document.uri, params.position, &params.new_name)?
                .map_or(Ok(JsonValue::Null), serialize_value)?;
            Ok(result_outcome(result))
        }
        "osiris/diagnostics" => {
            let uri = required_uri(params)?;
            let diagnostics = state
                .diagnostics(uri)
                .ok_or_else(|| document_not_found(uri))?;
            Ok(result_outcome(serialize_value(diagnostics)?))
        }
        "textDocument/diagnostic" => {
            let uri = required_uri(params)?;
            let diagnostics = state
                .diagnostics(uri)
                .ok_or_else(|| document_not_found(uri))?;
            Ok(result_outcome(json!({
                "kind": "full",
                "items": diagnostics.diagnostics,
            })))
        }
        "osiris/expand" | "osiris/expandPreview" | "textDocument/expandPreview" => {
            let uri = required_uri(params)?;
            let preview = state
                .expand_preview(uri)
                .ok_or_else(|| document_not_found(uri))?;
            Ok(result_outcome(serialize_value(preview)?))
        }
        "osiris/semanticView" => {
            let uri = required_uri(params)?;
            if let Some(locale) = find_string(params, &["locale"]) {
                state.set_display_locale(locale);
            }
            let semantic = state
                .semantic_document(uri)
                .ok_or_else(|| document_not_found(uri))?;
            Ok(result_outcome(serialize_value(semantic)?))
        }
        "osiris/syntaxSnapshot" => {
            let uri = required_uri(params)?;
            let document = state.document(uri).ok_or_else(|| document_not_found(uri))?;
            Ok(result_outcome(json!({
                "version": document.analysis.document.format_version,
                "documentVersion": document.version,
                "sourceLen": document.analysis.document.source_len,
                "nodes": &document.analysis.document.nodes,
            })))
        }
        "osiris/inspect" => {
            let uri = required_uri(params)?;
            ensure_document(state, uri)?;
            let query = find_string(params, &["bindingId"])
                .or_else(|| find_string(params, &["symbol"]))
                .or_else(|| find_string(params, &["query"]));
            let inspected = state
                .inspect(uri, query.as_deref())
                .unwrap_or(JsonValue::Null);
            Ok(result_outcome(inspected))
        }
        _ => Err(LspStateError::new(
            METHOD_NOT_FOUND,
            format!("method not found: {method}"),
        )),
    }
}
