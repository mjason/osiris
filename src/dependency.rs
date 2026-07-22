//! Deterministic, read-only dependency projection for Osiris projects.
//!
//! uv remains the resolver and installer. This module only validates and
//! projects `uv.lock`, extension markers, and `.osri` hashes. It performs no
//! network access and never imports Python.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::{
    extension::{self, ExtensionDistribution, ExtensionError},
    hir::{ContractTrustPolicy, InterfaceTrustPolicy},
    interface,
    project::{ProjectConfig, TrustContract},
    types::PythonVersion,
};

pub const UV_LOCK_FORMAT_VERSION: u64 = 1;
pub const TRUST_POLICY_HASH_VERSION: &str = "osiris-trust-policy-v1";

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct LockedDependency {
    pub name: String,
    pub normalized_name: String,
    pub version: Option<String>,
    pub marker: Option<String>,
    pub extras: Vec<String>,
}

impl LockedDependency {
    pub fn applies(&self, target: PythonVersion) -> Result<bool, DependencyError> {
        marker_applies(self.marker.as_deref().unwrap_or_default(), target)
            .map_err(DependencyError::UnsupportedMarker)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct LockedDistribution {
    pub name: String,
    pub normalized_name: String,
    pub version: String,
    pub source: String,
    /// Editable project roots are the only packages allowed to omit this.
    pub source_hash: Option<String>,
    pub source_hashes: Vec<String>,
    pub dependencies: Vec<LockedDependency>,
    pub resolution_markers: Vec<String>,
    pub editable: bool,
}

impl LockedDistribution {
    pub fn applicable(&self, target: PythonVersion) -> Result<bool, DependencyError> {
        if self.resolution_markers.is_empty() {
            return Ok(true);
        }
        for marker in &self.resolution_markers {
            if marker_applies(marker, target).map_err(DependencyError::UnsupportedMarker)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn dependencies_for_target(
        &self,
        target: PythonVersion,
    ) -> Result<Vec<&LockedDependency>, DependencyError> {
        self.dependencies
            .iter()
            .filter_map(|dependency| match dependency.applies(target) {
                Ok(true) => Some(Ok(dependency)),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            })
            .collect()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UvLock {
    pub format_version: u64,
    pub revision: Option<u64>,
    pub requires_python: Option<String>,
    pub target_python: PythonVersion,
    /// Only target-applicable package candidates are retained.
    pub packages: BTreeMap<String, LockedDistribution>,
    pub project: Option<String>,
}

impl UvLock {
    pub fn load(path: &Path, target_python: PythonVersion) -> Result<Self, DependencyError> {
        let source = fs::read_to_string(path)
            .map_err(|error| DependencyError::Io(path.to_path_buf(), error))?;
        Self::parse_at(&source, target_python, path)
    }

    pub fn parse(source: &str, target_python: PythonVersion) -> Result<Self, DependencyError> {
        Self::parse_at(source, target_python, Path::new("<uv.lock>"))
    }

    fn parse_at(
        source: &str,
        target_python: PythonVersion,
        path: &Path,
    ) -> Result<Self, DependencyError> {
        let document = source
            .parse::<toml::Table>()
            .map_err(|error| DependencyError::Toml(path.to_path_buf(), error.to_string()))?;
        parse_lock_table(&document, target_python, path)
    }

    #[must_use]
    pub fn package(&self, name: &str) -> Option<&LockedDistribution> {
        self.packages.get(&normalize_name(name))
    }

    pub fn reachable_from(&self, roots: &[String]) -> Result<Vec<String>, DependencyError> {
        let mut pending = roots
            .iter()
            .map(|root| normalize_name(root))
            .collect::<BTreeSet<_>>();
        let mut visited = BTreeSet::new();
        while let Some(name) = pending.pop_first() {
            if !visited.insert(name.clone()) {
                continue;
            }
            let package = self
                .packages
                .get(&name)
                .ok_or_else(|| DependencyError::MissingPackage(name.clone()))?;
            for dependency in package.dependencies_for_target(self.target_python)? {
                let target = self
                    .packages
                    .get(&dependency.normalized_name)
                    .ok_or_else(|| DependencyError::MissingDependency {
                        from: package.normalized_name.clone(),
                        to: dependency.normalized_name.clone(),
                    })?;
                if let Some(specifier) = dependency.version.as_deref() {
                    if !satisfies_specifier(specifier, &target.version)
                        .map_err(DependencyError::InvalidVersion)?
                    {
                        return Err(DependencyError::UnsatisfiedDependency {
                            from: package.normalized_name.clone(),
                            to: dependency.normalized_name.clone(),
                            requirement: specifier.to_owned(),
                            locked: target.version.clone(),
                        });
                    }
                }
                pending.insert(target.normalized_name.clone());
            }
        }
        Ok(visited.into_iter().collect())
    }

    #[must_use]
    pub fn project_package(&self, name: &str) -> Option<&LockedDistribution> {
        let normalized = normalize_name(name);
        if self
            .project
            .as_deref()
            .is_some_and(|project| project != normalized)
        {
            return None;
        }
        self.packages.get(&normalized)
    }

    pub fn validate_project(&self, project: &ProjectConfig) -> Result<(), DependencyError> {
        let root = self
            .project_package(&project.distribution)
            .ok_or_else(|| DependencyError::MissingProjectRoot(project.distribution.clone()))?;
        if !project.distribution_version.is_empty()
            && project.distribution_version != "0"
            && root.version != project.distribution_version
            && !root.editable
        {
            return Err(DependencyError::ProjectVersionMismatch {
                expected: project.distribution_version.clone(),
                locked: root.version.clone(),
            });
        }
        for raw in &project.dependencies {
            let requirement =
                parse_requirement(raw).map_err(DependencyError::InvalidRequirement)?;
            if let Some(marker) = requirement.marker.as_deref() {
                if !marker_applies(marker, self.target_python)
                    .map_err(DependencyError::UnsupportedMarker)?
                {
                    continue;
                }
            }
            let edge = root
                .dependencies
                .iter()
                .find(|edge| edge.normalized_name == requirement.normalized_name)
                .ok_or_else(|| DependencyError::MissingDependency {
                    from: root.normalized_name.clone(),
                    to: requirement.normalized_name.clone(),
                })?;
            let locked = self.packages.get(&edge.normalized_name).ok_or_else(|| {
                DependencyError::MissingDependency {
                    from: root.normalized_name.clone(),
                    to: edge.normalized_name.clone(),
                }
            })?;
            if let Some(specifier) = requirement.specifier.as_deref() {
                if !satisfies_specifier(specifier, &locked.version)
                    .map_err(DependencyError::InvalidVersion)?
                {
                    return Err(DependencyError::UnsatisfiedDependency {
                        from: root.normalized_name.clone(),
                        to: locked.normalized_name.clone(),
                        requirement: specifier.to_owned(),
                        locked: locked.version.clone(),
                    });
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SemanticInterfaceHash {
    pub distribution: String,
    pub version: String,
    pub interface_member_id: String,
    pub semantic_interface_hash: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EffectiveDependencyEdge {
    pub from: String,
    pub to: String,
    pub version: Option<String>,
    pub marker: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedExtension {
    pub id: String,
    pub interface: PathBuf,
    pub module: String,
    pub semantic_interface_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedExtensionDistribution {
    pub distribution: String,
    pub normalized_distribution: String,
    pub version: String,
    pub source_hash: Option<String>,
    pub site_root: PathBuf,
    pub dist_info: PathBuf,
    pub extensions: Vec<ResolvedExtension>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectiveExtensionGraph {
    pub target_python: PythonVersion,
    pub reachable_distributions: Vec<LockedDistribution>,
    pub edges: Vec<EffectiveDependencyEdge>,
    pub extensions: Vec<ResolvedExtensionDistribution>,
    pub semantic_interface_hashes: Vec<SemanticInterfaceHash>,
    pub trust_policy_hash: String,
    pub trust_policy: ContractTrustPolicy,
}

pub fn resolve_effective_extensions(
    project: &ProjectConfig,
    lock: &UvLock,
    site_roots: &[PathBuf],
) -> Result<EffectiveExtensionGraph, DependencyError> {
    if lock.target_python != project.target_python {
        return Err(DependencyError::TargetMismatch {
            project: project.target_python,
            lock: lock.target_python,
        });
    }
    lock.validate_project(project)?;
    let locked_names = lock.packages.keys().cloned().collect::<Vec<_>>();
    let discovered = extension::discover_reachable(site_roots, &project.extensions, &locked_names)
        .map_err(DependencyError::Extension)?;
    for distribution in &discovered.distributions {
        let pin = lock
            .package(&distribution.metadata.normalized_name)
            .ok_or_else(|| {
                DependencyError::MissingPackage(distribution.metadata.normalized_name.clone())
            })?;
        validate_marker_pin(distribution, pin)?;
    }
    let mut roots = vec![project.distribution.clone()];
    roots.extend(
        discovered
            .distributions
            .iter()
            .map(|distribution| distribution.metadata.normalized_name.clone()),
    );
    roots.sort();
    roots.dedup();
    let reachable_names = effective_reachable_from(lock, project, &roots)?;
    let reachable = reachable_names
        .iter()
        .filter_map(|name| lock.packages.get(name).cloned())
        .collect::<Vec<_>>();

    let requested = project.extensions.iter().cloned().collect::<BTreeSet<_>>();
    let mut resolved = Vec::new();
    let mut semantic_hashes = Vec::new();
    for distribution in discovered.distributions {
        let normalized = normalize_name(&distribution.metadata.name);
        if !reachable_names.contains(&normalized) {
            return Err(DependencyError::UnreachableExtension {
                distribution: distribution.metadata.name,
            });
        }
        let pin = lock
            .package(&normalized)
            .ok_or_else(|| DependencyError::MissingPackage(normalized.clone()))?;
        let mut extensions = Vec::new();
        for resource in distribution.extensions {
            if !requested.contains(&resource.id) {
                continue;
            }
            let source = fs::read_to_string(&resource.interface)
                .map_err(|error| DependencyError::Io(resource.interface.clone(), error))?;
            let parsed = interface::read(&source).map_err(|error| DependencyError::Interface {
                path: resource.interface.clone(),
                message: error.to_string(),
            })?;
            let semantic_interface_hash = parsed.semantic_interface_hash().to_owned();
            validate_hash(&semantic_interface_hash).map_err(DependencyError::InvalidHash)?;
            semantic_hashes.push(SemanticInterfaceHash {
                distribution: distribution.metadata.name.clone(),
                version: distribution.metadata.version.clone(),
                interface_member_id: parsed.module.clone(),
                semantic_interface_hash: semantic_interface_hash.clone(),
            });
            extensions.push(ResolvedExtension {
                id: resource.id,
                interface: resource.interface,
                module: parsed.module,
                semantic_interface_hash,
            });
        }
        extensions.sort_by(|left, right| left.id.cmp(&right.id));
        resolved.push(ResolvedExtensionDistribution {
            distribution: distribution.metadata.name,
            normalized_distribution: normalized,
            version: distribution.metadata.version,
            source_hash: pin.source_hash.clone(),
            site_root: distribution.site_root,
            dist_info: distribution.dist_info,
            extensions,
        });
    }
    resolved.sort_by(|left, right| {
        (&left.normalized_distribution, &left.version)
            .cmp(&(&right.normalized_distribution, &right.version))
    });
    semantic_hashes.sort();
    ensure_unique_interfaces(&semantic_hashes)?;
    semantic_hashes.dedup();

    let mut edges = Vec::new();
    for package in &reachable {
        for dependency in package.dependencies_for_target(lock.target_python)? {
            edges.push(EffectiveDependencyEdge {
                from: package.normalized_name.clone(),
                to: dependency.normalized_name.clone(),
                version: dependency.version.clone(),
                marker: dependency.marker.clone(),
            });
        }
    }
    edges.sort();
    let trust_policy = contract_trust_policy(&project.trust_contracts, &semantic_hashes)?;
    Ok(EffectiveExtensionGraph {
        target_python: lock.target_python,
        reachable_distributions: reachable,
        edges,
        extensions: resolved,
        trust_policy_hash: trust_policy.hash.clone(),
        trust_policy,
        semantic_interface_hashes: semantic_hashes,
    })
}

fn effective_reachable_from(
    lock: &UvLock,
    project: &ProjectConfig,
    roots: &[String],
) -> Result<Vec<String>, DependencyError> {
    let project_name = normalize_name(&project.distribution);
    let mut runtime_dependencies = BTreeSet::new();
    for raw in &project.dependencies {
        let requirement = parse_requirement(raw).map_err(DependencyError::InvalidRequirement)?;
        let applies = requirement.marker.as_deref().map_or(Ok(true), |marker| {
            marker_applies(marker, lock.target_python).map_err(DependencyError::UnsupportedMarker)
        })?;
        if applies {
            runtime_dependencies.insert(requirement.normalized_name);
        }
    }

    let mut pending = roots
        .iter()
        .map(|root| normalize_name(root))
        .collect::<BTreeSet<_>>();
    let mut visited = BTreeSet::new();
    while let Some(name) = pending.pop_first() {
        if !visited.insert(name.clone()) {
            continue;
        }
        let package = lock
            .packages
            .get(&name)
            .ok_or_else(|| DependencyError::MissingPackage(name.clone()))?;
        for dependency in package.dependencies_for_target(lock.target_python)? {
            if package.normalized_name == project_name
                && !runtime_dependencies.contains(&dependency.normalized_name)
            {
                continue;
            }
            let target = lock
                .packages
                .get(&dependency.normalized_name)
                .ok_or_else(|| DependencyError::MissingDependency {
                    from: package.normalized_name.clone(),
                    to: dependency.normalized_name.clone(),
                })?;
            if let Some(specifier) = dependency.version.as_deref() {
                if !satisfies_specifier(specifier, &target.version)
                    .map_err(DependencyError::InvalidVersion)?
                {
                    return Err(DependencyError::UnsatisfiedDependency {
                        from: package.normalized_name.clone(),
                        to: target.normalized_name.clone(),
                        requirement: specifier.to_owned(),
                        locked: target.version.clone(),
                    });
                }
            }
            pending.insert(target.normalized_name.clone());
        }
    }
    Ok(visited.into_iter().collect())
}

pub fn trust_policy_hash(
    contracts: &[TrustContract],
    resolved: &[SemanticInterfaceHash],
) -> Result<String, DependencyError> {
    let mut normalized_contracts = BTreeMap::<(String, String), BTreeSet<String>>::new();
    for contract in contracts {
        if !extension::is_valid_distribution_name(&contract.distribution) {
            return Err(DependencyError::Trust(format!(
                "invalid trust distribution `{}`",
                contract.distribution
            )));
        }
        let distribution = normalize_name(&contract.distribution);
        if distribution.is_empty() {
            return Err(DependencyError::Trust(
                "empty trust distribution".to_owned(),
            ));
        }
        validate_hash(&contract.semantic_interface_hash).map_err(DependencyError::InvalidHash)?;
        if contract.ids.is_empty()
            || contract.ids.iter().any(|id| {
                id.is_empty()
                    || id
                        .chars()
                        .any(|character| character.is_control() || character.is_whitespace())
            })
        {
            return Err(DependencyError::Trust(format!(
                "trust contract for `{distribution}` has invalid ids"
            )));
        }
        normalized_contracts
            .entry((distribution, contract.semantic_interface_hash.clone()))
            .or_default()
            .extend(contract.ids.iter().cloned());
    }

    let mut interfaces = resolved.to_vec();
    for item in &mut interfaces {
        if !extension::is_valid_distribution_name(&item.distribution) {
            return Err(DependencyError::Trust(format!(
                "invalid resolved distribution `{}`",
                item.distribution
            )));
        }
        item.distribution = normalize_name(&item.distribution);
        item.semantic_interface_hash = item.semantic_interface_hash.to_ascii_lowercase();
        if item.distribution.is_empty()
            || item.version.is_empty()
            || item.interface_member_id.is_empty()
        {
            return Err(DependencyError::Trust(
                "resolved interface hash has an empty identity field".to_owned(),
            ));
        }
        validate_hash(&item.semantic_interface_hash).map_err(DependencyError::InvalidHash)?;
    }
    interfaces.sort();
    ensure_unique_interfaces(&interfaces)?;
    interfaces.dedup();
    for (distribution, hash) in normalized_contracts.keys() {
        if !interfaces.iter().any(|interface| {
            &interface.distribution == distribution && &interface.semantic_interface_hash == hash
        }) {
            return Err(DependencyError::Trust(format!(
                "trust contract `{distribution}` references an unresolved semantic interface hash `{hash}`"
            )));
        }
    }

    let mut bytes = Vec::new();
    push_field(&mut bytes, TRUST_POLICY_HASH_VERSION);
    push_field(&mut bytes, interface::COMPILER_ABI);
    push_field(&mut bytes, interface::LANGUAGE_ABI);
    for ((distribution, hash), ids) in normalized_contracts {
        push_field(&mut bytes, "contract");
        push_field(&mut bytes, &distribution);
        push_field(&mut bytes, &hash);
        for id in ids {
            push_field(&mut bytes, &id);
        }
        push_field(&mut bytes, "end-contract");
    }
    for item in interfaces {
        push_field(&mut bytes, "interface");
        push_field(&mut bytes, &item.distribution);
        push_field(&mut bytes, &item.version);
        push_field(&mut bytes, &item.interface_member_id);
        push_field(&mut bytes, &item.semantic_interface_hash);
    }
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

pub fn contract_trust_policy(
    contracts: &[TrustContract],
    resolved: &[SemanticInterfaceHash],
) -> Result<ContractTrustPolicy, DependencyError> {
    let hash = trust_policy_hash(contracts, resolved)?;
    let mut interfaces = BTreeMap::new();
    for item in resolved {
        let distribution = normalize_name(&item.distribution);
        let semantic_interface_hash = item.semantic_interface_hash.to_ascii_lowercase();
        let trusted_contract_ids = contracts
            .iter()
            .filter(|contract| {
                normalize_name(&contract.distribution) == distribution
                    && contract.semantic_interface_hash.to_ascii_lowercase()
                        == semantic_interface_hash
            })
            .flat_map(|contract| contract.ids.iter().cloned())
            .collect::<BTreeSet<_>>();
        let policy = InterfaceTrustPolicy {
            distribution,
            semantic_interface_hash,
            trusted_contract_ids,
        };
        if let Some(previous) = interfaces.insert(item.interface_member_id.clone(), policy.clone())
            && previous != policy
        {
            return Err(DependencyError::Trust(format!(
                "module `{}` has conflicting resolved trust provenance",
                item.interface_member_id
            )));
        }
    }
    Ok(ContractTrustPolicy { hash, interfaces })
}

fn ensure_unique_interfaces(items: &[SemanticInterfaceHash]) -> Result<(), DependencyError> {
    let mut by_member = BTreeMap::<&str, &SemanticInterfaceHash>::new();
    for item in items {
        if let Some(previous) = by_member.insert(&item.interface_member_id, item) {
            if previous != item {
                return Err(DependencyError::InterfaceConflict {
                    interface_member_id: item.interface_member_id.clone(),
                    first_distribution: previous.distribution.clone(),
                    second_distribution: item.distribution.clone(),
                });
            }
        }
    }
    Ok(())
}

fn validate_marker_pin(
    distribution: &ExtensionDistribution,
    pin: &LockedDistribution,
) -> Result<(), DependencyError> {
    if distribution.metadata.normalized_name != pin.normalized_name {
        return Err(DependencyError::MarkerDistributionMismatch {
            marker: distribution.metadata.normalized_name.clone(),
            locked: pin.normalized_name.clone(),
        });
    }
    if distribution.metadata.version != pin.version {
        return Err(DependencyError::MarkerVersionMismatch {
            distribution: distribution.metadata.name.clone(),
            marker: distribution.metadata.version.clone(),
            locked: pin.version.clone(),
        });
    }
    if let Some(expected) = distribution.marker_source_hash() {
        if !pin
            .source_hashes
            .iter()
            .any(|hash| hash.eq_ignore_ascii_case(expected))
        {
            return Err(DependencyError::MarkerSourceHashMismatch {
                distribution: distribution.metadata.name.clone(),
                marker: expected.to_owned(),
                locked: pin.source_hash.clone(),
            });
        }
    }
    Ok(())
}

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

fn parse_lock_table(
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

fn same_pin(left: &LockedDistribution, right: &LockedDistribution) -> bool {
    left.normalized_name == right.normalized_name
        && left.version == right.version
        && left.source == right.source
        && left.source_hashes == right.source_hashes
        && left.dependencies == right.dependencies
        && left.editable == right.editable
}

fn parse_package(
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

fn canonical_source_hash(source: &str, hashes: &[String]) -> Option<String> {
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
            Some(format!("sha256:{:x}", Sha256::digest(bytes)))
        }
    }
}

fn required_string(table: &toml::Table, key: &str, path: &Path) -> Result<String, DependencyError> {
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

fn source_descriptor(source: Option<&toml::Table>, editable: bool) -> String {
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

fn source_hashes(table: &toml::Table, path: &Path) -> Result<Vec<String>, DependencyError> {
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

fn parse_markers(
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

fn parse_dependencies(
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

#[derive(Clone, Debug)]
struct Requirement {
    name: String,
    normalized_name: String,
    specifier: Option<String>,
    marker: Option<String>,
    extras: Vec<String>,
}

fn parse_requirement(value: &str) -> Result<Requirement, String> {
    let text = value.trim();
    if text.is_empty() {
        return Err("dependency requirement is empty".to_owned());
    }
    let (left, marker) = split_once_unquoted(text, ';');
    let left = left.trim();
    let marker = marker
        .map(str::trim)
        .filter(|marker| !marker.is_empty())
        .map(str::to_owned);
    if marker
        .as_deref()
        .is_some_and(|marker| marker.to_ascii_lowercase().contains("extra"))
    {
        return Err("dependency marker using `extra` is not statically resolvable".to_owned());
    }

    let bytes = left.as_bytes();
    let mut end = 0;
    while end < bytes.len()
        && (bytes[end].is_ascii_alphanumeric() || matches!(bytes[end], b'.' | b'_' | b'-'))
    {
        end += 1;
    }
    if end == 0 {
        return Err(format!("invalid dependency requirement `{value}`"));
    }
    let name = left[..end].to_owned();
    if !extension::is_valid_distribution_name(&name) {
        return Err(format!("invalid Python distribution name `{name}`"));
    }
    let mut remainder = left[end..].trim();
    let mut extras = Vec::new();
    if remainder.starts_with('[') {
        let close = remainder
            .find(']')
            .ok_or_else(|| format!("invalid extras in `{value}`"))?;
        extras = remainder[1..close]
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_owned)
            .collect();
        extras.sort();
        extras.dedup();
        remainder = remainder[close + 1..].trim();
    }
    if remainder.starts_with('@') || remainder.contains("://") {
        return Err(format!(
            "direct URL dependency is not represented by uv lock: `{value}`"
        ));
    }
    let specifier = (!remainder.is_empty()).then(|| remainder.to_owned());
    if let Some(specifier) = &specifier {
        parse_specifier(specifier)?;
    }
    if let Some(marker) = &marker {
        marker_applies(marker, PythonVersion::PYTHON_3_9)?;
    }
    Ok(Requirement {
        name: name.clone(),
        normalized_name: normalize_name(&name),
        specifier,
        marker,
        extras,
    })
}

fn split_once_unquoted(value: &str, delimiter: char) -> (&str, Option<&str>) {
    let mut quote = None;
    for (index, character) in value.char_indices() {
        if quote == Some(character) {
            quote = None;
        } else if quote.is_none() && matches!(character, '\'' | '"') {
            quote = Some(character);
        } else if quote.is_none() && character == delimiter {
            return (
                &value[..index],
                Some(&value[index + character.len_utf8()..]),
            );
        }
    }
    (value, None)
}

fn parse_specifier(value: &str) -> Result<(), String> {
    // A version stored on a uv dependency edge is exact even though project
    // requirements use PEP 440 comparator syntax.
    if !value.trim_start().starts_with(['=', '!', '~', '>', '<']) {
        version_key(value)?;
        return Ok(());
    }
    for clause in value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        let (operator, expected) = split_specifier_clause(clause)
            .ok_or_else(|| format!("unsupported version specifier `{value}`"))?;
        if expected.is_empty() {
            return Err(format!("version specifier has no value: `{value}`"));
        }
        if expected != "*" {
            let numeric = expected.strip_suffix(".*").unwrap_or(expected);
            version_key(numeric)?;
        }
        if expected.ends_with(".*") && !matches!(operator, "==" | "!=") {
            return Err(format!("unsupported wildcard version specifier `{value}`"));
        }
    }
    Ok(())
}

fn satisfies_specifier(specifier: &str, version: &str) -> Result<bool, String> {
    let specifier = specifier.trim();
    if specifier.is_empty() {
        return Ok(true);
    }
    let actual = version_key(version)?;
    if !specifier.starts_with(['=', '!', '~', '>', '<']) {
        return Ok(compare_versions(&actual, &version_key(specifier)?) == std::cmp::Ordering::Equal);
    }
    for clause in specifier
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        let (operator, expected_text) = split_specifier_clause(clause)
            .ok_or_else(|| format!("unsupported version specifier `{specifier}`"))?;
        if expected_text == "*" {
            continue;
        }
        if let Some(prefix) = expected_text.strip_suffix(".*") {
            let expected = version_key(prefix)?;
            let matches = actual.iter().take(expected.len()).eq(expected.iter());
            if (operator == "==" && !matches) || (operator == "!=" && matches) {
                return Ok(false);
            }
            continue;
        }
        let expected = version_key(expected_text)?;
        let compare = compare_versions(&actual, &expected);
        let applies = match operator {
            "==" | "===" => compare == std::cmp::Ordering::Equal,
            "!=" => compare != std::cmp::Ordering::Equal,
            ">=" => compare != std::cmp::Ordering::Less,
            "<=" => compare != std::cmp::Ordering::Greater,
            ">" => compare == std::cmp::Ordering::Greater,
            "<" => compare == std::cmp::Ordering::Less,
            "~=" => {
                let prefix_len = expected.len().saturating_sub(1).max(1);
                compare != std::cmp::Ordering::Less
                    && actual
                        .iter()
                        .take(prefix_len)
                        .eq(expected.iter().take(prefix_len))
            }
            _ => return Err(format!("unsupported version specifier `{specifier}`")),
        };
        if !applies {
            return Ok(false);
        }
    }
    Ok(true)
}

fn split_specifier_clause(value: &str) -> Option<(&str, &str)> {
    ["===", "==", "!=", "~=", ">=", "<=", ">", "<"]
        .into_iter()
        .find_map(|operator| {
            value
                .strip_prefix(operator)
                .map(|expected| (operator, expected.trim()))
        })
}

fn version_key(value: &str) -> Result<Vec<u64>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("empty version".to_owned());
    }
    let mut result = Vec::new();
    for component in value.split('.') {
        let digits = component
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>();
        if digits.is_empty() {
            return Err(format!("non-numeric version `{value}`"));
        }
        result.push(
            digits
                .parse::<u64>()
                .map_err(|_| format!("version overflow `{value}`"))?,
        );
    }
    Ok(result)
}

fn compare_versions(left: &[u64], right: &[u64]) -> std::cmp::Ordering {
    let length = left.len().max(right.len());
    (0..length)
        .map(|index| {
            (
                left.get(index).copied().unwrap_or(0),
                right.get(index).copied().unwrap_or(0),
            )
        })
        .find_map(|(left, right)| (left != right).then_some(left.cmp(&right)))
        .unwrap_or(std::cmp::Ordering::Equal)
}

fn marker_applies(marker: &str, target: PythonVersion) -> Result<bool, String> {
    let marker = strip_outer_parentheses(marker.trim())?;
    if marker.is_empty() {
        return Ok(true);
    }
    let alternatives = split_top_level(&marker, "or")?;
    if alternatives.len() > 1 {
        for alternative in alternatives {
            if marker_applies(&alternative, target)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    let conjunction = split_top_level(&marker, "and")?;
    if conjunction.len() > 1 {
        for clause in conjunction {
            if !marker_applies(&clause, target)? {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    let (left, operator, right) = marker_atom(&marker)?;
    let actual = match left.as_str() {
        "python_version" => format!("{}.{}", target.major, target.minor),
        "python_full_version" => format!("{}.{}.0", target.major, target.minor),
        other => return Err(format!("marker variable `{other}` is not supported")),
    };
    let right = unquote_marker_value(&right)?;
    if matches!(operator.as_str(), "in" | "not in") {
        let found = right.split_whitespace().any(|item| item == actual);
        return Ok(if operator == "in" { found } else { !found });
    }
    let left_version = version_key(&actual)?;
    let right_version = version_key(&right)?;
    let ordering = compare_versions(&left_version, &right_version);
    Ok(match operator.as_str() {
        "==" => ordering == std::cmp::Ordering::Equal,
        "!=" => ordering != std::cmp::Ordering::Equal,
        ">=" => ordering != std::cmp::Ordering::Less,
        "<=" => ordering != std::cmp::Ordering::Greater,
        ">" => ordering == std::cmp::Ordering::Greater,
        "<" => ordering == std::cmp::Ordering::Less,
        other => return Err(format!("marker operator `{other}` is not supported")),
    })
}

fn strip_outer_parentheses(value: &str) -> Result<String, String> {
    let mut result = value.trim().to_owned();
    loop {
        if !result.starts_with('(') || !result.ends_with(')') {
            return Ok(result);
        }
        let mut depth = 0i32;
        let mut quote = None;
        let mut encloses = true;
        for (index, character) in result.char_indices() {
            if quote == Some(character) {
                quote = None;
                continue;
            }
            if quote.is_none() && matches!(character, '\'' | '"') {
                quote = Some(character);
                continue;
            }
            if quote.is_some() {
                continue;
            }
            match character {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 && index != result.len() - 1 {
                        encloses = false;
                        break;
                    }
                    if depth < 0 {
                        return Err(format!("unbalanced marker `{value}`"));
                    }
                }
                _ => {}
            }
        }
        if quote.is_some() || depth != 0 {
            return Err(format!("unbalanced marker `{value}`"));
        }
        if encloses {
            result = result[1..result.len() - 1].trim().to_owned();
        } else {
            return Ok(result);
        }
    }
}

fn split_top_level(value: &str, operator: &str) -> Result<Vec<String>, String> {
    let needle = format!(" {operator} ");
    let mut result = Vec::new();
    let mut start = 0;
    let mut depth = 0i32;
    let mut quote = None;
    let mut index = 0;
    while index < value.len() {
        let character = value[index..]
            .chars()
            .next()
            .expect("index remains on a character boundary");
        if quote == Some(character) {
            quote = None;
            index += character.len_utf8();
            continue;
        }
        if quote.is_none() && matches!(character, '\'' | '"') {
            quote = Some(character);
            index += character.len_utf8();
            continue;
        }
        if quote.is_none() {
            match character {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth < 0 {
                        return Err(format!("unbalanced marker `{value}`"));
                    }
                }
                _ => {}
            }
            if depth == 0 && value[index..].starts_with(&needle) {
                result.push(value[start..index].trim().to_owned());
                index += needle.len();
                start = index;
                continue;
            }
        }
        index += character.len_utf8();
    }
    if quote.is_some() || depth != 0 {
        return Err(format!("unbalanced marker `{value}`"));
    }
    result.push(value[start..].trim().to_owned());
    Ok(result)
}

fn marker_atom(value: &str) -> Result<(String, String, String), String> {
    let operators = [" not in ", " in ", "==", "!=", ">=", "<=", ">", "<"];
    let mut quote = None;
    let mut depth = 0i32;
    let mut index = 0;
    while index < value.len() {
        let character = value[index..]
            .chars()
            .next()
            .expect("index remains on a character boundary");
        if quote == Some(character) {
            quote = None;
            index += character.len_utf8();
            continue;
        }
        if quote.is_none() && matches!(character, '\'' | '"') {
            quote = Some(character);
            index += character.len_utf8();
            continue;
        }
        if quote.is_none() && character == '(' {
            depth += 1;
        } else if quote.is_none() && character == ')' {
            depth -= 1;
        }
        if quote.is_none() && depth == 0 {
            for operator in operators {
                if value[index..].starts_with(operator) {
                    let left = value[..index].trim().to_ascii_lowercase();
                    let right = value[index + operator.len()..].trim().to_owned();
                    if left.is_empty() || right.is_empty() {
                        return Err(format!("invalid marker atom `{value}`"));
                    }
                    return Ok((left, operator.trim().to_owned(), right));
                }
            }
        }
        index += character.len_utf8();
    }
    Err(format!("unsupported marker atom `{value}`"))
}

fn unquote_marker_value(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.len() >= 2 {
        let first = value.as_bytes()[0] as char;
        let last = value.as_bytes()[value.len() - 1] as char;
        if matches!(first, '\'' | '"') {
            if last != first {
                return Err(format!("unterminated marker string `{value}`"));
            }
            return Ok(value[1..value.len() - 1].to_owned());
        }
    }
    Ok(value.to_owned())
}

fn normalize_name(value: &str) -> String {
    extension::normalize_distribution_name(value)
}

fn validate_hash(value: &str) -> Result<(), String> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(format!("`{value}` must use the `sha256:` prefix"));
    };
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("`{value}` must contain 64 hexadecimal digits"));
    }
    Ok(())
}

fn push_field(output: &mut Vec<u8>, value: &str) {
    output.extend_from_slice(value.len().to_string().as_bytes());
    output.push(b':');
    output.extend_from_slice(value.as_bytes());
    output.push(b'\n');
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use super::{
        DependencyError, SemanticInterfaceHash, UvLock, contract_trust_policy, marker_applies,
        resolve_effective_extensions, trust_policy_hash,
    };
    use crate::{
        compiler::{self, CompileOptions},
        project::{ProjectConfig, TrustContract},
        types::PythonVersion,
    };

    const HASH_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const HASH_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

    fn lock(package_marker: &str) -> String {
        format!(
            r#"version = 1
revision = 3
requires-python = ">=3.9"

[[package]]
name = "demo"
source = {{ editable = "." }}
dependencies = [{{ name = "numpy", version = "2.1.0" }}]

[[package]]
name = "numpy"
version = "2.1.0"
source = {{ registry = "https://pypi.org/simple" }}
sdist = {{ hash = "{HASH_A}" }}
resolution-markers = ["{package_marker}"]
"#
        )
    }

    #[test]
    fn parses_target_applicable_lock_hash_and_edges() {
        let parsed = UvLock::parse(&lock("python_version >= '3.11'"), PythonVersion::new(3, 11))
            .expect("lock should parse");
        let package = parsed.package("NumPy").expect("numpy pin should exist");
        assert_eq!(package.version, "2.1.0");
        assert_eq!(package.source_hash.as_deref(), Some(HASH_A));
        let reachable = parsed
            .reachable_from(&["demo".to_owned()])
            .expect("root closure should resolve");
        assert_eq!(reachable, ["demo", "numpy"]);
    }

    #[test]
    fn target_inapplicable_pin_cannot_satisfy_an_edge() {
        let parsed = UvLock::parse(&lock("python_version >= '3.12'"), PythonVersion::new(3, 11))
            .expect("non-applicable candidates are omitted");
        let error = parsed
            .reachable_from(&["demo".to_owned()])
            .expect_err("root edge must fail closed");
        assert!(matches!(error, DependencyError::MissingDependency { .. }));
    }

    #[test]
    fn rejects_non_hashed_registry_distribution() {
        let source = lock("python_version >= '3.11'")
            .replace(&format!("sdist = {{ hash = \"{HASH_A}\" }}\n"), "");
        let error = UvLock::parse(&source, PythonVersion::new(3, 11))
            .expect_err("registry packages require a source hash");
        assert!(matches!(error, DependencyError::InvalidLock(_, _)));
    }

    #[test]
    fn trust_hash_is_order_independent() {
        let first = TrustContract {
            distribution: "Demo.Ext".to_owned(),
            semantic_interface_hash: HASH_B.to_owned(),
            ids: vec!["z".to_owned(), "a".to_owned()],
        };
        let second = TrustContract {
            distribution: "demo-ext".to_owned(),
            semantic_interface_hash: HASH_B.to_owned(),
            ids: vec!["a".to_owned(), "z".to_owned()],
        };
        let resolved = vec![SemanticInterfaceHash {
            distribution: "demo-ext".to_owned(),
            version: "1.0.0".to_owned(),
            interface_member_id: "demo.core".to_owned(),
            semantic_interface_hash: HASH_B.to_owned(),
        }];
        assert_eq!(
            trust_policy_hash(&[first], &resolved).unwrap(),
            trust_policy_hash(&[second], &resolved).unwrap()
        );
        let split = vec![
            TrustContract {
                distribution: "demo-ext".to_owned(),
                semantic_interface_hash: HASH_B.to_owned(),
                ids: vec!["z".to_owned()],
            },
            TrustContract {
                distribution: "Demo.Ext".to_owned(),
                semantic_interface_hash: HASH_B.to_owned(),
                ids: vec!["a".to_owned()],
            },
        ];
        assert_eq!(
            trust_policy_hash(&split, &resolved).unwrap(),
            trust_policy_hash(
                &[TrustContract {
                    distribution: "demo-ext".to_owned(),
                    semantic_interface_hash: HASH_B.to_owned(),
                    ids: vec!["a".to_owned(), "z".to_owned()],
                }],
                &resolved
            )
            .unwrap()
        );
    }

    #[test]
    fn stale_trust_hash_fails_closed() {
        let contract = TrustContract {
            distribution: "demo-ext".to_owned(),
            semantic_interface_hash: HASH_A.to_owned(),
            ids: vec!["sample.contract".to_owned()],
        };
        let resolved = SemanticInterfaceHash {
            distribution: "demo-ext".to_owned(),
            version: "1.0".to_owned(),
            interface_member_id: "sample.core".to_owned(),
            semantic_interface_hash: HASH_B.to_owned(),
        };
        let error = trust_policy_hash(&[contract], &[resolved]).unwrap_err();
        assert!(matches!(error, DependencyError::Trust(_)));
    }

    #[test]
    fn contract_trust_policy_keeps_exact_resolved_provenance() {
        let resolved = vec![SemanticInterfaceHash {
            distribution: "Sample_Ext".to_owned(),
            version: "1.0".to_owned(),
            interface_member_id: "sample.core".to_owned(),
            semantic_interface_hash: HASH_A.to_owned(),
        }];
        let policy = contract_trust_policy(
            &[TrustContract {
                distribution: "sample-ext".to_owned(),
                semantic_interface_hash: HASH_A.to_owned(),
                ids: vec!["sample.contract".to_owned()],
            }],
            &resolved,
        )
        .expect("policy");
        let interface = &policy.interfaces["sample.core"];
        assert_eq!(interface.distribution, "sample-ext");
        assert_eq!(interface.semantic_interface_hash, HASH_A);
        assert!(interface.trusted_contract_ids.contains("sample.contract"));

        let untrusted = contract_trust_policy(&[], &resolved).expect("untrusted policy");
        assert!(
            untrusted.interfaces["sample.core"]
                .trusted_contract_ids
                .is_empty()
        );
        assert_ne!(policy.hash, untrusted.hash);
    }

    #[test]
    fn marker_parser_handles_boolean_python_constraints() {
        assert!(
            marker_applies(
                "python_version >= '3.11' and python_version < '3.13'",
                PythonVersion::new(3, 12)
            )
            .unwrap()
        );
        assert!(!marker_applies("python_version < '3.11'", PythonVersion::new(3, 11)).unwrap());
    }

    #[test]
    fn resolves_locked_marker_and_static_interface() {
        let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "osiris-effective-dependency-{}-{id}",
            std::process::id()
        ));
        let site_root = root.join("site");
        let dist_info = site_root.join("sample_ext-1.0.dist-info");
        let package = site_root.join("sample_ext");
        fs::create_dir_all(&dist_info).unwrap();
        fs::create_dir_all(&package).unwrap();

        let options = CompileOptions::new("sample.core", PythonVersion::new(3, 9))
            .with_provider("sample-ext", "1.0");
        let compiled = compiler::compile(
            "(module sample.core) (def answer Int 42) (export [answer])",
            &options,
        );
        assert!(
            compiled.analysis.diagnostics.is_empty(),
            "{:?}",
            compiled.analysis.diagnostics
        );
        let interface = compiled.interface.expect("interface should be emitted");
        let parsed = crate::interface::read(&interface).unwrap();
        fs::write(package.join("sample.osri"), interface).unwrap();
        fs::write(
            dist_info.join("METADATA"),
            "Metadata-Version: 2.4\nName: Sample.Ext\nVersion: 1.0\n\n",
        )
        .unwrap();
        fs::write(
            dist_info.join("osiris.toml"),
            format!(
                "schema = 1\ncompiler_abi = 1\nlanguage_abi = 2\nsource_hash = \"{HASH_A}\"\n\n[[extension]]\nid = \"sample\"\ninterface = \"sample_ext/sample.osri\"\n"
            ),
        )
        .unwrap();
        fs::write(
            root.join("pyproject.toml"),
            format!(
                r#"[project]
name = "demo"
version = "1.0"

[tool.osiris]
extensions = ["sample"]

[[tool.osiris.trust.contract]]
distribution = "sample-ext"
semantic-interface-hash = "{}"
ids = ["sample.contract"]
"#,
                parsed.semantic_interface_hash()
            ),
        )
        .unwrap();
        fs::write(
            root.join("uv.lock"),
            format!(
                r#"version = 1

[[package]]
name = "demo"
source = {{ editable = "." }}
dependencies = [{{ name = "builder", version = "4.0" }}]

[[package]]
name = "sample-ext"
version = "1.0"
source = {{ registry = "https://pypi.org/simple", hash = "{HASH_A}" }}

[[package]]
name = "builder"
version = "4.0"
source = {{ registry = "https://pypi.org/simple", hash = "{HASH_B}" }}
"#
            ),
        )
        .unwrap();

        let config = ProjectConfig::load(&root.join("pyproject.toml")).unwrap();
        let lock = config.load_lock().unwrap();
        let graph = resolve_effective_extensions(&config, &lock, &[site_root]).unwrap();
        assert_eq!(graph.extensions.len(), 1);
        assert_eq!(graph.extensions[0].normalized_distribution, "sample-ext");
        assert!(
            graph
                .reachable_distributions
                .iter()
                .any(|distribution| distribution.normalized_name == "sample-ext")
        );
        assert!(
            graph
                .reachable_distributions
                .iter()
                .all(|distribution| distribution.normalized_name != "builder")
        );
        assert_eq!(graph.semantic_interface_hashes.len(), 1);
        assert_eq!(
            graph.semantic_interface_hashes[0].semantic_interface_hash,
            parsed.semantic_interface_hash()
        );
        assert_ne!(
            graph.semantic_interface_hashes[0].semantic_interface_hash, parsed.hashes.semantic_body,
            "published dependency identity must use the graph hash, not the local body hash"
        );
        assert!(graph.trust_policy_hash.starts_with("sha256:"));
        fs::remove_dir_all(root).unwrap();
    }
}
