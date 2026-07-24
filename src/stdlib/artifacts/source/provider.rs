use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use sha2::{Digest, Sha256};

const EXPECTED_HASH: &str = env!("OSIRIS_STDLIB_TREE_HASH");

static ROOT: OnceLock<Result<PathBuf, String>> = OnceLock::new();

pub(super) fn read(relative: &str) -> Result<String, String> {
    let root = root()?;
    let path = root.join(relative);
    fs::read_to_string(&path).map_err(|error| {
        format!(
            "could not read standard resource `{}`: {error}",
            path.display()
        )
    })
}

pub(super) fn validate() -> Result<&'static Path, String> {
    root().map(PathBuf::as_path)
}

pub(super) const fn expected_hash() -> &'static str {
    EXPECTED_HASH
}

fn root() -> Result<&'static PathBuf, String> {
    ROOT.get_or_init(discover).as_ref().map_err(Clone::clone)
}

fn discover() -> Result<PathBuf, String> {
    let candidates = candidates();
    let mut failures = Vec::new();
    for candidate in &candidates {
        if !candidate.join("osiris.jsonc").is_file() || !candidate.join("src").is_dir() {
            continue;
        }
        match resource_hash(candidate) {
            Ok(found) if found == EXPECTED_HASH => return Ok(candidate.clone()),
            Ok(found) => failures.push(format!(
                "`{}` has {found}, expected {EXPECTED_HASH}",
                candidate.display()
            )),
            Err(error) => failures.push(error),
        }
    }
    let searched = candidates
        .iter()
        .map(|path| format!("`{}`", path.display()))
        .collect::<Vec<_>>()
        .join(", ");
    let detail = if failures.is_empty() {
        String::new()
    } else {
        format!("; invalid candidates: {}", failures.join("; "))
    };
    Err(format!(
        "could not locate the Osiris standard resource tree (expected {EXPECTED_HASH}; searched {searched}{detail})"
    ))
}

fn candidates() -> Vec<PathBuf> {
    if let Some(root) = env::var_os("OSIRIS_STDLIB_ROOT") {
        return vec![PathBuf::from(root)];
    }
    let mut result = Vec::new();
    if let Ok(executable) = env::current_exe() {
        if let Some(bin) = executable.parent() {
            result.push(bin.join("stdlib"));
            if let Some(prefix) = bin.parent() {
                result.push(prefix.join("stdlib"));
                result.push(prefix.join("Lib").join("site-packages").join("stdlib"));
                let lib = prefix.join("lib");
                if let Ok(entries) = fs::read_dir(lib) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| name.starts_with("python"))
                        {
                            result.push(path.join("site-packages").join("stdlib"));
                        }
                    }
                }
            }
        }
    }
    if cfg!(debug_assertions) {
        result.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("stdlib"));
    }
    result.sort();
    result.dedup();
    result
}

fn resource_hash(root: &Path) -> Result<String, String> {
    let mut files = Vec::new();
    collect(root, root, &mut files)?;
    files.sort();
    let mut digest = Sha256::new();
    for path in files {
        let relative = path
            .strip_prefix(root)
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = fs::read(&path).map_err(|error| error.to_string())?;
        digest.update((relative.len() as u64).to_be_bytes());
        digest.update(relative.as_bytes());
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    }
    Ok(format!("sha256:{:x}", digest.finalize()))
}

fn collect(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_dir() {
            collect(root, &path, files)?;
        } else {
            let relative = path.strip_prefix(root).map_err(|error| error.to_string())?;
            let named = matches!(
                relative.to_string_lossy().replace('\\', "/").as_str(),
                "README.md" | "pyproject.toml" | "osiris.jsonc" | "uv.lock"
            );
            if named || relative.extension().is_some_and(|value| value == "osr") {
                files.push(path);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn development_resource_tree_matches_the_compiler_identity() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("stdlib");
        assert_eq!(resource_hash(&root).unwrap(), EXPECTED_HASH);
        assert_eq!(validate().unwrap(), root);
    }
}
