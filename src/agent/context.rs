use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::stdlib;

#[derive(Clone, Debug)]
pub(super) struct ContextMaterial {
    pub(super) text: String,
    pub(super) references: Vec<String>,
}

pub(super) fn collect_material(
    root: &Path,
    requested_file: Option<&Path>,
    request: &str,
) -> Result<ContextMaterial, String> {
    let mut text = String::new();
    let mut references = Vec::new();
    if let Ok(syntax) = crate::documentation::syntax_markdown() {
        text.push_str("## Osiris syntax\n");
        text.push_str(&retrieve_syntax_sections(&syntax.markdown, request));
        text.push('\n');
        references.push(syntax.id);
    }
    let mut records = stdlib::api_catalog()
        .into_iter()
        .filter_map(|record| {
            let score = record_score(request, &record);
            (score >= 10).then_some((score, record))
        })
        .collect::<Vec<_>>();
    records.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .cmp(left_score)
            .then_with(|| left.binding_id.cmp(&right.binding_id))
    });
    for (_, record) in records.into_iter().take(8) {
        text.push_str("\n## Standard API\n");
        text.push_str(record.canonical);
        text.push('\n');
        text.push_str(&record.signature);
        text.push('\n');
        if let Some(documentation) = &record.documentation.default {
            text.push_str(documentation);
            text.push('\n');
        }
        for documentation in record.documentation.translations.values() {
            text.push_str(documentation);
            text.push('\n');
        }
        for example in record.examples {
            text.push_str("```osiris\n");
            text.push_str(&example.join("\n"));
            text.push_str("\n```\n");
        }
        references.push(record.binding_id);
    }
    if let Some(path) = requested_file {
        let path = safe_project_path(root, path)?;
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("could not read '{}': {error}", path.display()))?;
        text.push_str("\n## Requested source\n```osiris\n");
        text.push_str(&source.chars().take(24_000).collect::<String>());
        text.push_str("\n```\n");
        references.push(
            path.strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string(),
        );
    }
    Ok(ContextMaterial { text, references })
}

fn retrieve_syntax_sections(markdown: &str, request: &str) -> String {
    let mut starts = vec![0];
    starts.extend(markdown.match_indices("\n## ").map(|(index, _)| index + 1));
    starts.push(markdown.len());

    let terms = request_terms(request);
    let mut sections = starts
        .windows(2)
        .filter_map(|range| {
            let section = &markdown[range[0]..range[1]];
            let lower = section.to_lowercase();
            let score = terms
                .iter()
                .map(|term| lower.matches(term).count())
                .sum::<usize>();
            (score > 0).then_some((score, range[0], section))
        })
        .collect::<Vec<_>>();
    sections.sort_by(
        |(left_score, left_start, _), (right_score, right_start, _)| {
            right_score
                .cmp(left_score)
                .then_with(|| left_start.cmp(right_start))
        },
    );

    let mut output = String::new();
    for (_, start, section) in sections.into_iter().take(1) {
        if start == 0 {
            continue;
        }
        output.push_str("\n\n");
        output.push_str(&section.chars().take(6_000).collect::<String>());
    }
    output.push_str("\n\n## Manual preamble\n");
    output.push_str(&markdown.chars().take(1_000).collect::<String>());
    output
}

fn request_terms(request: &str) -> Vec<String> {
    const STOP_WORDS: &[&str] = &[
        "about", "also", "and", "complete", "example", "explain", "for", "from", "minimal",
        "module", "one", "provide", "show", "that", "the", "this", "typed", "using", "with",
    ];
    let mut terms = request
        .split(|character: char| character.is_whitespace() || character.is_ascii_punctuation())
        .map(str::to_lowercase)
        .filter(|term| term.chars().count() >= 2 && !STOP_WORDS.contains(&term.as_str()))
        .collect::<Vec<_>>();
    for symbol in ["->>", "->", "defstruct"] {
        if request.contains(symbol) {
            terms.push(symbol.to_owned());
        }
    }
    terms.sort();
    terms.dedup();
    terms
}

fn record_score(request: &str, record: &stdlib::StandardApiRecord) -> usize {
    let request = request.to_lowercase();
    let terms = request_terms(&request);
    let canonical = record.canonical.to_lowercase();
    let namespace = record.namespace.to_lowercase();
    let qualified = format!("{namespace}/{canonical}");
    let mut score = 0;
    if request.contains(&record.binding_id.to_lowercase()) {
        score += 200;
    }
    if request.contains(&qualified) {
        score += 150;
    }
    if terms.iter().any(|term| term == &canonical) {
        score += 100;
    }
    if request == record.signature.to_lowercase() {
        score += 40;
    }
    if terms.iter().any(|term| term == &namespace) {
        score += 10;
    }
    if let Some(documentation) = &record.documentation.default {
        score += documentation
            .split_whitespace()
            .map(|word| {
                word.trim_matches(|character: char| !character.is_alphanumeric())
                    .to_lowercase()
            })
            .filter(|word| word.len() > 4 && terms.contains(word))
            .count();
    }
    score
}

fn safe_project_path(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let root = fs::canonicalize(root).map_err(|error| {
        format!(
            "could not resolve project root '{}': {error}",
            root.display()
        )
    })?;
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let canonical = fs::canonicalize(&path)
        .map_err(|error| format!("could not resolve '{}': {error}", path.display()))?;
    if !canonical.starts_with(&root) {
        return Err("requested source path must stay inside the project".to_owned());
    }
    Ok(canonical)
}
