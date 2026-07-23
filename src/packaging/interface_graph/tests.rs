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

    let first = calculate_interface_graph_hashes(&bodies, edges.clone(), &first_external).unwrap();
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
