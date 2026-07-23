use std::{fs, sync::atomic::AtomicUsize};

use super::{Artifact, ArtifactKind, publish_artifacts};

static NEXT_TEST: AtomicUsize = AtomicUsize::new(0);

fn test_directory() -> std::path::PathBuf {
    let id = NEXT_TEST.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("osiris-artifacts-{}-{id}", std::process::id()))
}

#[test]
fn publishes_and_replaces_a_complete_artifact_set() {
    let root = test_directory();
    let out = root.join("out");
    publish_artifacts(
        &out,
        &[
            Artifact::text(ArtifactKind::Python, "example.py", "value = 1\n"),
            Artifact::text(ArtifactKind::Interface, "example.osri", "{}\n"),
        ],
    )
    .expect("first publication should succeed");
    publish_artifacts(
        &out,
        &[Artifact::text(
            ArtifactKind::Python,
            "example.py",
            "value = 2\n",
        )],
    )
    .expect("replacement should succeed");

    assert_eq!(
        fs::read_to_string(out.join("example.py")).expect("Python artifact should exist"),
        "value = 2\n"
    );
    assert!(!out.join("example.osri").exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn rejects_paths_that_escape_the_output_directory() {
    let root = test_directory();
    let out = root.join("out");
    let error = publish_artifacts(
        &out,
        &[Artifact::text(
            ArtifactKind::Python,
            "../escape.py",
            "pass\n",
        )],
    )
    .expect_err("parent path must be rejected");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(!root.join("escape.py").exists());
    let _ = fs::remove_dir_all(root);
}
