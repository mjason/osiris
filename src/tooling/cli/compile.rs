use super::*;

use sha2::{Digest, Sha256};

#[derive(Default)]
struct LinkedSupport {
    helpers: BTreeSet<String>,
    binding_ids: BTreeSet<String>,
    source_maps: BTreeSet<SourceMapIdentity>,
    embedded_modules: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceMapIdentity {
    source: String,
    source_hash: String,
    generated: String,
    build_hash: String,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum EmitKind {
    Python,
    Interface,
    SourceMap,
    Records,
}

pub(super) struct CompileArguments<'a> {
    pub(super) paths: Vec<&'a str>,
    pub(super) out_dir: Option<&'a str>,
    pub(super) site_roots: Vec<&'a str>,
    pub(super) emit: BTreeSet<EmitKind>,
    pub(super) explicit_emit: bool,
    /// OEP-0002-R017C: `osr build`/`osr run` share one runtime tree at the
    /// output root; bare `osr compile` keeps the per-package layout wheels
    /// need, which is why the flag rather than a default covers both.
    pub(super) runtime_layout: compiler::RuntimeLayout,
}

pub(super) fn run_compile(arguments: &[String]) -> CliOutcome {
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
            let source = match surface_to_canonical(Path::new(path), source) {
                Ok(source) => source,
                Err(message) => {
                    return CliOutcome::failure(1, String::new(), format!("osr: {message}\n"));
                }
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
        options.runtime_layout = arguments.runtime_layout;
    }
    let out_dir = arguments
        .out_dir
        .map_or_else(|| context.default_out_dir.clone(), PathBuf::from);
    // The whole-directory replacement in `publish_artifacts` is only correct
    // when every file in the output directory is a build product. With
    // `outDir: "."` the output directory is the project root — sources,
    // configuration and all — so publication switches to the in-place mode,
    // and the whole-directory freshness check (which would count authored
    // files as stale artifacts) does not apply.
    let publish_in_place = context
        .project
        .as_ref()
        .is_some_and(|project| project.output_dir == project.root)
        || fs::canonicalize(&out_dir).ok().is_some_and(|out| {
            context
                .project
                .as_ref()
                .and_then(|project| fs::canonicalize(&project.root).ok())
                .is_some_and(|root| out == root)
        });
    let publish = |out_dir: &Path, artifacts: &[Artifact]| {
        if publish_in_place {
            crate::artifact::publish_artifacts_in_place(out_dir, artifacts)
        } else {
            publish_artifacts(out_dir, artifacts)
        }
    };
    let workspace_cache = workspace_cache(&context, &sources, &loaded, &arguments);
    if let Some((cache, key)) = &workspace_cache
        && let Some(artifacts) = cache.load(key)
    {
        if (publish_in_place || !crate::cache::output_matches(&out_dir, &artifacts))
            && let Err(error) = publish(&out_dir, &artifacts)
        {
            return CliOutcome::failure(
                1,
                String::new(),
                format!(
                    "osr: could not restore cached artifacts to '{}': {error}\n",
                    out_dir.display()
                ),
            );
        }
        return CliOutcome::success(format!("{}\n", out_dir.display()));
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
    let mut artifacts = Vec::new();
    let mut runtime_packages = BTreeMap::<String, LinkedSupport>::new();
    for (_, _, result) in units {
        let module_name = result.analysis.hir.name.clone();
        // A compile-time-only module leaves no runtime shell in a project
        // build (shared layout): its macros are in the `.osri`, and an empty
        // `.py` next to authored code reads as code when it is noise. Wheels
        // keep the shell — their extension entries reference these files.
        let phase_only_skip = arguments.runtime_layout == compiler::RuntimeLayout::Shared
            && result
                .python
                .as_ref()
                .is_some_and(crate::backend::GeneratedPython::is_phase_only);
        if arguments.emit.contains(&EmitKind::Python) {
            for embedded in &result.analysis.embedded_python {
                let package = embedded
                    .logical_module
                    .split_once(".packages.")
                    .map_or("__osiris_runtime__", |(package, _)| package)
                    .to_owned();
                let previous = runtime_packages
                    .entry(package)
                    .or_default()
                    .embedded_modules
                    .insert(embedded.logical_module.clone(), embedded.source.clone());
                if previous
                    .as_ref()
                    .is_some_and(|source| source != &embedded.source)
                {
                    return CliOutcome::failure(
                        1,
                        String::new(),
                        format!(
                            "osr: conflicting embedded Python module `{}`\n",
                            embedded.logical_module
                        ),
                    );
                }
            }
            let Some(generated) = result.python else {
                return CliOutcome::failure(
                    1,
                    String::new(),
                    format!("osr: compiler produced no Python output for `{module_name}`\n"),
                );
            };
            if let Some(runtime) = generated.runtime_support.as_ref() {
                let support = runtime_packages.entry(runtime.package.clone()).or_default();
                support.helpers.extend(runtime.helpers.iter().cloned());
                support
                    .binding_ids
                    .extend(runtime.binding_ids.iter().cloned());
                // A provider's runtime module linked into this build: the
                // generated import names the relocated path, so the file must
                // exist there (OEP-0002-R033G). It rides the embedded-module
                // channel, whose same-path conflict detection applies.
                for (relocated, original) in &runtime.external_modules {
                    let source = match read_external_runtime_module(
                        original,
                        &arguments.site_roots,
                        context.project.as_ref(),
                    ) {
                        Ok(source) => source,
                        Err(message) => {
                            return CliOutcome::failure(
                                1,
                                String::new(),
                                format!("osr: {message}\n"),
                            );
                        }
                    };
                    let previous = support
                        .embedded_modules
                        .insert(relocated.clone(), source.clone());
                    if previous
                        .as_ref()
                        .is_some_and(|existing| existing != &source)
                    {
                        return CliOutcome::failure(
                            1,
                            String::new(),
                            format!("osr: conflicting linked runtime module `{relocated}`\n"),
                        );
                    }
                }
                if let Some(source_map) = result.source_map.as_ref() {
                    support.source_maps.insert(SourceMapIdentity {
                        source: source_map.source.clone(),
                        source_hash: source_map.source_hash.clone(),
                        generated: source_map.generated.clone(),
                        build_hash: source_map.build_hash.clone(),
                    });
                }
            }
            if !phase_only_skip {
                artifacts.push(Artifact::text(
                    ArtifactKind::Python,
                    python_module_path(&module_name),
                    generated.source,
                ));
            }
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
            if !phase_only_skip {
                artifacts.push(Artifact::text(
                    ArtifactKind::SourceMap,
                    source_map_artifact_path(&module_name),
                    contents,
                ));
            }
        }
    }
    for (package, support) in runtime_packages {
        let linked_standard = match crate::stdlib::linked_standard_support(
            &package,
            &support.binding_ids,
            context.options.target_python,
        ) {
            Ok(linked) => linked,
            Err(message) => {
                return CliOutcome::failure(
                    1,
                    String::new(),
                    format!("osr: could not link standard source: {message}\n"),
                );
            }
        };
        let mut helpers = support.helpers;
        helpers.extend(linked_standard.helpers);
        let mut binding_ids = support.binding_ids;
        binding_ids.extend(linked_standard.binding_ids);
        let mut source_maps = support.source_maps;
        source_maps.extend(linked_standard.source_maps.iter().map(|source_map| {
            SourceMapIdentity {
                source: source_map.source.clone(),
                source_hash: source_map.source_hash.clone(),
                generated: source_map.generated.clone(),
                build_hash: source_map.build_hash.clone(),
            }
        }));
        let mut support_files = crate::backend::runtime_support_files(&package, &helpers);
        support_files.extend(linked_standard.files);
        support_files.extend(
            support
                .embedded_modules
                .into_iter()
                .map(|(module, source)| (compiler::python_module_path(&module), source)),
        );
        let file_hashes = support_files
            .iter()
            .filter(|(path, _)| path.extension().and_then(|value| value.to_str()) == Some("py"))
            .map(|(path, source)| {
                let digest = Sha256::digest(source.as_bytes());
                (
                    path.to_string_lossy().replace('\\', "/"),
                    format!("sha256:{digest:x}"),
                )
            })
            .collect::<BTreeMap<_, _>>();
        for (path, source) in support_files {
            artifacts.push(Artifact::text(ArtifactKind::RuntimeSupport, path, source));
        }
        let manifest = serde_json::json!({
            "schema": "osiris-linked-support/v1",
            "languageVersion": crate::LANGUAGE_VERSION,
            "pythonTarget": context.options.target_python.to_string(),
            "standardLibraryAbi": crate::STANDARD_LIBRARY_ABI,
            "standardLibrarySemanticHash": crate::stdlib::semantic_hash(),
            "helperFormat": crate::LINKABLE_HELPER_FORMAT,
            "reachableBindingIds": binding_ids,
            "helperHashes": crate::backend::runtime_helper_hashes(&helpers),
            "fileHashes": file_hashes,
            "sourceMaps": source_maps,
        });
        let mut manifest = serde_json::to_string_pretty(&manifest)
            .expect("linked support manifest is serializable");
        manifest.push('\n');
        artifacts.push(Artifact::text(
            ArtifactKind::RuntimeSupport,
            PathBuf::from(package.replace('.', "/")).join("manifest.json"),
            manifest,
        ));
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
    if let Err(error) = publish(&out_dir, &artifacts) {
        return CliOutcome::failure(
            1,
            String::new(),
            format!(
                "osr: could not publish artifacts to '{}': {error}\n",
                out_dir.display()
            ),
        );
    }
    if let Some((cache, key)) = workspace_cache {
        let _ = cache.store(&key, &artifacts);
    }

    CliOutcome::success(format!("{}\n", out_dir.display()))
}

pub(super) fn parse_compile_arguments(
    arguments: &[String],
) -> Result<CompileArguments<'_>, String> {
    let mut paths = Vec::new();
    let mut out_dir = None;
    let mut site_roots = Vec::new();
    let mut emit = BTreeSet::from([EmitKind::Python, EmitKind::Interface, EmitKind::SourceMap]);
    let mut saw_emit = false;
    let mut runtime_layout = compiler::RuntimeLayout::PerPackage;
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
            "--runtime-layout" => {
                let Some(value) = arguments.get(index + 1) else {
                    return Err("missing value for '--runtime-layout'".to_owned());
                };
                runtime_layout = match value.as_str() {
                    "shared" => compiler::RuntimeLayout::Shared,
                    "package" => compiler::RuntimeLayout::PerPackage,
                    other => {
                        return Err(format!(
                            "unsupported '--runtime-layout {other}'; expected 'shared' or 'package'"
                        ));
                    }
                };
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
        runtime_layout,
    })
}

pub(super) fn aggregate_records(
    units: &[(String, String, compiler::CompileResult)],
) -> Result<records::EncodedSidecar, Vec<crate::diagnostic::Diagnostic>> {
    aggregate_result_records(units.iter().map(|(_, _, result)| result))
}

pub(super) fn aggregate_result_records<'a>(
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

/// Reads the provider file behind an externally linked runtime module.
///
/// Extension discovery already validated these files against the provider's
/// linked-support manifest hashes, so this is a lookup, not a trust decision.
/// Not finding the file is an error: the generated import names the relocated
/// copy, and silently skipping it would ship an import with no module behind
/// it.
pub(super) fn read_external_runtime_module(
    module: &str,
    site_roots: &[&str],
    project: Option<&ProjectConfig>,
) -> Result<String, String> {
    let relative = compiler::python_module_path(module);
    let mut roots = site_roots.iter().map(PathBuf::from).collect::<Vec<_>>();
    if let Some(project) = project {
        roots.extend(project.installed_package_roots());
    }
    for root in roots {
        let candidate = root.join(&relative);
        if candidate.is_file() {
            return fs::read_to_string(&candidate).map_err(|error| {
                format!(
                    "could not read linked runtime module '{}': {error}",
                    candidate.display()
                )
            });
        }
    }
    Err(format!(
        "linked runtime module `{module}` was not found in the installed environment"
    ))
}
