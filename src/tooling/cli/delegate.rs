use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// The osr a project locked beats the osr PATH happened to find.
///
/// A stale globally installed compiler running inside a project reports
/// errors its locked version fixed long ago, and nothing in the output says
/// which binary produced them. So every invocation first looks for the
/// project's own osr — the activated `VIRTUAL_ENV`, then a `.venv` on the
/// ancestor path of the working directory — and hands the invocation over
/// when it finds one that is not the running binary. `OSR_NO_DELEGATE=1`
/// opts out, and is set on the delegate so a copied or hard-linked binary
/// cannot loop.
pub fn project_local_osr() -> Option<PathBuf> {
    if env::var_os("OSR_NO_DELEGATE").is_some() {
        return None;
    }
    // Only a bare `osr` resolved through PATH delegates. An invocation that
    // spelled a path — `./target/debug/osr`, an absolute CI path — chose a
    // specific binary, and that choice stands; a test harness exercising a
    // freshly built compiler must not be silently handed to whatever an
    // enclosing project pinned.
    let invoked_as = env::args_os().next()?;
    if invoked_as.to_string_lossy().contains(['/', '\\']) {
        return None;
    }
    // If the running binary cannot be identified, delegation could recurse
    // forever; running in place is the safe failure.
    let current = fs::canonicalize(env::current_exe().ok()?).ok()?;
    let candidate = activated_environment_osr().or_else(ancestor_venv_osr)?;
    let canonical = fs::canonicalize(&candidate).ok()?;
    if canonical == current {
        return None;
    }
    Some(candidate)
}

fn environment_osr(environment: &Path) -> Option<PathBuf> {
    let candidate = if cfg!(windows) {
        environment.join("Scripts").join("osr.exe")
    } else {
        environment.join("bin").join("osr")
    };
    candidate.is_file().then_some(candidate)
}

fn activated_environment_osr() -> Option<PathBuf> {
    environment_osr(Path::new(&env::var_os("VIRTUAL_ENV")?))
}

fn ancestor_venv_osr() -> Option<PathBuf> {
    let start = env::current_dir().ok()?;
    start
        .ancestors()
        .find_map(|directory| environment_osr(&directory.join(".venv")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_environment_binary_only_when_present() {
        let root = std::env::temp_dir().join(format!(
            "osiris-delegate-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = fs::remove_dir_all(&root);
        assert!(environment_osr(&root.join(".venv")).is_none());
        let bin = root
            .join(".venv")
            .join(if cfg!(windows) { "Scripts" } else { "bin" });
        fs::create_dir_all(&bin).unwrap();
        let name = if cfg!(windows) { "osr.exe" } else { "osr" };
        fs::write(bin.join(name), "#!/bin/sh\n").unwrap();
        assert_eq!(environment_osr(&root.join(".venv")), Some(bin.join(name)));
        let _ = fs::remove_dir_all(root);
    }
}
