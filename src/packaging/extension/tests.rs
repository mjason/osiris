use std::{
    fs,
    sync::atomic::{AtomicUsize, Ordering},
};

use super::{ExtensionError, discover, discover_reachable, normalize_distribution_name, sha256};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

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
    fs::write(root.join("sample_ext/sample.osri"), "(osiris-interface)\n").unwrap();
    if let Some(records) = records {
        fs::write(root.join("sample_ext/sample.records.json"), records).unwrap();
    }
    root
}

fn marker(records: Option<&[u8]>) -> String {
    let records_section = records.map_or_else(String::new, |bytes| {
        format!(
            "records = \"sample_ext/sample.records.json\"\nrecords_hash = \"{}\"\n",
            sha256(bytes)
        )
    });
    format!(
        "schema = 1\ncompiler_abi = 1\nlanguage_abi = 2\n{records_section}\n[[extension]]\nid = \"sample\"\ninterface = \"sample_ext/sample.osri\"\n"
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
    let explicit = marker(None).replacen(
        "schema = 1\n",
        "schema = 1\ndistribution = \"different\"\nversion = \"1.0\"\n",
        1,
    );
    let root = fixture(&explicit, None);
    let error = discover(std::slice::from_ref(&root), &["sample".to_owned()]).unwrap_err();
    assert!(matches!(error, ExtensionError::InvalidMarker(_, _)));
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
fn normalizes_distribution_names_like_python_metadata() {
    assert_eq!(
        normalize_distribution_name("My_Package.Name"),
        "my-package-name"
    );
}
