//! Versioned, project-local cache for complete compiler artifact sets.

use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    artifact::{Artifact, ArtifactKind, publish_artifacts, validate_relative_artifact_path},
    hash,
};

const FORMAT_VERSION: u32 = 1;
const MAX_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ARTIFACT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ARTIFACTS: usize = 100_000;

#[derive(Clone, Debug)]
pub struct WorkspaceCache {
    directory: PathBuf,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CacheManifest {
    format_version: u32,
    key: String,
    artifacts: Vec<CachedArtifact>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CachedArtifact {
    kind: ArtifactKind,
    path: String,
    content_hash: String,
    byte_length: u64,
}

impl WorkspaceCache {
    #[must_use]
    pub fn for_project(project_root: &Path) -> Self {
        Self {
            directory: project_root.join(".osiris/cache/workspace-v1"),
        }
    }

    /// Loads a complete cache entry. Any invalid or partial cache is a miss.
    #[must_use]
    pub fn load(&self, key: &str) -> Option<Vec<Artifact>> {
        let manifest_path = self.directory.join("manifest.json");
        let bytes = read_regular_file(&manifest_path, MAX_MANIFEST_BYTES)?;
        let manifest: CacheManifest = serde_json::from_slice(&bytes).ok()?;
        if manifest.format_version != FORMAT_VERSION
            || manifest.key != key
            || manifest.artifacts.len() > MAX_ARTIFACTS
        {
            return None;
        }

        let mut paths = BTreeSet::new();
        let mut artifacts = Vec::with_capacity(manifest.artifacts.len());
        for cached in manifest.artifacts {
            let path = PathBuf::from(&cached.path);
            validate_relative_artifact_path(&path).ok()?;
            if !paths.insert(path.clone()) || cached.byte_length > MAX_ARTIFACT_BYTES {
                return None;
            }
            let cache_path = self.directory.join("artifacts").join(&path);
            let contents = read_regular_file(&cache_path, MAX_ARTIFACT_BYTES)?;
            if contents.len() as u64 != cached.byte_length
                || hash::sha256(&contents) != cached.content_hash
            {
                return None;
            }
            artifacts.push(Artifact {
                kind: cached.kind,
                path,
                contents,
            });
        }
        Some(artifacts)
    }

    /// Replaces the prior cache entry with one successful complete build.
    pub fn store(&self, key: &str, artifacts: &[Artifact]) -> io::Result<()> {
        ensure_cache_parent_is_safe(&self.directory)?;
        if artifacts.is_empty() || artifacts.len() > MAX_ARTIFACTS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cache requires a bounded, non-empty artifact set",
            ));
        }
        let mut paths = BTreeSet::new();
        let mut manifest_artifacts = Vec::with_capacity(artifacts.len());
        let mut cache_artifacts = Vec::with_capacity(artifacts.len() + 1);
        for artifact in artifacts {
            if artifact.contents.len() as u64 > MAX_ARTIFACT_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("cached artifact `{}` is too large", artifact.path.display()),
                ));
            }
            validate_relative_artifact_path(&artifact.path)?;
            if !paths.insert(artifact.path.clone()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "duplicate cached artifact path `{}`",
                        artifact.path.display()
                    ),
                ));
            }
            let path = artifact.path.to_str().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "cached artifact paths must be UTF-8",
                )
            })?;
            manifest_artifacts.push(CachedArtifact {
                kind: artifact.kind,
                path: path.replace('\\', "/"),
                content_hash: hash::sha256(&artifact.contents),
                byte_length: artifact.contents.len() as u64,
            });
            cache_artifacts.push(Artifact {
                kind: artifact.kind,
                path: Path::new("artifacts").join(&artifact.path),
                contents: artifact.contents.clone(),
            });
        }
        manifest_artifacts.sort_by(|left, right| left.path.cmp(&right.path));
        let manifest = CacheManifest {
            format_version: FORMAT_VERSION,
            key: key.to_owned(),
            artifacts: manifest_artifacts,
        };
        let mut bytes = serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?;
        bytes.push(b'\n');
        cache_artifacts.push(Artifact {
            kind: ArtifactKind::RuntimeSupport,
            path: PathBuf::from("manifest.json"),
            contents: bytes,
        });
        publish_artifacts(&self.directory, &cache_artifacts)
    }
}

/// Returns true only when a directory is exactly the requested artifact set.
#[must_use]
pub fn output_matches(out_dir: &Path, artifacts: &[Artifact]) -> bool {
    match fs::symlink_metadata(out_dir) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        _ => return false,
    }

    let mut actual = BTreeSet::new();
    if collect_regular_files(out_dir, out_dir, &mut actual).is_err() {
        return false;
    }
    let expected = artifacts
        .iter()
        .map(|artifact| artifact.path.clone())
        .collect::<BTreeSet<_>>();
    if actual != expected || expected.len() != artifacts.len() {
        return false;
    }
    artifacts.iter().all(|artifact| {
        read_regular_file(&out_dir.join(&artifact.path), MAX_ARTIFACT_BYTES)
            .is_some_and(|contents| contents == artifact.contents)
    })
}

fn read_regular_file(path: &Path, maximum: u64) -> Option<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file() || metadata.len() > maximum {
        return None;
    }
    fs::read(path).ok()
}

fn collect_regular_files(
    root: &Path,
    directory: &Path,
    files: &mut BTreeSet<PathBuf>,
) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_regular_files(root, &entry.path(), files)?;
        } else if file_type.is_file() {
            let relative = entry
                .path()
                .strip_prefix(root)
                .map_err(io::Error::other)?
                .to_path_buf();
            validate_relative_artifact_path(&relative)?;
            files.insert(relative);
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "artifact output contains a link or special file",
            ));
        }
    }
    Ok(())
}

fn ensure_cache_parent_is_safe(directory: &Path) -> io::Result<()> {
    let mut ancestors = directory.ancestors().take(3).collect::<Vec<_>>();
    ancestors.reverse();
    for path in ancestors {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("cache path `{}` is not a directory", path.display()),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
