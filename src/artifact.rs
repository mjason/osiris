//! Deterministic compiler artifacts and directory-level atomic publication.

use std::{
    collections::BTreeSet,
    fs, io,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::Serialize;

use crate::source::Span;

static NEXT_STAGING_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactKind {
    Python,
    Interface,
    SourceMap,
    Records,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Artifact {
    pub kind: ArtifactKind,
    pub path: PathBuf,
    pub contents: Vec<u8>,
}

impl Artifact {
    #[must_use]
    pub fn text(kind: ArtifactKind, path: impl Into<PathBuf>, contents: impl Into<String>) -> Self {
        Self {
            kind,
            path: path.into(),
            contents: contents.into().into_bytes(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SourceMap {
    pub version: u32,
    pub source: String,
    pub generated: String,
    pub trust_policy_hash: String,
    pub build_hash: String,
    pub mappings: Vec<SourceMapping>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SourceMapping {
    pub generated_start: GeneratedPosition,
    pub generated_end: GeneratedPosition,
    pub source_span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub expansion_origin: Vec<Span>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct GeneratedPosition {
    pub line: usize,
    pub column: usize,
}

/// Publishes a complete build directory with rollback if the final rename fails.
///
/// `out_dir` is compiler-owned: an existing directory is replaced as one unit,
/// which prevents a failed compile from mixing old and new artifacts.
pub fn publish_artifacts(out_dir: &Path, artifacts: &[Artifact]) -> io::Result<()> {
    let parent = out_dir.parent().unwrap_or_else(|| Path::new("."));
    let directory_name = out_dir.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "artifact output directory must have a final path component",
        )
    })?;
    fs::create_dir_all(parent)?;

    let staging_id = NEXT_STAGING_ID.fetch_add(1, Ordering::Relaxed);
    let suffix = format!("{}-{staging_id}", std::process::id());
    let staging = parent.join(format!(
        ".{}.osr-stage-{suffix}",
        directory_name.to_string_lossy()
    ));
    let backup = parent.join(format!(
        ".{}.osr-backup-{suffix}",
        directory_name.to_string_lossy()
    ));

    if staging.exists() || backup.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "compiler staging path already exists",
        ));
    }

    let result = (|| {
        fs::create_dir(&staging)?;
        let mut paths = BTreeSet::new();
        for artifact in artifacts {
            validate_relative_artifact_path(&artifact.path)?;
            if !paths.insert(artifact.path.clone()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("duplicate artifact path `{}`", artifact.path.display()),
                ));
            }
            let destination = staging.join(&artifact.path);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(destination, &artifact.contents)?;
        }

        let had_previous = out_dir.exists();
        if had_previous {
            fs::rename(out_dir, &backup)?;
        }
        if let Err(error) = fs::rename(&staging, out_dir) {
            if had_previous {
                let _ = fs::rename(&backup, out_dir);
            }
            return Err(error);
        }
        if had_previous {
            let _ = fs::remove_dir_all(&backup);
        }
        Ok(())
    })();

    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    if result.is_err() && backup.exists() && !out_dir.exists() {
        let _ = fs::rename(&backup, out_dir);
    }
    result
}

fn validate_relative_artifact_path(path: &Path) -> io::Result<()> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid artifact path `{}`", path.display()),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
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
}
