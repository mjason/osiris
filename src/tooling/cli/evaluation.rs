use super::*;

pub(crate) struct EvaluationWorkspace {
    pub(crate) result: compiler::WorkspaceCompileResult,
    pub(crate) entry_index: usize,
    pub(crate) target_python: PythonVersion,
    context: CompileContext,
    external_records_resolver: Vec<RuntimeRecordsResolverEntry>,
}

pub(crate) struct EvaluationRecords {
    pub(crate) records_path: PathBuf,
    pub(crate) resolver_path: PathBuf,
}

pub(crate) fn compile_evaluation_workspace(
    source: &str,
    project: Option<&ProjectConfig>,
) -> Result<EvaluationWorkspace, Vec<String>> {
    let target_python = project.map_or_else(PythonVersion::default, |value| value.target_python);
    let mut options = CompileOptions::new("lsa.example", target_python)
        .with_source_name("osiris-lsa:///example.osr");
    if let Some(project) = project {
        options = options.with_strict(project.strict).with_provider(
            project.distribution.clone(),
            project.distribution_version.clone(),
        );
    }

    let mut sources = match project {
        Some(project) => load_reachable_project_sources(project, source)?,
        None => WorkspaceSources {
            units: Vec::new(),
            entry_index: 0,
        },
    };
    let entry_index = sources.units.len();
    sources.units.push(WorkspaceSource {
        path: PathBuf::from("osiris-lsa:///example.osr"),
        source: source.to_owned(),
        options: options.clone(),
    });
    sources.entry_index = entry_index;

    let context = CompileContext {
        options,
        default_out_dir: project
            .map_or_else(|| PathBuf::from("dist"), ProjectConfig::default_output_dir),
        project: project.cloned(),
    };
    let loaded = load_external_interfaces(&context, &[]).map_err(|error| vec![error])?;
    sources.install_trust_policy(&loaded.trust_policy);
    let inputs = workspace_compile_inputs(&sources);
    let result = compiler::compile_workspace(&inputs, &loaded.interfaces);
    if result.has_errors() {
        return Err(result
            .diagnostics
            .iter()
            .map(|located| {
                let source = sources
                    .units
                    .get(located.input_index)
                    .map_or("<unknown source>", |unit| unit.options.source_name.as_str());
                format!(
                    "{source}: {}: {}",
                    located.diagnostic.code, located.diagnostic.message,
                )
            })
            .collect());
    }
    Ok(EvaluationWorkspace {
        result,
        entry_index,
        target_python,
        context,
        external_records_resolver: loaded.records_resolver,
    })
}

pub(crate) fn stage_evaluation_records(
    workspace: &EvaluationWorkspace,
    directory: &Path,
) -> Result<EvaluationRecords, String> {
    let records = aggregate_result_records(&workspace.result.units).map_err(|diagnostics| {
        diagnostics
            .into_iter()
            .map(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message))
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    let records_path = directory.join(records_artifact_path(
        &workspace.context.options.distribution,
    ));
    fs::write(&records_path, &records.bytes)
        .map_err(|error| format!("could not stage evaluation records: {error}"))?;
    let resolver = build_runtime_records_resolver(
        &workspace.context,
        &workspace.external_records_resolver,
        &records_path,
        &records,
        &workspace.result,
    )?;
    let resolver_path = directory.join("osiris.records-resolver.json");
    let resolver = serde_json::to_vec(&resolver)
        .map_err(|error| format!("could not serialize evaluation records resolver: {error}"))?;
    fs::write(&resolver_path, resolver)
        .map_err(|error| format!("could not stage evaluation records resolver: {error}"))?;
    Ok(EvaluationRecords {
        records_path,
        resolver_path,
    })
}

fn load_reachable_project_sources(
    project: &ProjectConfig,
    entry_source: &str,
) -> Result<WorkspaceSources, Vec<String>> {
    let mut paths = Vec::new();
    for root in &project.source_roots {
        collect_osiris_sources(root, project, &mut paths).map_err(|error| vec![error])?;
    }
    paths.sort();
    paths.dedup();

    let mut source_paths = BTreeMap::<String, Vec<PathBuf>>::new();
    for path in paths {
        let module_name = project
            .module_name_for_source(&path)
            .map_err(|error| vec![error.to_string()])?;
        source_paths.entry(module_name).or_default().push(path);
    }

    let mut pending = imported_modules(entry_source)
        .into_iter()
        .collect::<Vec<_>>();
    let mut units = BTreeMap::<String, WorkspaceSource>::new();
    while let Some(module_name) = pending.pop() {
        if units.contains_key(&module_name) {
            continue;
        }
        let Some(paths) = source_paths.get(&module_name) else {
            continue;
        };
        if paths.len() > 1 {
            return Err(vec![format!(
                "project module `{module_name}` has duplicate sources '{}' and '{}'",
                paths[0].display(),
                paths[1].display()
            )]);
        }
        let path = &paths[0];
        let source = fs::read_to_string(path)
            .map_err(|error| vec![format!("could not read '{}': {error}", path.display())])?;
        pending.extend(
            imported_modules(&source)
                .into_iter()
                .filter(|imported| !units.contains_key(imported)),
        );
        units.insert(
            module_name.clone(),
            WorkspaceSource {
                options: CompileOptions::new(&module_name, project.target_python)
                    .with_strict(project.strict)
                    .with_source_name(path.display().to_string())
                    .with_expected_module_name(module_name)
                    .with_provider(
                        project.distribution.clone(),
                        project.distribution_version.clone(),
                    ),
                path: path.clone(),
                source,
            },
        );
    }
    Ok(WorkspaceSources {
        units: units.into_values().collect(),
        entry_index: 0,
    })
}

fn imported_modules(source: &str) -> BTreeSet<String> {
    let lowered = crate::ast::lower_document(&crate::reader::read(source));
    lowered
        .module
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            crate::ast::ItemKind::Import(import)
            | crate::ast::ItemKind::ImportForSyntax(import) => {
                Some(import.module.canonical.clone())
            }
            _ => None,
        })
        .collect()
}
