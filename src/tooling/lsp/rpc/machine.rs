
#[derive(Clone, Debug, Default)]
pub struct JsonRpcOutcome {
    pub response: Option<JsonValue>,
    pub notifications: Vec<JsonValue>,
}

impl JsonRpcOutcome {
    #[must_use]
    pub fn messages(&self) -> Vec<String> {
        self.notifications
            .iter()
            .chain(self.response.iter())
            .filter_map(|message| serde_json::to_string(message).ok())
            .collect()
    }

    #[must_use]
    pub fn response_text(&self) -> Option<String> {
        self.response
            .as_ref()
            .and_then(|response| serde_json::to_string(response).ok())
    }
}

/// Thin state-machine wrapper useful to a future stdio transport.
#[derive(Clone, Debug, Default)]
pub struct JsonRpcMachine {
    pub state: LspState,
}

pub type LspServer = JsonRpcMachine;

impl JsonRpcMachine {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn handle(&mut self, input: &str) -> JsonRpcOutcome {
        handle_json_rpc(&mut self.state, input)
    }

    pub fn handle_json(&mut self, input: &str) -> Vec<String> {
        self.handle(input).messages()
    }

    /// Analyzes deferred edits and returns the diagnostics to publish.
    ///
    /// A transport calls this once the editor stops sending edits, so a burst
    /// of keystrokes costs one analysis instead of one per keystroke.
    pub fn flush(&mut self) -> JsonRpcOutcome {
        let started = std::time::Instant::now();
        let notifications = self
            .state
            .flush_analysis()
            .into_iter()
            .map(publish_diagnostics_notification)
            .collect::<Vec<_>>();
        if !notifications.is_empty() {
            lsp_info!(
                "analyzed {} deferred document(s) in {:.1}ms",
                notifications.len(),
                started.elapsed().as_secs_f64() * 1000.0
            );
        }
        JsonRpcOutcome {
            response: None,
            notifications,
        }
    }
}

impl LspState {
    pub fn handle_json_rpc(&mut self, input: &str) -> JsonRpcOutcome {
        handle_json_rpc(self, input)
    }
}

/// Parses and dispatches one JSON-RPC message without performing any IO.
pub fn handle_json_rpc(state: &mut LspState, input: &str) -> JsonRpcOutcome {
    let request = match serde_json::from_str::<JsonValue>(input) {
        Ok(request) => request,
        Err(error) => {
            return JsonRpcOutcome {
                response: Some(rpc_error(
                    JsonValue::Null,
                    PARSE_ERROR,
                    "parse error",
                    Some(json!({ "detail": error.to_string() })),
                )),
                notifications: Vec::new(),
            };
        }
    };
    let Some(object) = request.as_object() else {
        return JsonRpcOutcome {
            response: Some(rpc_error(
                JsonValue::Null,
                INVALID_REQUEST,
                "request must be a JSON object",
                None,
            )),
            notifications: Vec::new(),
        };
    };
    if object.get("jsonrpc").and_then(JsonValue::as_str) != Some(JSON_RPC_VERSION) {
        return JsonRpcOutcome {
            response: Some(rpc_error(
                object.get("id").cloned().unwrap_or(JsonValue::Null),
                INVALID_REQUEST,
                "jsonrpc must be 2.0",
                None,
            )),
            notifications: Vec::new(),
        };
    }
    let Some(method) = object.get("method").and_then(JsonValue::as_str) else {
        return JsonRpcOutcome {
            response: Some(rpc_error(
                object.get("id").cloned().unwrap_or(JsonValue::Null),
                INVALID_REQUEST,
                "request method must be a string",
                None,
            )),
            notifications: Vec::new(),
        };
    };
    let id = object.get("id").cloned();
    let params = object.get("params").cloned().unwrap_or(JsonValue::Null);
    let started = std::time::Instant::now();
    lsp_debug!("-> {method}{}{}", request_id(id.as_ref()), subject(&params));
    let outcome = match dispatch(state, method, &params) {
        Ok(dispatch) => JsonRpcOutcome {
            response: id.map(|id| rpc_success(id, dispatch.result.unwrap_or(JsonValue::Null))),
            notifications: dispatch.notifications,
        },
        Err(error) => {
            // A notification carries no id and gets no reply, and the protocol
            // lets a server ignore one it does not implement. Reporting that as
            // an error buries real failures in noise the client cannot act on.
            let unimplemented_notification =
                id.is_none() && error.code == METHOD_NOT_FOUND;
            let elapsed = started.elapsed().as_secs_f64() * 1000.0;
            if unimplemented_notification {
                lsp_debug!("ignored unimplemented notification {method}");
            } else {
                lsp_error!(
                    "{method} failed after {elapsed:.1}ms: [{}] {}",
                    error.code,
                    error.message
                );
            }
            return JsonRpcOutcome {
                response: id.map(|id| {
                    rpc_error(
                        id,
                        error.code,
                        &error.message,
                        Some(json!({ "method": method })),
                    )
                }),
                notifications: Vec::new(),
            };
        }
    };
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    // Document synchronization drives every reanalysis, so report it at info
    // level; individual editor queries are projections and stay at debug.
    if method.starts_with("textDocument/did") || method == "initialize" {
        lsp_info!(
            "<- {method} {elapsed:.1}ms{}{}",
            subject(&params),
            published_diagnostics(&outcome)
        );
    } else {
        lsp_debug!("<- {method} {elapsed:.1}ms");
    }
    outcome
}

/// ` id=7`, or empty for a notification.
fn request_id(id: Option<&JsonValue>) -> String {
    id.map_or_else(String::new, |id| format!(" id={id}"))
}

/// The document and version a message concerns, when it names one.
fn subject(params: &JsonValue) -> String {
    let document = params.get("textDocument");
    let Some(uri) = document
        .and_then(|document| document.get("uri"))
        .and_then(JsonValue::as_str)
    else {
        return String::new();
    };
    let name = uri.rsplit('/').next().unwrap_or(uri);
    match document.and_then(|document| document.get("version")) {
        Some(version) => format!(" {name}@{version}"),
        None => format!(" {name}"),
    }
}

/// ` diagnostics=3`, summarizing what the message published.
fn published_diagnostics(outcome: &JsonRpcOutcome) -> String {
    let count = outcome
        .notifications
        .iter()
        .filter(|notification| {
            notification.get("method").and_then(JsonValue::as_str)
                == Some("textDocument/publishDiagnostics")
        })
        .filter_map(|notification| {
            notification
                .get("params")?
                .get("diagnostics")?
                .as_array()
                .map(Vec::len)
        })
        .sum::<usize>();
    if count == 0 {
        String::new()
    } else {
        format!(" diagnostics={count}")
    }
}

pub fn handle_request(state: &mut LspState, request: &JsonRpcRequest) -> JsonRpcOutcome {
    match serde_json::to_string(request) {
        Ok(input) => handle_json_rpc(state, &input),
        Err(error) => JsonRpcOutcome {
            response: Some(rpc_error(
                request.id.clone().unwrap_or(JsonValue::Null),
                INTERNAL_ERROR,
                "could not encode request",
                Some(json!({ "detail": error.to_string() })),
            )),
            notifications: Vec::new(),
        },
    }
}
