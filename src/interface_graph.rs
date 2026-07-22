//! Deterministic SCC-level hashes for compilation-interface graphs.
//!
//! Interface body hashes are local to one module. This module lifts them to
//! graph identities that also cover runtime/phase-1 edges and transitive
//! interface dependencies without introducing a self-referential file hash.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    interface::{COMPILER_ABI, Interface, LANGUAGE_ABI},
    module_graph::{EdgeKind, ModuleEdge},
};

const SEMANTIC_GROUP_HASH_VERSION: &str = "osiris-semantic-interface-group-v1";
const TOOLING_GROUP_HASH_VERSION: &str = "osiris-tooling-metadata-group-v1";

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct InterfaceBodyHashes {
    pub semantic_body: String,
    pub tooling_body: String,
}

impl InterfaceBodyHashes {
    #[must_use]
    pub fn from_interface(interface: &Interface) -> Self {
        Self {
            semantic_body: interface.hashes.semantic_body.clone(),
            tooling_body: interface.hashes.tooling_body.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct PublishedInterfaceHashes {
    pub semantic_interface: String,
    pub tooling_metadata: String,
}

impl PublishedInterfaceHashes {
    #[must_use]
    pub fn legacy_body_hashes(interface: &Interface) -> Self {
        Self {
            semantic_interface: interface.hashes.semantic_body.clone(),
            tooling_metadata: interface.hashes.tooling_body.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct InterfaceHashEdge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
}

impl From<&ModuleEdge> for InterfaceHashEdge {
    fn from(edge: &ModuleEdge) -> Self {
        Self {
            from: edge.from.clone(),
            to: edge.to.clone(),
            kind: edge.kind,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ResolvedHashDependency {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
    pub semantic_interface_hash: String,
    pub tooling_metadata_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InterfaceHashMember {
    pub module: String,
    pub semantic_body_hash: String,
    pub tooling_body_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InterfaceHashGroup {
    pub id: String,
    pub members: Vec<InterfaceHashMember>,
    pub internal_edges: Vec<InterfaceHashEdge>,
    pub external_dependencies: Vec<ResolvedHashDependency>,
    pub semantic_interface_hash: String,
    pub tooling_metadata_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MemberInterfaceHashes {
    pub group: String,
    pub semantic_interface_hash: String,
    pub tooling_metadata_hash: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct InterfaceGraphHashes {
    pub groups: Vec<InterfaceHashGroup>,
    pub members: BTreeMap<String, MemberInterfaceHashes>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InterfaceGraphHashError {
    EmptyModule,
    DuplicateProvider(String),
    UnknownImporter(String),
    MissingDependency { from: String, to: String },
    InvalidHash { owner: String, value: String },
    InvalidGroup(String),
    ComponentCycle(String),
}

impl fmt::Display for InterfaceGraphHashError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyModule => formatter.write_str("interface graph contains an empty module id"),
            Self::DuplicateProvider(module) => write!(
                formatter,
                "interface `{module}` is present as both a local and external provider"
            ),
            Self::UnknownImporter(module) => {
                write!(formatter, "interface edge has unknown importer `{module}`")
            }
            Self::MissingDependency { from, to } => {
                write!(formatter, "interface `{from}` depends on missing `{to}`")
            }
            Self::InvalidHash { owner, value } => {
                write!(formatter, "interface `{owner}` has invalid hash `{value}`")
            }
            Self::InvalidGroup(message) => {
                write!(formatter, "invalid interface hash group: {message}")
            }
            Self::ComponentCycle(component) => write!(
                formatter,
                "condensed interface graph unexpectedly cycles at `{component}`"
            ),
        }
    }
}

impl Error for InterfaceGraphHashError {}

pub fn verify_interface_hash_group(
    group: &InterfaceHashGroup,
) -> Result<(), InterfaceGraphHashError> {
    if group.members.is_empty() {
        return Err(InterfaceGraphHashError::InvalidGroup(
            "group has no members".to_owned(),
        ));
    }
    let mut members = group.members.clone();
    members.sort_by(|left, right| left.module.cmp(&right.module));
    if members != group.members
        || members
            .windows(2)
            .any(|pair| pair[0].module == pair[1].module)
    {
        return Err(InterfaceGraphHashError::InvalidGroup(
            "members must be unique and sorted by module".to_owned(),
        ));
    }
    if group.id != members[0].module {
        return Err(InterfaceGraphHashError::InvalidGroup(
            "group id must equal its first module".to_owned(),
        ));
    }
    let member_names = members
        .iter()
        .map(|member| member.module.as_str())
        .collect::<BTreeSet<_>>();
    for member in &members {
        validate_hash(&member.module, &member.semantic_body_hash)?;
        validate_hash(&member.module, &member.tooling_body_hash)?;
    }

    let mut internal_edges = group.internal_edges.clone();
    internal_edges.sort();
    internal_edges.dedup();
    if internal_edges != group.internal_edges
        || internal_edges.iter().any(|edge| {
            !member_names.contains(edge.from.as_str()) || !member_names.contains(edge.to.as_str())
        })
    {
        return Err(InterfaceGraphHashError::InvalidGroup(
            "internal edges must be unique, sorted, and remain within the group".to_owned(),
        ));
    }

    let mut dependencies = group.external_dependencies.clone();
    dependencies.sort_by(|left, right| {
        (&left.from, left.kind, &left.to).cmp(&(&right.from, right.kind, &right.to))
    });
    dependencies.dedup();
    if dependencies != group.external_dependencies
        || dependencies.iter().any(|dependency| {
            !member_names.contains(dependency.from.as_str())
                || member_names.contains(dependency.to.as_str())
        })
    {
        return Err(InterfaceGraphHashError::InvalidGroup(
            "external dependencies must be unique, sorted, and leave the group".to_owned(),
        ));
    }
    let mut targets = BTreeMap::<&str, (&str, &str)>::new();
    for dependency in &dependencies {
        validate_hash(&dependency.to, &dependency.semantic_interface_hash)?;
        validate_hash(&dependency.to, &dependency.tooling_metadata_hash)?;
        let hashes = (
            dependency.semantic_interface_hash.as_str(),
            dependency.tooling_metadata_hash.as_str(),
        );
        if targets
            .insert(dependency.to.as_str(), hashes)
            .is_some_and(|previous| previous != hashes)
        {
            return Err(InterfaceGraphHashError::InvalidGroup(format!(
                "dependency `{}` resolves to inconsistent hashes",
                dependency.to
            )));
        }
    }

    validate_hash(&group.id, &group.semantic_interface_hash)?;
    validate_hash(&group.id, &group.tooling_metadata_hash)?;
    let semantic = semantic_group_hash(&members, &internal_edges, &dependencies);
    if semantic != group.semantic_interface_hash {
        return Err(InterfaceGraphHashError::InvalidGroup(
            "semantic interface hash does not match the group body".to_owned(),
        ));
    }
    let tooling = tooling_group_hash(&members, &semantic, &dependencies);
    if tooling != group.tooling_metadata_hash {
        return Err(InterfaceGraphHashError::InvalidGroup(
            "tooling metadata hash does not match the group body".to_owned(),
        ));
    }
    Ok(())
}

pub fn calculate_interface_graph_hashes(
    local: &BTreeMap<String, InterfaceBodyHashes>,
    edges: impl IntoIterator<Item = InterfaceHashEdge>,
    external: &BTreeMap<String, PublishedInterfaceHashes>,
) -> Result<InterfaceGraphHashes, InterfaceGraphHashError> {
    validate_inputs(local, external)?;
    let edges = normalize_edges(local, external, edges)?;
    let components = strongly_connected_components(local.keys(), &edges);
    let component_by_member = components
        .iter()
        .enumerate()
        .flat_map(|(index, members)| members.iter().cloned().map(move |member| (member, index)))
        .collect::<BTreeMap<_, _>>();

    let mut computed = BTreeMap::<usize, InterfaceHashGroup>::new();
    let mut visiting = BTreeSet::new();
    for component in 0..components.len() {
        calculate_component(
            component,
            &components,
            &component_by_member,
            local,
            &edges,
            external,
            &mut visiting,
            &mut computed,
        )?;
    }

    let mut groups = computed.into_values().collect::<Vec<_>>();
    groups.sort_by(|left, right| left.id.cmp(&right.id));
    let mut members = BTreeMap::new();
    for group in &groups {
        for member in &group.members {
            members.insert(
                member.module.clone(),
                MemberInterfaceHashes {
                    group: group.id.clone(),
                    semantic_interface_hash: group.semantic_interface_hash.clone(),
                    tooling_metadata_hash: group.tooling_metadata_hash.clone(),
                },
            );
        }
    }
    Ok(InterfaceGraphHashes { groups, members })
}

#[allow(clippy::too_many_arguments)]
fn calculate_component(
    component: usize,
    components: &[Vec<String>],
    component_by_member: &BTreeMap<String, usize>,
    local: &BTreeMap<String, InterfaceBodyHashes>,
    edges: &[InterfaceHashEdge],
    external: &BTreeMap<String, PublishedInterfaceHashes>,
    visiting: &mut BTreeSet<usize>,
    computed: &mut BTreeMap<usize, InterfaceHashGroup>,
) -> Result<(), InterfaceGraphHashError> {
    if computed.contains_key(&component) {
        return Ok(());
    }
    if !visiting.insert(component) {
        return Err(InterfaceGraphHashError::ComponentCycle(
            components[component][0].clone(),
        ));
    }

    let member_names = &components[component];
    let member_set = member_names
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut internal_edges = Vec::new();
    let mut dependency_edges = Vec::new();
    for edge in edges
        .iter()
        .filter(|edge| member_set.contains(edge.from.as_str()))
    {
        if member_set.contains(edge.to.as_str()) {
            internal_edges.push(edge.clone());
        } else {
            dependency_edges.push(edge.clone());
        }
    }

    let mut dependencies = Vec::new();
    for edge in dependency_edges {
        let hashes = if let Some(target_component) = component_by_member.get(&edge.to).copied() {
            calculate_component(
                target_component,
                components,
                component_by_member,
                local,
                edges,
                external,
                visiting,
                computed,
            )?;
            let target = computed
                .get(&target_component)
                .expect("dependency component was calculated");
            PublishedInterfaceHashes {
                semantic_interface: target.semantic_interface_hash.clone(),
                tooling_metadata: target.tooling_metadata_hash.clone(),
            }
        } else {
            external.get(&edge.to).cloned().ok_or_else(|| {
                InterfaceGraphHashError::MissingDependency {
                    from: edge.from.clone(),
                    to: edge.to.clone(),
                }
            })?
        };
        dependencies.push(ResolvedHashDependency {
            from: edge.from,
            to: edge.to,
            kind: edge.kind,
            semantic_interface_hash: hashes.semantic_interface,
            tooling_metadata_hash: hashes.tooling_metadata,
        });
    }
    dependencies.sort_by(|left, right| {
        (&left.from, left.kind, &left.to).cmp(&(&right.from, right.kind, &right.to))
    });
    internal_edges.sort();

    let members = member_names
        .iter()
        .map(|member| InterfaceHashMember {
            module: member.clone(),
            semantic_body_hash: local[member].semantic_body.clone(),
            tooling_body_hash: local[member].tooling_body.clone(),
        })
        .collect::<Vec<_>>();
    let semantic_interface_hash = semantic_group_hash(&members, &internal_edges, &dependencies);
    let tooling_metadata_hash =
        tooling_group_hash(&members, &semantic_interface_hash, &dependencies);
    let id = member_names
        .first()
        .expect("strongly connected components are non-empty")
        .clone();
    computed.insert(
        component,
        InterfaceHashGroup {
            id,
            members: members.clone(),
            internal_edges,
            external_dependencies: dependencies,
            semantic_interface_hash,
            tooling_metadata_hash,
        },
    );
    visiting.remove(&component);
    Ok(())
}

fn semantic_group_hash(
    members: &[InterfaceHashMember],
    internal_edges: &[InterfaceHashEdge],
    dependencies: &[ResolvedHashDependency],
) -> String {
    let mut bytes = Vec::new();
    push_field(&mut bytes, SEMANTIC_GROUP_HASH_VERSION);
    push_field(&mut bytes, COMPILER_ABI);
    push_field(&mut bytes, LANGUAGE_ABI);
    for member in members {
        push_field(&mut bytes, "member");
        push_field(&mut bytes, &member.module);
        push_field(&mut bytes, &member.semantic_body_hash);
    }
    for edge in internal_edges {
        push_edge(&mut bytes, "internal-edge", edge);
    }
    for dependency in dependencies {
        push_field(&mut bytes, "external-edge");
        push_field(&mut bytes, &dependency.from);
        push_field(&mut bytes, edge_kind_name(dependency.kind));
        push_field(&mut bytes, &dependency.to);
        push_field(&mut bytes, &dependency.semantic_interface_hash);
    }
    sha256(&bytes)
}

fn tooling_group_hash(
    members: &[InterfaceHashMember],
    semantic_interface_hash: &str,
    dependencies: &[ResolvedHashDependency],
) -> String {
    let mut bytes = Vec::new();
    push_field(&mut bytes, TOOLING_GROUP_HASH_VERSION);
    push_field(&mut bytes, COMPILER_ABI);
    push_field(&mut bytes, LANGUAGE_ABI);
    push_field(&mut bytes, semantic_interface_hash);
    for member in members {
        push_field(&mut bytes, "member");
        push_field(&mut bytes, &member.module);
        push_field(&mut bytes, &member.tooling_body_hash);
    }
    for dependency in dependencies {
        push_field(&mut bytes, "external-edge");
        push_field(&mut bytes, &dependency.from);
        push_field(&mut bytes, edge_kind_name(dependency.kind));
        push_field(&mut bytes, &dependency.to);
        push_field(&mut bytes, &dependency.tooling_metadata_hash);
    }
    sha256(&bytes)
}

fn validate_inputs(
    local: &BTreeMap<String, InterfaceBodyHashes>,
    external: &BTreeMap<String, PublishedInterfaceHashes>,
) -> Result<(), InterfaceGraphHashError> {
    for (module, hashes) in local {
        if module.is_empty() {
            return Err(InterfaceGraphHashError::EmptyModule);
        }
        if external.contains_key(module) {
            return Err(InterfaceGraphHashError::DuplicateProvider(module.clone()));
        }
        validate_hash(module, &hashes.semantic_body)?;
        validate_hash(module, &hashes.tooling_body)?;
    }
    for (module, hashes) in external {
        if module.is_empty() {
            return Err(InterfaceGraphHashError::EmptyModule);
        }
        validate_hash(module, &hashes.semantic_interface)?;
        validate_hash(module, &hashes.tooling_metadata)?;
    }
    Ok(())
}

fn normalize_edges(
    local: &BTreeMap<String, InterfaceBodyHashes>,
    external: &BTreeMap<String, PublishedInterfaceHashes>,
    edges: impl IntoIterator<Item = InterfaceHashEdge>,
) -> Result<Vec<InterfaceHashEdge>, InterfaceGraphHashError> {
    let mut normalized = edges.into_iter().collect::<Vec<_>>();
    normalized.sort();
    normalized.dedup();
    for edge in &normalized {
        if !local.contains_key(&edge.from) {
            return Err(InterfaceGraphHashError::UnknownImporter(edge.from.clone()));
        }
        if !local.contains_key(&edge.to) && !external.contains_key(&edge.to) {
            return Err(InterfaceGraphHashError::MissingDependency {
                from: edge.from.clone(),
                to: edge.to.clone(),
            });
        }
    }
    Ok(normalized)
}

fn strongly_connected_components<'a>(
    nodes: impl Iterator<Item = &'a String>,
    edges: &[InterfaceHashEdge],
) -> Vec<Vec<String>> {
    let nodes = nodes.cloned().collect::<BTreeSet<_>>();
    let mut adjacency = nodes
        .iter()
        .cloned()
        .map(|node| (node, BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();
    let mut reverse = adjacency.clone();
    for edge in edges {
        if nodes.contains(&edge.to) {
            adjacency
                .get_mut(&edge.from)
                .expect("local importer was validated")
                .insert(edge.to.clone());
            reverse
                .get_mut(&edge.to)
                .expect("local dependency is a graph node")
                .insert(edge.from.clone());
        }
    }

    let mut visited = BTreeSet::new();
    let mut order = Vec::new();
    for node in &nodes {
        dfs_finish(node, &adjacency, &mut visited, &mut order);
    }
    visited.clear();
    let mut components = Vec::new();
    for node in order.into_iter().rev() {
        if visited.contains(&node) {
            continue;
        }
        let mut members = Vec::new();
        dfs_collect(&node, &reverse, &mut visited, &mut members);
        members.sort();
        components.push(members);
    }
    components.sort_by(|left, right| left.first().cmp(&right.first()));
    components
}

fn dfs_finish(
    node: &str,
    adjacency: &BTreeMap<String, BTreeSet<String>>,
    visited: &mut BTreeSet<String>,
    order: &mut Vec<String>,
) {
    if !visited.insert(node.to_owned()) {
        return;
    }
    for target in adjacency.get(node).into_iter().flatten() {
        dfs_finish(target, adjacency, visited, order);
    }
    order.push(node.to_owned());
}

fn dfs_collect(
    node: &str,
    reverse: &BTreeMap<String, BTreeSet<String>>,
    visited: &mut BTreeSet<String>,
    component: &mut Vec<String>,
) {
    if !visited.insert(node.to_owned()) {
        return;
    }
    component.push(node.to_owned());
    for target in reverse.get(node).into_iter().flatten() {
        dfs_collect(target, reverse, visited, component);
    }
}

fn validate_hash(owner: &str, value: &str) -> Result<(), InterfaceGraphHashError> {
    let valid = value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()));
    if valid {
        Ok(())
    } else {
        Err(InterfaceGraphHashError::InvalidHash {
            owner: owner.to_owned(),
            value: value.to_owned(),
        })
    }
}

fn push_edge(bytes: &mut Vec<u8>, label: &str, edge: &InterfaceHashEdge) {
    push_field(bytes, label);
    push_field(bytes, &edge.from);
    push_field(bytes, edge_kind_name(edge.kind));
    push_field(bytes, &edge.to);
}

const fn edge_kind_name(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::Runtime => "runtime",
        EdgeKind::Phase1 => "phase-1",
    }
}

fn push_field(output: &mut Vec<u8>, value: &str) {
    output.extend_from_slice(value.len().to_string().as_bytes());
    output.push(b':');
    output.extend_from_slice(value.as_bytes());
    output.push(b'\n');
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        InterfaceBodyHashes, InterfaceGraphHashError, InterfaceHashEdge, PublishedInterfaceHashes,
        calculate_interface_graph_hashes, verify_interface_hash_group,
    };
    use crate::module_graph::EdgeKind;

    fn hash(character: char) -> String {
        format!("sha256:{}", character.to_string().repeat(64))
    }

    fn body(semantic: char, tooling: char) -> InterfaceBodyHashes {
        InterfaceBodyHashes {
            semantic_body: hash(semantic),
            tooling_body: hash(tooling),
        }
    }

    fn edge(from: &str, to: &str, kind: EdgeKind) -> InterfaceHashEdge {
        InterfaceHashEdge {
            from: from.to_owned(),
            to: to.to_owned(),
            kind,
        }
    }

    #[test]
    fn tooling_changes_do_not_invalidate_the_semantic_group() {
        let first = BTreeMap::from([("sample".to_owned(), body('a', 'b'))]);
        let second = BTreeMap::from([("sample".to_owned(), body('a', 'c'))]);

        let first = calculate_interface_graph_hashes(&first, [], &BTreeMap::new()).unwrap();
        let second = calculate_interface_graph_hashes(&second, [], &BTreeMap::new()).unwrap();

        assert_eq!(
            first.members["sample"].semantic_interface_hash,
            second.members["sample"].semantic_interface_hash
        );
        assert_ne!(
            first.members["sample"].tooling_metadata_hash,
            second.members["sample"].tooling_metadata_hash
        );
    }

    #[test]
    fn a_combined_runtime_phase_cycle_shares_one_order_independent_group() {
        let bodies = BTreeMap::from([
            ("alpha".to_owned(), body('a', 'b')),
            ("beta".to_owned(), body('c', 'd')),
        ]);
        let forward = vec![
            edge("alpha", "beta", EdgeKind::Runtime),
            edge("beta", "alpha", EdgeKind::Phase1),
        ];
        let reverse = forward.iter().cloned().rev().collect::<Vec<_>>();

        let first = calculate_interface_graph_hashes(&bodies, forward, &BTreeMap::new()).unwrap();
        let second = calculate_interface_graph_hashes(&bodies, reverse, &BTreeMap::new()).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.groups.len(), 1);
        assert_eq!(
            first.groups[0]
                .members
                .iter()
                .map(|member| member.module.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "beta"]
        );
        assert_eq!(
            first.members["alpha"].semantic_interface_hash,
            first.members["beta"].semantic_interface_hash
        );
    }

    #[test]
    fn edge_kind_and_transitive_dependency_hashes_are_semantic() {
        let bodies = BTreeMap::from([
            ("app".to_owned(), body('a', 'b')),
            ("dep".to_owned(), body('c', 'd')),
        ]);
        let runtime = calculate_interface_graph_hashes(
            &bodies,
            [edge("app", "dep", EdgeKind::Runtime)],
            &BTreeMap::new(),
        )
        .unwrap();
        let phase = calculate_interface_graph_hashes(
            &bodies,
            [edge("app", "dep", EdgeKind::Phase1)],
            &BTreeMap::new(),
        )
        .unwrap();

        assert_ne!(
            runtime.members["app"].semantic_interface_hash,
            phase.members["app"].semantic_interface_hash
        );
        assert_eq!(
            runtime.members["dep"].semantic_interface_hash,
            phase.members["dep"].semantic_interface_hash
        );
    }

    #[test]
    fn external_hash_changes_only_reachable_importers() {
        let bodies = BTreeMap::from([
            ("app".to_owned(), body('a', 'b')),
            ("other".to_owned(), body('c', 'd')),
        ]);
        let edges = [edge("app", "wheel.core", EdgeKind::Runtime)];
        let first_external = BTreeMap::from([(
            "wheel.core".to_owned(),
            PublishedInterfaceHashes {
                semantic_interface: hash('e'),
                tooling_metadata: hash('f'),
            },
        )]);
        let second_external = BTreeMap::from([(
            "wheel.core".to_owned(),
            PublishedInterfaceHashes {
                semantic_interface: hash('1'),
                tooling_metadata: hash('2'),
            },
        )]);

        let first =
            calculate_interface_graph_hashes(&bodies, edges.clone(), &first_external).unwrap();
        let second = calculate_interface_graph_hashes(&bodies, edges, &second_external).unwrap();

        assert_ne!(
            first.members["app"].semantic_interface_hash,
            second.members["app"].semantic_interface_hash
        );
        assert_eq!(
            first.members["other"].semantic_interface_hash,
            second.members["other"].semantic_interface_hash
        );
    }

    #[test]
    fn missing_dependencies_and_duplicate_providers_fail_closed() {
        let bodies = BTreeMap::from([("app".to_owned(), body('a', 'b'))]);
        let missing = calculate_interface_graph_hashes(
            &bodies,
            [edge("app", "missing", EdgeKind::Runtime)],
            &BTreeMap::new(),
        )
        .unwrap_err();
        assert!(matches!(
            missing,
            InterfaceGraphHashError::MissingDependency { .. }
        ));

        let external = BTreeMap::from([(
            "app".to_owned(),
            PublishedInterfaceHashes {
                semantic_interface: hash('c'),
                tooling_metadata: hash('d'),
            },
        )]);
        let duplicate = calculate_interface_graph_hashes(&bodies, [], &external).unwrap_err();
        assert_eq!(
            duplicate,
            InterfaceGraphHashError::DuplicateProvider("app".to_owned())
        );
    }

    #[test]
    fn group_envelope_is_self_verifying_and_rejects_tampering() {
        let bodies = BTreeMap::from([
            ("alpha".to_owned(), body('a', 'b')),
            ("beta".to_owned(), body('c', 'd')),
        ]);
        let graph = calculate_interface_graph_hashes(
            &bodies,
            [
                edge("alpha", "beta", EdgeKind::Runtime),
                edge("beta", "alpha", EdgeKind::Runtime),
            ],
            &BTreeMap::new(),
        )
        .unwrap();
        let mut group = graph.groups[0].clone();
        verify_interface_hash_group(&group).unwrap();

        group.members[0].semantic_body_hash = hash('e');
        assert!(verify_interface_hash_group(&group).is_err());
    }
}
