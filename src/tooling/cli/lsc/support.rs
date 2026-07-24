use super::*;

pub(super) struct At {
    pub(super) path: String,
    pub(super) position: Position,
}

pub(super) fn parse_at(raw: &str) -> Result<At, String> {
    let (head, column) = raw
        .rsplit_once(':')
        .ok_or_else(|| "--at must use PATH:LINE:COLUMN".to_owned())?;
    let (path, line) = head
        .rsplit_once(':')
        .ok_or_else(|| "--at must use PATH:LINE:COLUMN".to_owned())?;
    let line = line
        .parse::<u32>()
        .map_err(|_| "--at LINE must be a positive integer".to_owned())?;
    let column = column
        .parse::<u32>()
        .map_err(|_| "--at COLUMN must be a positive integer".to_owned())?;
    if line == 0 || column == 0 || path.is_empty() {
        return Err("--at uses one-based positive LINE and COLUMN values".to_owned());
    }
    Ok(At {
        path: path.strip_prefix("file://").unwrap_or(path).to_owned(),
        position: Position {
            line: line - 1,
            character: column - 1,
        },
    })
}

pub(super) fn parse_at_only(arguments: &[String]) -> Result<At, String> {
    match arguments {
        [option, value] if option == "--at" => parse_at(value),
        _ => Err("operation requires exactly '--at PATH:LINE:COLUMN'".to_owned()),
    }
}

pub(super) fn parse_rename_arguments(arguments: &[String]) -> Result<(At, String), String> {
    let mut at = None;
    let mut to = None;
    let mut index = 0;
    while let Some(argument) = arguments.get(index) {
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| format!("missing value for '{argument}'"))?;
        match argument.as_str() {
            "--at" if at.is_none() => at = Some(parse_at(value)?),
            "--to" if to.is_none() => to = Some(value.clone()),
            "--at" | "--to" => return Err(format!("duplicate option '{argument}'")),
            _ => return Err(format!("unknown rename option '{argument}'")),
        }
        index += 2;
    }
    Ok((
        at.ok_or_else(|| "rename requires '--at'".to_owned())?,
        to.ok_or_else(|| "rename requires '--to'".to_owned())?,
    ))
}

pub(super) fn required_single<'a>(arguments: &'a [String], label: &str) -> Result<&'a str, String> {
    match arguments {
        [value] => Ok(value),
        [] => Err(format!("missing {label}")),
        _ => Err("unexpected arguments".to_owned()),
    }
}

pub(super) fn optional_single_path(arguments: &[String]) -> Result<Option<String>, String> {
    match arguments {
        [] => Ok(None),
        [path] => Ok(Some(path.clone())),
        _ => Err("diagnostics accepts at most one path".to_owned()),
    }
}

pub(super) fn location_result(location: Option<Location>) -> Result<(JsonValue, String), String> {
    let text = location.as_ref().map(render_location).unwrap_or_default();
    serde_json::to_value(location)
        .map(|value| (value, text))
        .map_err(|error| error.to_string())
}

pub(super) fn locations_result(locations: Vec<Location>) -> Result<(JsonValue, String), String> {
    let text = locations.iter().map(render_location).collect();
    serde_json::to_value(locations)
        .map(|value| (value, text))
        .map_err(|error| error.to_string())
}

pub(super) fn render_location(location: &Location) -> String {
    format!(
        "{}:{}:{}\n",
        location.uri,
        location.range.start.line + 1,
        location.range.start.character + 1
    )
}

pub(super) fn render_workspace_edit(edit: &crate::lsp::WorkspaceEdit) -> String {
    let mut output = String::new();
    for (uri, edits) in &edit.changes {
        for edit in edits {
            let _ = writeln!(
                output,
                "{}:{}:{} -> {}",
                uri,
                edit.range.start.line + 1,
                edit.range.start.character + 1,
                edit.new_text
            );
        }
    }
    output
}

pub(super) fn render_semantic(document: &SemanticDocument) -> String {
    let mut output = format!("module {}\n", document.module);
    for symbol in &document.symbols {
        let _ = writeln!(
            output,
            "{}\t{}\t{}",
            symbol.binding_id, symbol.canonical, symbol.ty
        );
    }
    output
}

pub(super) fn render_result(
    operation: &str,
    result: JsonValue,
    text: String,
    failed: bool,
    format: LscFormat,
) -> CliOutcome {
    let stdout = match format {
        LscFormat::Text => text,
        LscFormat::Json => match serde_json::to_string_pretty(
            &json!({ "schema": LSC_SCHEMA, "operation": operation, "result": result }),
        ) {
            Ok(mut output) => {
                output.push('\n');
                output
            }
            Err(error) => {
                return CliOutcome::failure(
                    1,
                    String::new(),
                    format!("osr: could not serialize lsc result: {error}\n"),
                );
            }
        },
    };
    if failed {
        CliOutcome::failure(1, stdout, String::new())
    } else {
        CliOutcome::success(stdout)
    }
}
