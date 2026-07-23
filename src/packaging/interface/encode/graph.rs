use super::super::*;

pub(super) fn graph_form(group: &InterfaceHashGroup) -> Form {
    map(vec![
        ("group-id", string(&group.id)),
        (
            "members",
            vector(
                group
                    .members
                    .iter()
                    .map(|member| {
                        map(vec![
                            ("module", string(&member.module)),
                            ("semantic-body", string(&member.semantic_body_hash)),
                            ("tooling-body", string(&member.tooling_body_hash)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "internal-edges",
            vector(group.internal_edges.iter().map(graph_edge_form).collect()),
        ),
        (
            "external-dependencies",
            vector(
                group
                    .external_dependencies
                    .iter()
                    .map(|dependency| {
                        map(vec![
                            ("from", string(&dependency.from)),
                            ("to", string(&dependency.to)),
                            ("kind", edge_kind_form(dependency.kind)),
                            (
                                "semantic-interface-hash",
                                string(&dependency.semantic_interface_hash),
                            ),
                            (
                                "tooling-metadata-hash",
                                string(&dependency.tooling_metadata_hash),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "semantic-interface-hash",
            string(&group.semantic_interface_hash),
        ),
        (
            "tooling-metadata-hash",
            string(&group.tooling_metadata_hash),
        ),
    ])
}

pub(super) fn graph_edge_form(edge: &InterfaceHashEdge) -> Form {
    map(vec![
        ("from", string(&edge.from)),
        ("to", string(&edge.to)),
        ("kind", edge_kind_form(edge.kind)),
    ])
}

pub(super) fn edge_kind_form(kind: crate::module_graph::EdgeKind) -> Form {
    keyword(match kind {
        crate::module_graph::EdgeKind::Runtime => "runtime",
        crate::module_graph::EdgeKind::Phase1 => "phase-1",
    })
}

pub(super) fn hashes_form(
    interface_body: &str,
    semantic_body: &str,
    tooling_body: &str,
    integrity: Option<&str>,
) -> Form {
    let mut entries = vec![
        ("interface-body", string(interface_body)),
        ("semantic-body", string(semantic_body)),
        ("tooling-body", string(tooling_body)),
    ];
    if let Some(integrity) = integrity {
        entries.push(("content-integrity", string(integrity)));
    }
    wrap("osiris-interface/hashes", map(entries))
}
