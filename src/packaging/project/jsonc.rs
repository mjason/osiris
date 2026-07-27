#[derive(Default)]
struct JsoncConfig {
    _schema: Option<String>,
    source: Option<Vec<String>>,
    out_dir: Option<String>,
    exclude: Option<Vec<String>>,
    target_python: Option<String>,
    strict: Option<bool>,
    display_locale: Option<String>,
}

impl<'de> Deserialize<'de> for JsoncConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct JsoncVisitor;

        impl<'de> serde::de::Visitor<'de> for JsoncVisitor {
            type Value = JsoncConfig;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an Osiris JSONC configuration object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut config = JsoncConfig::default();
                let mut seen = BTreeSet::new();
                let mut unknown = BTreeSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !seen.insert(key.clone()) {
                        return Err(serde::de::Error::custom(format!(
                            "duplicate JSONC field `{key}`"
                        )));
                    }
                    match key.as_str() {
                        "$schema" => config._schema = map.next_value()?,
                        "source" => config.source = map.next_value()?,
                        "outDir" => config.out_dir = map.next_value()?,
                        "exclude" => config.exclude = map.next_value()?,
                        "targetPython" => config.target_python = map.next_value()?,
                        "strict" => config.strict = map.next_value()?,
                        "displayLocale" => config.display_locale = map.next_value()?,
                        _ => {
                            let _: serde_json::Value = map.next_value()?;
                            unknown.insert(key);
                        }
                    }
                }
                if config.strict.unwrap_or(true) && !unknown.is_empty() {
                    return Err(serde::de::Error::custom(format!(
                        "unknown osiris.jsonc field `{}`",
                        unknown.into_iter().next().expect("unknown field")
                    )));
                }
                Ok(config)
            }
        }

        deserializer.deserialize_map(JsoncVisitor)
    }
}

fn load_jsonc_config(path: &Path) -> Result<JsoncConfig, ConfigError> {
    if !path.is_file() {
        return Ok(JsoncConfig::default());
    }
    let source = fs::read_to_string(path).map_err(|error| ConfigError::Io(path.to_path_buf(), error))?;
    crate::jsonc::validate_no_duplicate_keys(&source)
        .map_err(|error| ConfigError::Invalid(format!("invalid osiris.jsonc: {error}")))?;
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
