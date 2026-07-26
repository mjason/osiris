use std::path::PathBuf;

use super::CliOutcome;

pub(super) fn run_lsa(arguments: &[String]) -> CliOutcome {
    let mut request = Vec::new();
    let mut session = None;
    let mut locale = None;
    let mut file = None;
    let mut at = None;
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
            "--at" => {
                let Some(value) = arguments.get(index + 1) else {
                    return CliOutcome::usage_error("missing value for '--at'");
                };
                if at.is_some() {
                    return CliOutcome::usage_error("duplicate option '--at' for 'lsa'");
                }
                let parsed = match super::lsc::support::parse_at(value) {
                    Ok(value) => value,
                    Err(error) => return CliOutcome::usage_error(error),
                };
                at = Some(crate::lsc::SourcePosition {
                    path: parsed.path.into(),
                    line: parsed.position.line + 1,
                    column: parsed.position.character + 1,
                });
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
        at,
    };
    match crate::agent::run(&options).and_then(|response| crate::agent::render(&response, format)) {
        Ok(stdout) => CliOutcome::success(stdout),
        Err(error) => CliOutcome::failure(1, String::new(), format!("osr: lsa: {error}\n")),
    }
}

fn help() -> String {
    "Usage: osr lsa [options] <request>\n\nOptions:\n  --session <id>  Continue a project-local session.\n  --locale <tag>  Select the response language.\n  --file <path>   Add one Osiris source, interface, or osiris.jsonc as explicit context.\n  --at <path:line:column>  Anchor the question to an exact source position.\n  --format json|text  Select the output format (default: json).\n".to_owned()
}
