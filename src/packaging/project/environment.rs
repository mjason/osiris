use std::collections::BTreeSet;

impl ProjectConfig {
    /// Finds installed-package roots without invoking a Python interpreter.
    /// Explicit CLI roots are handled separately by the caller.
    #[must_use]
    pub fn installed_package_roots(&self) -> Vec<PathBuf> {
        let mut environments = BTreeSet::from([self.root.join(".venv")]);
        if let Some(environment) = std::env::var_os("VIRTUAL_ENV") {
            let environment = PathBuf::from(environment);
            if environment_belongs_to_project(&environment, &self.root) {
                environments.insert(environment);
            }
        }
        if let Ok(executable) = std::env::current_exe()
            && let Some(scripts) = executable.parent()
            && scripts
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("bin") || name == "Scripts")
            && let Some(environment) = scripts.parent()
            && environment_belongs_to_project(environment, &self.root)
        {
            environments.insert(environment.to_path_buf());
        }

        let mut roots = BTreeSet::new();
        for environment in environments {
            collect_installed_package_roots(&environment, &mut roots);
        }
        roots.into_iter().collect()
    }
}

fn environment_belongs_to_project(environment: &Path, project_root: &Path) -> bool {
    environment.parent().is_some_and(|parent| {
        fs::canonicalize(parent)
            .map(|parent| parent == project_root)
            .unwrap_or(false)
    })
}

fn collect_installed_package_roots(environment: &Path, roots: &mut BTreeSet<PathBuf>) {
    let windows_root = environment.join("Lib/site-packages");
    if windows_root.is_dir() {
        roots.insert(windows_root);
    }
    for library_dir in [environment.join("lib"), environment.join("lib64")] {
        let Ok(entries) = fs::read_dir(library_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_python_directory = entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("python"));
            let site_packages = path.join("site-packages");
            if is_python_directory && site_packages.is_dir() {
                roots.insert(site_packages);
            }
        }
    }
}
