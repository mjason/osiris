use std::{
    fs,
    sync::atomic::{AtomicUsize, Ordering},
};

use super::{
    ExtensionError, discover, discover_reachable, discover_reachable_all,
    normalize_distribution_name, sha256, validate_linked_support_manifest,
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);
const EXTENSION_SOURCE: &[u8] = b"(module sample.core)\n";

fn interface_source() -> String {
    let interface = crate::stdlib::interface_artifact(crate::stdlib::CORE_NAMESPACE).unwrap();
    crate::interface::render(&interface).unwrap()
}

fn source_map() -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "version": 3,
        "language_version": crate::LANGUAGE_VERSION,
        "python_target": "3.11",
        "source": "sample_ext/sample.osr",
        "source_hash": sha256(EXTENSION_SOURCE),
        "generated": "sample_ext/sample.py",
        "mappings": [],
    }))
    .unwrap()
}

fn fixture(marker: &str, records: Option<&[u8]>) -> std::path::PathBuf {
    let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("osiris-extension-{}-{id}", std::process::id()));
    let dist = root.join("sample_ext-1.0.dist-info");
    fs::create_dir_all(root.join("sample_ext")).unwrap();
    fs::create_dir_all(&dist).unwrap();
    fs::write(
        dist.join("METADATA"),
        "Metadata-Version: 2.4\nName: Sample.Ext\nVersion: 1.0\nRequires-Dist: numpy>=2\n\n",
    )
    .unwrap();
    fs::write(dist.join("osiris.toml"), marker).unwrap();
    fs::write(root.join("sample_ext/sample.osri"), interface_source()).unwrap();
    fs::write(root.join("sample_ext/sample.osr"), EXTENSION_SOURCE).unwrap();
    fs::write(root.join("sample_ext/sample.py.map"), source_map()).unwrap();
    if let Some(records) = records {
        fs::write(root.join("sample_ext/sample.records.json"), records).unwrap();
    }
    root
}

fn marker(records: Option<&[u8]>) -> String {
    let interface = crate::stdlib::interface_artifact(crate::stdlib::CORE_NAMESPACE).unwrap();
    let map = source_map();
    let records_section = records.map_or_else(String::new, |bytes| {
        format!(
            "records = \"sample_ext/sample.records.json\"\nrecords_hash = \"{}\"\n",
            sha256(bytes)
        )
    });
    format!(
        "schema = 2\ncompiler_abi = 1\nlanguage_abi = 2\nlanguage_version = \"{}\"\nstandard_library_abi = {}\nlinkable_helper_format = {}\ndistribution = \"sample-ext\"\nversion = \"1.0\"\npython_target = \"3.11\"\ndependencies = [\"numpy>=2\"]\n{records_section}\n[[extension]]\nid = \"sample\"\ninterface = \"sample_ext/sample.osri\"\ninterface_hash = \"{}\"\nsource = \"sample_ext/sample.osr\"\nsource_hash = \"{}\"\nsource_map = \"sample_ext/sample.py.map\"\nsource_map_hash = \"{}\"\n",
        crate::LANGUAGE_VERSION,
        crate::STANDARD_LIBRARY_ABI,
        crate::LINKABLE_HELPER_FORMAT,
        interface.semantic_interface_hash(),
        sha256(EXTENSION_SOURCE),
        sha256(&map),
    )
}

#[test]
fn discovers_only_enabled_static_extensions() {
    let records = b"{}";
    let root = fixture(&marker(Some(records)), Some(records));
    let graph = discover(std::slice::from_ref(&root), &["sample".to_owned()]).unwrap();
    let (distribution, extension) = graph.extension("sample").unwrap();
    assert_eq!(distribution.metadata.normalized_name, "sample-ext");
    assert_eq!(distribution.metadata.requires_dist, ["numpy>=2"]);
    assert!(extension.interface.ends_with("sample_ext/sample.osri"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_resource_paths_that_escape_the_wheel_root() {
    let root = fixture(
        &marker(None).replace("sample_ext/sample.osri", "../outside.osri"),
        None,
    );
    let error = discover(std::slice::from_ref(&root), &["sample".to_owned()]).unwrap_err();
    assert!(matches!(error, ExtensionError::ResourceEscape(_)));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_tampered_records() {
    let root = fixture(&marker(Some(b"expected")), Some(b"changed"));
    let error = discover(std::slice::from_ref(&root), &["sample".to_owned()]).unwrap_err();
    assert!(matches!(error, ExtensionError::HashMismatch { .. }));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_marker_identity_that_disagrees_with_metadata() {
    let explicit = marker(None).replace("distribution = \"sample-ext\"", "distribution = \"different\"");
    let root = fixture(&explicit, None);
    let error = discover(std::slice::from_ref(&root), &["sample".to_owned()]).unwrap_err();
    assert!(matches!(error, ExtensionError::InvalidMarker(_, _)));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_pre_v2_markers_without_a_compatibility_path() {
    let root = fixture(&marker(None).replacen("schema = 2", "schema = 1", 1), None);
    let error = discover(std::slice::from_ref(&root), &["sample".to_owned()]).unwrap_err();
    assert!(matches!(error, ExtensionError::InvalidMarker(_, _)));
    assert!(error.to_string().contains("unsupported schema 1; expected 2"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reachable_discovery_ignores_unrelated_installed_markers() {
    let root = fixture(&marker(None), None);
    let unrelated = root.join("unrelated-9.0.dist-info");
    fs::create_dir_all(&unrelated).unwrap();
    fs::write(unrelated.join("osiris.toml"), "not valid TOML = [").unwrap();
    let graph = discover_reachable(
        std::slice::from_ref(&root),
        &["sample".to_owned()],
        &["sample-ext".to_owned()],
    )
    .unwrap();
    assert!(graph.extension("sample").is_some());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn automatic_discovery_ignores_unreachable_installed_markers() {
    let root = fixture(&marker(None), None);
    let unrelated = root.join("unrelated-9.0.dist-info");
    fs::create_dir_all(&unrelated).unwrap();
    fs::write(unrelated.join("osiris.toml"), "not valid TOML = [").unwrap();
    let graph = discover_reachable_all(
        std::slice::from_ref(&root),
        &["sample-ext".to_owned()],
    )
    .unwrap();
    assert!(graph.extension("sample").is_some());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn normalizes_distribution_names_like_python_metadata() {
    assert_eq!(
        normalize_distribution_name("My_Package.Name"),
        "my-package-name"
    );
}

fn linked_support_fixture() -> (
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
    serde_json::Value,
) {
    let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "osiris-linked-support-{}-{id}",
        std::process::id()
    ));
    let runtime = root.join("sample/__osiris_runtime__");
    let marker = root.join("sample-1.0.dist-info/osiris.toml");
    fs::create_dir_all(&runtime).unwrap();
    fs::create_dir_all(marker.parent().unwrap()).unwrap();
    let source = b"(module sample.core)\n";
    let generated = b"value = 1\n";
    let support = b"__all__ = []\n";
    fs::write(root.join("sample/core.osr"), source).unwrap();
    fs::write(root.join("sample/core.py"), generated).unwrap();
    fs::write(runtime.join("__init__.py"), support).unwrap();
    let source_hash = sha256(source);
    let build_hash = sha256(b"build");
    let source_map = serde_json::json!({
        "python_target": "3.11",
        "source": "sample/core.osr",
        "source_hash": source_hash,
        "generated": "sample/core.py",
        "build_hash": build_hash,
    });
    fs::write(
        root.join("sample/core.py.map"),
        serde_json::to_vec(&source_map).unwrap(),
    )
    .unwrap();
    let manifest_path = runtime.join("manifest.json");
    let manifest = serde_json::json!({
        "schema": "osiris-linked-support/v1",
        "languageVersion": crate::LANGUAGE_VERSION,
        "pythonTarget": "3.11",
        "standardLibraryAbi": crate::STANDARD_LIBRARY_ABI,
        "standardLibrarySemanticHash": crate::stdlib::semantic_hash(),
        "helperFormat": crate::LINKABLE_HELPER_FORMAT,
        "reachableBindingIds": ["osiris.core::function::identity"],
        "helperHashes": {"identity": sha256(b"helper")},
        "fileHashes": {"sample/__osiris_runtime__/__init__.py": sha256(support)},
        "sourceMaps": [{
            "source": "sample/core.osr",
            "sourceHash": source_hash,
            "generated": "sample/core.py",
            "buildHash": build_hash,
        }],
    });
    (root, manifest_path, marker, manifest)
}

#[test]
fn linked_support_manifest_validates_files_and_source_map_provenance() {
    let (root, manifest, marker, value) = linked_support_fixture();
    validate_linked_support_manifest(&root, &manifest, &marker, Some("3.11"), value).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn linked_support_manifest_rejects_a_stale_source_map_identity() {
    let (root, manifest, marker, mut value) = linked_support_fixture();
    value["sourceMaps"][0]["buildHash"] = serde_json::json!(sha256(b"stale"));
    let error =
        validate_linked_support_manifest(&root, &manifest, &marker, Some("3.11"), value)
            .unwrap_err();
    assert!(matches!(error, ExtensionError::InvalidMarker(_, _)));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn linked_support_manifest_accepts_nested_standard_source_provenance() {
    let (root, manifest, marker, mut value) = linked_support_fixture();
    let runtime = root.join("sample/__osiris_runtime__/stdlib");
    fs::create_dir_all(&runtime).unwrap();
    let generated = b"def identity(value):\n    return value\n";
    fs::write(runtime.join("core.py"), generated).unwrap();
    let source_uri = "osiris-stdlib:///osiris/core.osr";
    let source = crate::stdlib::source_artifact_by_uri(source_uri).unwrap();
    let source_hash = sha256(source.as_bytes());
    let build_hash = sha256(b"standard-build");
    let generated_path = "sample/__osiris_runtime__/stdlib/core.py";
    let source_map = serde_json::json!({
        "python_target": "3.11",
        "source": source_uri,
        "source_hash": source_hash,
        "generated": generated_path,
        "build_hash": build_hash,
    });
    fs::write(
        runtime.join("core.py.map"),
        serde_json::to_vec(&source_map).unwrap(),
    )
    .unwrap();
    value["fileHashes"][generated_path] = serde_json::json!(sha256(generated));
    let standard_identity = serde_json::json!({
        "source": source_uri,
        "sourceHash": source_hash,
        "generated": generated_path,
        "buildHash": build_hash,
    });
    let authored_identity = value["sourceMaps"][0].clone();
    value["sourceMaps"] = serde_json::json!([standard_identity, authored_identity]);

    validate_linked_support_manifest(&root, &manifest, &marker, Some("3.11"), value).unwrap();
    fs::remove_dir_all(root).unwrap();
}
