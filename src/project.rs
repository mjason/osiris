//! `pyproject.toml` discovery and validated Osiris project configuration.

use std::{
    fmt, fs, io,
    path::{Component, Path, PathBuf},
    str::FromStr,
};

use serde::Deserialize;
use unicode_normalization::UnicodeNormalization;

use crate::extension::{is_valid_distribution_name, normalize_distribution_name};

pub use crate::types::PythonVersion;

impl PythonVersion {
    pub const MINIMUM: Self = Self { major: 3, minor: 9 };
}

impl Default for PythonVersion {
    fn default() -> Self {
        Self::MINIMUM
    }
}

impl fmt::Display for PythonVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}", self.major, self.minor)
    }
}

impl FromStr for PythonVersion {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (major, minor) = value.split_once('.').ok_or_else(|| {
            ConfigError::Invalid(format!("target-python `{value}` must use MAJOR.MINOR form"))
        })?;
        let version = Self {
            major: major.parse().map_err(|_| {
                ConfigError::Invalid(format!("invalid target-python major version `{major}`"))
            })?,
            minor: minor.parse().map_err(|_| {
                ConfigError::Invalid(format!("invalid target-python minor version `{minor}`"))
            })?,
        };
        if version < Self::MINIMUM {
            return Err(ConfigError::Invalid(format!(
                "target-python {version} is below the supported minimum {}",
                Self::MINIMUM
            )));
        }
        Ok(version)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrustContract {
    pub distribution: String,
    pub semantic_interface_hash: String,
    pub ids: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectConfig {
    pub root: PathBuf,
    pub distribution: String,
    pub distribution_version: String,
    pub dependencies: Vec<String>,
    pub source_roots: Vec<PathBuf>,
    pub target_python: PythonVersion,
    pub strict: bool,
    pub extensions: Vec<String>,
    pub build_groups: Vec<String>,
    pub display_locale: Option<String>,
    pub trust_contracts: Vec<TrustContract>,
}

impl ProjectConfig {
    pub fn discover(start: &Path) -> Result<Self, ConfigError> {
        let mut directory = if start.is_file() {
            start.parent().unwrap_or_else(|| Path::new("."))
        } else {
            start
        };

        loop {
            let candidate = directory.join("pyproject.toml");
            if candidate.is_file() {
                let contents = fs::read_to_string(&candidate)
                    .map_err(|error| ConfigError::Io(candidate.clone(), error))?;
                let document = contents
                    .parse::<toml::Table>()
                    .map_err(|error| ConfigError::Toml(candidate.clone(), error))?;
                if document
                    .get("tool")
                    .and_then(toml::Value::as_table)
                    .and_then(|tool| tool.get("osiris"))
                    .is_some()
                {
                    return Self::load(&candidate);
                }
            }

            let Some(parent) = directory.parent() else {
                break;
            };
            directory = parent;
        }

        Err(ConfigError::NotFound(start.to_path_buf()))
    }

    pub fn load(pyproject: &Path) -> Result<Self, ConfigError> {
        let contents = fs::read_to_string(pyproject)
            .map_err(|error| ConfigError::Io(pyproject.to_path_buf(), error))?;
        let parsed: PyProject = toml::from_str(&contents)
            .map_err(|error| ConfigError::Toml(pyproject.to_path_buf(), error))?;
        let project = parsed.project.unwrap_or_default();
        let raw = parsed
            .tool
            .and_then(|tool| tool.osiris)
            .ok_or_else(|| ConfigError::MissingTable(pyproject.to_path_buf()))?;
        let parent = pyproject
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let root = if parent == Path::new(".") {
            std::env::current_dir()
                .map_err(|error| ConfigError::Io(pyproject.to_path_buf(), error))?
        } else {
            parent.to_path_buf()
        };
        let distribution_name = project.name.unwrap_or_else(|| {
            root.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("osiris-project")
                .to_owned()
        });
        if !is_valid_distribution_name(&distribution_name) {
            return Err(ConfigError::Invalid(format!(
                "[project].name `{distribution_name}` is not a valid Python distribution name"
            )));
        }
        let distribution = normalize_distribution_name(&distribution_name);
        let distribution_version = project.version.unwrap_or_else(|| "0".to_owned());
        let dependencies = project.dependencies;
        if distribution.is_empty() {
            return Err(ConfigError::Invalid(
                "[project].name must not be empty".to_owned(),
            ));
        }
        if distribution_version.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "[project].version must not be empty".to_owned(),
            ));
        }

        let sources = if raw.source.is_empty() {
            vec!["src".to_owned()]
        } else {
            raw.source
        };
        let mut source_roots = Vec::with_capacity(sources.len());
        for source in sources {
            let relative = PathBuf::from(&source);
            validate_relative_path(&relative, "source root")?;
            source_roots.push(root.join(relative));
        }

        let target_python = raw.target_python.as_deref().unwrap_or("3.9").parse()?;
        let mut extensions = raw.extensions;
        if extensions
            .iter()
            .any(|extension| extension.trim().is_empty())
        {
            return Err(ConfigError::Invalid(
                "[tool.osiris].extensions entries must not be empty".to_owned(),
            ));
        }
        extensions.sort();
        if extensions.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(ConfigError::Invalid(
                "[tool.osiris].extensions must not contain duplicates".to_owned(),
            ));
        }
        let mut build_groups = raw.build_groups;
        if build_groups.iter().any(|group| group.trim().is_empty()) {
            return Err(ConfigError::Invalid(
                "[tool.osiris].build-groups entries must not be empty".to_owned(),
            ));
        }
        build_groups.sort();
        if build_groups.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(ConfigError::Invalid(
                "[tool.osiris].build-groups must not contain duplicates".to_owned(),
            ));
        }
        let mut trust_contracts = raw
            .trust
            .contract
            .into_iter()
            .map(|mut contract| {
                let hash = contract
                    .semantic_interface_hash
                    .strip_prefix("sha256:")
                    .ok_or_else(|| {
                        ConfigError::Invalid(format!(
                            "trust hash for `{}` must start with `sha256:`",
                            contract.distribution
                        ))
                    })?;
                if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(ConfigError::Invalid(format!(
                        "trust hash for `{}` must contain 64 hexadecimal digits",
                        contract.distribution
                    )));
                }
                if contract.ids.is_empty()
                    || contract.ids.iter().any(|id| {
                        id.is_empty()
                            || id.chars().any(|character| {
                                character.is_control() || character.is_whitespace()
                            })
                    })
                {
                    return Err(ConfigError::Invalid(format!(
                        "trust contract for `{}` must list non-empty ids",
                        contract.distribution
                    )));
                }
                contract.ids.sort();
                contract.ids.dedup();
                if !is_valid_distribution_name(&contract.distribution) {
                    return Err(ConfigError::Invalid(format!(
                        "trust contract distribution `{}` is not a valid Python distribution name",
                        contract.distribution
                    )));
                }
                let distribution = normalize_distribution_name(&contract.distribution);
                if distribution.is_empty() {
                    return Err(ConfigError::Invalid(
                        "trust contract distribution must not be empty".to_owned(),
                    ));
                }
                Ok(TrustContract {
                    distribution,
                    semantic_interface_hash: format!("sha256:{}", hash.to_ascii_lowercase()),
                    ids: contract.ids,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        trust_contracts.sort_by(|left, right| {
            (&left.distribution, &left.semantic_interface_hash, &left.ids).cmp(&(
                &right.distribution,
                &right.semantic_interface_hash,
                &right.ids,
            ))
        });

        Ok(Self {
            root,
            distribution,
            distribution_version,
            dependencies,
            source_roots,
            target_python,
            strict: raw.strict,
            extensions,
            build_groups,
            display_locale: raw.display_locale,
            trust_contracts,
        })
    }

    #[must_use]
    pub fn default_output_dir(&self) -> PathBuf {
        self.root.join("target/osr")
    }

    /// Maps an existing `.osr` source to its module name using exactly one
    /// configured source root. Symlinks and lexical/canonical escapes are
    /// rejected so the same source cannot acquire an environment-dependent
    /// identity.
    pub fn module_name_for_source(&self, source: &Path) -> Result<String, ConfigError> {
        if source.extension().and_then(|extension| extension.to_str()) != Some("osr") {
            return Err(ConfigError::Invalid(format!(
                "source `{}` must use the .osr extension",
                source.display()
            )));
        }
        if source
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        {
            return Err(ConfigError::Invalid(format!(
                "source `{}` must not contain `.` or `..` components",
                source.display()
            )));
        }
        let candidate = if source.is_absolute() {
            source.to_path_buf()
        } else {
            self.root.join(source)
        };
        reject_symlink_components(&candidate)?;
        let canonical_project = fs::canonicalize(&self.root)
            .map_err(|error| ConfigError::Io(self.root.clone(), error))?;
        let canonical_source = fs::canonicalize(&candidate)
            .map_err(|error| ConfigError::Io(candidate.clone(), error))?;
        if !canonical_source.is_file() {
            return Err(ConfigError::Invalid(format!(
                "source `{}` is not a regular file",
                source.display()
            )));
        }
        if !canonical_source.starts_with(&canonical_project) {
            return Err(ConfigError::Invalid(format!(
                "source `{}` escapes project root {}",
                source.display(),
                self.root.display()
            )));
        }

        let mut matches = Vec::new();
        for root in &self.source_roots {
            reject_symlink_components(root)?;
            let canonical_root =
                fs::canonicalize(root).map_err(|error| ConfigError::Io(root.clone(), error))?;
            if !canonical_root.starts_with(&canonical_project) {
                return Err(ConfigError::Invalid(format!(
                    "source root `{}` escapes the project",
                    root.display()
                )));
            }
            if canonical_source.starts_with(&canonical_root) {
                matches.push(canonical_root);
            }
        }
        if matches.len() != 1 {
            return Err(ConfigError::Invalid(if matches.is_empty() {
                format!(
                    "source `{}` is outside configured source roots",
                    source.display()
                )
            } else {
                format!(
                    "source `{}` belongs to multiple configured source roots",
                    source.display()
                )
            }));
        }
        let relative = canonical_source
            .strip_prefix(&matches[0])
            .expect("source-root membership checked above");
        let without_extension = relative.with_extension("");
        let mut components = Vec::new();
        for component in without_extension.components() {
            let Component::Normal(component) = component else {
                return Err(ConfigError::Invalid(format!(
                    "source `{}` has an invalid module path",
                    source.display()
                )));
            };
            let component = component.to_str().ok_or_else(|| {
                ConfigError::Invalid(format!(
                    "source `{}` has a non-UTF-8 module component",
                    source.display()
                ))
            })?;
            if component.is_empty() || component.contains('.') {
                return Err(ConfigError::Invalid(format!(
                    "source `{}` has an ambiguous module component `{component}`",
                    source.display()
                )));
            }
            components.push(component.nfc().collect::<String>());
        }
        if components.is_empty() {
            return Err(ConfigError::Invalid(format!(
                "source `{}` has no module name",
                source.display()
            )));
        }
        Ok(components.join("."))
    }

    pub fn load_lock(
        &self,
    ) -> Result<crate::dependency::UvLock, crate::dependency::DependencyError> {
        crate::dependency::UvLock::load(&self.root.join("uv.lock"), self.target_python)
    }

    pub fn resolve_effective_extensions(
        &self,
        lock: &crate::dependency::UvLock,
        site_roots: &[PathBuf],
    ) -> Result<crate::dependency::EffectiveExtensionGraph, crate::dependency::DependencyError>
    {
        crate::dependency::resolve_effective_extensions(self, lock, site_roots)
    }

    pub fn trust_policy_hash(
        &self,
        resolved: &[crate::dependency::SemanticInterfaceHash],
    ) -> Result<String, crate::dependency::DependencyError> {
        crate::dependency::trust_policy_hash(&self.trust_contracts, resolved)
    }
}

#[derive(Debug)]
pub enum ConfigError {
    NotFound(PathBuf),
    MissingTable(PathBuf),
    Io(PathBuf, io::Error),
    Toml(PathBuf, toml::de::Error),
    Invalid(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(path) => write!(
                formatter,
                "no pyproject.toml with [tool.osiris] found from {}",
                path.display()
            ),
            Self::MissingTable(path) => {
                write!(formatter, "{} has no [tool.osiris] table", path.display())
            }
            Self::Io(path, error) => {
                write!(formatter, "could not read {}: {error}", path.display())
            }
            Self::Toml(path, error) => {
                write!(formatter, "invalid TOML in {}: {error}", path.display())
            }
            Self::Invalid(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ConfigError {}

#[derive(Deserialize)]
struct PyProject {
    #[serde(default)]
    project: Option<RawProject>,
    tool: Option<ToolTable>,
}

#[derive(Default, Deserialize)]
struct RawProject {
    name: Option<String>,
    version: Option<String>,
    #[serde(default)]
    dependencies: Vec<String>,
}

#[derive(Deserialize)]
struct ToolTable {
    osiris: Option<RawConfig>,
}

#[derive(Deserialize)]
#[serde(default, rename_all = "kebab-case")]
struct RawConfig {
    source: Vec<String>,
    target_python: Option<String>,
    strict: bool,
    extensions: Vec<String>,
    build_groups: Vec<String>,
    display_locale: Option<String>,
    trust: RawTrust,
}

impl Default for RawConfig {
    fn default() -> Self {
        Self {
            source: Vec::new(),
            target_python: None,
            strict: true,
            extensions: Vec::new(),
            build_groups: Vec::new(),
            display_locale: None,
            trust: RawTrust::default(),
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct RawTrust {
    contract: Vec<RawTrustContract>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct RawTrustContract {
    distribution: String,
    semantic_interface_hash: String,
    ids: Vec<String>,
}

fn validate_relative_path(path: &Path, label: &str) -> Result<(), ConfigError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ConfigError::Invalid(format!(
            "{label} `{}` must be a normalized relative path",
            path.display()
        )));
    }
    Ok(())
}

fn reject_symlink_components(path: &Path) -> Result<(), ConfigError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(ConfigError::Invalid(format!(
                    "path `{}` must not contain symlinks",
                    path.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(ConfigError::Io(current, error)),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use super::{ProjectConfig, PythonVersion};

    static NEXT_TEST: AtomicUsize = AtomicUsize::new(0);

    fn fixture(contents: &str) -> std::path::PathBuf {
        let id = NEXT_TEST.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("osiris-project-config-{}-{id}", std::process::id()));
        fs::create_dir(&root).expect("fixture root should be created");
        let path = root.join("pyproject.toml");
        fs::write(&path, contents).expect("fixture TOML should be written");
        path
    }

    #[test]
    fn loads_tool_configuration_and_trust_contracts() {
        let path = fixture(
            r#"
[project]
name = "sample"

[tool.osiris]
source = ["osr-src"]
target-python = "3.11"
strict = true
extensions = ["osiris-data-ext"]
display-locale = "zh-CN"

[[tool.osiris.trust.contract]]
distribution = "osiris-data-ext"
semantic-interface-hash = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
ids = ["osiris.data.mean"]
"#,
        );
        let config = ProjectConfig::load(&path).expect("configuration should load");

        assert_eq!(
            config.target_python,
            PythonVersion {
                major: 3,
                minor: 11
            }
        );
        assert_eq!(config.extensions, ["osiris-data-ext"]);
        assert_eq!(config.distribution, "sample");
        assert_eq!(config.distribution_version, "0");
        assert_eq!(config.display_locale.as_deref(), Some("zh-CN"));
        assert_eq!(config.trust_contracts.len(), 1);
        let root = path.parent().expect("fixture has parent");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn loads_bare_relative_pyproject_path_from_project_root() {
        let id = NEXT_TEST.fetch_add(1, Ordering::Relaxed);
        let fixture_name = format!(
            ".osiris-relative-project-config-{}-{id}.toml",
            std::process::id()
        );
        let source_root_name = format!(
            ".osiris-relative-project-source-{}-{id}",
            std::process::id()
        );
        let pyproject = std::path::PathBuf::from(&fixture_name);
        let source_root = std::path::PathBuf::from(&source_root_name);
        let source = source_root.join("value.osr");
        fs::create_dir(&source_root).expect("relative source root should be created");
        fs::write(&source, "(module value)\n").expect("relative source should be written");
        fs::write(
            &pyproject,
            format!("[tool.osiris]\nsource = [\"{source_root_name}\"]\n"),
        )
        .expect("relative configuration should be written");

        let config = ProjectConfig::load(&pyproject).expect("relative configuration should load");
        let current_dir = std::env::current_dir().expect("current directory should be available");
        assert_eq!(config.root, current_dir);
        let absolute_source = config.root.join(&source);
        assert_eq!(
            config.module_name_for_source(&absolute_source).unwrap(),
            "value"
        );

        let _ = fs::remove_file(pyproject);
        let _ = fs::remove_dir_all(source_root);
    }

    #[test]
    fn maps_source_paths_to_unique_module_names() {
        let path = fixture("[tool.osiris]\nsource = [\"src\"]\n");
        let root = path.parent().expect("fixture has parent");
        let source = root.join("src/数据/归一化.osr");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, "(module 数据.归一化)\n").unwrap();
        let config = ProjectConfig::load(&path).unwrap();
        assert_eq!(
            config.module_name_for_source(&source).unwrap(),
            "数据.归一化"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_ambiguous_nested_source_roots() {
        let path = fixture("[tool.osiris]\nsource = [\"src\", \"src/pkg\"]\n");
        let root = path.parent().expect("fixture has parent");
        let source = root.join("src/pkg/value.osr");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, "(module value)\n").unwrap();
        let config = ProjectConfig::load(&path).unwrap();
        let error = config.module_name_for_source(&source).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("multiple configured source roots")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_source_identity() {
        use std::os::unix::fs::symlink;

        let path = fixture("[tool.osiris]\nsource = [\"src\"]\n");
        let root = path.parent().expect("fixture has parent");
        let real = root.join("src/real.osr");
        let linked = root.join("src/linked.osr");
        fs::create_dir_all(real.parent().unwrap()).unwrap();
        fs::write(&real, "(module real)\n").unwrap();
        symlink(&real, &linked).unwrap();
        let config = ProjectConfig::load(&path).unwrap();
        let error = config.module_name_for_source(&linked).unwrap_err();
        assert!(error.to_string().contains("must not contain symlinks"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_source_paths_that_escape_the_project() {
        let path = fixture("[tool.osiris]\nsource = [\"../outside\"]\n");
        let error = ProjectConfig::load(&path).expect_err("escaping source root must fail");
        assert!(error.to_string().contains("normalized relative path"));
        let root = path.parent().expect("fixture has parent");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_unsupported_python_versions() {
        let error = "3.8"
            .parse::<PythonVersion>()
            .expect_err("Python 3.8 must be rejected");
        assert!(error.to_string().contains("supported minimum"));
    }

    #[test]
    fn rejects_empty_and_duplicate_build_groups() {
        for (groups, expected) in [
            (r#"["osiris", ""]"#, "entries must not be empty"),
            (r#"["osiris", "osiris"]"#, "must not contain duplicates"),
        ] {
            let path = fixture(&format!("[tool.osiris]\nbuild-groups = {groups}\n"));
            let error = ProjectConfig::load(&path).expect_err("invalid groups must be rejected");
            assert!(error.to_string().contains(expected), "{error}");
            let root = path.parent().expect("fixture has parent");
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn omitted_build_groups_do_not_select_an_implicit_dependency_group() {
        let path = fixture("[tool.osiris]\n");
        let config = ProjectConfig::load(&path).expect("minimal configuration should load");
        assert!(config.build_groups.is_empty());
        let root = path.parent().expect("fixture has parent");
        let _ = fs::remove_dir_all(root);
    }
}
