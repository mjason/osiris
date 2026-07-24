use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use super::*;

static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

pub(super) fn run_fmt(arguments: &[String]) -> CliOutcome {
    let request = match parse_fmt_arguments(arguments) {
        Ok(request) => request,
        Err(message) => return CliOutcome::usage_error(message),
    };
    if request.stdin {
        return CliOutcome::usage_error("'fmt -' requires standard input");
    }
    format_paths(&request.paths, request.check)
}

pub fn run_fmt_stdio(arguments: &[String]) -> io::Result<CliOutcome> {
    let request = match parse_fmt_arguments(arguments) {
        Ok(request) => request,
        Err(message) => return Ok(CliOutcome::usage_error(message)),
    };
    if !request.stdin {
        return Ok(format_paths(&request.paths, request.check));
    }
    let mut source = String::new();
    io::stdin().lock().read_to_string(&mut source)?;
    Ok(match crate::formatter::format_source(&source) {
        Ok(formatted) => CliOutcome::success(formatted),
        Err(error) => CliOutcome::failure(
            1,
            String::new(),
            diagnostic::render_all("<stdin>", &source, &error.diagnostics),
        ),
    })
}

struct FmtRequest {
    paths: Vec<PathBuf>,
    check: bool,
    stdin: bool,
}

fn parse_fmt_arguments(arguments: &[String]) -> Result<FmtRequest, String> {
    let mut paths = Vec::new();
    let mut check = false;
    let mut stdin = false;
    let mut all = false;
    for argument in arguments {
        match argument.as_str() {
            "--check" if check => return Err("duplicate option '--check' for 'fmt'".to_owned()),
            "--check" => check = true,
            "--all" if all => return Err("duplicate option '--all' for 'fmt'".to_owned()),
            "--all" if stdin || !paths.is_empty() => {
                return Err("'--all' cannot be combined with paths or '-' for 'fmt'".to_owned());
            }
            "--all" => all = true,
            "-" if stdin || !paths.is_empty() => {
                return Err("'-' cannot be combined with paths for 'fmt'".to_owned());
            }
            "-" if all => return Err("'-' cannot be combined with '--all' for 'fmt'".to_owned()),
            "-" => stdin = true,
            option if option.starts_with('-') => {
                return Err(format!("unknown option '{option}' for 'fmt'"));
            }
            path if stdin => return Err(format!("path '{path}' cannot be combined with '-'")),
            path if all => return Err(format!("path '{path}' cannot be combined with '--all'")),
            path => paths.push(PathBuf::from(path)),
        }
    }
    if stdin && check {
        return Err("'--check' cannot be combined with '-' for 'fmt'".to_owned());
    }
    Ok(FmtRequest {
        paths,
        check,
        stdin,
    })
}

fn format_paths(requested: &[PathBuf], check: bool) -> CliOutcome {
    let (mut paths, root) = match selected_paths(requested) {
        Ok(selection) => selection,
        Err(message) => return CliOutcome::failure(1, String::new(), format!("osr: {message}\n")),
    };
    paths.sort();
    paths.dedup();

    let mut changed = Vec::new();
    let mut failures = String::new();
    for path in paths {
        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) => {
                failures.push_str(&format!(
                    "osr: could not read '{}': {error}\n",
                    path.display()
                ));
                continue;
            }
        };
        let formatted = match crate::formatter::format_source(&source) {
            Ok(formatted) => formatted,
            Err(error) => {
                failures.push_str(&diagnostic::render_all(
                    &display_path(&path, &root),
                    &source,
                    &error.diagnostics,
                ));
                continue;
            }
        };
        if formatted == source {
            continue;
        }
        if !check && let Err(error) = replace_file(&path, formatted.as_bytes()) {
            failures.push_str(&format!(
                "osr: could not format '{}': {error}\n",
                path.display()
            ));
            continue;
        }
        changed.push(display_path(&path, &root));
    }

    let stdout = changed
        .iter()
        .map(|path| format!("{path}\n"))
        .collect::<String>();
    if !failures.is_empty() || (check && !changed.is_empty()) {
        let stderr = if check && !changed.is_empty() {
            format!(
                "{failures}osr: {} file(s) require formatting\n",
                changed.len()
            )
        } else {
            failures
        };
        CliOutcome::failure(1, stdout, stderr)
    } else {
        CliOutcome::success(stdout)
    }
}

fn selected_paths(requested: &[PathBuf]) -> Result<(Vec<PathBuf>, PathBuf), String> {
    if requested.is_empty() {
        let project = ProjectConfig::discover(Path::new(".")).map_err(|error| error.to_string())?;
        let mut paths = Vec::new();
        for source_root in &project.source_roots {
            collect_osiris_sources(source_root, &project, &mut paths)?;
        }
        return Ok((paths, project.root));
    }

    let current = std::env::current_dir().map_err(|error| error.to_string())?;
    let mut paths = Vec::new();
    for requested in requested {
        if requested.is_dir() {
            collect_explicit_directory(requested, &mut paths)?;
        } else if requested.extension().and_then(|value| value.to_str()) == Some("osr") {
            paths.push(requested.clone());
        } else {
            return Err(format!(
                "'{}' is not an .osr file or directory",
                requested.display()
            ));
        }
    }
    Ok((paths, current))
}

fn collect_explicit_directory(directory: &Path, paths: &mut Vec<PathBuf>) -> Result<(), String> {
    let project = ProjectConfig::discover(directory).ok();
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("could not scan '{}': {error}", directory.display()))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("could not scan '{}': {error}", directory.display()))?;
        let path = entry.path();
        if project
            .as_ref()
            .is_some_and(|project| project.is_excluded(&path))
        {
            continue;
        }
        let kind = entry
            .file_type()
            .map_err(|error| format!("could not inspect '{}': {error}", path.display()))?;
        if kind.is_dir() {
            collect_explicit_directory(&path, paths)?;
        } else if kind.is_file() && path.extension().and_then(|value| value.to_str()) == Some("osr")
        {
            paths.push(path);
        }
    }
    Ok(())
}

fn display_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn replace_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    let permissions = fs::metadata(path)?.permissions();
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("source.osr");
    for _ in 0..100 {
        let id = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(".{name}.osr-fmt-{}-{id}", std::process::id()));
        let opened = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary);
        let mut file = match opened {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let result = (|| {
            file.set_permissions(permissions.clone())?;
            file.write_all(contents)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        return result;
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate formatter temporary file",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdin_is_exclusive() {
        assert!(parse_fmt_arguments(&["-".to_owned(), "file.osr".to_owned()]).is_err());
        assert!(parse_fmt_arguments(&["-".to_owned(), "--check".to_owned()]).is_err());
        assert!(parse_fmt_arguments(&["--all".to_owned(), "file.osr".to_owned()]).is_err());
        assert!(parse_fmt_arguments(&["--all".to_owned(), "-".to_owned()]).is_err());
        assert!(parse_fmt_arguments(&["--all".to_owned(), "--all".to_owned()]).is_err());
    }
}
