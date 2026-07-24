use std::{
    collections::BTreeMap,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

use notify::{RecursiveMode, Watcher};

use super::{ProjectConfig, run_project_build};

struct WatchArguments {
    path: PathBuf,
    site_roots: Vec<String>,
}

#[derive(Clone, Debug)]
struct WatchRegistration {
    path: PathBuf,
    recursive: bool,
}

pub fn run_watch_stdio(arguments: &[String]) -> Result<(), String> {
    let arguments = parse_watch_arguments(arguments)?;
    let mut project =
        ProjectConfig::discover(&arguments.path).map_err(|error| error.to_string())?;
    if !build_and_render(&arguments.path, &arguments.site_roots)? {
        return Err("initial build failed".to_owned());
    }

    let (sender, receiver) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = sender.send(event);
    })
    .map_err(|error| format!("could not create file watcher: {error}"))?;
    let mut watched = BTreeMap::new();
    sync_watch_inputs(&mut watcher, &project, &arguments.site_roots, &mut watched)?;
    writeln!(
        io::stdout().lock(),
        "Watching {}",
        project
            .source_roots
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
    .map_err(|error| error.to_string())?;

    loop {
        let event = receiver
            .recv()
            .map_err(|_| "file watcher stopped unexpectedly".to_owned())?
            .map_err(|error| format!("file watcher error: {error}"))?;
        if !event
            .paths
            .iter()
            .any(|path| should_recompile(&project, &arguments.site_roots, path))
        {
            continue;
        }
        while receiver.recv_timeout(Duration::from_millis(75)).is_ok() {}
        let _ = build_and_render(&arguments.path, &arguments.site_roots)?;
        if let Ok(updated) = ProjectConfig::discover(&arguments.path) {
            sync_watch_inputs(&mut watcher, &updated, &arguments.site_roots, &mut watched)?;
            project = updated;
        }
    }
}

fn sync_watch_inputs(
    watcher: &mut impl Watcher,
    project: &ProjectConfig,
    site_roots: &[String],
    watched: &mut BTreeMap<PathBuf, WatchRegistration>,
) -> Result<(), String> {
    let mut desired = BTreeMap::new();
    add_watch_input(&mut desired, &project.root, false);
    for root in &project.source_roots {
        add_watch_input(&mut desired, root, true);
    }
    for root in project
        .installed_package_roots()
        .into_iter()
        .chain(site_roots.iter().map(PathBuf::from))
    {
        if root.is_dir() {
            add_watch_input(&mut desired, &root, true);
        }
    }

    let stale = watched
        .iter()
        .filter(|(identity, registration)| {
            desired
                .get(*identity)
                .is_none_or(|next| next.recursive != registration.recursive)
        })
        .map(|(identity, _)| identity.clone())
        .collect::<Vec<_>>();
    for identity in stale {
        if let Some(registration) = watched.remove(&identity) {
            // A removed directory may already be absent. Scope filtering still
            // ignores any late event if the backend cannot unregister it.
            let _ = watcher.unwatch(&registration.path);
        }
    }
    for (identity, registration) in desired {
        if watched.contains_key(&identity) {
            continue;
        }
        watcher
            .watch(
                &registration.path,
                if registration.recursive {
                    RecursiveMode::Recursive
                } else {
                    RecursiveMode::NonRecursive
                },
            )
            .map_err(|error| {
                format!("could not watch '{}': {error}", registration.path.display())
            })?;
        watched.insert(identity, registration);
    }
    Ok(())
}

fn add_watch_input(
    desired: &mut BTreeMap<PathBuf, WatchRegistration>,
    path: &Path,
    recursive: bool,
) {
    let identity = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if let Some(existing) = desired.get_mut(&identity) {
        existing.recursive |= recursive;
    } else {
        desired.insert(
            identity,
            WatchRegistration {
                path: path.to_path_buf(),
                recursive,
            },
        );
    }
}

fn parse_watch_arguments(arguments: &[String]) -> Result<WatchArguments, String> {
    let mut path = None;
    let mut site_roots = Vec::new();
    let mut index = 0;
    while let Some(argument) = arguments.get(index) {
        match argument.as_str() {
            "--site-root" => {
                let value = arguments
                    .get(index + 1)
                    .ok_or_else(|| "missing value for '--site-root'".to_owned())?;
                site_roots.push(value.clone());
                index += 1;
            }
            option if option.starts_with('-') => {
                return Err(format!("unknown option '{option}' for 'watch'"));
            }
            value if path.is_none() => path = Some(PathBuf::from(value)),
            _ => return Err("unexpected arguments for 'watch'".to_owned()),
        }
        index += 1;
    }
    Ok(WatchArguments {
        path: path.unwrap_or_else(|| PathBuf::from(".")),
        site_roots,
    })
}

fn should_recompile(project: &ProjectConfig, site_roots: &[String], path: &Path) -> bool {
    if ["osiris.jsonc", "pyproject.toml", "uv.lock"]
        .iter()
        .any(|name| path == project.root.join(name))
    {
        return true;
    }
    if project.is_excluded(path) {
        return false;
    }
    if project
        .source_roots
        .iter()
        .any(|root| path.starts_with(root))
    {
        return path.extension().and_then(|value| value.to_str()) == Some("osr");
    }
    let in_extension_root = project
        .installed_package_roots()
        .into_iter()
        .chain(site_roots.iter().map(PathBuf::from))
        .any(|root| path.starts_with(root.canonicalize().unwrap_or(root)));
    in_extension_root && is_static_extension_input(path)
}

fn is_static_extension_input(path: &Path) -> bool {
    if path
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| matches!(name, "osiris.toml" | "METADATA"))
    {
        return true;
    }
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| matches!(extension, "osr" | "osri" | "json" | "py" | "map"))
}

fn build_and_render(path: &Path, site_roots: &[String]) -> Result<bool, String> {
    let outcome = run_project_build(path, site_roots);
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(outcome.stdout.as_bytes())
        .and_then(|()| stdout.flush())
        .map_err(|error| error.to_string())?;
    let mut stderr = io::stderr().lock();
    stderr
        .write_all(outcome.stderr.as_bytes())
        .and_then(|()| stderr.flush())
        .map_err(|error| error.to_string())?;
    Ok(outcome.exit_code == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_defaults_to_the_current_directory() {
        let arguments = parse_watch_arguments(&[]).unwrap();
        assert_eq!(arguments.path, Path::new("."));
    }

    #[test]
    fn watch_classifies_project_and_static_interface_inputs() {
        let root = std::env::temp_dir().join(format!("osiris-watch-inputs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("pyproject.toml"),
            "[project]\nname = \"watch-inputs\"\nversion = \"0\"\n",
        )
        .unwrap();
        std::fs::write(root.join("osiris.jsonc"), r#"{"source":["src"]}"#).unwrap();
        let project = ProjectConfig::discover(&root).unwrap();
        let site_root = root.join("packages");
        std::fs::create_dir_all(&site_root).unwrap();
        let site_roots = vec![site_root.display().to_string()];
        assert!(should_recompile(
            &project,
            &site_roots,
            &root.join("osiris.jsonc")
        ));
        assert!(should_recompile(
            &project,
            &site_roots,
            &root.join("pyproject.toml")
        ));
        assert!(should_recompile(
            &project,
            &site_roots,
            &root.join("uv.lock")
        ));
        assert!(should_recompile(
            &project,
            &site_roots,
            &root.join("src/main.osr")
        ));
        assert!(!should_recompile(
            &project,
            &site_roots,
            &root.join("other/main.osr")
        ));
        assert!(should_recompile(
            &project,
            &site_roots,
            &root.join("packages/api.osri")
        ));
        assert!(should_recompile(
            &project,
            &site_roots,
            &root.join("packages/demo.dist-info/osiris.toml")
        ));
        assert!(should_recompile(
            &project,
            &site_roots,
            &root.join("packages/demo/__osiris_runtime__/support.py")
        ));
        assert!(!should_recompile(
            &project,
            &site_roots,
            &root.join("dist/main.py")
        ));
        let _ = std::fs::remove_dir_all(root);
    }
}
