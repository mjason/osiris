#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
struct JsoncConfig {
    #[serde(rename = "$schema")]
    _schema: Option<String>,
    source: Option<Vec<String>>,
    out_dir: Option<String>,
    exclude: Option<Vec<String>>,
    target_python: Option<String>,
    strict: Option<bool>,
    display_locale: Option<String>,
}

fn load_jsonc_config(path: &Path) -> Result<JsoncConfig, ConfigError> {
    if !path.is_file() {
        return Ok(JsoncConfig::default());
    }
    let source = fs::read_to_string(path).map_err(|error| ConfigError::Io(path.to_path_buf(), error))?;
    json5::from_str(&source).map_err(|error| ConfigError::Jsonc(path.to_path_buf(), error))
}

fn compile_exclude_patterns(values: Vec<String>) -> Result<GlobSet, ConfigError> {
    let mut builder = GlobSetBuilder::new();
    for value in values {
        if value.is_empty() || Path::new(&value).is_absolute() || value.split('/').any(|part| part == "..") {
            return Err(ConfigError::Invalid(format!(
                "exclude pattern `{value}` must be non-empty and relative to the project root"
            )));
        }
        let normalized = value.trim_end_matches('/');
        let has_glob = normalized
            .bytes()
            .any(|byte| matches!(byte, b'*' | b'?' | b'['));
        let mut patterns = if has_glob {
            vec![normalized.to_owned()]
        } else {
            vec![normalized.to_owned(), format!("{normalized}/**")]
        };
        if let Some(directory) = normalized.strip_suffix("/**") {
            patterns.push(directory.trim_end_matches('/').to_owned());
        }
        for pattern in patterns {
            let glob = globset::GlobBuilder::new(&pattern)
                .literal_separator(true)
                .build()
                .map_err(|error| {
                ConfigError::Invalid(format!("invalid exclude pattern `{value}`: {error}"))
            })?;
            builder.add(glob);
        }
    }
    builder
        .build()
        .map_err(|error| ConfigError::Invalid(format!("invalid exclude patterns: {error}")))
}
