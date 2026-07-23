use super::*;

pub(super) fn run_program(arguments: &[String]) -> CliOutcome {
    let arguments = match parse_run_arguments(arguments) {
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
    let staged_records = match aggregate_result_records(&workspace.units) {
        Ok(records) => records,
        Err(diagnostics) => {
            let stderr = diagnostics
                .iter()
                .map(|diagnostic| format!("error[{}]: {}\n", diagnostic.code, diagnostic.message))
                .collect();
            return CliOutcome::failure(1, String::new(), stderr);
        }
    };

    let temporary = std::env::temp_dir().join(format!(
        "osiris-run-{}-{}",
        std::process::id(),
        NEXT_RUN_ID.fetch_add(1, Ordering::Relaxed)
    ));
    if let Err(error) = fs::create_dir(&temporary) {
        return CliOutcome::failure(
            1,
            String::new(),
            format!("osr: could not create run directory: {error}\n"),
        );
    }
    let records_path = temporary.join(records_artifact_path(&context.options.distribution));
    if let Err(error) = fs::write(&records_path, &staged_records.bytes) {
        let _ = fs::remove_dir_all(&temporary);
        return CliOutcome::failure(
            1,
            String::new(),
            format!("osr: could not stage runtime records: {error}\n"),
        );
    }
    let records_resolver = match build_runtime_records_resolver(
        &context,
        &loaded.records_resolver,
        &records_path,
        &staged_records,
        &workspace,
    ) {
        Ok(resolver) => resolver,
        Err(message) => {
            let _ = fs::remove_dir_all(&temporary);
            return CliOutcome::failure(1, String::new(), format!("osr: {message}\n"));
        }
    };
    let resolver_bytes = match serde_json::to_vec(&records_resolver) {
        Ok(bytes) => bytes,
        Err(error) => {
            let _ = fs::remove_dir_all(&temporary);
            return CliOutcome::failure(
                1,
                String::new(),
                format!("osr: could not serialize runtime records resolver: {error}\n"),
            );
        }
    };
    let records_resolver_path = temporary.join("osiris.records-resolver.json");
    if let Err(error) = fs::write(&records_resolver_path, resolver_bytes) {
        let _ = fs::remove_dir_all(&temporary);
        return CliOutcome::failure(
            1,
            String::new(),
            format!("osr: could not stage runtime records resolver: {error}\n"),
        );
    }
    let mut entry_path = None;
    for (index, result) in workspace.units.into_iter().enumerate() {
        let module_name = &result.analysis.hir.name;
        let Some(generated) = result.python else {
            let _ = fs::remove_dir_all(&temporary);
            return CliOutcome::failure(
                1,
                String::new(),
                format!("osr: compiler produced no Python output for `{module_name}`\n"),
            );
        };
        let generated_path = temporary.join(python_module_path(module_name));
        let Some(parent) = generated_path.parent() else {
            let _ = fs::remove_dir_all(&temporary);
            return CliOutcome::failure(
                1,
                String::new(),
                format!("osr: invalid generated module path for `{module_name}`\n"),
            );
        };
        if let Err(error) =
            fs::create_dir_all(parent).and_then(|()| fs::write(&generated_path, generated.source))
        {
            let _ = fs::remove_dir_all(&temporary);
            return CliOutcome::failure(
                1,
                String::new(),
                format!("osr: could not write temporary Python module: {error}\n"),
            );
        }
        if index == sources.entry_index {
            entry_path = Some(generated_path);
        }
    }
    let Some(entry_path) = entry_path else {
        let _ = fs::remove_dir_all(&temporary);
        return CliOutcome::failure(
            1,
            String::new(),
            "osr: workspace compiler did not return the entry module\n".to_owned(),
        );
    };
    let mut python_paths = vec![temporary.clone()];
    if let Some(existing) = std::env::var_os("PYTHONPATH") {
        python_paths.extend(std::env::split_paths(&existing));
    }
    let python_path = match std::env::join_paths(python_paths) {
        Ok(path) => path,
        Err(error) => {
            let _ = fs::remove_dir_all(&temporary);
            return CliOutcome::failure(
                1,
                String::new(),
                format!("osr: could not construct Python import path: {error}\n"),
            );
        }
    };
    let output = Command::new("python3")
        .arg(&entry_path)
        .args(arguments.program_arguments)
        .env("PYTHONPATH", python_path)
        .env("OSIRIS_PROJECT_RECORDS", &records_path)
        .env("OSIRIS_RECORDS_RESOLVER", &records_resolver_path)
        .output();
    let _ = fs::remove_dir_all(&temporary);
    match output {
        Ok(output) => CliOutcome::failure(
            output.status.code().unwrap_or(1).clamp(0, u8::MAX.into()) as u8,
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ),
        Err(error) => CliOutcome::failure(
            1,
            String::new(),
            format!("osr: could not start Python: {error}\n"),
        ),
    }
}

pub(super) struct RunArguments<'a> {
    path: &'a str,
    site_roots: Vec<&'a str>,
    program_arguments: &'a [String],
}

pub(super) fn parse_run_arguments(arguments: &[String]) -> Result<RunArguments<'_>, String> {
    let separator = arguments.iter().position(|argument| argument == "--");
    let (compiler_arguments, program_arguments) = separator.map_or_else(
        || (arguments, &[][..]),
        |index| (&arguments[..index], &arguments[index + 1..]),
    );
    let mut path = None;
    let mut site_roots = Vec::new();
    let mut index = 0;
    while let Some(argument) = compiler_arguments.get(index) {
        match argument.as_str() {
            "--site-root" => {
                let Some(value) = compiler_arguments.get(index + 1) else {
                    return Err("missing value for '--site-root'".to_owned());
                };
                site_roots.push(value.as_str());
                index += 1;
            }
            option if option.starts_with('-') => {
                return Err(format!("unknown option '{option}' for 'run'"));
            }
            positional if path.is_none() => path = Some(positional),
            _ => return Err("program arguments must follow '--'".to_owned()),
        }
        index += 1;
    }
    Ok(RunArguments {
        path: path.ok_or_else(|| "missing FILE for 'run'".to_owned())?,
        site_roots,
        program_arguments,
    })
}
