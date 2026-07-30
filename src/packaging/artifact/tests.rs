use std::{fs, path::PathBuf, sync::atomic::AtomicUsize};

use super::{Artifact, ArtifactKind, publish_artifacts, publish_artifacts_in_place};

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

#[test]
fn in_place_publication_never_deletes_what_it_did_not_write() {
    let root = std::env::temp_dir().join(format!(
        "osiris-in-place-{}-{}",
        std::process::id(),
        line!()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("pyproject.toml"), "authored").unwrap();
    fs::write(root.join("src/main.osr"), "authored").unwrap();

    let first = vec![
        Artifact::text(ArtifactKind::Python, PathBuf::from("app/main.py"), "one"),
        Artifact::text(ArtifactKind::Interface, PathBuf::from("app/main.osri"), "i"),
    ];
    publish_artifacts_in_place(&root, &first).unwrap();
    assert_eq!(fs::read_to_string(root.join("app/main.py")).unwrap(), "one");
    assert_eq!(
        fs::read_to_string(root.join("pyproject.toml")).unwrap(),
        "authored"
    );

    // A renamed module: the stale artifact goes, its emptied directory goes,
    // the authored files stay.
    let second = vec![Artifact::text(
        ArtifactKind::Python,
        PathBuf::from("core/main.py"),
        "two",
    )];
    publish_artifacts_in_place(&root, &second).unwrap();
    assert!(!root.join("app/main.py").exists());
    assert!(!root.join("app").exists());
    assert_eq!(
        fs::read_to_string(root.join("core/main.py")).unwrap(),
        "two"
    );
    assert_eq!(
        fs::read_to_string(root.join("src/main.osr")).unwrap(),
        "authored"
    );

    // Without a manifest nothing is ever deleted: a pre-existing file at an
    // artifact-shaped path survives until a manifest names it.
    let fresh = root.join("fresh");
    fs::create_dir_all(fresh.join("old")).unwrap();
    fs::write(fresh.join("old/left.py"), "not ours").unwrap();
    publish_artifacts_in_place(&fresh, &second).unwrap();
    assert_eq!(
        fs::read_to_string(fresh.join("old/left.py")).unwrap(),
        "not ours"
    );

    let _ = fs::remove_dir_all(root);
}
