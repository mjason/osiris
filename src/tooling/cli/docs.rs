use std::io::{self, Read};

use super::*;

pub(super) fn run_syntax(arguments: &[String]) -> CliOutcome {
    let format = match arguments {
        [] => "markdown",
        [option, format]
            if option == "--format" && matches!(format.as_str(), "markdown" | "json") =>
        {
            format
        }
        [option, _] if option == "--format" => {
            return CliOutcome::usage_error("--format must be 'markdown' or 'json' for 'syntax'");
        }
        _ => return CliOutcome::usage_error("usage: osr syntax [--format markdown|json]"),
    };
    let document = match crate::documentation::syntax_markdown() {
        Ok(document) => document,
        Err(message) => return CliOutcome::failure(1, String::new(), format!("osr: {message}\n")),
    };
    if format == "markdown" {
        let mut markdown = document.markdown;
        if !markdown.ends_with('\n') {
            markdown.push('\n');
        }
        return CliOutcome::success(markdown);
    }
    match serde_json::to_string_pretty(&document) {
        Ok(mut json) => {
            json.push('\n');
            CliOutcome::success(json)
        }
        Err(error) => CliOutcome::failure(
            1,
            String::new(),
            format!("osr: could not serialize syntax document: {error}\n"),
        ),
    }
}

pub(super) fn run_doc(arguments: &[String]) -> CliOutcome {
    match arguments {
        [] => {
            CliOutcome::success("Usage: osr doc <graphql-document>\n       osr doc -\n".to_owned())
        }
        [query] if query != "-" => execute(query),
        [query] if query == "-" => CliOutcome::usage_error("'doc -' requires standard input"),
        _ => CliOutcome::usage_error("doc accepts exactly one GraphQL document"),
    }
}

pub fn run_doc_stdio() -> io::Result<CliOutcome> {
    let mut query = String::new();
    io::stdin().lock().read_to_string(&mut query)?;
    Ok(execute(&query))
}

fn execute(query: &str) -> CliOutcome {
    match crate::documentation::execute_graphql(query) {
        Ok(response) => CliOutcome::success(response),
        Err(message) => CliOutcome::failure(1, String::new(), format!("osr: {message}\n")),
    }
}
