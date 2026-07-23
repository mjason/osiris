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
