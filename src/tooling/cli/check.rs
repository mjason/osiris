use super::*;

pub(super) fn run_check(arguments: &[String]) -> CliOutcome {
    let timing_started = std::time::Instant::now();
    let timings = std::env::var_os("OSIRIS_TIMINGS").is_some();
    let arguments = match parse_check_arguments(arguments) {
        Ok(arguments) => arguments,
        Err(message) => return CliOutcome::usage_error(message),
    };
    let requested = Path::new(arguments.path.unwrap_or("."));
    let entry = if requested.is_dir() {
        let project = match ProjectConfig::discover(requested) {
            Ok(project) => project,
            Err(error) => return config_error(&error),
        };
        match first_project_source(&project) {
            Ok(entry) => entry,
            Err(message) => {
                return CliOutcome::failure(1, String::new(), format!("osr: {message}\n"));
            }
        }
    } else {
        requested.to_path_buf()
    };
    let context = match compile_context(&entry) {
        Ok(context) => context,
        Err(error) => return config_error(&error),
    };
    if timings {
        eprintln!("osr timing: project context {:?}", timing_started.elapsed());
    }
    let sources_started = std::time::Instant::now();
    let mut sources = match load_workspace_sources(&entry, &context) {
        Ok(sources) => sources,
        Err(message) => return CliOutcome::failure(1, String::new(), format!("osr: {message}\n")),
    };
    if timings {
        eprintln!(
            "osr timing: source discovery {:?}",
            sources_started.elapsed()
        );
    }
    let extensions_started = std::time::Instant::now();
    let loaded = match load_external_interfaces(&context, &arguments.site_roots) {
        Ok(loaded) => loaded,
        Err(message) => return CliOutcome::failure(1, String::new(), format!("osr: {message}\n")),
    };
    if timings {
        eprintln!(
            "osr timing: extension discovery {:?}",
            extensions_started.elapsed()
        );
    }
    sources.install_trust_policy(&loaded.trust_policy);
    let inputs = workspace_compile_inputs(&sources);
    let workspace = compiler::analyze_workspace(&inputs, &loaded.interfaces);
    if workspace.has_errors() {
        return CliOutcome::failure(
            1,
            String::new(),
            render_workspace_diagnostics(&sources, &workspace.diagnostics),
        );
    }
    let locale = context
        .project
        .as_ref()
        .and_then(|project| project.display_locale.as_deref());
    let chinese = locale.is_some_and(|locale| locale == "zh" || locale.starts_with("zh-"));
    let mut stderr = String::new();
    for (source, result) in sources.units.iter().zip(&workspace.units) {
        for advisory in &result.analysis.migration_advisories {
            let replacement = advisory.replacement(locale);
            let message = if chinese {
                format!(
                    "`{}` 是兼容旧源码的别名；请改用 `{replacement}`",
                    advisory.alias
                )
            } else {
                format!(
                    "`{}` is a source-compatibility alias; use `{replacement}`",
                    advisory.alias
                )
            };
            stderr.push_str(&diagnostic::render_warning(
                &source.path.display().to_string(),
                &source.source,
                advisory.span,
                "OSR-L0002",
                &message,
            ));
        }
    }
    CliOutcome {
        exit_code: 0,
        stdout: String::new(),
        stderr,
    }
}

pub(super) struct CheckArguments<'a> {
    path: Option<&'a str>,
    site_roots: Vec<&'a str>,
}

pub(super) fn parse_check_arguments(arguments: &[String]) -> Result<CheckArguments<'_>, String> {
    let mut path = None;
    let mut site_roots = Vec::new();
    let mut index = 0;
    while let Some(argument) = arguments.get(index) {
        match argument.as_str() {
            "--site-root" => {
                let Some(value) = arguments.get(index + 1) else {
                    return Err("missing value for '--site-root'".to_owned());
                };
                site_roots.push(value.as_str());
                index += 1;
            }
            option if option.starts_with('-') => {
                return Err(format!("unknown option '{option}' for 'check'"));
            }
            positional if path.is_none() => path = Some(positional),
            _ => return Err("unexpected arguments for 'check'".to_owned()),
        }
        index += 1;
    }
    Ok(CheckArguments { path, site_roots })
}
