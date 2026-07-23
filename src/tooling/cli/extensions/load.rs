use super::*;

pub(super) struct LoadedExternalInterfaces {
    pub(super) interfaces: BTreeMap<String, interface::Interface>,
    pub(super) trust_policy: crate::hir::ContractTrustPolicy,
    pub(super) records_resolver: Vec<RuntimeRecordsResolverEntry>,
}

const RUNTIME_RECORDS_RESOLVER_FORMAT_VERSION: u32 = 1;

/// The run-time record lookup contract is deliberately a small, data-only
/// manifest.  Python extensions never get to choose a path or discover other
/// manifests; every entry was validated from the lock-selected wheel and its
/// `.osri` files before this value is serialized.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(super) struct RuntimeRecordsResolver {
    #[serde(rename = "format-version")]
    format_version: u32,
    entries: Vec<RuntimeRecordsResolverEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(super) struct RuntimeRecordsResolverEntry {
    distribution: String,
    version: String,
    #[serde(rename = "interface-member-id")]
    interface_member_id: String,
    #[serde(rename = "semantic-interface-hash")]
    semantic_interface_hash: String,
    #[serde(rename = "records-path")]
    records_path: String,
    #[serde(rename = "records-hash")]
    records_hash: String,
}

#[derive(Clone, Debug)]
pub(super) struct ValidatedExternalRecords {
    path: PathBuf,
    hash: String,
    bytes: Vec<u8>,
    sidecar: records::RecordSidecar,
}

pub(super) fn load_external_interfaces(
    context: &CompileContext,
    site_roots: &[&str],
) -> Result<LoadedExternalInterfaces, String> {
    let Some(project) = &context.project else {
        let trust_policy = dependency::contract_trust_policy(&[], &[])
            .map_err(|error| format!("could not construct contract trust policy: {error}"))?;
        return Ok(LoadedExternalInterfaces {
            interfaces: BTreeMap::new(),
            trust_policy,
            records_resolver: Vec::new(),
        });
    };
    let mut roots = site_roots.iter().map(PathBuf::from).collect::<Vec<_>>();
    roots.extend(project.installed_package_roots());
    roots.sort();
    roots.dedup();
    if roots.is_empty() {
        let trust_policy = dependency::contract_trust_policy(&[], &[])
            .map_err(|error| format!("could not validate contract trust policy: {error}"))?;
        return Ok(LoadedExternalInterfaces {
            interfaces: BTreeMap::new(),
            trust_policy,
            records_resolver: Vec::new(),
        });
    }
    let lock = project
        .load_lock()
        .map_err(|error| format!("could not validate uv.lock: {error}"))?;
    let graph = dependency::resolve_effective_extensions(project, &lock, &roots)
        .map_err(|error| format!("could not resolve static extensions: {error}"))?;
    let reachable_distributions = graph
        .reachable_distributions
        .iter()
        .map(|distribution| distribution.name.clone())
        .collect::<Vec<_>>();
    // `dependency::resolve_effective_extensions` retains only explicitly
    // enabled extension IDs.  A distribution-level records sidecar, however,
    // covers every interface in that wheel.  Discover the same lock-reachable
    // distributions once more so sidecar reconstruction includes disabled
    // (but still published) interfaces as well.
    let discovered = extension::discover_reachable_all(&roots, &reachable_distributions)
        .map_err(|error| format!("could not discover static extension interfaces: {error}"))?;
    let all_distributions = discovered
        .distributions
        .into_iter()
        .map(|distribution| {
            (
                (
                    distribution.metadata.normalized_name.clone(),
                    distribution.metadata.version.clone(),
                ),
                distribution,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let trust_policy = graph.trust_policy.clone();
    let mut interfaces = BTreeMap::<String, interface::Interface>::new();
    let mut hashes = BTreeMap::<String, String>::new();
    let mut records_resolver = Vec::new();
    for distribution in graph.extensions {
        let external_records = validate_external_records(&distribution)?;
        let all_distribution = all_distributions
            .get(&(
                distribution.normalized_distribution.clone(),
                distribution.version.clone(),
            ))
            .ok_or_else(|| {
                format!(
                    "could not match discovered interfaces for distribution '{}' version '{}'",
                    distribution.distribution, distribution.version
                )
            })?;
        let all_distribution_interfaces = read_extension_interfaces(all_distribution)?;
        if external_records.is_none()
            && all_distribution_interfaces
                .iter()
                .any(|(_, model)| !model.owned_records.is_empty())
        {
            return Err(format!(
                "distribution '{}' publishes static records but has no records sidecar",
                distribution.distribution
            ));
        }
        let mut distribution_interfaces = Vec::with_capacity(distribution.extensions.len());
        for extension in &distribution.extensions {
            let text = fs::read_to_string(&extension.interface).map_err(|error| {
                format!(
                    "could not read extension interface '{}': {error}",
                    extension.interface.display()
                )
            })?;
            let model = interface::read(&text).map_err(|error| {
                format!(
                    "invalid extension interface '{}': {error}",
                    extension.interface.display()
                )
            })?;
            if model.module != extension.module
                || model.semantic_interface_hash() != extension.semantic_interface_hash
            {
                return Err(format!(
                    "extension interface '{}' changed after dependency validation (interface-member-id or semantic interface hash mismatch)",
                    extension.interface.display()
                ));
            }
            if !model.owned_records.is_empty() && external_records.is_none() {
                return Err(format!(
                    "extension interface '{}' publishes static records but distribution '{}' has no records sidecar",
                    extension.interface.display(),
                    distribution.distribution
                ));
            }
            if let Some(sidecar) = &external_records
                && !sidecar
                    .sidecar
                    .interface_semantic_hashes
                    .iter()
                    .any(|hash| hash == model.semantic_interface_hash())
            {
                return Err(format!(
                    "records sidecar for distribution '{}' does not name semantic interface hash '{}'",
                    distribution.distribution,
                    model.semantic_interface_hash()
                ));
            }
            distribution_interfaces.push((extension.clone(), model));
        }
        if let Some(sidecar) = &external_records {
            validate_external_records_against_interfaces(
                &distribution,
                &all_distribution_interfaces,
                sidecar,
            )?;
            let records_path = path_to_utf8(&sidecar.path, "records sidecar")?;
            for (extension, model) in &distribution_interfaces {
                records_resolver.push(RuntimeRecordsResolverEntry {
                    distribution: normalize_distribution_name(&distribution.distribution),
                    version: distribution.version.clone(),
                    interface_member_id: extension.module.clone(),
                    semantic_interface_hash: model.semantic_interface_hash().to_owned(),
                    records_path: records_path.clone(),
                    records_hash: sidecar.hash.clone(),
                });
            }
        }
        for (_extension, model) in distribution_interfaces {
            if let Some(previous) = hashes.get(&model.module) {
                if previous != model.semantic_interface_hash() {
                    return Err(format!(
                        "module '{}' resolves to multiple semantic interface hashes",
                        model.module
                    ));
                }
                continue;
            }
            hashes.insert(
                model.module.clone(),
                model.semantic_interface_hash().to_owned(),
            );
            interfaces.insert(model.module.clone(), model);
        }
    }
    records_resolver.sort_by(|left, right| {
        (
            &left.distribution,
            &left.version,
            &left.interface_member_id,
            &left.semantic_interface_hash,
        )
            .cmp(&(
                &right.distribution,
                &right.version,
                &right.interface_member_id,
                &right.semantic_interface_hash,
            ))
    });
    Ok(LoadedExternalInterfaces {
        interfaces,
        trust_policy,
        records_resolver,
    })
}
