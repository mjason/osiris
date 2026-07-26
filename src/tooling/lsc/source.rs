use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value as JsonValue, json};

use crate::{
    lsp::{Position, Range, offset_to_position, position_to_offset},
    project::ProjectConfig,
    reader,
};

const MAX_CONTEXT_BYTES: usize = 6 * 1024;
const MAX_DOCUMENT_SYMBOLS: usize = 8;

pub(super) fn source_context_from_text(
    uri: &str,
    source: &str,
    range: Range,
) -> Result<JsonValue, String> {
    let offset = position_to_offset(source, range.start)
        .ok_or_else(|| format!("source position is outside `{uri}`"))?;
    let document = reader::read(source);
    let form = document
        .forms
        .iter()
        .filter(|form| form.span.start <= offset && offset <= form.span.end)
        .min_by_key(|form| form.span.end.saturating_sub(form.span.start));
    let (start, end) = form.map_or_else(
        || line_window(source, range.start.line as usize, 2),
        |form| (form.span.start, form.span.end),
    );
    let end = end
        .min(source.len())
        .min(start.saturating_add(MAX_CONTEXT_BYTES));
    let text = source
        .get(start..end)
        .ok_or_else(|| format!("could not slice source context for `{uri}`"))?;
    Ok(json!({
        "uri": uri,
        "range": {
            "start": offset_to_position(source, start),
            "end": offset_to_position(source, end),
        },
        "selectionRange": range,
        "text": text,
        "truncated": end < form.map_or(end, |form| form.span.end),
    }))
}

fn line_window(source: &str, line: usize, radius: usize) -> (usize, usize) {
    let starts = std::iter::once(0)
        .chain(source.match_indices('\n').map(|(index, _)| index + 1))
        .collect::<Vec<_>>();
    let start_line = line
        .saturating_sub(radius)
        .min(starts.len().saturating_sub(1));
    let end_line = line.saturating_add(radius + 1).min(starts.len());
    let start = starts.get(start_line).copied().unwrap_or_default();
    let end = starts.get(end_line).copied().unwrap_or(source.len());
    (start, end)
}

pub(super) fn project_sources(project: &ProjectConfig) -> Result<Vec<PathBuf>, String> {
    fn visit(
        directory: &Path,
        project: &ProjectConfig,
        paths: &mut Vec<PathBuf>,
    ) -> Result<(), String> {
        let entries = fs::read_dir(directory)
            .map_err(|error| format!("could not scan '{}': {error}", directory.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            if project.is_excluded(&path) {
                continue;
            }
            let file_type = entry.file_type().map_err(|error| error.to_string())?;
            if file_type.is_dir() {
                visit(&path, project, paths)?;
            } else if file_type.is_file()
                && path.extension().and_then(|value| value.to_str()) == Some("osr")
            {
                paths.push(path);
            }
        }
        Ok(())
    }
    let mut paths = Vec::new();
    for root in &project.source_roots {
        visit(root, project, &mut paths)?;
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

pub(super) fn first_source(project: &ProjectConfig) -> Result<PathBuf, String> {
    project_sources(project)?
        .into_iter()
        .next()
        .ok_or_else(|| "project has no Osiris sources for language-service queries".to_owned())
}

pub(super) fn json_position(value: Option<&JsonValue>) -> Option<Position> {
    serde_json::from_value(value?.clone()).ok()
}

pub(super) fn bound_document_symbols(result: &mut JsonValue, position: Position) {
    let Some(symbols) = result["value"].as_array_mut() else {
        return;
    };
    let mut containing = symbols
        .iter()
        .filter(|symbol| {
            serde_json::from_value::<Range>(symbol["range"].clone()).is_ok_and(|range| {
                position_after_or_equal(position, range.start)
                    && position_after_or_equal(range.end, position)
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    if containing.is_empty() {
        symbols.sort_by_key(|symbol| {
            let line = symbol["range"]["start"]["line"]
                .as_u64()
                .unwrap_or_default();
            line.abs_diff(u64::from(position.line))
        });
        symbols.truncate(MAX_DOCUMENT_SYMBOLS);
    } else {
        containing.truncate(MAX_DOCUMENT_SYMBOLS);
        *symbols = containing;
    }
}

const fn position_after_or_equal(left: Position, right: Position) -> bool {
    left.line > right.line || (left.line == right.line && left.character >= right.character)
}

pub(super) fn path_to_uri(path: &Path) -> Result<String, String> {
    let canonical = fs::canonicalize(path)
        .map_err(|error| format!("could not resolve '{}': {error}", path.display()))?;
    Ok(format!("file://{}", canonical.display()))
}

pub(super) fn uri_to_path(uri: &str) -> Result<PathBuf, String> {
    let path = uri
        .strip_prefix("file://")
        .ok_or_else(|| format!("unsupported source URI `{uri}`"))?;
    if !path.starts_with('/') {
        return Err(format!("unsupported non-local source URI `{uri}`"));
    }
    percent_decode(path).map(PathBuf::from)
}

pub(super) fn percent_decode(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex(*bytes.get(index + 1).ok_or("invalid percent-encoded URI")?)?;
            let low = hex(*bytes.get(index + 2).ok_or("invalid percent-encoded URI")?)?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).map_err(|_| "source URI is not valid UTF-8".to_owned())
}

fn hex(value: u8) -> Result<u8, String> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err("invalid percent-encoded URI".to_owned()),
    }
}
