use super::*;

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
    /// Local project roots are the only packages allowed to omit this.
    pub source_hash: Option<String>,
    pub source_hashes: Vec<String>,
    pub dependencies: Vec<LockedDependency>,
    pub resolution_markers: Vec<String>,
    pub editable: bool,
    pub project_root: bool,
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

/// Internal representation for compiler-verifiable contract authorities.
/// Project configuration cannot construct this value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrustContract {
    pub distribution: String,
    pub semantic_interface_hash: String,
    pub ids: Vec<String>,
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
