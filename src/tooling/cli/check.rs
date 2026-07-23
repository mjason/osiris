use super::*;

pub(super) fn run_check(arguments: &[String]) -> CliOutcome {
    let arguments = match parse_check_arguments(arguments) {
        Ok(arguments) => arguments,
        Err(message) => return CliOutcome::usage_error(message),
    };
    let context = match compile_context(Path::new(arguments.path)) {
        Ok(context) => context,
        Err(error) => return config_error(&error),
    };
    let mut sources = match load_workspace_sources(Path::new(arguments.path), &context) {
        Ok(sources) => sources,
        Err(message) => return CliOutcome::failure(1, String::new(), format!("osr: {message}\n")),
    };
    let loaded = match load_external_interfaces(&context, &arguments.site_roots) {
        Ok(loaded) => loaded,
        Err(message) => return CliOutcome::failure(1, String::new(), format!("osr: {message}\n")),
    };
    sources.install_trust_policy(&loaded.trust_policy);
    let inputs = workspace_compile_inputs(&sources);
    let workspace = compiler::compile_workspace(&inputs, &loaded.interfaces);
    if workspace.has_errors() {
        return CliOutcome::failure(
            1,
            String::new(),
            render_workspace_diagnostics(&sources, &workspace.diagnostics),
        );
    }
    CliOutcome::success(String::new())
}

pub(super) struct CheckArguments<'a> {
    path: &'a str,
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
    Ok(CheckArguments {
        path: path.ok_or_else(|| "missing FILE for 'check'".to_owned())?,
        site_roots,
    })
}
