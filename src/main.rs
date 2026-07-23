use std::{
    env,
    io::{self, Write},
    process::ExitCode,
};

fn main() -> ExitCode {
    let arguments = env::args_os()
        .skip(1)
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    if arguments.as_slice() == ["lsp"] {
        return match _core::lsp_stdio::run_stdio() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                let _ = writeln!(io::stderr().lock(), "osr: LSP transport failed: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "watch")
    {
        return match _core::cli::run_watch_stdio(&arguments[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                let _ = writeln!(io::stderr().lock(), "osr: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let outcome = _core::cli::run_cli(&arguments);

    let _ = io::stdout().lock().write_all(outcome.stdout.as_bytes());
    let _ = io::stderr().lock().write_all(outcome.stderr.as_bytes());
    ExitCode::from(outcome.exit_code)
}
