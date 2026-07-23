use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt, fs, io,
    path::{Component, Path, PathBuf},
};

use serde::Deserialize;

use crate::hash::sha256;

pub const MARKER_SCHEMA: u32 = 1;
pub const COMPILER_ABI: u32 = 1;
pub const LANGUAGE_ABI: u32 = 2;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DistributionMetadata {
    pub name: String,
    pub normalized_name: String,
    pub version: String,
    pub requires_dist: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtensionResource {
    pub id: String,
    pub interface: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtensionDistribution {
    pub metadata: DistributionMetadata,
    pub site_root: PathBuf,
    pub dist_info: PathBuf,
    pub extensions: Vec<ExtensionResource>,
    pub records: Option<PathBuf>,
    pub records_hash: Option<String>,
    marker_distribution: Option<String>,
    marker_version: Option<String>,
    marker_source_hash: Option<String>,
}

impl ExtensionDistribution {
    /// Optional artifact hash explicitly recorded by a future marker schema.
    /// v0 markers normally derive distribution/version from standard
    /// `METADATA`; exposing this value lets the lock layer validate stricter
    /// producers without making uv's wheel metadata redundant.
    #[must_use]
    pub fn marker_source_hash(&self) -> Option<&str> {
        self.marker_source_hash.as_deref()
    }

    #[must_use]
    pub fn marker_distribution(&self) -> Option<&str> {
        self.marker_distribution.as_deref()
    }

    #[must_use]
    pub fn marker_version(&self) -> Option<&str> {
        self.marker_version.as_deref()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExtensionGraph {
    pub distributions: Vec<ExtensionDistribution>,
    pub by_id: BTreeMap<String, (usize, usize)>,
}

impl ExtensionGraph {
    #[must_use]
    pub fn extension(&self, id: &str) -> Option<(&ExtensionDistribution, &ExtensionResource)> {
        let (distribution, extension) = *self.by_id.get(id)?;
        Some((
            self.distributions.get(distribution)?,
            self.distributions
                .get(distribution)?
                .extensions
                .get(extension)?,
        ))
    }
}

#[derive(Debug)]
pub enum ExtensionError {
    Io(PathBuf, io::Error),
    InvalidMarker(PathBuf, String),
    InvalidMetadata(PathBuf, String),
    MissingResource(PathBuf),
    ResourceEscape(PathBuf),
    HashMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    DuplicateId {
        id: String,
        first: String,
        second: String,
    },
    MissingEnabled(String),
}

impl fmt::Display for ExtensionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(path, error) => {
                write!(formatter, "could not read {}: {error}", path.display())
            }
            Self::InvalidMarker(path, message) => {
                write!(
                    formatter,
                    "invalid extension marker {}: {message}",
                    path.display()
                )
            }
            Self::InvalidMetadata(path, message) => {
                write!(
                    formatter,
                    "invalid distribution metadata {}: {message}",
                    path.display()
                )
            }
            Self::MissingResource(path) => {
                write!(
                    formatter,
                    "declared extension resource is missing: {}",
                    path.display()
                )
            }
            Self::ResourceEscape(path) => write!(
                formatter,
                "declared extension resource escapes its site root: {}",
                path.display()
            ),
            Self::HashMismatch {
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "resource hash mismatch for {}: expected {expected}, found {actual}",
                path.display()
            ),
            Self::DuplicateId { id, first, second } => write!(
                formatter,
                "extension id `{id}` is provided by both `{first}` and `{second}`"
            ),
            Self::MissingEnabled(id) => {
                write!(formatter, "enabled extension `{id}` is not installed")
            }
        }
    }
}

impl Error for ExtensionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(_, error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMarker {
    schema: u32,
    compiler_abi: u32,
    language_abi: u32,
    distribution: Option<String>,
    version: Option<String>,
    source_hash: Option<String>,
    records: Option<String>,
    records_hash: Option<String>,
    #[serde(default, rename = "extension")]
    extensions: Vec<RawExtension>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExtension {
    id: String,
    interface: String,
}
