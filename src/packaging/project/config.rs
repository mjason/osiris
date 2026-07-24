use std::{
    fmt, fs, io,
    path::{Component, Path, PathBuf},
    str::FromStr,
};

use serde::Deserialize;
use unicode_normalization::UnicodeNormalization;
use globset::{GlobSet, GlobSetBuilder};
use oxilangtag::LanguageTag;

use crate::extension::{is_valid_distribution_name, normalize_distribution_name};

pub use crate::types::PythonVersion;

impl PythonVersion {
    pub const MINIMUM: Self = Self { major: 3, minor: 11 };
    pub const DEFAULT_TARGET: Self = Self { major: 3, minor: 11 };
}

impl Default for PythonVersion {
    fn default() -> Self {
        Self::DEFAULT_TARGET
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

#[derive(Clone, Debug)]
pub struct ProjectConfig {
    pub root: PathBuf,
    pub distribution: String,
    pub distribution_version: String,
    pub dependencies: Vec<String>,
    pub source_roots: Vec<PathBuf>,
    pub output_dir: PathBuf,
    exclude: GlobSet,
    pub target_python: PythonVersion,
    pub strict: bool,
    pub display_locale: Option<String>,
}

impl ProjectConfig {
    pub fn discover(start: &Path) -> Result<Self, ConfigError> {
        let mut directory = if start.is_file() {
            start.parent().unwrap_or_else(|| Path::new("."))
        } else {
            start
        };

        loop {
            let jsonc = directory.join("osiris.jsonc");
            let candidate = directory.join("pyproject.toml");
            if jsonc.is_file() {
                if !candidate.is_file() {
                    return Err(ConfigError::MissingConfig(candidate));
                }
                return Self::load(&candidate);
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
        let parent = pyproject
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let root = if parent == Path::new(".") {
            std::env::current_dir()
                .map_err(|error| ConfigError::Io(pyproject.to_path_buf(), error))?
        } else {
            fs::canonicalize(parent)
                .map_err(|error| ConfigError::Io(pyproject.to_path_buf(), error))?
        };
        let jsonc_path = root.join("osiris.jsonc");
        if !jsonc_path.is_file() {
            return Err(ConfigError::MissingConfig(jsonc_path));
        }
        let jsonc = load_jsonc_config(&jsonc_path)?;
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

        let output_relative = PathBuf::from(
            jsonc
                .out_dir
                .clone()
                .unwrap_or_else(|| "dist".to_owned()),
        );
        validate_relative_path(&output_relative, "output directory")?;
        let sources = jsonc.source.unwrap_or_else(|| vec!["src".to_owned()]);
        if sources.is_empty() {
            return Err(ConfigError::Invalid(
                "source must contain at least one source root".to_owned(),
            ));
        }
        let mut source_roots = Vec::with_capacity(sources.len());
        let mut seen_sources = std::collections::BTreeSet::new();
        for source in sources {
            let relative = PathBuf::from(&source);
            validate_relative_path(&relative, "source root")?;
            if !seen_sources.insert(relative.clone()) {
                return Err(ConfigError::Invalid(format!(
                    "duplicate normalized source root `{}`",
                    relative.display()
                )));
            }
            if relative == output_relative || relative.starts_with(&output_relative) {
                return Err(ConfigError::Invalid(format!(
                    "source root `{}` must not be inside output directory `{}`",
                    relative.display(),
                    output_relative.display()
                )));
            }
            source_roots.push(root.join(relative));
        }

        let target_python = jsonc
            .target_python
            .as_deref()
            .unwrap_or("3.11")
            .parse()?;
        let display_locale = Some(
            jsonc
                .display_locale
                .unwrap_or_else(|| "zh-CN".to_owned()),
        )
            .map(|locale| {
                LanguageTag::parse_and_normalize(&locale)
                    .map(|tag| tag.to_string())
                    .map_err(|error| {
                        ConfigError::Invalid(format!(
                            "displayLocale `{locale}` is not a well-formed BCP 47 language tag: {error}"
                        ))
                    })
            })
            .transpose()?;
        let exclude = compile_exclude_patterns(jsonc.exclude.unwrap_or_default())?;
        let output_dir = root.join(output_relative);

        Ok(Self {
            root,
            distribution,
            distribution_version,
            dependencies,
            source_roots,
            output_dir,
            exclude,
            target_python,
            strict: jsonc.strict.unwrap_or(true),
            display_locale,
        })
    }

    #[must_use]
    pub fn default_output_dir(&self) -> PathBuf {
        self.output_dir.clone()
    }

    #[must_use]
    pub fn is_excluded(&self, path: &Path) -> bool {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        absolute.strip_prefix(&self.root).is_ok_and(|relative| {
            absolute.starts_with(&self.output_dir) || self.exclude.is_match(relative)
        })
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

}
