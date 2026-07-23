use super::*;

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

pub(super) fn effective_reachable_from(
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
    Ok(sha256(&bytes))
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

pub(super) fn ensure_unique_interfaces(
    items: &[SemanticInterfaceHash],
) -> Result<(), DependencyError> {
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

pub(super) fn validate_marker_pin(
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
