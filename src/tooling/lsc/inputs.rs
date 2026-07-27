use std::{collections::BTreeMap, fs, path::Path, time::UNIX_EPOCH};

use rayon::prelude::*;
use serde_json::json;

use crate::project::ProjectConfig;

use super::source::project_sources;

const INPUT_SCHEMA: &str = "osiris.semantic-graph-input/v2";

#[derive(Clone, Debug)]
pub(super) struct CachedInput {
    pub size: u64,
    pub stamp: String,
    pub content_hash: String,
}

#[derive(Clone, Debug)]
pub(super) struct InputEntry {
    pub identity: String,
    pub size: u64,
    pub stamp: String,
    pub content_hash: String,
    pub reused_hash: bool,
}

pub(super) struct InputSnapshot {
    pub fingerprint: String,
    pub entries: Vec<InputEntry>,
}

impl InputSnapshot {
    pub(super) fn reused_hashes(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.reused_hash)
            .count()
    }

    pub(super) fn hashed_inputs(&self) -> usize {
        self.entries.len().saturating_sub(self.reused_hashes())
    }
}

pub(super) fn fingerprint(
    project: &ProjectConfig,
    cached: Option<&BTreeMap<String, CachedInput>>,
) -> Result<InputSnapshot, String> {
    let mut facts = Vec::new();
    let mut entries = Vec::new();
    for name in ["osiris.jsonc", "pyproject.toml", "uv.lock"] {
        let path = project.root.join(name);
        let identity = format!("project-metadata:{name}");
        let content_hash = if path.is_file() {
            let entry = fingerprint_file(identity, &path, cached)?;
            let hash = Some(entry.content_hash.clone());
            entries.push(entry);
            hash
        } else {
            None
        };
        facts.push(json!({
            "kind": "project-metadata",
            "path": name,
            "hash": content_hash,
        }));
    }

    let source_facts = project_sources(project)?
        .par_iter()
        .map(|path| {
            let module = project
                .module_name_for_source(path)
                .map_err(|error| error.to_string())?;
            let relative = path
                .strip_prefix(&project.root)
                .map_err(|_| format!("source '{}' escapes project root", path.display()))?
                .to_string_lossy()
                .replace('\\', "/");
            let entry = fingerprint_file(format!("source:{relative}"), path, cached)?;
            let fact = json!({
                "kind": "source",
                "path": relative,
                "module": module,
                "hash": entry.content_hash,
            });
            Ok::<_, String>((fact, entry))
        })
        .collect::<Result<Vec<_>, _>>()?;
    for (fact, entry) in source_facts {
        facts.push(fact);
        entries.push(entry);
    }

    let site_roots = project.installed_package_roots();
    if !site_roots.is_empty() {
        let lock = project.load_lock().map_err(|error| error.to_string())?;
        let graph = project
            .resolve_effective_extensions(&lock, &site_roots)
            .map_err(|error| error.to_string())?;
        for distribution in graph.extensions {
            for extension in distribution.extensions {
                let identity = format!(
                    "extension:{}:{}:{}:{}",
                    distribution.normalized_distribution,
                    distribution.version,
                    extension.id,
                    extension.module
                );
                let entry = fingerprint_file(identity, &extension.interface, cached)?;
                facts.push(json!({
                    "kind": "extension",
                    "distribution": distribution.normalized_distribution,
                    "version": distribution.version,
                    "id": extension.id,
                    "module": extension.module,
                    "semanticInterfaceHash": extension.semantic_interface_hash,
                    // Documentation and aliases are tooling facts, so retain
                    // the complete interface hash as part of cache identity.
                    "contentHash": entry.content_hash,
                }));
                entries.push(entry);
            }
        }
    }

    let encoded = serde_json::to_vec(&json!({
        "schema": INPUT_SCHEMA,
        "compiler": crate::version(),
        // The graph is a projection of compiler output, so two builds that
        // report one version but analyze differently must not share an entry.
        // A released version never changes within a build, and a local build
        // changes on every source edit.
        "compilerBuild": env!("OSIRIS_COMPILER_BUILD_HASH"),
        "language": crate::LANGUAGE_VERSION,
        "targetPython": project.target_python.to_string(),
        "strict": project.strict,
        "facts": facts,
    }))
    .map_err(|error| error.to_string())?;
    Ok(InputSnapshot {
        fingerprint: crate::hash::sha256(&encoded),
        entries,
    })
}

fn fingerprint_file(
    identity: String,
    path: &Path,
    cached: Option<&BTreeMap<String, CachedInput>>,
) -> Result<InputEntry, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("could not inspect '{}': {error}", path.display()))?;
    let size = metadata.len();
    let modified = metadata
        .modified()
        .map_err(|error| format!("could not inspect '{}': {error}", path.display()))?
        .duration_since(UNIX_EPOCH)
        .map_err(|_| format!("'{}' predates the filesystem epoch", path.display()))?;
    let stamp = format!("{}:{:09}", modified.as_secs(), modified.subsec_nanos());
    let cached_hash = cached
        .and_then(|entries| entries.get(&identity))
        .filter(|entry| entry.size == size && entry.stamp == stamp)
        .map(|entry| entry.content_hash.clone());
    let reused_hash = cached_hash.is_some();
    let content_hash = cached_hash.map_or_else(
        || {
            fs::read(path)
                .map(|bytes| crate::hash::sha256(&bytes))
                .map_err(|error| format!("could not fingerprint '{}': {error}", path.display()))
        },
        Ok,
    )?;
    Ok(InputEntry {
        identity,
        size,
        stamp,
        content_hash,
        reused_hash,
    })
}
