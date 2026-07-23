pub(super) fn build_runtime_records_resolver(
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

pub(super) fn source_map_artifact_path(module_name: &str) -> PathBuf {
    PathBuf::from(format!("{}.map", python_module_path(module_name).display()))
}

pub(super) fn interface_artifact_path(module_name: &str) -> PathBuf {
    let mut path = python_module_path(module_name);
    path.set_extension("osri");
    path
}

pub(super) fn records_artifact_path(distribution: &str) -> PathBuf {
    let normalized = normalize_distribution_name(distribution);
    let name = if normalized.is_empty() {
        "osiris-project"
    } else {
        &normalized
    };
    PathBuf::from(format!("{name}.records.json"))
}
