//! Standard input/output transport for the IO-independent LSP state machine.

use std::io::{self, BufRead, BufReader, BufWriter, Write};

use crate::lsp::JsonRpcMachine;

const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

/// Runs an LSP server over the process standard streams.
pub fn run_stdio() -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    serve(
        &mut BufReader::new(stdin.lock()),
        &mut BufWriter::new(stdout.lock()),
    )
}

/// Serves framed LSP messages until EOF or an `exit` notification.
pub fn serve<R: BufRead, W: Write>(reader: &mut R, writer: &mut W) -> io::Result<()> {
    let mut machine = JsonRpcMachine::new();
    while let Some(payload) = read_message(reader)? {
        let input = std::str::from_utf8(&payload).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("LSP payload is not UTF-8: {error}"),
            )
        })?;
        let exit = is_exit_notification(input);
        for message in machine.handle(input).messages() {
            write_message(writer, message.as_bytes())?;
        }
        writer.flush()?;
        if exit {
            break;
        }
    }
    Ok(())
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
