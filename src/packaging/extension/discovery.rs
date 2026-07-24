/// Discovers and validates explicitly enabled extensions in site-package roots.
pub fn discover(
    site_roots: &[PathBuf],
    enabled: &[String],
) -> Result<ExtensionGraph, ExtensionError> {
    discover_filtered(site_roots, Some(enabled), None)
}

/// Discovers enabled extensions only from lock-reachable distributions. The
/// allowlist is applied to `.dist-info` names before marker contents are read,
/// so an unrelated installed wheel cannot affect compilation.
pub fn discover_reachable(
    site_roots: &[PathBuf],
    enabled: &[String],
    reachable_distributions: &[String],
) -> Result<ExtensionGraph, ExtensionError> {
    let reachable = reachable_distributions
        .iter()
        .map(|name| normalize_distribution_name(name))
        .collect::<BTreeSet<_>>();
    discover_filtered(site_roots, Some(enabled), Some(&reachable))
}

/// Discovers every extension marker published by a lock-reachable
/// distribution. Installed packages outside the effective dependency graph
/// are ignored.
pub fn discover_reachable_all(
    site_roots: &[PathBuf],
    reachable_distributions: &[String],
) -> Result<ExtensionGraph, ExtensionError> {
    let reachable = reachable_distributions
        .iter()
        .map(|name| normalize_distribution_name(name))
        .collect::<BTreeSet<_>>();
    discover_filtered(site_roots, None, Some(&reachable))
}

fn discover_filtered(
    site_roots: &[PathBuf],
    enabled: Option<&[String]>,
    reachable_distributions: Option<&BTreeSet<String>>,
) -> Result<ExtensionGraph, ExtensionError> {
    let enabled = enabled.map(|values| values.iter().cloned().collect::<BTreeSet<_>>());
    if enabled.as_ref().is_some_and(BTreeSet::is_empty) {
        return Ok(ExtensionGraph::default());
    }
    let mut candidates = Vec::new();
    for root in site_roots {
        let entries =
            fs::read_dir(root).map_err(|error| ExtensionError::Io(root.clone(), error))?;
        for entry in entries {
            let entry = entry.map_err(|error| ExtensionError::Io(root.clone(), error))?;
            let path = entry.path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".dist-info"))
                && path.is_dir()
                && path.join("osiris.toml").is_file()
            {
                if reachable_distributions.is_some_and(|reachable| {
                    dist_info_distribution_name(&path).is_none_or(|name| !reachable.contains(&name))
                }) {
                    continue;
                }
                candidates.push((root.clone(), path));
            }
        }
    }
    candidates.sort();

    let mut distributions = Vec::new();
    for (site_root, dist_info) in candidates {
        let distribution = load_distribution(&site_root, &dist_info)?;
        if enabled.as_ref().is_none_or(|enabled| {
            distribution
                .extensions
                .iter()
                .any(|extension| enabled.contains(&extension.id))
        }) {
            distributions.push(distribution);
        }
    }
    distributions.sort_by(|left, right| {
        (&left.metadata.normalized_name, &left.metadata.version)
            .cmp(&(&right.metadata.normalized_name, &right.metadata.version))
    });

    let mut by_id = BTreeMap::new();
    for (distribution_index, distribution) in distributions.iter().enumerate() {
        for (extension_index, extension) in distribution.extensions.iter().enumerate() {
            if enabled
                .as_ref()
                .is_some_and(|enabled| !enabled.contains(&extension.id))
            {
                continue;
            }
            if let Some((first_distribution, _)) =
                by_id.insert(extension.id.clone(), (distribution_index, extension_index))
            {
                return Err(ExtensionError::DuplicateId {
                    id: extension.id.clone(),
                    first: distributions[first_distribution].metadata.name.clone(),
                    second: distribution.metadata.name.clone(),
                });
            }
        }
    }
    if let Some(enabled) = enabled {
        for id in enabled {
            if !by_id.contains_key(&id) {
                return Err(ExtensionError::MissingEnabled(id));
            }
        }
    }

    Ok(ExtensionGraph {
        distributions,
        by_id,
    })
}

fn dist_info_distribution_name(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?.strip_suffix(".dist-info")?;
    let (distribution, _) = name.rsplit_once('-')?;
    Some(normalize_distribution_name(distribution))
}

fn load_distribution(
    site_root: &Path,
    dist_info: &Path,
) -> Result<ExtensionDistribution, ExtensionError> {
    let marker_path = dist_info.join("osiris.toml");
    let marker_text = fs::read_to_string(&marker_path)
        .map_err(|error| ExtensionError::Io(marker_path.clone(), error))?;
    let marker: RawMarker = toml::from_str(&marker_text)
        .map_err(|error| ExtensionError::InvalidMarker(marker_path.clone(), error.to_string()))?;
    validate_abi(&marker_path, &marker)?;
    if marker.extensions.is_empty() {
        return Err(ExtensionError::InvalidMarker(
            marker_path,
            "marker must contain at least one [[extension]]".to_owned(),
        ));
    }
    if marker.records.is_some() != marker.records_hash.is_some() {
        return Err(ExtensionError::InvalidMarker(
            marker_path,
            "records and records_hash must be declared together".to_owned(),
        ));
    }
    let metadata = read_metadata(&dist_info.join("METADATA"))?;
    if normalize_distribution_name(&marker.distribution) != metadata.normalized_name {
        return Err(ExtensionError::InvalidMarker(
            marker_path.clone(),
            format!(
                "distribution `{}` does not match METADATA `{}`",
                marker.distribution, metadata.name
            ),
        ));
    }
    if marker.version != metadata.version {
        return Err(ExtensionError::InvalidMarker(
            marker_path.clone(),
            format!(
                "version `{}` does not match METADATA `{}`",
                marker.version, metadata.version
            ),
        ));
    }
    marker
        .python_target
        .parse::<crate::project::PythonVersion>()
        .map_err(|error| ExtensionError::InvalidMarker(marker_path.clone(), error.to_string()))?;
    let mut declared = marker.dependencies.clone();
    declared.sort();
    if declared.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(ExtensionError::InvalidMarker(
            marker_path.clone(),
            "dependencies must be unique".to_owned(),
        ));
    }
    let mut metadata_requirements = metadata.requires_dist.clone();
    metadata_requirements.sort();
    if declared != metadata_requirements {
        return Err(ExtensionError::InvalidMarker(
            marker_path.clone(),
            "marker dependencies do not match METADATA Requires-Dist".to_owned(),
        ));
    }
    let mut ids = BTreeSet::new();
    let mut extensions = Vec::new();
    for extension in marker.extensions {
        validate_extension_id(&extension.id).map_err(|message| {
            ExtensionError::InvalidMarker(dist_info.join("osiris.toml"), message)
        })?;
        if !ids.insert(extension.id.clone()) {
            return Err(ExtensionError::InvalidMarker(
                dist_info.join("osiris.toml"),
                format!("duplicate extension id `{}`", extension.id),
            ));
        }
        let interface = resolve_resource(site_root, &extension.interface)?;
        let (source, source_map) = {
            let interface_hash = &extension.interface_hash;
            validate_sha256(interface_hash)
                .map_err(|message| ExtensionError::InvalidMarker(marker_path.clone(), message))?;
            let interface_source = fs::read_to_string(&interface)
                .map_err(|error| ExtensionError::Io(interface.clone(), error))?;
            let parsed = crate::interface::read(&interface_source).map_err(|error| {
                ExtensionError::InvalidMarker(marker_path.clone(), error.to_string())
            })?;
            if parsed.semantic_interface_hash() != interface_hash {
                return Err(ExtensionError::HashMismatch {
                    path: interface.clone(),
                    expected: interface_hash.to_owned(),
                    actual: parsed.semantic_interface_hash().to_owned(),
                });
            }
            let interface_target = parsed.python_target.to_string();
            if marker.python_target != interface_target {
                return Err(ExtensionError::InvalidMarker(
                    marker_path.clone(),
                    format!("extension `{}` target does not match its interface", extension.id),
                ));
            }
            let source_name = &extension.source;
            let source_hash = &extension.source_hash;
            let source = validate_hashed_resource(site_root, source_name, source_hash)?;
            let map_name = &extension.source_map;
            let map_hash = &extension.source_map_hash;
            let source_map = validate_hashed_resource(site_root, map_name, map_hash)?;
            let map_bytes = fs::read(&source_map)
                .map_err(|error| ExtensionError::Io(source_map.clone(), error))?;
            let parsed_map: serde_json::Value = serde_json::from_slice(&map_bytes).map_err(|error| {
                ExtensionError::InvalidMarker(marker_path.clone(), format!("invalid source map: {error}"))
            })?;
            if parsed_map.get("version").and_then(serde_json::Value::as_u64) != Some(3)
                || parsed_map
                    .get("language_version")
                    .and_then(serde_json::Value::as_str)
                    != Some(crate::LANGUAGE_VERSION)
                || parsed_map.get("source").and_then(serde_json::Value::as_str) != Some(source_name)
                || parsed_map.get("source_hash").and_then(serde_json::Value::as_str) != Some(source_hash)
                || parsed_map.get("python_target").and_then(serde_json::Value::as_str)
                    != Some(marker.python_target.as_str())
            {
                return Err(ExtensionError::InvalidMarker(
                    marker_path.clone(),
                    format!("extension `{}` source map does not identify its source", extension.id),
                ));
            }
            (Some(source), Some(source_map))
        };
        extensions.push(ExtensionResource {
            id: extension.id,
            interface,
            source,
            source_map,
        });
    }
    extensions.sort_by(|left, right| left.id.cmp(&right.id));

    let records = marker
        .records
        .as_deref()
        .map(|path| resolve_resource(site_root, path))
        .transpose()?;
    if let (Some(path), Some(expected)) = (&records, &marker.records_hash) {
        validate_sha256(expected).map_err(|message| {
            ExtensionError::InvalidMarker(dist_info.join("osiris.toml"), message)
        })?;
        let bytes = fs::read(path).map_err(|error| ExtensionError::Io(path.clone(), error))?;
        let actual = sha256(&bytes);
        if &actual != expected {
            return Err(ExtensionError::HashMismatch {
                path: path.clone(),
                expected: expected.clone(),
                actual,
            });
        }
    }
    for support in &marker.linked_support {
        let manifest = validate_hashed_resource(
            site_root,
            &support.manifest,
            &support.manifest_hash,
        )?;
        let bytes = fs::read(&manifest)
            .map_err(|error| ExtensionError::Io(manifest.clone(), error))?;
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
            ExtensionError::InvalidMarker(marker_path.clone(), format!("invalid linked support manifest: {error}"))
        })?;
        validate_linked_support_manifest(
            site_root,
            &manifest,
            &marker_path,
            Some(marker.python_target.as_str()),
            value,
        )?;
    }

    Ok(ExtensionDistribution {
        metadata,
        site_root: site_root.to_path_buf(),
        dist_info: dist_info.to_path_buf(),
        extensions,
        records,
        records_hash: marker.records_hash,
        language_version: marker.language_version,
    })
}

fn validate_hashed_resource(
    site_root: &Path,
    relative: &str,
    expected: &str,
) -> Result<PathBuf, ExtensionError> {
    validate_sha256(expected)
        .map_err(|message| ExtensionError::InvalidMarker(site_root.to_path_buf(), message))?;
    let path = resolve_resource(site_root, relative)?;
    let bytes = fs::read(&path).map_err(|error| ExtensionError::Io(path.clone(), error))?;
    let actual = sha256(&bytes);
    if actual != expected {
        return Err(ExtensionError::HashMismatch {
            path,
            expected: expected.to_owned(),
            actual,
        });
    }
    Ok(path)
}
