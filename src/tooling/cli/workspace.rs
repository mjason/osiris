use super::*;

pub(super) fn config_error(error: &ConfigError) -> CliOutcome {
    CliOutcome::failure(1, String::new(), format!("osr: {error}\n"))
}

pub(super) struct CompileContext {
    pub(super) options: CompileOptions,
    pub(super) default_out_dir: PathBuf,
    pub(super) project: Option<ProjectConfig>,
}

pub(super) struct WorkspaceSource {
    pub(super) path: PathBuf,
    pub(super) source: String,
    pub(super) options: CompileOptions,
}

pub(super) struct WorkspaceSources {
    pub(super) units: Vec<WorkspaceSource>,
    pub(super) entry_index: usize,
}

impl WorkspaceSources {
    pub(super) fn install_trust_policy(&mut self, policy: &crate::hir::ContractTrustPolicy) {
        for unit in &mut self.units {
            unit.options.trust_policy = policy.clone();
        }
    }
}

pub(super) fn load_workspace_sources(
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
        collect_osiris_sources(root, project, &mut paths)?;
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
        let embedded_sources = read_embedded_sources(project, &path, &source)?;
        units.push(WorkspaceSource {
            options: CompileOptions::new(&module_name, project.target_python)
                .with_strict(project.strict)
                .with_source_name(path.display().to_string())
                .with_expected_module_name(module_name)
                .with_embedded_sources(embedded_sources)
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

pub(super) fn collect_osiris_sources(
    directory: &Path,
    project: &ProjectConfig,
    paths: &mut Vec<PathBuf>,
) -> Result<(), String> {
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
        if project.is_excluded(&entry.path()) {
            continue;
        }
        let file_type = entry.file_type().map_err(|error| {
            format!(
                "could not inspect source '{}': {error}",
                entry.path().display()
            )
        })?;
        if file_type.is_dir() {
            collect_osiris_sources(&entry.path(), project, paths)?;
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

pub(super) fn first_project_source(project: &ProjectConfig) -> Result<PathBuf, String> {
    let mut paths = Vec::new();
    for root in &project.source_roots {
        collect_osiris_sources(root, project, &mut paths)?;
    }
    paths.sort();
    paths
        .into_iter()
        .next()
        .ok_or_else(|| "project has no Osiris sources".to_owned())
}

pub(super) fn workspace_compile_inputs(
    sources: &WorkspaceSources,
) -> Vec<compiler::CompileInput<'_>> {
    sources
        .units
        .iter()
        .map(|unit| compiler::CompileInput::new(&unit.source, &unit.options))
        .collect()
}

pub(super) fn render_workspace_diagnostics(
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

pub(super) fn compile_context(source_path: &Path) -> Result<CompileContext, ConfigError> {
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
                    .with_strict(config.strict)
                    .with_source_name(source_path.display().to_string())
                    .with_expected_module_name(module_name)
                    .with_provider(
                        config.distribution.clone(),
                        config.distribution_version.clone(),
                    ),
                default_out_dir: config.default_output_dir(),
                project: Some(config),
            })
        }
        Err(ConfigError::NotFound(_)) => Ok(CompileContext {
            options: CompileOptions::new(fallback_module_name, PythonVersion::default())
                .with_source_name(source_path.display().to_string()),
            default_out_dir: source_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("dist"),
            project: None,
        }),
        Err(error) => Err(error),
    }
}

/// Resolve every `py/embed` reference a source names.
///
/// Compilation is a function of source text and options, so this is where the
/// filesystem is touched (OEP-0001-R006CC). The checks that need the disk live
/// here too: the file must exist, must resolve inside a source root, and must not
/// be a symlink (OEP-0001-R006CB). A path that fails one of those is reported
/// here rather than reaching the compiler as an absent entry.
fn read_embedded_sources(
    project: &ProjectConfig,
    source_path: &Path,
    source: &str,
) -> Result<BTreeMap<String, String>, String> {
    let document = crate::reader::read(source);
    let lowered = crate::ast::lower_document(&document);
    let mut resolved = BTreeMap::new();
    for item in &lowered.module.items {
        let crate::ast::ItemKind::EmbeddedPython(provider) = &item.kind else {
            continue;
        };
        let Some(relative) = provider.source_path.as_deref() else {
            continue;
        };
        if resolved.contains_key(relative) {
            continue;
        }
        let owner = source_path.parent().unwrap_or(Path::new("."));
        let candidate = owner.join(relative);
        let metadata = fs::symlink_metadata(&candidate).map_err(|error| {
            format!(
                "'{}' references embedded provider source '{relative}', which could not be read: {error}",
                source_path.display()
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "embedded provider source '{relative}' must not be a symlink"
            ));
        }
        let canonical = fs::canonicalize(&candidate)
            .map_err(|error| format!("could not resolve '{relative}': {error}"))?;
        let inside_a_root = project.source_roots.iter().any(|root| {
            fs::canonicalize(root).is_ok_and(|root| canonical.starts_with(root))
        });
        if !inside_a_root {
            return Err(format!(
                "embedded provider source '{relative}' resolves outside every configured source root"
            ));
        }
        let content = fs::read_to_string(&canonical)
            .map_err(|error| format!("could not read '{relative}': {error}"))?;
        resolved.insert(relative.to_owned(), content);
    }
    Ok(resolved)
}
