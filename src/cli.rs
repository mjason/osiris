use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use serde::Serialize;

use crate::{
    artifact::{Artifact, ArtifactKind, publish_artifacts},
    compiler::{self, CompileOptions, python_module_path},
    dependency, diagnostic,
    extension::{self, normalize_distribution_name},
    interface,
    macro_expand::{self, ExpansionOptions},
    printer::{render_document_json, render_document_text},
    project::{ConfigError, ProjectConfig, PythonVersion},
    reader, records,
    semantic::SemanticDocument,
    source::Span,
};

pub const USAGE: &str = "Usage: osr [OPTIONS]\n       osr check FILE [--site-root DIR]\n       osr compile FILE... [--out-dir DIR] [--emit py,osri,map,records] [--site-root DIR]\n       osr run FILE [--site-root DIR] [-- ARGS...]\n       osr expand [--once] FILE\n       osr inspect [--syntax|--semantic] FILE [--format text|json]\n       osr lsp\n\nCommands:\n  check FILE    Analyze an Osiris project or standalone source file\n  compile FILE  Compile one distribution to Python\n  run FILE      Compile and run an Osiris project entry module\n  expand FILE   Print macro-expanded Osiris forms\n  inspect FILE  Inspect syntax or the semantic model\n  lsp           Run the Language Server Protocol server\n\nOptions:\n  --site-root DIR  Search this installed-package root for locked static extensions\n  -V, --version    Print version\n  -h, --help       Print help";

static NEXT_RUN_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CliOutcome {
    pub exit_code: u8,
    pub stdout: String,
    pub stderr: String,
}

impl CliOutcome {
    fn success(stdout: String) -> Self {
        Self {
            exit_code: 0,
            stdout,
            stderr: String::new(),
        }
    }

    fn failure(exit_code: u8, stdout: String, stderr: String) -> Self {
        Self {
            exit_code,
            stdout,
            stderr,
        }
    }

    fn usage_error(message: impl AsRef<str>) -> Self {
        Self::failure(
            2,
            String::new(),
            format!(
                "osr: {}\nTry 'osr --help' for more information.\n",
                message.as_ref()
            ),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InspectFormat {
    Text,
    Json,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InspectView {
    Syntax,
    Semantic,
}

/// Runs the command-line interface without writing to process streams or exiting.
#[must_use]
pub fn run_cli(arguments: &[String]) -> CliOutcome {
    match arguments {
        [] => CliOutcome::success(format!("{USAGE}\n")),
        [argument] if argument == "-h" || argument == "--help" => {
            CliOutcome::success(format!("{USAGE}\n"))
        }
        [argument] if argument == "-V" || argument == "--version" => {
            CliOutcome::success(format!("osr {}\n", crate::version()))
        }
        [command, rest @ ..] if command == "check" => run_check(rest),
        [command, rest @ ..] if command == "compile" => run_compile(rest),
        [command, rest @ ..] if command == "run" => run_program(rest),
        [command, rest @ ..] if command == "expand" => run_expand(rest),
        [command, rest @ ..] if command == "inspect" => run_inspect(rest),
        _ => CliOutcome::usage_error("unexpected arguments"),
    }
}

fn run_check(arguments: &[String]) -> CliOutcome {
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

struct CheckArguments<'a> {
    path: &'a str,
    site_roots: Vec<&'a str>,
}

fn parse_check_arguments(arguments: &[String]) -> Result<CheckArguments<'_>, String> {
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

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum EmitKind {
    Python,
    Interface,
    SourceMap,
    Records,
}

struct CompileArguments<'a> {
    paths: Vec<&'a str>,
    out_dir: Option<&'a str>,
    site_roots: Vec<&'a str>,
    emit: BTreeSet<EmitKind>,
    explicit_emit: bool,
}

fn run_compile(arguments: &[String]) -> CliOutcome {
    let arguments = match parse_compile_arguments(arguments) {
        Ok(arguments) => arguments,
        Err(message) => return CliOutcome::usage_error(message),
    };
    let first_path = arguments
        .paths
        .first()
        .expect("compile parser requires a source path");
    let context = match compile_context(Path::new(first_path)) {
        Ok(context) => context,
        Err(error) => return config_error(&error),
    };
    let mut explicit_contexts = Vec::with_capacity(arguments.paths.len());
    for path in &arguments.paths {
        let unit_context = match compile_context(Path::new(path)) {
            Ok(context) => context,
            Err(error) => return config_error(&error),
        };
        if unit_context.options.distribution != context.options.distribution
            || unit_context.options.distribution_version != context.options.distribution_version
            || unit_context.options.target_python != context.options.target_python
            || unit_context.default_out_dir != context.default_out_dir
        {
            return CliOutcome::usage_error(
                "all compile inputs must belong to the same Osiris project and distribution",
            );
        }
        explicit_contexts.push((path, unit_context));
    }
    let mut sources = if context.project.is_some() {
        let discovered = match load_workspace_sources(Path::new(first_path), &context) {
            Ok(sources) => sources,
            Err(message) => {
                return CliOutcome::failure(1, String::new(), format!("osr: {message}\n"));
            }
        };
        let discovered_paths = discovered
            .units
            .iter()
            .filter_map(|unit| fs::canonicalize(&unit.path).ok())
            .collect::<BTreeSet<_>>();
        for (path, _) in &explicit_contexts {
            let canonical = match fs::canonicalize(path) {
                Ok(path) => path,
                Err(error) => return io_error(path, &error),
            };
            if !discovered_paths.contains(&canonical) {
                return CliOutcome::usage_error(
                    "all compile inputs must belong to the configured source roots",
                );
            }
        }
        discovered
            .units
            .into_iter()
            .map(|unit| (unit.path.display().to_string(), unit.source, unit.options))
            .collect::<Vec<_>>()
    } else {
        let mut sources = Vec::with_capacity(explicit_contexts.len());
        for (path, unit_context) in explicit_contexts {
            let source = match fs::read_to_string(path) {
                Ok(source) => source,
                Err(error) => return io_error(path, &error),
            };
            sources.push(((*path).to_owned(), source, unit_context.options));
        }
        sources
    };
    if sources.is_empty() {
        return CliOutcome::failure(
            1,
            String::new(),
            "osr: project has no Osiris sources\n".to_owned(),
        );
    }
    let loaded = match load_external_interfaces(&context, &arguments.site_roots) {
        Ok(loaded) => loaded,
        Err(message) => return CliOutcome::failure(1, String::new(), format!("osr: {message}\n")),
    };
    for (_, _, options) in &mut sources {
        options.trust_policy = loaded.trust_policy.clone();
    }
    let compile_inputs = sources
        .iter()
        .map(|(_, source, options)| compiler::CompileInput::new(source, options))
        .collect::<Vec<_>>();
    let workspace = compiler::compile_workspace(&compile_inputs, &loaded.interfaces);
    if workspace.has_errors() {
        let mut rendered = String::new();
        for located in &workspace.diagnostics {
            let Some((path, source, _)) = sources.get(located.input_index) else {
                continue;
            };
            rendered.push_str(&diagnostic::render_all(
                path,
                source,
                std::slice::from_ref(&located.diagnostic),
            ));
        }
        return CliOutcome::failure(1, String::new(), rendered);
    }
    let units = sources
        .into_iter()
        .zip(workspace.units)
        .map(|((path, source, _), result)| (path, source, result))
        .collect::<Vec<_>>();

    let records = match aggregate_records(&units) {
        Ok(records) => records,
        Err(diagnostics) => {
            let stderr = diagnostics
                .iter()
                .map(|diagnostic| format!("error[{}]: {}\n", diagnostic.code, diagnostic.message))
                .collect();
            return CliOutcome::failure(1, String::new(), stderr);
        }
    };
    let out_dir = arguments
        .out_dir
        .map_or(context.default_out_dir, PathBuf::from);
    let mut artifacts = Vec::new();
    for (_, _, result) in units {
        let module_name = result.analysis.hir.name.clone();
        if arguments.emit.contains(&EmitKind::Python) {
            let Some(generated) = result.python else {
                return CliOutcome::failure(
                    1,
                    String::new(),
                    format!("osr: compiler produced no Python output for `{module_name}`\n"),
                );
            };
            artifacts.push(Artifact::text(
                ArtifactKind::Python,
                python_module_path(&module_name),
                generated.source,
            ));
        }
        if arguments.emit.contains(&EmitKind::Interface) {
            let Some(interface) = result.interface else {
                return CliOutcome::failure(
                    1,
                    String::new(),
                    format!("osr: compiler produced no interface for `{module_name}`\n"),
                );
            };
            artifacts.push(Artifact::text(
                ArtifactKind::Interface,
                interface_artifact_path(&module_name),
                interface,
            ));
        }
        if arguments.emit.contains(&EmitKind::SourceMap) {
            let Some(source_map) = result.source_map else {
                return CliOutcome::failure(
                    1,
                    String::new(),
                    format!("osr: compiler produced no source map for `{module_name}`\n"),
                );
            };
            let mut contents = match serde_json::to_string_pretty(&source_map) {
                Ok(contents) => contents,
                Err(error) => {
                    return CliOutcome::failure(
                        1,
                        String::new(),
                        format!("osr: could not serialize source map: {error}\n"),
                    );
                }
            };
            contents.push('\n');
            artifacts.push(Artifact::text(
                ArtifactKind::SourceMap,
                source_map_artifact_path(&module_name),
                contents,
            ));
        }
    }
    let emit_records = arguments.emit.contains(&EmitKind::Records)
        || (!arguments.explicit_emit && !records.sidecar.records.is_empty());
    if emit_records {
        artifacts.push(Artifact {
            kind: ArtifactKind::Records,
            path: records_artifact_path(&context.options.distribution),
            contents: records.bytes,
        });
    }
    if let Err(error) = publish_artifacts(&out_dir, &artifacts) {
        return CliOutcome::failure(
            1,
            String::new(),
            format!(
                "osr: could not publish artifacts to '{}': {error}\n",
                out_dir.display()
            ),
        );
    }

    CliOutcome::success(format!("{}\n", out_dir.display()))
}

fn parse_compile_arguments(arguments: &[String]) -> Result<CompileArguments<'_>, String> {
    let mut paths = Vec::new();
    let mut out_dir = None;
    let mut site_roots = Vec::new();
    let mut emit = BTreeSet::from([EmitKind::Python, EmitKind::Interface, EmitKind::SourceMap]);
    let mut saw_emit = false;
    let mut index = 0;

    while let Some(argument) = arguments.get(index) {
        match argument.as_str() {
            "--out-dir" if out_dir.is_some() => {
                return Err("duplicate option '--out-dir' for 'compile'".to_owned());
            }
            "--out-dir" => {
                let Some(value) = arguments.get(index + 1) else {
                    return Err("missing value for '--out-dir'".to_owned());
                };
                out_dir = Some(value.as_str());
                index += 1;
            }
            "--emit" if saw_emit => {
                return Err("duplicate option '--emit' for 'compile'".to_owned());
            }
            "--emit" => {
                let Some(value) = arguments.get(index + 1) else {
                    return Err("missing value for '--emit'".to_owned());
                };
                emit.clear();
                for item in value.split(',') {
                    match item {
                        "py" => {
                            emit.insert(EmitKind::Python);
                        }
                        "osri" => {
                            emit.insert(EmitKind::Interface);
                        }
                        "map" => {
                            emit.insert(EmitKind::SourceMap);
                        }
                        "records" => {
                            emit.insert(EmitKind::Records);
                        }
                        "" => return Err("empty artifact name in '--emit'".to_owned()),
                        _ => {
                            return Err(format!(
                                "unsupported artifact '{item}' in '--emit'; expected 'py', 'osri', 'map', or 'records'"
                            ));
                        }
                    }
                }
                saw_emit = true;
                index += 1;
            }
            "--site-root" => {
                let Some(value) = arguments.get(index + 1) else {
                    return Err("missing value for '--site-root'".to_owned());
                };
                site_roots.push(value.as_str());
                index += 1;
            }
            option if option.starts_with('-') => {
                return Err(format!("unknown option '{option}' for 'compile'"));
            }
            positional => paths.push(positional),
        }
        index += 1;
    }

    if paths.is_empty() {
        return Err("missing FILE for 'compile'".to_owned());
    }
    Ok(CompileArguments {
        paths,
        out_dir,
        site_roots,
        emit,
        explicit_emit: saw_emit,
    })
}

fn aggregate_records(
    units: &[(String, String, compiler::CompileResult)],
) -> Result<records::EncodedSidecar, Vec<crate::diagnostic::Diagnostic>> {
    aggregate_result_records(units.iter().map(|(_, _, result)| result))
}

fn aggregate_result_records<'a>(
    results: impl IntoIterator<Item = &'a compiler::CompileResult>,
) -> Result<records::EncodedSidecar, Vec<crate::diagnostic::Diagnostic>> {
    let mut interface_hashes = Vec::new();
    let mut indexed = Vec::new();
    for result in results {
        let Some(sidecar) = &result.records else {
            return Err(vec![crate::diagnostic::Diagnostic::error(
                records::RECORD_SIDECAR,
                format!(
                    "module `{}` produced no records projection",
                    result.analysis.hir.name
                ),
                result.analysis.hir.span,
            )]);
        };
        interface_hashes.extend(sidecar.sidecar.interface_semantic_hashes.iter().cloned());
        indexed.extend(
            sidecar
                .sidecar
                .records
                .iter()
                .map(|entry| records::IndexedRecord {
                    occurrence: entry.occurrence.clone(),
                    record: entry.record.clone(),
                    dependency_path: vec![result.analysis.hir.name.clone()],
                }),
        );
    }
    records::merge_unique_indexes(indexed.clone())?;
    records::encode_sidecar(interface_hashes, indexed).map_err(|error| {
        vec![crate::diagnostic::Diagnostic::error(
            error.code,
            error.message,
            error.span.unwrap_or_else(|| Span::empty(0)),
        )]
    })
}

fn run_program(arguments: &[String]) -> CliOutcome {
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

struct RunArguments<'a> {
    path: &'a str,
    site_roots: Vec<&'a str>,
    program_arguments: &'a [String],
}

fn parse_run_arguments(arguments: &[String]) -> Result<RunArguments<'_>, String> {
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

struct CompileContext {
    options: CompileOptions,
    default_out_dir: PathBuf,
    project: Option<ProjectConfig>,
}

struct WorkspaceSource {
    path: PathBuf,
    source: String,
    options: CompileOptions,
}

struct WorkspaceSources {
    units: Vec<WorkspaceSource>,
    entry_index: usize,
}

impl WorkspaceSources {
    fn install_trust_policy(&mut self, policy: &crate::hir::ContractTrustPolicy) {
        for unit in &mut self.units {
            unit.options.trust_policy = policy.clone();
        }
    }
}

fn load_workspace_sources(
    entry_path: &Path,
    context: &CompileContext,
) -> Result<WorkspaceSources, String> {
    let Some(project) = &context.project else {
        let source = fs::read_to_string(entry_path)
            .map_err(|error| format!("could not read '{}': {error}", entry_path.display()))?;
        return Ok(WorkspaceSources {
            units: vec![WorkspaceSource {
                path: entry_path.to_path_buf(),
                source,
                options: context.options.clone(),
            }],
            entry_index: 0,
        });
    };

    let entry = fs::canonicalize(entry_path)
        .map_err(|error| format!("could not resolve '{}': {error}", entry_path.display()))?;
    let mut paths = Vec::new();
    for root in &project.source_roots {
        collect_osiris_sources(root, &mut paths)?;
    }
    paths.sort();
    paths.dedup();

    let mut units = Vec::with_capacity(paths.len());
    let mut entry_index = None;
    for path in paths {
        let canonical = fs::canonicalize(&path)
            .map_err(|error| format!("could not resolve '{}': {error}", path.display()))?;
        let module_name = project
            .module_name_for_source(&path)
            .map_err(|error| error.to_string())?;
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("could not read '{}': {error}", path.display()))?;
        if canonical == entry {
            entry_index = Some(units.len());
        }
        units.push(WorkspaceSource {
            options: CompileOptions::new(&module_name, project.target_python)
                .with_source_name(path.display().to_string())
                .with_expected_module_name(module_name)
                .with_provider(
                    project.distribution.clone(),
                    project.distribution_version.clone(),
                ),
            path,
            source,
        });
    }
    let entry_index = entry_index.ok_or_else(|| {
        format!(
            "entry source '{}' was not found under the configured source roots",
            entry_path.display()
        )
    })?;
    Ok(WorkspaceSources { units, entry_index })
}

fn collect_osiris_sources(directory: &Path, paths: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(directory).map_err(|error| {
        format!(
            "could not scan source root '{}': {error}",
            directory.display()
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "could not scan source root '{}': {error}",
                directory.display()
            )
        })?;
        let file_type = entry.file_type().map_err(|error| {
            format!(
                "could not inspect source '{}': {error}",
                entry.path().display()
            )
        })?;
        if file_type.is_dir() {
            collect_osiris_sources(&entry.path(), paths)?;
        } else if file_type.is_file()
            && entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                == Some("osr")
        {
            paths.push(entry.path());
        }
    }
    Ok(())
}

fn workspace_compile_inputs(sources: &WorkspaceSources) -> Vec<compiler::CompileInput<'_>> {
    sources
        .units
        .iter()
        .map(|unit| compiler::CompileInput::new(&unit.source, &unit.options))
        .collect()
}

fn render_workspace_diagnostics(
    sources: &WorkspaceSources,
    diagnostics: &[compiler::LocatedDiagnostic],
) -> String {
    let mut rendered = String::new();
    for located in diagnostics {
        let Some(unit) = sources.units.get(located.input_index) else {
            continue;
        };
        rendered.push_str(&diagnostic::render_all(
            &unit.path.display().to_string(),
            &unit.source,
            std::slice::from_ref(&located.diagnostic),
        ));
    }
    rendered
}

fn compile_context(source_path: &Path) -> Result<CompileContext, ConfigError> {
    let fallback_module_name = source_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("module");
    match ProjectConfig::discover(source_path) {
        Ok(config) => {
            let source_identity = if source_path.is_absolute() {
                source_path.to_path_buf()
            } else {
                std::env::current_dir()
                    .map_err(|error| ConfigError::Io(source_path.to_path_buf(), error))?
                    .join(source_path)
            };
            let module_name = config.module_name_for_source(&source_identity)?;
            Ok(CompileContext {
                options: CompileOptions::new(&module_name, config.target_python)
                    .with_source_name(source_path.display().to_string())
                    .with_expected_module_name(module_name)
                    .with_provider(
                        config.distribution.clone(),
                        config.distribution_version.clone(),
                    ),
                default_out_dir: config.root.join("target/osr"),
                project: Some(config),
            })
        }
        Err(ConfigError::NotFound(_)) => Ok(CompileContext {
            options: CompileOptions::new(fallback_module_name, PythonVersion::default())
                .with_source_name(source_path.display().to_string()),
            default_out_dir: source_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("target/osr"),
            project: None,
        }),
        Err(error) => Err(error),
    }
}

struct LoadedExternalInterfaces {
    interfaces: BTreeMap<String, interface::Interface>,
    trust_policy: crate::hir::ContractTrustPolicy,
    records_resolver: Vec<RuntimeRecordsResolverEntry>,
}

const RUNTIME_RECORDS_RESOLVER_FORMAT_VERSION: u32 = 1;

/// The run-time record lookup contract is deliberately a small, data-only
/// manifest.  Python extensions never get to choose a path or discover other
/// manifests; every entry was validated from the lock-selected wheel and its
/// `.osri` files before this value is serialized.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct RuntimeRecordsResolver {
    #[serde(rename = "format-version")]
    format_version: u32,
    entries: Vec<RuntimeRecordsResolverEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct RuntimeRecordsResolverEntry {
    distribution: String,
    version: String,
    #[serde(rename = "interface-member-id")]
    interface_member_id: String,
    #[serde(rename = "semantic-interface-hash")]
    semantic_interface_hash: String,
    #[serde(rename = "records-path")]
    records_path: String,
    #[serde(rename = "records-hash")]
    records_hash: String,
}

#[derive(Clone, Debug)]
struct ValidatedExternalRecords {
    path: PathBuf,
    hash: String,
    bytes: Vec<u8>,
    sidecar: records::RecordSidecar,
}

fn load_external_interfaces(
    context: &CompileContext,
    site_roots: &[&str],
) -> Result<LoadedExternalInterfaces, String> {
    let Some(project) = &context.project else {
        let trust_policy = dependency::contract_trust_policy(&[], &[])
            .map_err(|error| format!("could not construct contract trust policy: {error}"))?;
        return Ok(LoadedExternalInterfaces {
            interfaces: BTreeMap::new(),
            trust_policy,
            records_resolver: Vec::new(),
        });
    };
    if project.extensions.is_empty() {
        let trust_policy = dependency::contract_trust_policy(&project.trust_contracts, &[])
            .map_err(|error| format!("could not validate contract trust policy: {error}"))?;
        return Ok(LoadedExternalInterfaces {
            interfaces: BTreeMap::new(),
            trust_policy,
            records_resolver: Vec::new(),
        });
    }
    if site_roots.is_empty() {
        return Err(format!(
            "project enables static extensions ({}) but no installed-package --site-root was provided",
            project.extensions.join(", ")
        ));
    }
    let roots = site_roots.iter().map(PathBuf::from).collect::<Vec<_>>();
    let lock = project
        .load_lock()
        .map_err(|error| format!("could not validate uv.lock: {error}"))?;
    let graph = dependency::resolve_effective_extensions(project, &lock, &roots)
        .map_err(|error| format!("could not resolve static extensions: {error}"))?;
    let reachable_distributions = graph
        .reachable_distributions
        .iter()
        .map(|distribution| distribution.name.clone())
        .collect::<Vec<_>>();
    // `dependency::resolve_effective_extensions` retains only explicitly
    // enabled extension IDs.  A distribution-level records sidecar, however,
    // covers every interface in that wheel.  Discover the same lock-reachable
    // distributions once more so sidecar reconstruction includes disabled
    // (but still published) interfaces as well.
    let discovered =
        extension::discover_reachable(&roots, &project.extensions, &reachable_distributions)
            .map_err(|error| format!("could not discover static extension interfaces: {error}"))?;
    let all_distributions = discovered
        .distributions
        .into_iter()
        .map(|distribution| {
            (
                (
                    distribution.metadata.normalized_name.clone(),
                    distribution.metadata.version.clone(),
                ),
                distribution,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let trust_policy = graph.trust_policy.clone();
    let mut interfaces = BTreeMap::<String, interface::Interface>::new();
    let mut hashes = BTreeMap::<String, String>::new();
    let mut records_resolver = Vec::new();
    for distribution in graph.extensions {
        let external_records = validate_external_records(&distribution)?;
        let all_distribution = all_distributions
            .get(&(
                distribution.normalized_distribution.clone(),
                distribution.version.clone(),
            ))
            .ok_or_else(|| {
                format!(
                    "could not match discovered interfaces for distribution '{}' version '{}'",
                    distribution.distribution, distribution.version
                )
            })?;
        let all_distribution_interfaces = read_extension_interfaces(all_distribution)?;
        if external_records.is_none()
            && all_distribution_interfaces
                .iter()
                .any(|(_, model)| !model.owned_records.is_empty())
        {
            return Err(format!(
                "distribution '{}' publishes static records but has no records sidecar",
                distribution.distribution
            ));
        }
        let mut distribution_interfaces = Vec::with_capacity(distribution.extensions.len());
        for extension in &distribution.extensions {
            let text = fs::read_to_string(&extension.interface).map_err(|error| {
                format!(
                    "could not read extension interface '{}': {error}",
                    extension.interface.display()
                )
            })?;
            let model = interface::read(&text).map_err(|error| {
                format!(
                    "invalid extension interface '{}': {error}",
                    extension.interface.display()
                )
            })?;
            if model.module != extension.module
                || model.semantic_interface_hash() != extension.semantic_interface_hash
            {
                return Err(format!(
                    "extension interface '{}' changed after dependency validation (interface-member-id or semantic interface hash mismatch)",
                    extension.interface.display()
                ));
            }
            if !model.owned_records.is_empty() && external_records.is_none() {
                return Err(format!(
                    "extension interface '{}' publishes static records but distribution '{}' has no records sidecar",
                    extension.interface.display(),
                    distribution.distribution
                ));
            }
            if let Some(sidecar) = &external_records
                && !sidecar
                    .sidecar
                    .interface_semantic_hashes
                    .iter()
                    .any(|hash| hash == model.semantic_interface_hash())
            {
                return Err(format!(
                    "records sidecar for distribution '{}' does not name semantic interface hash '{}'",
                    distribution.distribution,
                    model.semantic_interface_hash()
                ));
            }
            distribution_interfaces.push((extension.clone(), model));
        }
        if let Some(sidecar) = &external_records {
            validate_external_records_against_interfaces(
                &distribution,
                &all_distribution_interfaces,
                sidecar,
            )?;
            let records_path = path_to_utf8(&sidecar.path, "records sidecar")?;
            for (extension, model) in &distribution_interfaces {
                records_resolver.push(RuntimeRecordsResolverEntry {
                    distribution: normalize_distribution_name(&distribution.distribution),
                    version: distribution.version.clone(),
                    interface_member_id: extension.module.clone(),
                    semantic_interface_hash: model.semantic_interface_hash().to_owned(),
                    records_path: records_path.clone(),
                    records_hash: sidecar.hash.clone(),
                });
            }
        }
        for (_extension, model) in distribution_interfaces {
            if let Some(previous) = hashes.get(&model.module) {
                if previous != model.semantic_interface_hash() {
                    return Err(format!(
                        "module '{}' resolves to multiple semantic interface hashes",
                        model.module
                    ));
                }
                continue;
            }
            hashes.insert(
                model.module.clone(),
                model.semantic_interface_hash().to_owned(),
            );
            interfaces.insert(model.module.clone(), model);
        }
    }
    records_resolver.sort_by(|left, right| {
        (
            &left.distribution,
            &left.version,
            &left.interface_member_id,
            &left.semantic_interface_hash,
        )
            .cmp(&(
                &right.distribution,
                &right.version,
                &right.interface_member_id,
                &right.semantic_interface_hash,
            ))
    });
    Ok(LoadedExternalInterfaces {
        interfaces,
        trust_policy,
        records_resolver,
    })
}

fn validate_external_records(
    distribution: &dependency::ResolvedExtensionDistribution,
) -> Result<Option<ValidatedExternalRecords>, String> {
    let marker_path = distribution.dist_info.join("osiris.toml");
    let marker = fs::read_to_string(&marker_path)
        .map_err(|error| {
            format!(
                "could not read extension marker '{}': {error}",
                marker_path.display()
            )
        })?
        .parse::<toml::Table>()
        .map_err(|error| {
            format!(
                "invalid extension marker '{}': {error}",
                marker_path.display()
            )
        })?;
    let path = marker.get("records").and_then(toml::Value::as_str);
    let hash = marker.get("records_hash").and_then(toml::Value::as_str);
    let (Some(path), Some(hash)) = (path, hash) else {
        return if path.is_none() && hash.is_none() {
            Ok(None)
        } else {
            Err(format!(
                "extension marker '{}' must declare records and records_hash together",
                marker_path.display()
            ))
        };
    };
    let relative = PathBuf::from(path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(format!(
            "records sidecar path '{}' escapes extension site root",
            path
        ));
    }
    let path = distribution.site_root.join(relative);
    let bytes = fs::read(&path).map_err(|error| {
        format!(
            "could not read records sidecar '{}': {error}",
            path.display()
        )
    })?;
    let sidecar = records::decode_sidecar(&bytes, Some(hash))
        .map_err(|error| format!("invalid records sidecar '{}': {error}", path.display()))?;
    Ok(Some(ValidatedExternalRecords {
        path,
        hash: hash.to_owned(),
        bytes,
        sidecar,
    }))
}

fn read_extension_interfaces(
    distribution: &extension::ExtensionDistribution,
) -> Result<Vec<(dependency::ResolvedExtension, interface::Interface)>, String> {
    let mut interfaces = Vec::with_capacity(distribution.extensions.len());
    for resource in &distribution.extensions {
        let text = fs::read_to_string(&resource.interface).map_err(|error| {
            format!(
                "could not read extension interface '{}': {error}",
                resource.interface.display()
            )
        })?;
        let model = interface::read(&text).map_err(|error| {
            format!(
                "invalid extension interface '{}': {error}",
                resource.interface.display()
            )
        })?;
        let semantic_interface_hash = model.semantic_interface_hash().to_owned();
        interfaces.push((
            dependency::ResolvedExtension {
                id: resource.id.clone(),
                interface: resource.interface.clone(),
                module: model.module.clone(),
                semantic_interface_hash,
            },
            model,
        ));
    }
    interfaces.sort_by(|left, right| left.0.id.cmp(&right.0.id));
    Ok(interfaces)
}

fn path_to_utf8(path: &Path, label: &str) -> Result<String, String> {
    let path = fs::canonicalize(path).map_err(|error| {
        format!(
            "could not resolve {label} path '{}': {error}",
            path.display()
        )
    })?;
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{label} path '{}' is not valid UTF-8", path.display()))
}

/// Reconstruct the distribution-owned sidecar from the parsed interfaces and
/// compare it byte-for-byte with the wheel marker.  This is intentionally
/// performed before a resolver entry is exposed to generated Python.
fn validate_external_records_against_interfaces(
    distribution: &dependency::ResolvedExtensionDistribution,
    interfaces: &[(dependency::ResolvedExtension, interface::Interface)],
    external: &ValidatedExternalRecords,
) -> Result<(), String> {
    let canonical_distribution = normalize_distribution_name(&distribution.distribution);
    let expected_hashes = interfaces
        .iter()
        .map(|(_, model)| model.semantic_interface_hash().to_owned())
        .collect::<Vec<_>>();
    let expected_records = interfaces
        .iter()
        .flat_map(|(_, model)| {
            let distribution_name = canonical_distribution.clone();
            let distribution_version = distribution.version.clone();
            let module = model.module.clone();
            let semantic_hash = model.semantic_interface_hash().to_owned();
            model
                .owned_records
                .iter()
                .map(move |record| records::IndexedRecord {
                    occurrence: record.occurrence(
                        distribution_name.clone(),
                        distribution_version.clone(),
                        module.clone(),
                        semantic_hash.clone(),
                    ),
                    record: record.clone(),
                    dependency_path: vec![module.clone()],
                })
        })
        .collect::<Vec<_>>();

    // Give callers a stable, actionable reason for the common identity drift
    // cases before the generic canonical-sidecar comparison below.
    let mut expected_by_identity = BTreeMap::new();
    for expected in &expected_records {
        expected_by_identity.insert(
            (
                expected.occurrence.stable_record_id.clone(),
                expected.occurrence.record_body_hash.clone(),
            ),
            expected,
        );
    }
    for actual in &external.sidecar.records {
        let key = (
            actual.occurrence.stable_record_id.clone(),
            actual.occurrence.record_body_hash.clone(),
        );
        let Some(expected) = expected_by_identity.get(&key) else {
            return Err(format!(
                "records resolver: record identity mismatch for distribution '{}' (stable-record-id '{}')",
                distribution.distribution, actual.occurrence.stable_record_id
            ));
        };
        let actual_occurrence = &actual.occurrence;
        let expected_occurrence = &expected.occurrence;
        if actual_occurrence.distribution != expected_occurrence.distribution {
            return Err(format!(
                "records resolver: distribution mismatch for record '{}': expected '{}', got '{}'",
                actual_occurrence.stable_record_id,
                expected_occurrence.distribution,
                actual_occurrence.distribution
            ));
        }
        if actual_occurrence.version != expected_occurrence.version {
            return Err(format!(
                "records resolver: version mismatch for distribution '{}' record '{}': expected '{}', got '{}'",
                distribution.distribution,
                actual_occurrence.stable_record_id,
                expected_occurrence.version,
                actual_occurrence.version
            ));
        }
        if actual_occurrence.interface_member_id != expected_occurrence.interface_member_id {
            return Err(format!(
                "records resolver: interface-member-id mismatch for distribution '{}' record '{}': expected '{}', got '{}'",
                distribution.distribution,
                actual_occurrence.stable_record_id,
                expected_occurrence.interface_member_id,
                actual_occurrence.interface_member_id
            ));
        }
        if actual_occurrence.semantic_interface_hash != expected_occurrence.semantic_interface_hash
        {
            return Err(format!(
                "records resolver: semantic interface hash mismatch for distribution '{}' member '{}': expected '{}', got '{}'",
                distribution.distribution,
                expected_occurrence.interface_member_id,
                expected_occurrence.semantic_interface_hash,
                actual_occurrence.semantic_interface_hash
            ));
        }
    }

    if external.sidecar.interface_semantic_hashes != {
        let mut expected = expected_hashes.clone();
        expected.sort();
        expected.dedup();
        expected
    } {
        let expected = expected_hashes
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let actual = external.sidecar.interface_semantic_hashes.join(", ");
        return Err(format!(
            "records resolver: semantic interface hash mismatch for distribution '{}': expected [{}], got [{}]",
            distribution.distribution,
            expected.join(", "),
            actual
        ));
    }

    records::verify_sidecar_against_records(
        &external.bytes,
        Some(&external.hash),
        &expected_hashes,
        &expected_records,
    )
    .map_err(|error| {
        format!(
            "records resolver: sidecar for distribution '{}' does not match its interfaces: {}",
            distribution.distribution, error.message
        )
    })
}

fn build_runtime_records_resolver(
    context: &CompileContext,
    external_entries: &[RuntimeRecordsResolverEntry],
    project_records_path: &Path,
    project_records: &records::EncodedSidecar,
    workspace: &compiler::WorkspaceCompileResult,
) -> Result<RuntimeRecordsResolver, String> {
    let project_records_path = path_to_utf8(project_records_path, "project records")?;
    let mut entries = external_entries.to_vec();
    let distribution = normalize_distribution_name(&context.options.distribution);
    for unit in &workspace.units {
        let Some(interface_text) = &unit.interface else {
            return Err(format!(
                "records resolver: compiler produced no interface for '{}'",
                unit.analysis.hir.name
            ));
        };
        let model = interface::read(interface_text).map_err(|error| {
            format!(
                "records resolver: invalid local interface '{}': {error}",
                unit.analysis.hir.name
            )
        })?;
        if model.module != unit.analysis.hir.name {
            return Err(format!(
                "records resolver: local interface member mismatch: expected '{}', got '{}'",
                unit.analysis.hir.name, model.module
            ));
        }
        if model.semantic_interface_hash().is_empty() {
            return Err(format!(
                "records resolver: local interface '{}' has no semantic interface hash",
                model.module
            ));
        }
        let semantic_interface_hash = model.semantic_interface_hash().to_owned();
        entries.push(RuntimeRecordsResolverEntry {
            distribution: distribution.clone(),
            version: context.options.distribution_version.clone(),
            interface_member_id: model.module,
            semantic_interface_hash,
            records_path: project_records_path.clone(),
            records_hash: project_records.records_hash.clone(),
        });
    }
    entries.sort_by(|left, right| {
        (
            &left.distribution,
            &left.version,
            &left.interface_member_id,
            &left.semantic_interface_hash,
            &left.records_path,
            &left.records_hash,
        )
            .cmp(&(
                &right.distribution,
                &right.version,
                &right.interface_member_id,
                &right.semantic_interface_hash,
                &right.records_path,
                &right.records_hash,
            ))
    });
    for pair in entries.windows(2) {
        if pair[0].distribution == pair[1].distribution
            && pair[0].version == pair[1].version
            && pair[0].interface_member_id == pair[1].interface_member_id
        {
            return Err(format!(
                "records resolver: duplicate interface-member-id '{}' for distribution '{}' version '{}'",
                pair[0].interface_member_id, pair[0].distribution, pair[0].version
            ));
        }
    }
    Ok(RuntimeRecordsResolver {
        format_version: RUNTIME_RECORDS_RESOLVER_FORMAT_VERSION,
        entries,
    })
}

fn source_map_artifact_path(module_name: &str) -> PathBuf {
    PathBuf::from(format!("{}.map", python_module_path(module_name).display()))
}

fn interface_artifact_path(module_name: &str) -> PathBuf {
    let mut path = python_module_path(module_name);
    path.set_extension("osri");
    path
}

fn records_artifact_path(distribution: &str) -> PathBuf {
    let normalized = normalize_distribution_name(distribution);
    let name = if normalized.is_empty() {
        "osiris-project"
    } else {
        &normalized
    };
    PathBuf::from(format!("{name}.records.json"))
}

fn config_error(error: &ConfigError) -> CliOutcome {
    CliOutcome::failure(1, String::new(), format!("osr: {error}\n"))
}

fn run_expand(arguments: &[String]) -> CliOutcome {
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

fn run_inspect(arguments: &[String]) -> CliOutcome {
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

fn run_semantic_inspect(path: &str, format: InspectFormat) -> CliOutcome {
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

fn render_semantic_text(document: &SemanticDocument) -> String {
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

fn parse_inspect_arguments(
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

fn read_source(path: &str) -> io::Result<(String, crate::syntax::Document)> {
    let source = fs::read_to_string(path)?;
    let document = reader::read(&source);
    Ok((source, document))
}

fn io_error(path: &str, error: &io::Error) -> CliOutcome {
    CliOutcome::failure(
        1,
        String::new(),
        format!("osr: could not read '{path}': {error}\n"),
    )
}

#[cfg(test)]
mod tests {
    use super::{InspectFormat, InspectView, parse_inspect_arguments, run_cli};

    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn bare_source_path_remains_an_error() {
        let outcome = run_cli(&arguments(&["source.osr"]));
        assert_eq!(outcome.exit_code, 2);
        assert!(outcome.stderr.contains("unexpected arguments"));
    }

    #[test]
    fn inspect_accepts_syntax_and_format_in_any_order() {
        let arguments = arguments(&["--format", "json", "--syntax", "source.osr"]);
        assert_eq!(
            parse_inspect_arguments(&arguments),
            Ok(("source.osr", InspectFormat::Json, InspectView::Syntax))
        );
    }

    #[test]
    fn inspect_rejects_invalid_format() {
        let arguments = arguments(&["source.osr", "--format", "yaml"]);
        assert!(parse_inspect_arguments(&arguments).is_err());
    }
}
