//! Standard input/output transport for the IO-independent LSP state machine.

use std::{
    collections::BTreeSet,
    io::{self, BufRead, BufReader, BufWriter, Write},
    sync::mpsc::{self, RecvTimeoutError},
    time::{Duration, Instant},
};

use serde_json::Value as JsonValue;

use crate::lsp::{JsonRpcMachine, log};

const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

/// Quiet period after an edit before it is analyzed.
const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(150);

/// Longest an edit waits while the editor keeps talking, so continuous typing
/// still yields diagnostics instead of starving them.
const MAX_ANALYSIS_DELAY: Duration = Duration::from_millis(600);

/// JSON-RPC "request cancelled", per the LSP specification.
const REQUEST_CANCELLED: i64 = -32800;

fn debounce() -> Duration {
    std::env::var("OSIRIS_LSP_DEBOUNCE_MS")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .map_or(DEFAULT_DEBOUNCE, Duration::from_millis)
}

/// Runs an LSP server over the process standard streams.
pub fn run_stdio() -> io::Result<()> {
    // A real editor session is the case where nobody can see what the server
    // is doing, so record document synchronization by default. `OSIRIS_LSP_LOG`
    // overrides this in either direction.
    log::set_default_level(log::Level::Info);
    let stdout = io::stdout();
    // `StdinLock` is not `Send`, and reading happens on its own thread.
    let result = serve(
        &mut BufReader::new(io::stdin()),
        &mut BufWriter::new(stdout.lock()),
    );
    match &result {
        Ok(()) => log::info("session ended"),
        Err(error) => log::error(&format!("session ended: {error}")),
    }
    result
}

/// Serves framed LSP messages until EOF or an `exit` notification.
///
/// Reading runs on its own thread so the main loop can wait on a timeout. That
/// is what makes two things possible: coalescing a burst of edits into one
/// analysis, and observing a `$/cancelRequest` that is already queued behind
/// the request it cancels.
pub fn serve<R: BufRead + Send, W: Write>(reader: &mut R, writer: &mut W) -> io::Result<()> {
    log::info(&format!(
        "osr {} language server started",
        crate::version()
    ));
    let debounce = debounce();
    let mut machine = JsonRpcMachine::new();
    std::thread::scope(|scope| {
        let (sender, receiver) = mpsc::channel::<io::Result<Vec<u8>>>();
        scope.spawn(move || {
            loop {
                match read_message(reader) {
                    Ok(Some(payload)) => {
                        if sender.send(Ok(payload)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = sender.send(Err(error));
                        break;
                    }
                }
            }
        });

        let mut cancelled = BTreeSet::new();
        let mut deferred_since: Option<Instant> = None;
        loop {
            // Wait out the quiet period, but never past the ceiling.
            let waiting = deferred_since.map(|since| {
                debounce.min(MAX_ANALYSIS_DELAY.saturating_sub(since.elapsed()))
            });
            let received = match waiting {
                Some(timeout) => receiver.recv_timeout(timeout),
                None => receiver.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            let payload = match received {
                Ok(Ok(payload)) => payload,
                Ok(Err(error)) => return Err(error),
                Err(RecvTimeoutError::Timeout) => {
                    // The editor went quiet: analyze what it sent.
                    for message in machine.flush().messages() {
                        write_message(writer, message.as_bytes())?;
                    }
                    writer.flush()?;
                    deferred_since = None;
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            };
            let input = std::str::from_utf8(&payload).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("LSP payload is not UTF-8: {error}"),
                )
            })?;
            let exit = is_exit_notification(input);
            if let Some(id) = cancellation_target(input) {
                cancelled.insert(id);
                continue;
            }
            if let Some(id) = request_id(input)
                && cancelled.remove(&id)
            {
                lsp_cancelled(writer, &id)?;
                continue;
            }
            for message in machine.handle(input).messages() {
                write_message(writer, message.as_bytes())?;
            }
            writer.flush()?;
            if machine.state.has_deferred_changes() {
                deferred_since.get_or_insert_with(Instant::now);
            } else {
                deferred_since = None;
            }
            if exit {
                break;
            }
        }
        // A client that closes the stream mid-edit still gets a settled model
        // for anything that outlives this call.
        machine.flush();
        Ok(())
    })
}

/// The request id carried by a `$/cancelRequest` notification.
fn cancellation_target(input: &str) -> Option<String> {
    let message = serde_json::from_str::<JsonValue>(input).ok()?;
    if message.get("method")?.as_str()? != "$/cancelRequest" {
        return None;
    }
    Some(message.get("params")?.get("id")?.to_string())
}

/// The id of a request, or `None` for a notification.
fn request_id(input: &str) -> Option<String> {
    let message = serde_json::from_str::<JsonValue>(input).ok()?;
    message.get("method")?.as_str()?;
    Some(message.get("id")?.to_string())
}

fn lsp_cancelled(writer: &mut impl Write, id: &str) -> io::Result<()> {
    let id = serde_json::from_str::<JsonValue>(id).unwrap_or(JsonValue::Null);
    let response = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": REQUEST_CANCELLED, "message": "request cancelled" },
    });
    log::info(&format!("skipped cancelled request id={response}"));
    write_message(writer, response.to_string().as_bytes())?;
    writer.flush()
}

fn read_message<R: BufRead>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut content_length = None;
    let mut saw_header = false;
    loop {
        let mut line = Vec::new();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 {
            return if saw_header {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "LSP header ended before the blank separator",
                ))
            } else {
                Ok(None)
            };
        }
        saw_header = true;
        if line == b"\n" || line == b"\r\n" {
            break;
        }
        while matches!(line.last(), Some(b'\n' | b'\r')) {
            line.pop();
        }
        let line = std::str::from_utf8(&line).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "LSP header is not ASCII/UTF-8")
        })?;
        let Some((name, value)) = line.split_once(':') else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "malformed LSP header",
            ));
        };
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "duplicate Content-Length header",
                ));
            }
            let length = value.trim().parse::<usize>().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid Content-Length header")
            })?;
            if length > MAX_MESSAGE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "LSP message exceeds the transport size limit",
                ));
            }
            content_length = Some(length);
        }
    }
    let length = content_length.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "LSP message has no Content-Length header",
        )
    })?;
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload)?;
    Ok(Some(payload))
}

fn write_message(writer: &mut impl Write, payload: &[u8]) -> io::Result<()> {
    write!(writer, "Content-Length: {}\r\n\r\n", payload.len())?;
    writer.write_all(payload)
}

fn is_exit_notification(input: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(input)
        .ok()
        .and_then(|value| value.get("method").cloned())
        .and_then(|method| method.as_str().map(str::to_owned))
        .is_some_and(|method| method == "exit")
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
