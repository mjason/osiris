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
            if argument.values.is_empty() {
                continue;
            }
            // Wrap the accepted set so a long list stays readable in a terminal
            // instead of relying on the emulator to fold one very long line.
            let mut line = format!("  {:<18} One of: ", "");
            let indent = line.len();
            for (position, value) in argument.values.iter().enumerate() {
                let separator = if position + 1 == argument.values.len() {
                    ""
                } else {
                    ","
                };
                if line.len() > indent && line.len() + value.len() + 1 > 78 {
                    output.push_str(line.trim_end());
                    output.push('\n');
                    line = " ".repeat(indent);
                }
                line.push_str(value);
                line.push_str(separator);
                line.push(' ');
            }
            output.push_str(line.trim_end());
            output.push('\n');
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
            "syntax", "agents", "doc",
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

    /// The accepted set is enumerated in help, so it has to be the set the
    /// command actually validates against rather than a copy that can drift.
    #[test]
    fn lsc_help_enumerates_every_accepted_operation() {
        let help = command_help("lsc").stdout;
        for operation in LSC_OPERATIONS {
            assert!(
                help.contains(operation),
                "`osr lsc --help` omits `{operation}`"
            );
        }
        assert!(
            help.lines().all(|line| line.len() <= 80),
            "help lines must stay within 80 columns:\n{help}"
        );
    }

    /// An argument without a closed set must not print an empty enumeration.
    #[test]
    fn help_omits_the_accepted_set_for_open_arguments() {
        for command in COMMANDS {
            if command.positionals.iter().any(|a| !a.values.is_empty()) {
                continue;
            }
            let help = command_help(command.name).stdout;
            assert!(
                !help.contains("One of:"),
                "`osr {} --help` prints an empty accepted set",
                command.name
            );
        }
    }
}
