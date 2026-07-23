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

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
