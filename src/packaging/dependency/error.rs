use super::*;

#[derive(Debug)]
pub enum DependencyError {
    Io(PathBuf, io::Error),
    Toml(PathBuf, String),
    InvalidLock(PathBuf, String),
    InvalidRequirement(String),
    InvalidVersion(String),
    UnsupportedMarker(String),
    InvalidHash(String),
    MissingPackage(String),
    MissingProjectRoot(String),
    MissingDependency {
        from: String,
        to: String,
    },
    UnsatisfiedDependency {
        from: String,
        to: String,
        requirement: String,
        locked: String,
    },
    AmbiguousPackage {
        name: String,
        versions: Vec<String>,
    },
    ProjectVersionMismatch {
        expected: String,
        locked: String,
    },
    TargetMismatch {
        project: PythonVersion,
        lock: PythonVersion,
    },
    UnreachableExtension {
        distribution: String,
    },
    MarkerDistributionMismatch {
        marker: String,
        locked: String,
    },
    MarkerVersionMismatch {
        distribution: String,
        marker: String,
        locked: String,
    },
    MarkerSourceHashMismatch {
        distribution: String,
        marker: String,
        locked: Option<String>,
    },
    Extension(ExtensionError),
    Interface {
        path: PathBuf,
        message: String,
    },
    InterfaceConflict {
        interface_member_id: String,
        first_distribution: String,
        second_distribution: String,
    },
    Trust(String),
}

impl fmt::Display for DependencyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(path, error) => {
                write!(formatter, "could not read {}: {error}", path.display())
            }
            Self::Toml(path, message) => {
                write!(formatter, "invalid TOML in {}: {message}", path.display())
            }
            Self::InvalidLock(path, message) => {
                write!(formatter, "invalid uv.lock {}: {message}", path.display())
            }
            Self::InvalidRequirement(message) => {
                write!(formatter, "invalid dependency requirement: {message}")
            }
            Self::InvalidVersion(message) => {
                write!(formatter, "invalid dependency version: {message}")
            }
            Self::UnsupportedMarker(message) => {
                write!(formatter, "unsupported dependency marker: {message}")
            }
            Self::InvalidHash(message) => write!(formatter, "invalid SHA-256 hash: {message}"),
            Self::MissingPackage(name) => {
                write!(formatter, "uv.lock has no applicable package `{name}`")
            }
            Self::MissingProjectRoot(name) => {
                write!(formatter, "uv.lock has no project root for `{name}`")
            }
            Self::MissingDependency { from, to } => {
                write!(
                    formatter,
                    "locked dependency `{from}` refers to missing `{to}`"
                )
            }
            Self::UnsatisfiedDependency {
                from,
                to,
                requirement,
                locked,
            } => write!(
                formatter,
                "locked `{from}` dependency `{to}` at {locked} does not satisfy `{requirement}`"
            ),
            Self::AmbiguousPackage { name, versions } => write!(
                formatter,
                "uv.lock has multiple applicable pins for `{name}`: {}",
                versions.join(", ")
            ),
            Self::ProjectVersionMismatch { expected, locked } => write!(
                formatter,
                "project version `{expected}` does not match lock pin `{locked}`"
            ),
            Self::TargetMismatch { project, lock } => write!(
                formatter,
                "project target Python {project} differs from lock target {lock}"
            ),
            Self::UnreachableExtension { distribution } => write!(
                formatter,
                "extension distribution `{distribution}` is not reachable from the project lock root"
            ),
            Self::MarkerDistributionMismatch { marker, locked } => write!(
                formatter,
                "extension marker distribution `{marker}` does not match lock pin `{locked}`"
            ),
            Self::MarkerVersionMismatch {
                distribution,
                marker,
                locked,
            } => write!(
                formatter,
                "extension marker `{distribution}` version `{marker}` does not match lock pin `{locked}`"
            ),
            Self::MarkerSourceHashMismatch {
                distribution,
                marker,
                locked,
            } => write!(
                formatter,
                "extension marker `{distribution}` source hash `{marker}` is not in lock pin `{}`",
                locked.as_deref().unwrap_or("none")
            ),
            Self::Extension(error) => error.fmt(formatter),
            Self::Interface { path, message } => {
                write!(formatter, "invalid interface {}: {message}", path.display())
            }
            Self::InterfaceConflict {
                interface_member_id,
                first_distribution,
                second_distribution,
            } => write!(
                formatter,
                "interface `{interface_member_id}` resolves through both `{first_distribution}` and `{second_distribution}`"
            ),
            Self::Trust(message) => write!(formatter, "invalid trust policy: {message}"),
        }
    }
}

impl Error for DependencyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(_, error) => Some(error),
            Self::Extension(error) => Some(error),
            _ => None,
        }
    }
}
