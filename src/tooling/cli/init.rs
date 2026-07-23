use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use toml_edit::{Array, DocumentMut, Item, Table, Value, value};

use super::*;

const STARTER_SOURCE: &str = r#"(module main)

;; `uv run osr run src/main.osr` 会编译并执行这个入口。
(defn main []
  (py.print "Hello from Osiris"))
"#;

pub(super) fn run_init(arguments: &[String]) -> CliOutcome {
    let arguments = match parse_init_arguments(arguments) {
        Ok(arguments) => arguments,
        Err(message) => return CliOutcome::usage_error(message),
    };
    let root = PathBuf::from(arguments.path);

    if arguments.existing {
        if !root.join("pyproject.toml").is_file() {
            return init_failure(format!(
                "{} is not an existing uv project (pyproject.toml was not found)",
                root.display()
            ));
        }
    } else if root.exists() {
        return init_failure(format!(
            "cannot create project because {} already exists; use 'osr init --existing {}' to add Osiris",
            root.display(),
            root.display()
        ));
    } else if let Err(message) = uv_init(&root) {
        return init_failure(message);
    }

    let pyproject = root.join("pyproject.toml");
    let configured = match configure_pyproject(&pyproject) {
        Ok(configured) => configured,
        Err(message) => return init_failure(message),
    };
    if let Err(error) = create_starter(&root, &configured.source_root) {
        return init_failure(format!("could not create starter source: {error}"));
    }
    if configured.needs_dependency {
        if let Err(message) = uv_add_osiris(&root) {
            return init_failure(message);
        }
    }

    CliOutcome::success(format!(
        "Initialized Osiris project in {}\nRun: cd {} && uv run osr run src/main.osr\n",
        root.display(),
        root.display()
    ))
}

struct InitArguments<'a> {
    path: &'a str,
    existing: bool,
}

fn parse_init_arguments(arguments: &[String]) -> Result<InitArguments<'_>, String> {
    let mut existing = false;
    let mut path = None;
    for argument in arguments {
        match argument.as_str() {
            "--existing" if existing => {
                return Err("duplicate option '--existing' for 'init'".to_owned());
            }
            "--existing" => existing = true,
            option if option.starts_with('-') => {
                return Err(format!("unknown option '{option}' for 'init'"));
            }
            positional if path.is_none() => path = Some(positional),
            _ => return Err("unexpected arguments for 'init'".to_owned()),
        }
    }
    let path = match (existing, path) {
        (true, None) => ".",
        (_, Some(path)) => path,
        (false, None) => return Err("missing PROJECT for 'init'".to_owned()),
    };
    Ok(InitArguments { path, existing })
}

fn uv_init(root: &Path) -> Result<(), String> {
    let output = Command::new("uv")
        .args(["init", "--bare", "--vcs", "none", "--python", "3.9"])
        .arg(root)
        .output()
        .map_err(|error| format!("could not run uv: {error}"))?;
    command_result("uv init", output)
}

fn uv_add_osiris(root: &Path) -> Result<(), String> {
    let output = Command::new("uv")
        .args(["add", "--dev", "osiris-lang"])
        .current_dir(root)
        .output()
        .map_err(|error| format!("could not run uv: {error}"))?;
    command_result("uv add --dev osiris-lang", output)
}

fn command_result(label: &str, output: std::process::Output) -> Result<(), String> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(format!("{label} failed: {}", stderr.trim()))
}

struct InitConfiguration {
    needs_dependency: bool,
    source_root: PathBuf,
}

fn configure_pyproject(path: &Path) -> Result<InitConfiguration, String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let mut document = source
        .parse::<DocumentMut>()
        .map_err(|error| format!("could not parse {}: {error}", path.display()))?;
    let needs_dependency = !has_osiris_dependency(&document);

    let tool = table_at(&mut document, "tool")?;
    if tool.contains_key("osiris") && !tool["osiris"].is_table() {
        return Err("[tool.osiris] conflicts with a non-table value".to_owned());
    }
    let osiris = table_at_item(tool, "osiris")?;
    set_default(osiris, "source", array_item(["src"]));
    set_default(osiris, "target-python", value("3.9"));
    set_default(osiris, "strict", value(true));
    set_default(osiris, "extensions", array_item([]));
    set_default(osiris, "build-groups", array_item([]));
    let source_root = configured_source_root(osiris)?;

    fs::write(path, document.to_string())
        .map_err(|error| format!("could not update {}: {error}", path.display()))?;
    Ok(InitConfiguration {
        needs_dependency,
        source_root,
    })
}

fn configured_source_root(osiris: &Table) -> Result<PathBuf, String> {
    let source = osiris
        .get("source")
        .and_then(Item::as_array)
        .and_then(|roots| roots.get(0))
        .and_then(Value::as_str)
        .ok_or_else(|| "[tool.osiris].source must be a non-empty string array".to_owned())?;
    let path = PathBuf::from(source);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::RootDir
            )
        })
    {
        return Err("[tool.osiris].source must stay inside the project".to_owned());
    }
    Ok(path)
}

fn table_at<'a>(document: &'a mut DocumentMut, key: &str) -> Result<&'a mut Table, String> {
    if !document.contains_key(key) {
        document[key] = Item::Table(Table::new());
    }
    document[key]
        .as_table_mut()
        .ok_or_else(|| format!("[{key}] conflicts with a non-table value"))
}

fn table_at_item<'a>(parent: &'a mut Table, key: &str) -> Result<&'a mut Table, String> {
    if !parent.contains_key(key) {
        parent[key] = Item::Table(Table::new());
    }
    parent[key]
        .as_table_mut()
        .ok_or_else(|| format!("table '{key}' conflicts with a non-table value"))
}

fn set_default(table: &mut Table, key: &str, item: Item) {
    if !table.contains_key(key) {
        table[key] = item;
    }
}

fn array_item<const N: usize>(entries: [&str; N]) -> Item {
    let mut array = Array::new();
    for entry in entries {
        array.push(entry);
    }
    Item::Value(Value::Array(array))
}

fn has_osiris_dependency(document: &DocumentMut) -> bool {
    dependency_items(document).any(|dependency| dependency_name(dependency) == "osiris-lang")
}

fn dependency_items(document: &DocumentMut) -> impl Iterator<Item = &str> {
    let project = document.get("project").and_then(Item::as_table);
    let project_dependencies = project
        .and_then(|table| table.get("dependencies"))
        .and_then(Item::as_array)
        .into_iter()
        .flat_map(|array| array.iter().filter_map(|value| value.as_str()));
    let groups = document
        .get("dependency-groups")
        .and_then(Item::as_table)
        .into_iter()
        .flat_map(|table| table.iter())
        .flat_map(|(_, item)| item.as_array().into_iter())
        .flat_map(|array| array.iter().filter_map(|value| value.as_str()));
    let legacy_dev = document
        .get("tool")
        .and_then(Item::as_table)
        .and_then(|table| table.get("uv"))
        .and_then(Item::as_table)
        .and_then(|table| table.get("dev-dependencies"))
        .and_then(Item::as_array)
        .into_iter()
        .flat_map(|array| array.iter().filter_map(|value| value.as_str()));
    project_dependencies.chain(groups).chain(legacy_dev)
}

fn dependency_name(requirement: &str) -> String {
    requirement
        .split([' ', '<', '>', '=', '!', '~', '[', ';', '@'])
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .replace('_', "-")
}

fn create_starter(root: &Path, source_root: &Path) -> std::io::Result<()> {
    let source = root.join(source_root).join("main.osr");
    if source.exists() {
        return Ok(());
    }
    fs::create_dir_all(source.parent().expect("starter source has a parent"))?;
    fs::write(source, STARTER_SOURCE)
}

fn init_failure(message: String) -> CliOutcome {
    CliOutcome::failure(1, String::new(), format!("osr: {message}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_new_and_existing_forms() {
        let new_arguments = ["demo".to_owned()];
        let new = parse_init_arguments(&new_arguments).unwrap();
        assert_eq!(new.path, "demo");
        assert!(!new.existing);
        let existing_arguments = ["--existing".to_owned()];
        let existing = parse_init_arguments(&existing_arguments).unwrap();
        assert_eq!(existing.path, ".");
        assert!(existing.existing);
    }

    #[test]
    fn recognizes_normalized_requirements() {
        assert_eq!(dependency_name("Osiris_Lang>=0.1.0"), "osiris-lang");
        assert_eq!(dependency_name("osiris-lang[build]"), "osiris-lang");
    }
}
