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
