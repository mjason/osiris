use std::{fs, path::PathBuf};

use super::{ProjectConfig, config_error};
use crate::cli::CliOutcome;

/// `osr clean` removes what builds produced and nothing else.
///
/// Two output modes, two removal strategies. A dedicated `outDir` is
/// compiler-owned and goes whole. With `outDir: "."` the artifacts sit among
/// authored files — often another framework's tree — so removal follows the
/// in-place publication manifest exactly: every build recorded what it wrote,
/// and clean deletes recorded paths only. The `.osiris/` cache goes in both
/// modes; it is machine-local derived state.
pub(super) fn run_clean(arguments: &[String]) -> CliOutcome {
    let path = match parse_clean_arguments(arguments) {
        Ok(path) => path,
        Err(message) => return CliOutcome::usage_error(message),
    };
    let project = match ProjectConfig::discover(&path) {
        Ok(project) => project,
        Err(error) => return config_error(&error),
    };

    let mut report = String::new();
    match crate::artifact::clean_published_artifacts(&project.root) {
        Ok(0) => {}
        Ok(removed) => {
            report.push_str(&format!("removed {removed} tracked in-place artifacts\n"));
        }
        Err(error) => {
            return CliOutcome::failure(
                1,
                String::new(),
                format!("osr: could not remove tracked artifacts: {error}\n"),
            );
        }
    }
    if project.output_dir != project.root && project.output_dir.exists() {
        if let Err(error) = fs::remove_dir_all(&project.output_dir) {
            return CliOutcome::failure(
                1,
                String::new(),
                format!(
                    "osr: could not remove '{}': {error}\n",
                    project.output_dir.display()
                ),
            );
        }
        report.push_str(&format!("removed {}\n", project.output_dir.display()));
    }
    let cache = project.root.join(".osiris");
    if cache.exists() {
        if let Err(error) = fs::remove_dir_all(&cache) {
            return CliOutcome::failure(
                1,
                String::new(),
                format!("osr: could not remove '{}': {error}\n", cache.display()),
            );
        }
        report.push_str(&format!("removed {}\n", cache.display()));
    }
    if report.is_empty() {
        report.push_str("nothing to clean\n");
    }
    CliOutcome::success(report)
}

fn parse_clean_arguments(arguments: &[String]) -> Result<PathBuf, String> {
    let mut path = None;
    for argument in arguments {
        match argument.as_str() {
            option if option.starts_with('-') => {
                return Err(format!("unknown option '{option}' for 'clean'"));
            }
            positional if path.is_none() => path = Some(PathBuf::from(positional)),
            _ => return Err("unexpected arguments for 'clean'".to_owned()),
        }
    }
    Ok(path.unwrap_or_else(|| PathBuf::from(".")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_accepts_at_most_one_path() {
        assert_eq!(parse_clean_arguments(&[]).unwrap(), PathBuf::from("."));
        assert_eq!(
            parse_clean_arguments(&["demo".to_owned()]).unwrap(),
            PathBuf::from("demo")
        );
        assert!(parse_clean_arguments(&["--force".to_owned()]).is_err());
        assert!(parse_clean_arguments(&["a".to_owned(), "b".to_owned()]).is_err());
    }
}
