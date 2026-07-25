use sha2::{Digest, Sha256};

use super::*;

const WORKSPACE_CACHE_PROTOCOL: &str = "osiris-workspace-artifacts-v1";

pub(super) fn workspace_cache(
    context: &CompileContext,
    sources: &[(String, String, compiler::CompileOptions)],
    loaded: &LoadedExternalInterfaces,
    arguments: &CompileArguments<'_>,
) -> Option<(crate::cache::WorkspaceCache, String)> {
    let project = context.project.as_ref()?;
    let mut hasher = Sha256::new();
    push_bytes(&mut hasher, "protocol", WORKSPACE_CACHE_PROTOCOL.as_bytes());
    push_text(&mut hasher, "compiler-version", crate::version());
    push_text(
        &mut hasher,
        "compiler-build-hash",
        env!("OSIRIS_COMPILER_BUILD_HASH"),
    );
    push_text(&mut hasher, "language-version", crate::LANGUAGE_VERSION);
    push_text(&mut hasher, "compiler-abi", interface::COMPILER_ABI);
    push_text(&mut hasher, "language-abi", interface::LANGUAGE_ABI);
    push_text(
        &mut hasher,
        "interface-format",
        &interface::FORMAT_VERSION.to_string(),
    );
    push_text(
        &mut hasher,
        "standard-library-abi",
        &crate::STANDARD_LIBRARY_ABI.to_string(),
    );
    push_text(
        &mut hasher,
        "standard-library-resource-hash",
        env!("OSIRIS_STDLIB_TREE_HASH"),
    );
    push_text(
        &mut hasher,
        "linkable-helper-format",
        &crate::LINKABLE_HELPER_FORMAT.to_string(),
    );
    push_text(
        &mut hasher,
        "python-formatter-abi",
        crate::backend::PYTHON_FORMATTER_ABI,
    );
    push_text(
        &mut hasher,
        "explicit-emit",
        &arguments.explicit_emit.to_string(),
    );
    for emit in &arguments.emit {
        push_text(&mut hasher, "emit", emit_name(*emit));
    }

    for name in ["osiris.jsonc", "pyproject.toml", "uv.lock"] {
        let path = project.root.join(name);
        match fs::read(path) {
            Ok(bytes) => push_bytes(&mut hasher, name, &bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                push_text(&mut hasher, name, "missing")
            }
            Err(_) => return None,
        }
    }

    for (path, source, options) in sources {
        let identity = Path::new(path)
            .strip_prefix(&project.root)
            .unwrap_or_else(|_| Path::new(path))
            .to_string_lossy()
            .replace('\\', "/");
        push_text(&mut hasher, "source-path", &identity);
        push_bytes(&mut hasher, "source", source.as_bytes());
        push_text(
            &mut hasher,
            "fallback-module",
            &options.fallback_module_name,
        );
        push_text(
            &mut hasher,
            "expected-module",
            options.expected_module_name.as_deref().unwrap_or("none"),
        );
        push_text(&mut hasher, "distribution", &options.distribution);
        push_text(
            &mut hasher,
            "distribution-version",
            &options.distribution_version,
        );
        push_text(
            &mut hasher,
            "target-python",
            &options.target_python.to_string(),
        );
        push_text(&mut hasher, "strict", &options.strict.to_string());
        push_text(&mut hasher, "trust-policy", &options.trust_policy.hash);
    }

    for (module, model) in &loaded.interfaces {
        push_text(&mut hasher, "interface-module", module);
        push_text(
            &mut hasher,
            "interface-semantic-hash",
            model.semantic_interface_hash(),
        );
        push_text(
            &mut hasher,
            "interface-tooling-hash",
            model.tooling_metadata_hash(),
        );
        push_text(
            &mut hasher,
            "interface-content-hash",
            &model.hashes.content_integrity,
        );
    }

    let key = format!("sha256:{:x}", hasher.finalize());
    Some((
        crate::cache::WorkspaceCache::for_project(&project.root),
        key,
    ))
}

fn emit_name(kind: EmitKind) -> &'static str {
    match kind {
        EmitKind::Python => "python",
        EmitKind::Interface => "interface",
        EmitKind::SourceMap => "source-map",
        EmitKind::Records => "records",
    }
}

fn push_text(hasher: &mut Sha256, name: &str, value: &str) {
    push_bytes(hasher, name, value.as_bytes());
}

fn push_bytes(hasher: &mut Sha256, name: &str, value: &[u8]) {
    hasher.update(name.len().to_be_bytes());
    hasher.update(name.as_bytes());
    hasher.update(value.len().to_be_bytes());
    hasher.update(value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_fields_are_unambiguous() {
        let mut first = Sha256::new();
        push_text(&mut first, "a", "bc");
        let mut second = Sha256::new();
        push_text(&mut second, "ab", "c");
        assert_ne!(first.finalize(), second.finalize());
    }
}
