//! Deterministic compiler artifacts and directory-level atomic publication.

use std::{
    collections::BTreeSet,
    fs, io,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};

use crate::source::Span;

static NEXT_STAGING_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
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

/// Publishes into a directory the compiler does not own — the project root,
/// when `outDir` is `.`.
///
/// [`publish_artifacts`] replaces the whole directory as one unit, which is
/// correct only when every file in it is a build product. Here artifacts are
/// written file by file, and stale products are removed by consulting the
/// previous publication's manifest: only a path this function itself recorded
/// is ever deleted, and with no manifest nothing is deleted at all — leaving a
/// stale generated file behind is recoverable, deleting an authored one is
/// not.
pub fn publish_artifacts_in_place(out_dir: &Path, artifacts: &[Artifact]) -> io::Result<()> {
    let mut paths = BTreeSet::new();
    for artifact in artifacts {
        validate_relative_artifact_path(&artifact.path)?;
        if !paths.insert(artifact.path.clone()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("duplicate artifact path `{}`", artifact.path.display()),
            ));
        }
    }

    let manifest_path = published_manifest_path(out_dir);
    let previous = read_published_manifest(out_dir);
    for stale in previous.difference(&paths) {
        remove_recorded_artifact(out_dir, stale);
    }

    for artifact in artifacts {
        let destination = out_dir.join(&artifact.path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&destination, &artifact.contents)?;
    }

    if let Some(parent) = manifest_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let manifest = serde_json::json!({
        "format-version": PUBLISHED_MANIFEST_FORMAT,
        "artifacts": paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
    });
    fs::write(&manifest_path, format!("{:#}\n", manifest))?;
    let _ = fs::remove_file(legacy_published_manifest_path(out_dir));
    Ok(())
}

fn published_manifest_path(out_dir: &Path) -> PathBuf {
    out_dir.join(".osiris").join("published-artifacts.json")
}

/// 0.3.22 wrote the manifest as bare lines; it is read once so an upgrade can
/// still clean that build, and every write replaces it with the JSON form.
fn legacy_published_manifest_path(out_dir: &Path) -> PathBuf {
    out_dir.join(".osiris").join("published-artifacts-v1")
}

const PUBLISHED_MANIFEST_FORMAT: u32 = 1;

fn read_published_manifest(out_dir: &Path) -> BTreeSet<PathBuf> {
    if let Ok(contents) = fs::read_to_string(published_manifest_path(out_dir)) {
        let parsed: Option<BTreeSet<PathBuf>> =
            serde_json::from_str::<serde_json::Value>(&contents)
                .ok()
                .filter(|value| {
                    value
                        .get("format-version")
                        .and_then(serde_json::Value::as_u64)
                        == Some(u64::from(PUBLISHED_MANIFEST_FORMAT))
                })
                .and_then(|value| {
                    Some(
                        value
                            .get("artifacts")?
                            .as_array()?
                            .iter()
                            .filter_map(|item| item.as_str().map(PathBuf::from))
                            .collect(),
                    )
                });
        return parsed.unwrap_or_default();
    }
    fs::read_to_string(legacy_published_manifest_path(out_dir))
        .map(|contents| {
            contents
                .lines()
                .filter(|line| !line.is_empty())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Removes one manifest-recorded artifact and any directories the removal
/// empties — never a non-empty directory, never the output root itself.
fn remove_recorded_artifact(out_dir: &Path, path: &Path) {
    if validate_relative_artifact_path(path).is_err() {
        return;
    }
    let absolute = out_dir.join(path);
    let _ = fs::remove_file(&absolute);
    let mut parent = absolute.parent();
    while let Some(directory) = parent {
        if directory == out_dir || fs::remove_dir(directory).is_err() {
            break;
        }
        parent = directory.parent();
    }
}

/// Removes every artifact the in-place publication manifest records, then the
/// manifest itself. Returns how many entries the manifest named.
///
/// This is the `osr clean` path for `outDir: "."`: generated files sit among
/// authored ones — often another framework's tree — so cleaning must not
/// guess. Only recorded paths are deleted; with no manifest nothing is.
pub fn clean_published_artifacts(out_dir: &Path) -> io::Result<usize> {
    let recorded = read_published_manifest(out_dir);
    for path in &recorded {
        remove_recorded_artifact(out_dir, path);
    }
    let manifest_path = published_manifest_path(out_dir);
    if manifest_path.exists() {
        fs::remove_file(&manifest_path)?;
    }
    let _ = fs::remove_file(legacy_published_manifest_path(out_dir));
    Ok(recorded.len())
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

pub(crate) fn validate_relative_artifact_path(path: &Path) -> io::Result<()> {
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
