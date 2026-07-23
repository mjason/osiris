pub(super) fn validate_external_records(
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

pub(super) fn read_extension_interfaces(
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

pub(super) fn path_to_utf8(path: &Path, label: &str) -> Result<String, String> {
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
pub(super) fn validate_external_records_against_interfaces(
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
