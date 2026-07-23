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
