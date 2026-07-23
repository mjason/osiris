use super::*;

struct BuildArguments {
    path: PathBuf,
    site_roots: Vec<String>,
}

pub(super) fn run_build(arguments: &[String]) -> CliOutcome {
    let arguments = match parse_build_arguments(arguments) {
        Ok(arguments) => arguments,
        Err(message) => return CliOutcome::usage_error(message),
    };
    run_project_build(&arguments.path, &arguments.site_roots)
}

pub(super) fn run_project_build(path: &Path, site_roots: &[String]) -> CliOutcome {
    let project = match ProjectConfig::discover(path) {
        Ok(project) => project,
        Err(error) => return config_error(&error),
    };
    let entry = match first_project_source(&project) {
        Ok(entry) => entry,
        Err(message) => {
            return CliOutcome::failure(1, String::new(), format!("osr: {message}\n"));
        }
    };
    let mut arguments = vec![entry.display().to_string()];
    for root in site_roots {
        arguments.extend(["--site-root".to_owned(), root.clone()]);
    }
    run_compile(&arguments)
}

fn parse_build_arguments(arguments: &[String]) -> Result<BuildArguments, String> {
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
                return Err(format!("unknown option '{option}' for 'build'"));
            }
            value if path.is_none() => path = Some(PathBuf::from(value)),
            _ => return Err("unexpected arguments for 'build'".to_owned()),
        }
        index += 1;
    }
    Ok(BuildArguments {
        path: path.unwrap_or_else(|| PathBuf::from(".")),
        site_roots,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_defaults_to_the_current_directory() {
        let arguments = parse_build_arguments(&[]).unwrap();
        assert_eq!(arguments.path, Path::new("."));
    }
}
