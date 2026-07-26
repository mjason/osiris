use std::path::PathBuf;

use super::CliOutcome;

pub(super) fn run_lsa(arguments: &[String]) -> CliOutcome {
    let mut request = Vec::new();
    let mut session = None;
    let mut locale = None;
    let mut file = None;
    let mut format = crate::agent::OutputFormat::Json;
    let mut index = 0;
    while let Some(argument) = arguments.get(index) {
        match argument.as_str() {
            "--session" => {
                let Some(value) = arguments.get(index + 1) else {
                    return CliOutcome::usage_error("missing value for '--session'");
                };
                session = Some(value.clone());
                index += 1;
            }
            "--locale" => {
                let Some(value) = arguments.get(index + 1) else {
                    return CliOutcome::usage_error("missing value for '--locale'");
                };
                let value = match oxilangtag::LanguageTag::parse_and_normalize(value) {
                    Ok(value) => value.to_string(),
                    Err(error) => {
                        return CliOutcome::usage_error(format!(
                            "invalid BCP 47 locale '{}': {error}",
                            arguments[index + 1]
                        ));
                    }
                };
                locale = Some(value);
                index += 1;
            }
            "--format" => {
                let Some(value) = arguments.get(index + 1) else {
                    return CliOutcome::usage_error("missing value for '--format'");
                };
                format = match value.as_str() {
                    "json" => crate::agent::OutputFormat::Json,
                    "text" => crate::agent::OutputFormat::Text,
                    _ => {
                        return CliOutcome::usage_error(
                            "--format must be 'json' or 'text' for 'lsa'",
                        );
                    }
                };
                index += 1;
            }
            "--file" => {
                let Some(value) = arguments.get(index + 1) else {
                    return CliOutcome::usage_error("missing value for '--file'");
                };
                file = Some(PathBuf::from(value));
                index += 1;
            }
            "-h" | "--help" => return CliOutcome::success(help()),
            value if value.starts_with('-') => {
                return CliOutcome::usage_error(format!("unknown option '{value}' for 'lsa'"));
            }
            value => request.push(value.to_owned()),
        }
        index += 1;
    }
    if request.is_empty() {
        return CliOutcome::usage_error("usage: osr lsa [options] <request>");
    }
    let options = crate::agent::LsaOptions {
        request: request.join(" "),
        session,
        locale,
        file,
    };
    match crate::agent::run(&options).and_then(|response| crate::agent::render(&response, format)) {
        Ok(stdout) => CliOutcome::success(stdout),
        Err(error) => CliOutcome::failure(1, String::new(), format!("osr: lsa: {error}\n")),
    }
}

fn help() -> String {
    "Usage: osr lsa [options] <request>\n\nOptions:\n  --session <id>  Continue a project-local session.\n  --locale <tag>  Select the response language.\n  --file <path>   Add one project source file as context.\n  --format json|text  Select the output format (default: json).\n".to_owned()
}
