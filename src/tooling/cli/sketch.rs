use super::*;

/// EXPLORATORY — translate Elixir-flavoured surface text (`.oisr`) into
/// canonical Osiris source. `osr sketch FILE [-o OUT]`; without `-o` the
/// translation goes to stdout.
pub(super) fn run_sketch(arguments: &[String]) -> CliOutcome {
    let mut path = None;
    let mut out = None;
    let mut iterator = arguments.iter();
    while let Some(argument) = iterator.next() {
        match argument.as_str() {
            "-o" | "--out" => match iterator.next() {
                Some(target) if out.is_none() => out = Some(target.clone()),
                Some(_) => return CliOutcome::usage_error("duplicate option '-o' for 'sketch'"),
                None => return CliOutcome::usage_error("missing value for '-o'"),
            },
            option if option.starts_with('-') => {
                return CliOutcome::usage_error(format!("unknown option '{option}' for 'sketch'"));
            }
            positional if path.is_none() => path = Some(positional.to_owned()),
            _ => return CliOutcome::usage_error("unexpected arguments for 'sketch'"),
        }
    }
    let Some(path) = path else {
        return CliOutcome::usage_error("missing FILE for 'sketch'");
    };
    let source = match std::fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) => return io_error(&path, &error),
    };
    match crate::sketch::translate(&source) {
        Ok(translated) => {
            if let Some(out) = out {
                if let Err(error) = std::fs::write(&out, &translated) {
                    return io_error(&out, &error);
                }
                CliOutcome::success(String::new())
            } else {
                CliOutcome::success(translated)
            }
        }
        Err(errors) => {
            let mut stderr = String::new();
            for error in errors {
                stderr.push_str(&format!("{path}: {error}\n"));
            }
            CliOutcome::failure(1, String::new(), stderr)
        }
    }
}
