use super::*;

pub(super) fn parse_lock_table(
    document: &toml::Table,
    target: PythonVersion,
    path: &Path,
) -> Result<UvLock, DependencyError> {
    let version = document
        .get("version")
        .and_then(toml::Value::as_integer)
        .ok_or_else(|| {
            DependencyError::InvalidLock(path.to_path_buf(), "missing integer `version`".to_owned())
        })?;
    if version != UV_LOCK_FORMAT_VERSION as i64 {
        return Err(DependencyError::InvalidLock(
            path.to_path_buf(),
            format!("unsupported lock format version {version}; expected {UV_LOCK_FORMAT_VERSION}"),
        ));
    }
    let revision = document
        .get("revision")
        .map(|value| {
            let value = value.as_integer().ok_or_else(|| {
                DependencyError::InvalidLock(
                    path.to_path_buf(),
                    "`revision` must be a non-negative integer".to_owned(),
                )
            })?;
            u64::try_from(value).map_err(|_| {
                DependencyError::InvalidLock(
                    path.to_path_buf(),
                    "`revision` must be a non-negative integer".to_owned(),
                )
            })
        })
        .transpose()?;
    let requires_python = document
        .get("requires-python")
        .or_else(|| document.get("requires_python"))
        .map(|value| {
            value.as_str().ok_or_else(|| {
                DependencyError::InvalidLock(
                    path.to_path_buf(),
                    "`requires-python` must be a string".to_owned(),
                )
            })
        })
        .transpose()?
        .map(str::to_owned);
    if let Some(expression) = &requires_python {
        if !satisfies_specifier(expression, &format!("{}.{}", target.major, target.minor))
            .map_err(DependencyError::InvalidVersion)?
        {
            return Err(DependencyError::InvalidLock(
                path.to_path_buf(),
                format!("requires-python `{expression}` excludes target {target}"),
            ));
        }
    }
    let package_values = document
        .get("package")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| {
            DependencyError::InvalidLock(
                path.to_path_buf(),
                "missing `[[package]]` entries".to_owned(),
            )
        })?;
    let mut candidates = BTreeMap::<String, Vec<LockedDistribution>>::new();
    let mut project = None;
    for value in package_values {
        let table = value.as_table().ok_or_else(|| {
            DependencyError::InvalidLock(
                path.to_path_buf(),
                "package entry must be a table".to_owned(),
            )
        })?;
        let package = parse_package(table, target, path)?;
        if package.editable && project.replace(package.normalized_name.clone()).is_some() {
            return Err(DependencyError::InvalidLock(
                path.to_path_buf(),
                "uv.lock contains multiple editable project roots".to_owned(),
            ));
        }
        if package.applicable(target)? {
            candidates
                .entry(package.normalized_name.clone())
                .or_default()
                .push(package);
        }
    }
    let mut packages = BTreeMap::new();
    for (name, mut values) in candidates {
        values.sort();
        values.dedup();
        let versions = values
            .iter()
            .map(|value| value.version.clone())
            .collect::<BTreeSet<_>>();
        if versions.len() > 1 {
            return Err(DependencyError::AmbiguousPackage {
                name,
                versions: versions.into_iter().collect(),
            });
        }
        if values
            .get(1..)
            .is_some_and(|rest| rest.iter().any(|value| !same_pin(&values[0], value)))
        {
            return Err(DependencyError::AmbiguousPackage {
                name,
                versions: values
                    .iter()
                    .map(|value| {
                        format!(
                            "{} ({})",
                            value.version,
                            value.source_hash.as_deref().unwrap_or("no hash")
                        )
                    })
                    .collect(),
            });
        }
        let first = values
            .into_iter()
            .next()
            .expect("candidate map entry cannot be empty");
        packages.insert(name, first);
    }
    Ok(UvLock {
        format_version: version as u64,
        revision,
        requires_python,
        target_python: target,
        packages,
        project,
    })
}

pub(super) fn same_pin(left: &LockedDistribution, right: &LockedDistribution) -> bool {
    left.normalized_name == right.normalized_name
        && left.version == right.version
        && left.source == right.source
        && left.source_hashes == right.source_hashes
        && left.dependencies == right.dependencies
        && left.editable == right.editable
}

pub(super) fn parse_package(
    table: &toml::Table,
    target: PythonVersion,
    path: &Path,
) -> Result<LockedDistribution, DependencyError> {
    let name = required_string(table, "name", path)?;
    if !extension::is_valid_distribution_name(&name) {
        return Err(DependencyError::InvalidLock(
            path.to_path_buf(),
            format!("invalid Python distribution name `{name}`"),
        ));
    }
    let normalized_name = normalize_name(&name);
    if normalized_name.is_empty() {
        return Err(DependencyError::InvalidLock(
            path.to_path_buf(),
            "package name normalizes to empty".to_owned(),
        ));
    }
    let source_table = table.get("source").and_then(toml::Value::as_table);
    let editable = source_table
        .and_then(|source| source.get("editable"))
        .and_then(toml::Value::as_str)
        .is_some_and(|value| value == ".");
    let version = match table.get("version").and_then(toml::Value::as_str) {
        Some(value) if !value.is_empty() => value.to_owned(),
        _ if editable => "0".to_owned(),
        _ => {
            return Err(DependencyError::InvalidLock(
                path.to_path_buf(),
                format!("package `{name}` is missing a locked version"),
            ));
        }
    };
    let resolution_markers = parse_markers(
        table
            .get("resolution-markers")
            .or_else(|| table.get("resolution_markers")),
        target,
    )
    .map_err(|message| DependencyError::InvalidLock(path.to_path_buf(), message))?;
    let source = source_descriptor(source_table, editable);
    let source_hashes = source_hashes(table, path)?;
    if !editable && source_hashes.is_empty() {
        return Err(DependencyError::InvalidLock(
            path.to_path_buf(),
            format!("package `{name}` has no source hash"),
        ));
    }
    let dependencies = parse_dependencies(table.get("dependencies"), path)?;
    let source_hash = canonical_source_hash(&source, &source_hashes);
    Ok(LockedDistribution {
        name,
        normalized_name,
        version,
        source,
        source_hash,
        source_hashes,
        dependencies,
        resolution_markers,
        editable,
    })
}

pub(super) fn canonical_source_hash(source: &str, hashes: &[String]) -> Option<String> {
    match hashes {
        [] => None,
        [hash] => Some(hash.clone()),
        _ => {
            let mut bytes = Vec::new();
            push_field(&mut bytes, "uv-lock-source-v1");
            push_field(&mut bytes, source);
            for hash in hashes {
                push_field(&mut bytes, hash);
            }
            Some(sha256(&bytes))
        }
    }
}

pub(super) fn required_string(
    table: &toml::Table,
    key: &str,
    path: &Path,
) -> Result<String, DependencyError> {
    table
        .get(key)
        .and_then(toml::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            DependencyError::InvalidLock(
                path.to_path_buf(),
                format!("package is missing string `{key}`"),
            )
        })
}

pub(super) fn source_descriptor(source: Option<&toml::Table>, editable: bool) -> String {
    if editable {
        return "editable".to_owned();
    }
    let Some(source) = source else {
        return "unknown".to_owned();
    };
    if source.contains_key("registry") {
        "registry".to_owned()
    } else if source.contains_key("git") {
        "git".to_owned()
    } else if source.contains_key("directory") || source.contains_key("path") {
        "path".to_owned()
    } else {
        source
            .keys()
            .next()
            .cloned()
            .unwrap_or_else(|| "unknown".to_owned())
    }
}

pub(super) fn source_hashes(
    table: &toml::Table,
    path: &Path,
) -> Result<Vec<String>, DependencyError> {
    let mut hashes = BTreeSet::new();
    let mut collect = |value: Option<&toml::Value>, label: &str| -> Result<(), DependencyError> {
        let Some(value) = value else {
            return Ok(());
        };
        let Some(table) = value.as_table() else {
            return Err(DependencyError::InvalidLock(
                path.to_path_buf(),
                format!("`{label}` must be a table"),
            ));
        };
        if let Some(hash) = table.get("hash").and_then(toml::Value::as_str) {
            validate_hash(hash).map_err(DependencyError::InvalidHash)?;
            let hex = hash
                .strip_prefix("sha256:")
                .expect("validated SHA-256 prefix above");
            hashes.insert(format!("sha256:{}", hex.to_ascii_lowercase()));
        }
        Ok(())
    };
    collect(table.get("source"), "source")?;
    collect(table.get("sdist"), "sdist")?;
    if let Some(value) = table.get("wheels") {
        let wheels = value.as_array().ok_or_else(|| {
            DependencyError::InvalidLock(path.to_path_buf(), "`wheels` must be an array".to_owned())
        })?;
        for wheel in wheels {
            collect(Some(wheel), "wheel")?;
        }
    }
    Ok(hashes.into_iter().collect())
}

pub(super) fn parse_markers(
    value: Option<&toml::Value>,
    target: PythonVersion,
) -> Result<Vec<String>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let values = if let Some(single) = value.as_str() {
        vec![single.to_owned()]
    } else if let Some(array) = value.as_array() {
        array
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "resolution markers must be strings".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?
    } else {
        return Err("resolution markers must be a string or array".to_owned());
    };
    for marker in &values {
        marker_applies(marker, target)?;
    }
    Ok(values)
}

pub(super) fn parse_dependencies(
    value: Option<&toml::Value>,
    path: &Path,
) -> Result<Vec<LockedDependency>, DependencyError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let values = value.as_array().ok_or_else(|| {
        DependencyError::InvalidLock(
            path.to_path_buf(),
            "package dependencies must be an array".to_owned(),
        )
    })?;
    let mut result = Vec::new();
    for value in values {
        if let Some(text) = value.as_str() {
            let requirement =
                parse_requirement(text).map_err(DependencyError::InvalidRequirement)?;
            result.push(LockedDependency {
                name: requirement.name,
                normalized_name: requirement.normalized_name,
                version: requirement.specifier,
                marker: requirement.marker,
                extras: requirement.extras,
            });
            continue;
        }
        let table = value.as_table().ok_or_else(|| {
            DependencyError::InvalidLock(
                path.to_path_buf(),
                "dependency edge must be a string or table".to_owned(),
            )
        })?;
        let name = table
            .get("name")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| {
                DependencyError::InvalidLock(
                    path.to_path_buf(),
                    "dependency edge has no name".to_owned(),
                )
            })?;
        let normalized_name = normalize_name(name);
        if normalized_name.is_empty() {
            return Err(DependencyError::InvalidLock(
                path.to_path_buf(),
                "dependency edge name normalizes to empty".to_owned(),
            ));
        }
        let version = table
            .get("version")
            .and_then(toml::Value::as_str)
            .map(str::to_owned);
        if let Some(version) = &version {
            parse_specifier(version).map_err(DependencyError::InvalidRequirement)?;
        }
        let marker = table
            .get("marker")
            .or_else(|| table.get("markers"))
            .map(|value| {
                value.as_str().map(str::to_owned).ok_or_else(|| {
                    DependencyError::InvalidLock(
                        path.to_path_buf(),
                        "dependency marker must be a string".to_owned(),
                    )
                })
            })
            .transpose()?;
        if let Some(marker) = &marker {
            marker_applies(marker, PythonVersion::PYTHON_3_9)
                .map_err(DependencyError::UnsupportedMarker)?;
        }
        let extras = table
            .get("extra")
            .or_else(|| table.get("extras"))
            .map(|value| {
                if let Some(single) = value.as_str() {
                    Ok(vec![single.to_owned()])
                } else if let Some(array) = value.as_array() {
                    array
                        .iter()
                        .map(|item| {
                            item.as_str().map(str::to_owned).ok_or_else(|| {
                                DependencyError::InvalidLock(
                                    path.to_path_buf(),
                                    "dependency extras must be strings".to_owned(),
                                )
                            })
                        })
                        .collect()
                } else {
                    Err(DependencyError::InvalidLock(
                        path.to_path_buf(),
                        "dependency extras must be strings or an array".to_owned(),
                    ))
                }
            })
            .transpose()?
            .unwrap_or_default();
        result.push(LockedDependency {
            name: name.to_owned(),
            normalized_name,
            version,
            marker,
            extras,
        });
    }
    result.sort();
    Ok(result)
}
