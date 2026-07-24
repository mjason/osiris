#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LinkedSupportManifest {
    schema: String,
    language_version: String,
    python_target: String,
    standard_library_abi: u32,
    standard_library_semantic_hash: String,
    helper_format: u32,
    reachable_binding_ids: Vec<String>,
    helper_hashes: BTreeMap<String, String>,
    file_hashes: BTreeMap<String, String>,
    source_maps: Vec<LinkedSourceMapIdentity>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LinkedSourceMapIdentity {
    source: String,
    source_hash: String,
    generated: String,
    build_hash: String,
}

fn invalid_support(marker: &Path, message: impl Into<String>) -> ExtensionError {
    ExtensionError::InvalidMarker(marker.to_path_buf(), message.into())
}

fn validate_linked_support_manifest(
    site_root: &Path,
    manifest_path: &Path,
    marker_path: &Path,
    marker_target: Option<&str>,
    value: serde_json::Value,
) -> Result<(), ExtensionError> {
    let manifest: LinkedSupportManifest = serde_json::from_value(value)
        .map_err(|error| invalid_support(marker_path, format!("invalid linked support manifest: {error}")))?;
    if manifest.schema != "osiris-linked-support/v1"
        || manifest.language_version != crate::LANGUAGE_VERSION
        || manifest.python_target != marker_target.unwrap_or("")
        || manifest.standard_library_abi != crate::STANDARD_LIBRARY_ABI
        || manifest.helper_format != crate::LINKABLE_HELPER_FORMAT
        || manifest.standard_library_semantic_hash != crate::stdlib::semantic_hash()
    {
        return Err(invalid_support(
            marker_path,
            "linked support manifest has an incompatible schema or compiler identity",
        ));
    }
    validate_sha256(&manifest.standard_library_semantic_hash)
        .map_err(|message| invalid_support(marker_path, message))?;
    if manifest.reachable_binding_ids.windows(2).any(|pair| pair[0] >= pair[1])
        || manifest.reachable_binding_ids.iter().any(|id| id.is_empty())
    {
        return Err(invalid_support(
            marker_path,
            "linked support binding IDs must be sorted, unique, and non-empty",
        ));
    }
    validate_support_hashes(
        site_root,
        manifest_path,
        marker_path,
        &manifest.helper_hashes,
        &manifest.file_hashes,
    )?;
    validate_support_source_maps(
        site_root,
        marker_path,
        marker_target,
        &manifest.source_maps,
    )
}

fn validate_support_hashes(
    site_root: &Path,
    manifest_path: &Path,
    marker_path: &Path,
    helper_hashes: &BTreeMap<String, String>,
    file_hashes: &BTreeMap<String, String>,
) -> Result<(), ExtensionError> {
    if helper_hashes.is_empty() || file_hashes.is_empty() {
        return Err(invalid_support(
            marker_path,
            "linked support helper and file hashes must be non-empty",
        ));
    }
    for (name, hash) in helper_hashes {
        if name.is_empty() {
            return Err(invalid_support(marker_path, "linked support helper name is empty"));
        }
        validate_sha256(hash).map_err(|message| invalid_support(marker_path, message))?;
    }
    let support_root = manifest_path
        .parent()
        .ok_or_else(|| invalid_support(marker_path, "linked support manifest has no parent"))?;
    let mut expected_files = BTreeSet::new();
    collect_python_support_files(support_root, &mut expected_files)?;
    let mut declared_files = BTreeSet::new();
    for (relative, expected) in file_hashes {
        validate_sha256(expected).map_err(|message| invalid_support(marker_path, message))?;
        let path = validate_hashed_resource(site_root, relative, expected)?;
        if !path.starts_with(support_root) {
            return Err(invalid_support(
                marker_path,
                format!("linked support file `{relative}` is outside its private package"),
            ));
        }
        declared_files.insert(path);
    }
    if declared_files != expected_files {
        return Err(invalid_support(
            marker_path,
            "linked support manifest does not cover exactly its Python support files",
        ));
    }
    Ok(())
}

fn collect_python_support_files(
    directory: &Path,
    files: &mut BTreeSet<PathBuf>,
) -> Result<(), ExtensionError> {
    let entries =
        fs::read_dir(directory).map_err(|error| ExtensionError::Io(directory.to_path_buf(), error))?;
    for entry in entries {
        let entry = entry.map_err(|error| ExtensionError::Io(directory.to_path_buf(), error))?;
        let path = entry.path();
        if path.is_dir() {
            collect_python_support_files(&path, files)?;
        } else if path.extension().and_then(|value| value.to_str()) == Some("py") {
            files.insert(path);
        }
    }
    Ok(())
}

fn validate_support_source(
    site_root: &Path,
    marker_path: &Path,
    source: &str,
    expected: &str,
) -> Result<(), ExtensionError> {
    if source.starts_with("osiris-stdlib:///") {
        let authored = crate::stdlib::source_artifact_by_uri(source).ok_or_else(|| {
            invalid_support(
                marker_path,
                format!("linked support names unknown standard source `{source}`"),
            )
        })?;
        let actual = sha256(authored.as_bytes());
        if actual != expected {
            return Err(invalid_support(
                marker_path,
                format!("linked support standard source hash is stale for `{source}`"),
            ));
        }
        return Ok(());
    }
    validate_hashed_resource(site_root, source, expected).map(|_| ())
}

fn validate_support_source_maps(
    site_root: &Path,
    marker_path: &Path,
    marker_target: Option<&str>,
    identities: &[LinkedSourceMapIdentity],
) -> Result<(), ExtensionError> {
    if identities.is_empty() || identities.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(invalid_support(
            marker_path,
            "linked support source-map identities must be sorted, unique, and non-empty",
        ));
    }
    for identity in identities {
        validate_sha256(&identity.source_hash)
            .and_then(|_| validate_sha256(&identity.build_hash))
            .map_err(|message| invalid_support(marker_path, message))?;
        validate_support_source(
            site_root,
            marker_path,
            &identity.source,
            &identity.source_hash,
        )?;
        let generated = resolve_resource(site_root, &identity.generated)?;
        let map_name = format!("{}.map", identity.generated);
        let source_map = resolve_resource(site_root, &map_name)?;
        let bytes = fs::read(&source_map)
            .map_err(|error| ExtensionError::Io(source_map.clone(), error))?;
        let map: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
            invalid_support(marker_path, format!("invalid source map `{map_name}`: {error}"))
        })?;
        if map.get("source").and_then(serde_json::Value::as_str) != Some(identity.source.as_str())
            || map.get("python_target").and_then(serde_json::Value::as_str) != marker_target
            || map.get("source_hash").and_then(serde_json::Value::as_str)
                != Some(identity.source_hash.as_str())
            || map.get("generated").and_then(serde_json::Value::as_str)
                != Some(identity.generated.as_str())
            || map.get("build_hash").and_then(serde_json::Value::as_str)
                != Some(identity.build_hash.as_str())
        {
            return Err(invalid_support(
                marker_path,
                format!("linked support source-map identity is stale for `{map_name}`"),
            ));
        }
        let _ = generated;
    }
    Ok(())
}
