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
    if let Some(distribution) = marker.distribution.as_deref() {
        if normalize_distribution_name(distribution) != metadata.normalized_name {
            return Err(ExtensionError::InvalidMarker(
                marker_path.clone(),
                format!(
                    "distribution `{distribution}` does not match METADATA `{}`",
                    metadata.name
                ),
            ));
        }
    }
    if let Some(version) = marker.version.as_deref() {
        if version != metadata.version {
            return Err(ExtensionError::InvalidMarker(
                marker_path.clone(),
                format!(
                    "version `{version}` does not match METADATA `{}`",
                    metadata.version
                ),
            ));
        }
    }
    if let Some(source_hash) = marker.source_hash.as_deref() {
        validate_sha256(source_hash)
            .map_err(|message| ExtensionError::InvalidMarker(marker_path.clone(), message))?;
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
        extensions.push(ExtensionResource {
            id: extension.id,
            interface,
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

    Ok(ExtensionDistribution {
        metadata,
        site_root: site_root.to_path_buf(),
        dist_info: dist_info.to_path_buf(),
        extensions,
        records,
        records_hash: marker.records_hash,
        marker_distribution: marker.distribution,
        marker_version: marker.version,
        marker_source_hash: marker.source_hash,
    })
}
