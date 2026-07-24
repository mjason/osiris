use super::*;

pub(super) fn run_expand(arguments: &[String]) -> CliOutcome {
    let mut path = None;
    let mut once = false;
    for argument in arguments {
        match argument.as_str() {
            "--once" if !once => once = true,
            "--once" => return CliOutcome::usage_error("duplicate option '--once' for 'expand'"),
            option if option.starts_with('-') => {
                return CliOutcome::usage_error(format!("unknown option '{option}' for 'expand'"));
            }
            positional if path.is_none() => path = Some(positional),
            _ => return CliOutcome::usage_error("unexpected arguments for 'expand'"),
        }
    }
    let Some(path) = path else {
        return CliOutcome::usage_error("missing FILE for 'expand'");
    };
    let (source, document) = match read_source(path) {
        Ok(result) => result,
        Err(error) => return io_error(path, &error),
    };
    let expanded = macro_expand::expand(
        &document,
        ExpansionOptions {
            once,
            ..ExpansionOptions::default()
        },
    );
    let stdout = render_document_text(&expanded.document);
    let stderr = diagnostic::render_all(path, &source, &expanded.document.diagnostics);
    if expanded.document.has_errors() {
        CliOutcome::failure(1, stdout, stderr)
    } else {
        CliOutcome::success(stdout)
    }
}
