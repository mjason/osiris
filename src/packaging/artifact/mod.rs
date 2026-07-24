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
    RuntimeSupport,
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
    pub language_version: String,
    pub python_target: String,
    pub source: String,
    pub source_hash: String,
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub macro_definitions: Vec<MacroDefinitionOrigin>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MacroDefinitionOrigin {
    pub binding_id: String,
    pub source: String,
    pub line: u32,
    pub column: u32,
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
#[path = "tests.rs"]
mod tests;
