use std::{
    env,
    io::{self, Write},
    process::ExitCode,
};

fn main() -> ExitCode {
    // Before anything else — help, version, LSP included: a project's locked
    // osr beats the one PATH happened to find. Whatever the project pinned is
    // what must answer, or a stale global install reports errors the locked
    // version fixed long ago.
    if let Some(project_osr) = osiris::cli::project_local_osr() {
        return delegate(&project_osr);
    }
    let arguments = env::args_os()
        .skip(1)
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    if arguments.as_slice() == ["lsp"] {
        if let Err(error) = osiris::stdlib::validate_resources() {
            let _ = writeln!(
                io::stderr().lock(),
                "osr: invalid compiler installation: {error}"
            );
            return ExitCode::FAILURE;
        }
        return match osiris::lsp_stdio::run_stdio() {
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
        if let Err(error) = osiris::stdlib::validate_resources() {
            let _ = writeln!(
                io::stderr().lock(),
                "osr: invalid compiler installation: {error}"
            );
            return ExitCode::FAILURE;
        }
        return match osiris::cli::run_watch_stdio(&arguments[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                let _ = writeln!(io::stderr().lock(), "osr: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if arguments.as_slice() == ["fmt", "-"] {
        return match osiris::cli::run_fmt_stdio(&arguments[1..]) {
            Ok(outcome) => {
                let _ = io::stdout().lock().write_all(outcome.stdout.as_bytes());
                let _ = io::stderr().lock().write_all(outcome.stderr.as_bytes());
                ExitCode::from(outcome.exit_code)
            }
            Err(error) => {
                let _ = writeln!(io::stderr().lock(), "osr: could not read stdin: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if arguments.as_slice() == ["doc", "-"] {
        return match osiris::cli::run_doc_stdio() {
            Ok(outcome) => {
                let _ = io::stdout().lock().write_all(outcome.stdout.as_bytes());
                let _ = io::stderr().lock().write_all(outcome.stderr.as_bytes());
                ExitCode::from(outcome.exit_code)
            }
            Err(error) => {
                let _ = writeln!(io::stderr().lock(), "osr: could not read stdin: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let outcome = osiris::cli::run_cli(&arguments);

    let _ = io::stdout().lock().write_all(outcome.stdout.as_bytes());
    let _ = io::stderr().lock().write_all(outcome.stderr.as_bytes());
    ExitCode::from(outcome.exit_code)
}

/// Hands this invocation to the project's own osr, arguments and all.
///
/// `OSR_NO_DELEGATE` is set on the child as the loop guard of last resort —
/// path comparison already prevents self-delegation, but a copied binary
/// would defeat it. If the handover itself fails, running in place is wrong
/// in exactly the way delegation exists to prevent, so that is an error, not
/// a fallback.
fn delegate(project_osr: &std::path::Path) -> ExitCode {
    let mut command = std::process::Command::new(project_osr);
    command
        .args(env::args_os().skip(1))
        .env("OSR_NO_DELEGATE", "1");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = command.exec();
        let _ = writeln!(
            io::stderr().lock(),
            "osr: could not run project osr '{}': {error}",
            project_osr.display()
        );
        ExitCode::FAILURE
    }
    #[cfg(not(unix))]
    {
        match command.status() {
            Ok(status) => ExitCode::from(status.code().unwrap_or(1).clamp(0, 255) as u8),
            Err(error) => {
                let _ = writeln!(
                    io::stderr().lock(),
                    "osr: could not run project osr '{}': {error}",
                    project_osr.display()
                );
                ExitCode::FAILURE
            }
        }
    }
}
