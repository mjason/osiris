use super::*;

pub(crate) fn root_help() -> String {
    let mut output = String::from("Usage: osr <command> [options]\n\nCommands:\n");
    for command in COMMANDS {
        output.push_str(&format!("  {:<10} {}\n", command.name, command.summary));
    }
    output.push_str("\nOptions:\n  -V, --version  Print version\n  -h, --help     Print help\n");
    output
}

pub(crate) fn help_request(arguments: &[String]) -> Option<CliOutcome> {
    match arguments {
        [flag] if matches!(flag.as_str(), "-h" | "--help") => {
            Some(CliOutcome::success(root_help()))
        }
        [command, flag] if matches!(flag.as_str(), "-h" | "--help") => Some(command_help(command)),
        [flag, format_option, format] if flag == "--help" && format_option == "--format" => {
            Some(machine_help(format))
        }
        _ => None,
    }
}

fn command_help(name: &str) -> CliOutcome {
    let Some(command) = COMMANDS
        .iter()
        .find(|command| command.name == name || command.aliases.contains(&name))
    else {
        return CliOutcome::usage_error(format!("unknown command '{name}'"));
    };
    let mut output = format!("Usage: {}\n\n{}\n", command.synopsis, command.summary);
    if !command.positionals.is_empty() {
        output.push_str("\nArguments:\n");
        for argument in command.positionals {
            output.push_str(&format!("  {:<18} {}\n", argument.name, argument.summary));
        }
    }
    output.push_str("\nOptions:\n  -h, --help         Print help\n");
    for option in command.options {
        output.push_str(&format!("  {:<18} {}\n", option.name, option.summary));
    }
    if let Some(example) = command.examples.first() {
        output.push_str(&format!("\nExample:\n  {example}\n"));
    }
    CliOutcome::success(output)
}

fn machine_help(format: &str) -> CliOutcome {
    let value = match format {
        "json" => serde_json::json!({"schema": REGISTRY_SCHEMA, "commands": COMMANDS}),
        "completion" => serde_json::json!({
            "schema": REGISTRY_SCHEMA,
            "commands": COMMANDS.iter().map(|command| serde_json::json!({
                "name": command.name,
                "aliases": command.aliases,
                "options": command.options.iter().map(|option| option.name).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        }),
        _ => return CliOutcome::usage_error("--help --format must be 'json' or 'completion'"),
    };
    match serde_json::to_string_pretty(&value) {
        Ok(mut output) => {
            output.push('\n');
            CliOutcome::success(output)
        }
        Err(error) => CliOutcome::failure(
            1,
            String::new(),
            format!("osr: could not serialize command registry: {error}\n"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_contains_each_required_command_once() {
        let names = COMMANDS
            .iter()
            .map(|command| command.name)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(names.len(), COMMANDS.len());
        for required in [
            "init", "check", "build", "compile", "watch", "run", "fmt", "expand", "lsc", "lsp",
            "syntax", "doc",
        ] {
            assert!(names.contains(required));
        }
        for command in COMMANDS {
            assert!(!command.requirements.is_empty(), "{}", command.name);
            assert!(!command.diagnostics.is_empty(), "{}", command.name);
            if !command.positionals.is_empty() {
                assert!(!command.examples.is_empty(), "{}", command.name);
            }
        }
    }
}
