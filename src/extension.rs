//! Static discovery of Osiris interfaces installed in Python distributions.
//!
//! This module deliberately does not import Python packages or resolve
//! dependencies. Package installation and version solving remain uv's job;
//! the compiler only validates wheel metadata and declared static resources.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt, fs, io,
    path::{Component, Path, PathBuf},
};

use serde::Deserialize;
use sha2::{Digest, Sha256};

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

/// Discovers and validates explicitly enabled extensions in site-package roots.
pub fn discover(
    site_roots: &[PathBuf],
    enabled: &[String],
) -> Result<ExtensionGraph, ExtensionError> {
    discover_filtered(site_roots, enabled, None)
}

/// Discovers enabled extensions only from lock-reachable distributions. The
/// allowlist is applied to `.dist-info` names before marker contents are read,
/// so an unrelated installed wheel cannot affect compilation.
pub fn discover_reachable(
    site_roots: &[PathBuf],
    enabled: &[String],
    reachable_distributions: &[String],
) -> Result<ExtensionGraph, ExtensionError> {
    let reachable = reachable_distributions
        .iter()
        .map(|name| normalize_distribution_name(name))
        .collect::<BTreeSet<_>>();
    discover_filtered(site_roots, enabled, Some(&reachable))
}

fn discover_filtered(
    site_roots: &[PathBuf],
    enabled: &[String],
    reachable_distributions: Option<&BTreeSet<String>>,
) -> Result<ExtensionGraph, ExtensionError> {
    let enabled = enabled.iter().cloned().collect::<BTreeSet<_>>();
    if enabled.is_empty() {
        return Ok(ExtensionGraph::default());
    }
    let mut candidates = Vec::new();
    for root in site_roots {
        let entries =
            fs::read_dir(root).map_err(|error| ExtensionError::Io(root.clone(), error))?;
        for entry in entries {
            let entry = entry.map_err(|error| ExtensionError::Io(root.clone(), error))?;
            let path = entry.path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".dist-info"))
                && path.is_dir()
                && path.join("osiris.toml").is_file()
            {
                if reachable_distributions.is_some_and(|reachable| {
                    dist_info_distribution_name(&path).is_none_or(|name| !reachable.contains(&name))
                }) {
                    continue;
                }
                candidates.push((root.clone(), path));
            }
        }
    }
    candidates.sort();

    let mut distributions = Vec::new();
    for (site_root, dist_info) in candidates {
        let distribution = load_distribution(&site_root, &dist_info)?;
        if distribution
            .extensions
            .iter()
            .any(|extension| enabled.contains(&extension.id))
        {
            distributions.push(distribution);
        }
    }
    distributions.sort_by(|left, right| {
        (&left.metadata.normalized_name, &left.metadata.version)
            .cmp(&(&right.metadata.normalized_name, &right.metadata.version))
    });

    let mut by_id = BTreeMap::new();
    for (distribution_index, distribution) in distributions.iter().enumerate() {
        for (extension_index, extension) in distribution.extensions.iter().enumerate() {
            if !enabled.contains(&extension.id) {
                continue;
            }
            if let Some((first_distribution, _)) =
                by_id.insert(extension.id.clone(), (distribution_index, extension_index))
            {
                return Err(ExtensionError::DuplicateId {
                    id: extension.id.clone(),
                    first: distributions[first_distribution].metadata.name.clone(),
                    second: distribution.metadata.name.clone(),
                });
            }
        }
    }
    for id in enabled {
        if !by_id.contains_key(&id) {
            return Err(ExtensionError::MissingEnabled(id));
        }
    }

    Ok(ExtensionGraph {
        distributions,
        by_id,
    })
}

fn dist_info_distribution_name(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?.strip_suffix(".dist-info")?;
    let (distribution, _) = name.rsplit_once('-')?;
    Some(normalize_distribution_name(distribution))
}

fn load_distribution(
    site_root: &Path,
    dist_info: &Path,
) -> Result<ExtensionDistribution, ExtensionError> {
    let marker_path = dist_info.join("osiris.toml");
    let marker_text = fs::read_to_string(&marker_path)
        .map_err(|error| ExtensionError::Io(marker_path.clone(), error))?;
    let marker: RawMarker = toml::from_str(&marker_text)
        .map_err(|error| ExtensionError::InvalidMarker(marker_path.clone(), error.to_string()))?;
    validate_abi(&marker_path, &marker)?;
    if marker.extensions.is_empty() {
        return Err(ExtensionError::InvalidMarker(
            marker_path,
            "marker must contain at least one [[extension]]".to_owned(),
        ));
    }
    if marker.records.is_some() != marker.records_hash.is_some() {
        return Err(ExtensionError::InvalidMarker(
            marker_path,
            "records and records_hash must be declared together".to_owned(),
        ));
    }

    let metadata = read_metadata(&dist_info.join("METADATA"))?;
    if let Some(distribution) = marker.distribution.as_deref() {
        if normalize_distribution_name(distribution) != metadata.normalized_name {
            return Err(ExtensionError::InvalidMarker(
                marker_path.clone(),
                format!(
                    "distribution `{distribution}` does not match METADATA `{}`",
                    metadata.name
                ),
            ));
        }
    }
    if let Some(version) = marker.version.as_deref() {
        if version != metadata.version {
            return Err(ExtensionError::InvalidMarker(
                marker_path.clone(),
                format!(
                    "version `{version}` does not match METADATA `{}`",
                    metadata.version
                ),
            ));
        }
    }
    if let Some(source_hash) = marker.source_hash.as_deref() {
        validate_sha256(source_hash)
            .map_err(|message| ExtensionError::InvalidMarker(marker_path.clone(), message))?;
    }
    let mut ids = BTreeSet::new();
    let mut extensions = Vec::new();
    for extension in marker.extensions {
        validate_extension_id(&extension.id).map_err(|message| {
            ExtensionError::InvalidMarker(dist_info.join("osiris.toml"), message)
        })?;
        if !ids.insert(extension.id.clone()) {
            return Err(ExtensionError::InvalidMarker(
                dist_info.join("osiris.toml"),
                format!("duplicate extension id `{}`", extension.id),
            ));
        }
        let interface = resolve_resource(site_root, &extension.interface)?;
        extensions.push(ExtensionResource {
            id: extension.id,
            interface,
        });
    }
    extensions.sort_by(|left, right| left.id.cmp(&right.id));

    let records = marker
        .records
        .as_deref()
        .map(|path| resolve_resource(site_root, path))
        .transpose()?;
    if let (Some(path), Some(expected)) = (&records, &marker.records_hash) {
        validate_sha256(expected).map_err(|message| {
            ExtensionError::InvalidMarker(dist_info.join("osiris.toml"), message)
        })?;
        let bytes = fs::read(path).map_err(|error| ExtensionError::Io(path.clone(), error))?;
        let actual = sha256(&bytes);
        if &actual != expected {
            return Err(ExtensionError::HashMismatch {
                path: path.clone(),
                expected: expected.clone(),
                actual,
            });
        }
    }

    Ok(ExtensionDistribution {
        metadata,
        site_root: site_root.to_path_buf(),
        dist_info: dist_info.to_path_buf(),
        extensions,
        records,
        records_hash: marker.records_hash,
        marker_distribution: marker.distribution,
        marker_version: marker.version,
        marker_source_hash: marker.source_hash,
    })
}

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

fn validate_extension_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 128
        || id.chars().any(char::is_whitespace)
        || id.chars().any(char::is_control)
    {
        Err(format!("invalid extension id `{id}`"))
    } else {
        Ok(())
    }
}

fn validate_sha256(value: &str) -> Result<(), String> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err("hash must use the `sha256:` prefix".to_owned());
    };
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("hash must contain 64 hexadecimal digits".to_owned());
    }
    Ok(())
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

#[must_use]
pub fn normalize_distribution_name(name: &str) -> String {
    let mut normalized = String::new();
    let mut separator = false;
    for character in name.chars() {
        if matches!(character, '-' | '_' | '.') {
            separator = true;
        } else {
            if separator && !normalized.is_empty() {
                normalized.push('-');
            }
            normalized.extend(character.to_lowercase());
            separator = false;
        }
    }
    normalized
}

#[must_use]
pub fn is_valid_distribution_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use super::{
        ExtensionError, discover, discover_reachable, normalize_distribution_name, sha256,
    };

    static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

    fn fixture(marker: &str, records: Option<&[u8]>) -> std::path::PathBuf {
        let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("osiris-extension-{}-{id}", std::process::id()));
        let dist = root.join("sample_ext-1.0.dist-info");
        fs::create_dir_all(root.join("sample_ext")).unwrap();
        fs::create_dir_all(&dist).unwrap();
        fs::write(
            dist.join("METADATA"),
            "Metadata-Version: 2.4\nName: Sample.Ext\nVersion: 1.0\nRequires-Dist: numpy>=2\n\n",
        )
        .unwrap();
        fs::write(dist.join("osiris.toml"), marker).unwrap();
        fs::write(root.join("sample_ext/sample.osri"), "(osiris-interface)\n").unwrap();
        if let Some(records) = records {
            fs::write(root.join("sample_ext/sample.records.json"), records).unwrap();
        }
        root
    }

    fn marker(records: Option<&[u8]>) -> String {
        let records_section = records.map_or_else(String::new, |bytes| {
            format!(
                "records = \"sample_ext/sample.records.json\"\nrecords_hash = \"{}\"\n",
                sha256(bytes)
            )
        });
        format!(
            "schema = 1\ncompiler_abi = 1\nlanguage_abi = 2\n{records_section}\n[[extension]]\nid = \"sample\"\ninterface = \"sample_ext/sample.osri\"\n"
        )
    }

    #[test]
    fn discovers_only_enabled_static_extensions() {
        let records = b"{}";
        let root = fixture(&marker(Some(records)), Some(records));
        let graph = discover(std::slice::from_ref(&root), &["sample".to_owned()]).unwrap();
        let (distribution, extension) = graph.extension("sample").unwrap();
        assert_eq!(distribution.metadata.normalized_name, "sample-ext");
        assert_eq!(distribution.metadata.requires_dist, ["numpy>=2"]);
        assert!(extension.interface.ends_with("sample_ext/sample.osri"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_resource_paths_that_escape_the_wheel_root() {
        let root = fixture(
            &marker(None).replace("sample_ext/sample.osri", "../outside.osri"),
            None,
        );
        let error = discover(std::slice::from_ref(&root), &["sample".to_owned()]).unwrap_err();
        assert!(matches!(error, ExtensionError::ResourceEscape(_)));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_tampered_records() {
        let root = fixture(&marker(Some(b"expected")), Some(b"changed"));
        let error = discover(std::slice::from_ref(&root), &["sample".to_owned()]).unwrap_err();
        assert!(matches!(error, ExtensionError::HashMismatch { .. }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_marker_identity_that_disagrees_with_metadata() {
        let explicit = marker(None).replacen(
            "schema = 1\n",
            "schema = 1\ndistribution = \"different\"\nversion = \"1.0\"\n",
            1,
        );
        let root = fixture(&explicit, None);
        let error = discover(std::slice::from_ref(&root), &["sample".to_owned()]).unwrap_err();
        assert!(matches!(error, ExtensionError::InvalidMarker(_, _)));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reachable_discovery_ignores_unrelated_installed_markers() {
        let root = fixture(&marker(None), None);
        let unrelated = root.join("unrelated-9.0.dist-info");
        fs::create_dir_all(&unrelated).unwrap();
        fs::write(unrelated.join("osiris.toml"), "not valid TOML = [").unwrap();
        let graph = discover_reachable(
            std::slice::from_ref(&root),
            &["sample".to_owned()],
            &["sample-ext".to_owned()],
        )
        .unwrap();
        assert!(graph.extension("sample").is_some());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn normalizes_distribution_names_like_python_metadata() {
        assert_eq!(
            normalize_distribution_name("My_Package.Name"),
            "my-package-name"
        );
    }
}
