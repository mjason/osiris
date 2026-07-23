use std::{
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

pub fn run_watch_stdio(arguments: &[String]) -> Result<(), String> {
    let arguments = parse_watch_arguments(arguments)?;
    let project = ProjectConfig::discover(&arguments.path).map_err(|error| error.to_string())?;
    if !build_and_render(&arguments.path, &arguments.site_roots)? {
        return Err("initial build failed".to_owned());
    }

    let (sender, receiver) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = sender.send(event);
    })
    .map_err(|error| format!("could not create file watcher: {error}"))?;
    for root in &project.source_roots {
        watcher
            .watch(root, RecursiveMode::Recursive)
            .map_err(|error| format!("could not watch '{}': {error}", root.display()))?;
    }
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
            .any(|path| should_recompile(&project, path))
        {
            continue;
        }
        while receiver.recv_timeout(Duration::from_millis(75)).is_ok() {}
        let _ = build_and_render(&arguments.path, &arguments.site_roots)?;
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

fn should_recompile(project: &ProjectConfig, path: &Path) -> bool {
    !project.is_excluded(path) && path.extension().and_then(|value| value.to_str()) == Some("osr")
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
}
