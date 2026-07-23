
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
    match dispatch(state, method, &params) {
        Ok(dispatch) => JsonRpcOutcome {
            response: id.map(|id| rpc_success(id, dispatch.result.unwrap_or(JsonValue::Null))),
            notifications: dispatch.notifications,
        },
        Err(error) => JsonRpcOutcome {
            response: id.map(|id| {
                rpc_error(
                    id,
                    error.code,
                    &error.message,
                    Some(json!({ "method": method })),
                )
            }),
            notifications: Vec::new(),
        },
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
