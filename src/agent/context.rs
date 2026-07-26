use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{project::ProjectConfig, stdlib};

#[derive(Clone, Debug)]
pub(super) struct ContextMaterial {
    pub(super) text: String,
    pub(super) references: Vec<String>,
}

pub(super) fn collect_material(
    root: &Path,
    requested_file: Option<&Path>,
    request: &str,
    project: Option<&ProjectConfig>,
) -> Result<ContextMaterial, String> {
    let mut text = String::new();
    let mut references = Vec::new();
    let project_request = is_project_request(request);
    if (!project_request || is_language_request(request))
        && let Ok(syntax) = crate::documentation::syntax_markdown()
    {
        text.push_str("## Osiris syntax\n");
        text.push_str(&retrieve_syntax_sections(&syntax.markdown, request));
        text.push('\n');
        references.push(syntax.id);
    }
    if project_request && let Some(manual) = crate::documentation::document_markdown("tooling/cli")?
    {
        text.push_str("\n## Osiris project and package manual\n");
        text.push_str(&retrieve_project_sections(&manual.markdown, request));
        text.push('\n');
        references.push(manual.id);
    }
    let mut records = stdlib::NAMESPACES
        .iter()
        .flat_map(|namespace| stdlib::exports(namespace))
        .filter_map(|binding| {
            let score = binding_score(request, binding);
            (score >= 10).then_some((score, binding))
        })
        .collect::<Vec<_>>();
    records.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .cmp(left_score)
            .then_with(|| left.id().as_str().cmp(right.id().as_str()))
    });
    for (_, binding) in records.into_iter().take(8) {
        let record = stdlib::retrieval_record(binding);
        text.push_str("\n## Standard API\n");
        text.push_str(record.canonical);
        text.push('\n');
        text.push_str("Type: ");
        text.push_str(&record.signature);
        text.push('\n');
        text.push_str("Call forms: ");
        text.push_str(&record.call_shapes.join(" | "));
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
        let context_kind = context_kind(root, &path, project)?;
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("could not read '{}': {error}", path.display()))?;
        match context_kind {
            ContextKind::Osiris => {
                text.push_str("\n## Explicitly requested Osiris source\n```osiris\n");
                text.push_str(&source.chars().take(24_000).collect::<String>());
                text.push_str("\n```\n");
            }
            ContextKind::ProjectConfig => {
                text.push_str("\n## Explicitly requested project configuration\n```jsonc\n");
                text.push_str(&redact_project_config(&source)?);
                text.push_str("\n```\n");
            }
        }
        references.push(
            path.strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string(),
        );
    }
    Ok(ContextMaterial { text, references })
}

fn binding_score(request: &str, binding: stdlib::StandardBinding) -> usize {
    let request = request.to_lowercase();
    let terms = request_terms(&request);
    let canonical = binding.canonical.to_lowercase();
    let namespace = binding.namespace.to_lowercase();
    let id = binding.id().as_str().to_lowercase();
    let slash_qualified = format!("{namespace}/{canonical}");
    let dot_qualified = format!("{namespace}.{canonical}");
    if request.contains(&id) {
        200
    } else if request.contains(&slash_qualified) || request.contains(&dot_qualified) {
        150
    } else if terms.iter().any(|term| term == &canonical) {
        100
    } else if terms.iter().any(|term| term == &namespace) {
        10
    } else {
        0
    }
}

#[derive(Clone, Copy)]
enum ContextKind {
    Osiris,
    ProjectConfig,
}

fn context_kind(
    root: &Path,
    path: &Path,
    project: Option<&ProjectConfig>,
) -> Result<ContextKind, String> {
    if let Some("osr" | "osri") = path.extension().and_then(|value| value.to_str()) {
        return Ok(ContextKind::Osiris);
    }
    let config_root = project.map_or(root, |project| project.root.as_path());
    if path.file_name().and_then(|value| value.to_str()) == Some("osiris.jsonc")
        && path.parent() == Some(config_root)
    {
        return Ok(ContextKind::ProjectConfig);
    }
    Err("--file accepts only project .osr/.osri files or the project-root osiris.jsonc; credential-bearing files such as .env are forbidden".to_owned())
}

fn redact_project_config(source: &str) -> Result<String, String> {
    let mut value: serde_json::Value = json5::from_str(source)
        .map_err(|error| format!("could not parse explicitly requested osiris.jsonc: {error}"))?;
    redact_sensitive_values(&mut value);
    serde_json::to_string_pretty(&value)
        .map_err(|error| format!("could not render explicitly requested osiris.jsonc: {error}"))
}

fn redact_sensitive_values(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(values) => {
            for (key, value) in values {
                let key = key.to_ascii_lowercase();
                if key == "baseurl"
                    || matches!(
                        key.as_str(),
                        "key"
                            | "apikey"
                            | "api_key"
                            | "token"
                            | "apitoken"
                            | "access_token"
                            | "accesstoken"
                            | "secret"
                            | "clientsecret"
                            | "client_secret"
                            | "password"
                            | "authorization"
                    )
                    || ["token", "secret", "password", "apikey"]
                        .iter()
                        .any(|marker| key.ends_with(marker))
                {
                    *value = serde_json::Value::String("<redacted by osr lsa>".to_owned());
                } else {
                    redact_sensitive_values(value);
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                redact_sensitive_values(value);
            }
        }
        _ => {}
    }
}

fn is_project_request(request: &str) -> bool {
    let request = request.to_lowercase();
    [
        "osiris.jsonc",
        "pyproject",
        "config",
        "project",
        "outdir",
        "output",
        "exclude",
        "targetpython",
        "init",
        "build",
        "watch",
        "publish",
        "package",
        "pypi",
        "uv",
        "配置",
        "项目",
        "输出",
        "排除",
        "构建",
        "发布",
        "依赖",
        "发布包",
        "发布库",
        "依赖包",
        "扩展包",
        "软件包",
        "包管理",
        "設定",
        "プロジェクト",
        "出力",
        "ビルド",
        "公開",
        "パッケージ",
    ]
    .iter()
    .any(|marker| request.contains(marker))
}

fn is_language_request(request: &str) -> bool {
    let request = request.to_lowercase();
    [
        "example",
        "syntax",
        "defn",
        "defstruct",
        "macro",
        "import",
        "type",
        "示例",
        "例子",
        "源码",
        "语法",
        "函数",
        "宏",
        "类型",
        "サンプル",
        "構文",
        "関数",
        "マクロ",
        "型",
    ]
    .iter()
    .any(|marker| request.contains(marker))
}

fn retrieve_project_sections(markdown: &str, request: &str) -> String {
    let mut terms = request_terms(request);
    let request = request.to_lowercase();
    if [
        "osiris.jsonc",
        "config",
        "outdir",
        "output",
        "exclude",
        "配置",
        "输出",
        "排除",
        "設定",
        "出力",
    ]
    .iter()
    .any(|marker| request.contains(marker))
    {
        terms.extend(
            [
                "project",
                "configuration",
                "outdir",
                "output",
                "source",
                "exclude",
            ]
            .into_iter()
            .map(str::to_owned),
        );
    }
    if [
        "publish",
        "package",
        "pypi",
        "发布",
        "发布库",
        "依赖库",
        "软件包",
        "依赖",
        "公開",
        "パッケージ",
    ]
    .iter()
    .any(|marker| request.contains(marker))
    {
        terms.extend(
            [
                "publishing",
                "publish",
                "package",
                "pypi",
                "dependency",
                "uv",
            ]
            .into_iter()
            .map(str::to_owned),
        );
    }
    if [
        "init",
        "build",
        "watch",
        "run",
        "初始化",
        "构建",
        "运行",
        "ビルド",
    ]
    .iter()
    .any(|marker| request.contains(marker))
    {
        terms.extend(
            ["projects", "init", "build", "watch", "run"]
                .into_iter()
                .map(str::to_owned),
        );
    }
    terms.sort();
    terms.dedup();
    retrieve_ranked_sections(markdown, &terms, 3, 14_000)
}

fn retrieve_ranked_sections(
    markdown: &str,
    terms: &[String],
    maximum_sections: usize,
    maximum_characters: usize,
) -> String {
    let mut starts = vec![0];
    starts.extend(markdown.match_indices("\n## ").map(|(index, _)| index + 1));
    starts.push(markdown.len());
    let mut sections = starts
        .windows(2)
        .filter_map(|range| {
            let section = &markdown[range[0]..range[1]];
            let lower = section.to_lowercase();
            let heading = lower.lines().next().unwrap_or_default();
            let score = terms
                .iter()
                .map(|term| {
                    lower.matches(term).count() + heading.matches(term).count().saturating_mul(20)
                })
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
    for (_, _, section) in sections.into_iter().take(maximum_sections) {
        if output.chars().count() >= maximum_characters {
            break;
        }
        output.push_str(section);
        output.push('\n');
    }
    output.chars().take(maximum_characters).collect()
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
        "hello", "module", "one", "provide", "show", "that", "the", "this", "typed", "using",
        "with", "word", "world", "write",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_hello_world_terms_do_not_retrieve_keyword_sections() {
        let markdown = format!(
            "# Syntax\n\n{}\n\n## Keywords\n\nA keyword section that is intentionally large.\n\n## Functions\n\nFunction syntax.",
            "Preamble. ".repeat(120)
        );
        let retrieved = retrieve_syntax_sections(&markdown, "Write a hello world example");

        assert!(retrieved.contains("## Manual preamble"));
        assert!(!retrieved.contains("## Keywords"));
        assert!(!retrieved.contains("## Functions"));
    }
}
