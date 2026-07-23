fn validate_abi(path: &Path, marker: &RawMarker) -> Result<(), ExtensionError> {
    for (label, actual, expected) in [
        ("schema", marker.schema, MARKER_SCHEMA),
        ("compiler_abi", marker.compiler_abi, COMPILER_ABI),
        ("language_abi", marker.language_abi, LANGUAGE_ABI),
    ] {
        if actual != expected {
            return Err(ExtensionError::InvalidMarker(
                path.to_path_buf(),
                format!("unsupported {label} {actual}; expected {expected}"),
            ));
        }
    }
    Ok(())
}

fn resolve_resource(site_root: &Path, relative: &str) -> Result<PathBuf, ExtensionError> {
    let relative = Path::new(relative);
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ExtensionError::ResourceEscape(relative.to_path_buf()));
    }
    let path = site_root.join(relative);
    if !path.is_file() {
        return Err(ExtensionError::MissingResource(path));
    }
    let canonical_root = fs::canonicalize(site_root)
        .map_err(|error| ExtensionError::Io(site_root.to_path_buf(), error))?;
    let canonical_path =
        fs::canonicalize(&path).map_err(|error| ExtensionError::Io(path.clone(), error))?;
    if !canonical_path.starts_with(canonical_root) {
        return Err(ExtensionError::ResourceEscape(path));
    }
    Ok(canonical_path)
}

fn read_metadata(path: &Path) -> Result<DistributionMetadata, ExtensionError> {
    let text =
        fs::read_to_string(path).map_err(|error| ExtensionError::Io(path.to_path_buf(), error))?;
    let mut headers = BTreeMap::<String, Vec<String>>::new();
    let mut current = None::<String>;
    for line in text.lines() {
        if line.is_empty() {
            break;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            let Some(name) = &current else {
                return Err(ExtensionError::InvalidMetadata(
                    path.to_path_buf(),
                    "continuation line has no preceding header".to_owned(),
                ));
            };
            if let Some(value) = headers.get_mut(name).and_then(|values| values.last_mut()) {
                value.push_str(line.trim());
            }
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(ExtensionError::InvalidMetadata(
                path.to_path_buf(),
                format!("malformed metadata header `{line}`"),
            ));
        };
        let name = name.to_ascii_lowercase();
        headers
            .entry(name.clone())
            .or_default()
            .push(value.trim().to_owned());
        current = Some(name);
    }
    let one = |name: &str| -> Result<String, ExtensionError> {
        let values = headers.get(name).ok_or_else(|| {
            ExtensionError::InvalidMetadata(path.to_path_buf(), format!("missing `{name}` header"))
        })?;
        if values.len() != 1 || values[0].is_empty() {
            return Err(ExtensionError::InvalidMetadata(
                path.to_path_buf(),
                format!("metadata requires exactly one non-empty `{name}` header"),
            ));
        }
        Ok(values[0].clone())
    };
    let name = one("name")?;
    if !is_valid_distribution_name(&name) {
        return Err(ExtensionError::InvalidMetadata(
            path.to_path_buf(),
            format!("invalid Python distribution name `{name}`"),
        ));
    }
    let normalized_name = normalize_distribution_name(&name);
    if normalized_name.is_empty() {
        return Err(ExtensionError::InvalidMetadata(
            path.to_path_buf(),
            "distribution name normalizes to empty".to_owned(),
        ));
    }
    Ok(DistributionMetadata {
        normalized_name,
        name,
        version: one("version")?,
        requires_dist: headers.remove("requires-dist").unwrap_or_default(),
    })
}
