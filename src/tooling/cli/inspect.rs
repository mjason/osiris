use super::*;

pub(super) fn config_error(error: &ConfigError) -> CliOutcome {
    CliOutcome::failure(1, String::new(), format!("osr: {error}\n"))
}

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

pub(super) fn run_inspect(arguments: &[String]) -> CliOutcome {
    let (path, format, view) = match parse_inspect_arguments(arguments) {
        Ok(parsed) => parsed,
        Err(message) => return CliOutcome::usage_error(message),
    };

    if view == InspectView::Semantic {
        return run_semantic_inspect(path, format);
    }

    let (source, document) = match read_source(path) {
        Ok(result) => result,
        Err(error) => return io_error(path, &error),
    };
    let diagnostics = diagnostic::render_all(path, &source, &document.diagnostics);
    let rendered = match format {
        InspectFormat::Text => render_document_text(&document),
        InspectFormat::Json => match render_document_json(&document) {
            Ok(rendered) => rendered,
            Err(error) => {
                return CliOutcome::failure(
                    1,
                    String::new(),
                    format!("{diagnostics}osr: could not render '{path}' as JSON: {error}\n"),
                );
            }
        },
    };
    if document.has_errors() {
        CliOutcome::failure(1, rendered, diagnostics)
    } else {
        CliOutcome::success(rendered)
    }
}

pub(super) fn run_semantic_inspect(path: &str, format: InspectFormat) -> CliOutcome {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) => return io_error(path, &error),
    };
    let context = match compile_context(Path::new(path)) {
        Ok(context) => context,
        Err(error) => return config_error(&error),
    };
    let analysis = compiler::analyze(&source, &context.options);
    let diagnostics = diagnostic::render_all(path, &source, &analysis.diagnostics);
    let semantic = SemanticDocument::from_analysis(&analysis, path);
    let rendered = match format {
        InspectFormat::Json => semantic.to_pretty_json(),
        InspectFormat::Text => Ok(render_semantic_text(&semantic)),
    };
    let mut rendered = match rendered {
        Ok(rendered) => rendered,
        Err(error) => {
            return CliOutcome::failure(
                1,
                String::new(),
                format!("{diagnostics}osr: could not render semantic model: {error}\n"),
            );
        }
    };
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    if analysis.has_errors() {
        CliOutcome::failure(1, rendered, diagnostics)
    } else {
        CliOutcome::success(rendered)
    }
}

pub(super) fn render_semantic_text(document: &SemanticDocument) -> String {
    use std::fmt::Write as _;

    let mut output = format!("module {}\n", document.module);
    for symbol in &document.symbols {
        let visibility = if symbol.public { "public" } else { "private" };
        let _ = writeln!(
            output,
            "{visibility} {:?} {} :: {:?}",
            symbol.kind, symbol.canonical, symbol.ty
        );
        if !symbol.aliases.is_empty() {
            let aliases = symbol
                .aliases
                .iter()
                .map(|alias| alias.spelling.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(output, "  aliases: {aliases}");
        }
    }
    if !document.operation_graph.nodes.is_empty() {
        output.push_str("operations\n");
        for operation in &document.operation_graph.nodes {
            let _ = writeln!(
                output,
                "  {} [{}..{}]",
                operation.labels.zh_cn, operation.span.start, operation.span.end
            );
        }
    }
    output
}

pub(super) fn parse_inspect_arguments(
    arguments: &[String],
) -> Result<(&str, InspectFormat, InspectView), String> {
    let mut path = None;
    let mut format = InspectFormat::Text;
    let mut view = InspectView::Syntax;
    let mut saw_format = false;
    let mut saw_view = false;
    let mut index = 0;

    while let Some(argument) = arguments.get(index) {
        match argument.as_str() {
            "--syntax" if !saw_view => {
                view = InspectView::Syntax;
                saw_view = true;
            }
            "--semantic" if !saw_view => {
                view = InspectView::Semantic;
                saw_view = true;
            }
            "--syntax" | "--semantic" => {
                return Err("inspect accepts only one of '--syntax' or '--semantic'".to_owned());
            }
            "--format" if saw_format => {
                return Err("duplicate option '--format' for 'inspect'".to_owned());
            }
            "--format" => {
                let Some(value) = arguments.get(index + 1) else {
                    return Err("missing value for '--format'".to_owned());
                };
                format = match value.as_str() {
                    "text" => InspectFormat::Text,
                    "json" => InspectFormat::Json,
                    _ => {
                        return Err(format!(
                            "invalid value '{value}' for '--format'; expected 'text' or 'json'"
                        ));
                    }
                };
                saw_format = true;
                index += 1;
            }
            option if option.starts_with('-') => {
                return Err(format!("unknown option '{option}' for 'inspect'"));
            }
            positional if path.is_none() => path = Some(positional),
            _ => return Err("unexpected arguments for 'inspect'".to_owned()),
        }
        index += 1;
    }

    path.map(|path| (path, format, view))
        .ok_or_else(|| "missing FILE for 'inspect'".to_owned())
}
