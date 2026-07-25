use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

static NEXT_TEST: AtomicUsize = AtomicUsize::new(0);

fn test_directory() -> PathBuf {
    let id = NEXT_TEST.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("osiris-cache-{}-{id}", std::process::id()))
}

fn artifacts() -> Vec<Artifact> {
    vec![
        Artifact::text(ArtifactKind::Interface, "demo/main.osri", "interface\n"),
        Artifact::text(ArtifactKind::Python, "demo/main.py", "value = 1\n"),
    ]
}

#[test]
fn stores_loads_and_replaces_one_complete_workspace() {
    let root = test_directory();
    fs::create_dir_all(&root).unwrap();
    let cache = WorkspaceCache::for_project(&root);
    cache.store("first", &artifacts()).unwrap();
    assert_eq!(cache.load("first"), Some(artifacts()));
    assert!(cache.load("other").is_none());

    let replacement = vec![Artifact::text(
        ArtifactKind::Python,
        "demo/main.py",
        "value = 2\n",
    )];
    cache.store("second", &replacement).unwrap();
    assert!(cache.load("first").is_none());
    assert_eq!(cache.load("second"), Some(replacement));
    assert!(
        !root
            .join(".osiris/cache/workspace-v1/artifacts/demo/main.osri")
            .exists()
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn malformed_or_partial_entries_are_cache_misses() {
    let root = test_directory();
    fs::create_dir_all(&root).unwrap();
    let cache = WorkspaceCache::for_project(&root);
    cache.store("key", &artifacts()).unwrap();
    fs::write(
        root.join(".osiris/cache/workspace-v1/artifacts/demo/main.py"),
        "changed\n",
    )
    .unwrap();
    assert!(cache.load("key").is_none());

    fs::write(
        root.join(".osiris/cache/workspace-v1/manifest.json"),
        "not json",
    )
    .unwrap();
    assert!(cache.load("key").is_none());

    let empty = CacheManifest {
        format_version: FORMAT_VERSION,
        key: "key".to_owned(),
        artifacts: Vec::new(),
    };
    fs::write(
        root.join(".osiris/cache/workspace-v1/manifest.json"),
        serde_json::to_vec(&empty).unwrap(),
    )
    .unwrap();
    assert!(cache.load("key").is_none());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn detects_exact_outputs_without_requiring_publication() {
    let root = test_directory();
    let out = root.join("dist");
    let expected = artifacts();
    publish_artifacts(&out, &expected).unwrap();
    assert!(output_matches(&out, &expected));
    fs::write(out.join("extra.py"), "pass\n").unwrap();
    assert!(!output_matches(&out, &expected));
    let _ = fs::remove_dir_all(root);
}
