use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde::Deserialize;
use toml_edit::{Array, DocumentMut, Item, Table, Value, value};

use super::*;

const STARTER_SOURCE: &str = r#"(module main)

;; Python 互操作保持显式；编译阶段不会导入或执行 Python 模块。
(py/import builtins :as py)

;; `uv run osr run src/main.osr` 会编译并执行顶层表达式。
(py.print "Hello from Osiris")
"#;

fn package_starter_source(module: &str) -> String {
    format!(
        r#"(module {module}.core)

;; 公开声明会进入 wheel 内的 .osri 接口，并可由下游 Osiris 项目导入。
(export [identity])

^{{:doc {{:default "Return the input value."
          "zh-CN" "返回输入值。"}}}}
(defn ^Any identity [^Any value] value)
"#
    )
}

const PROJECT_CONFIG: &str = r#"{
  "$schema": "https://raw.githubusercontent.com/mjason/osiris/main/schemas/osiris.schema.json",

  // Osiris 模块根目录；目录层级映射为模块名中的点。
  "source": ["src"],
  "outDir": "dist",

  // 单次编译只对应一个 Python target。
  "targetPython": "3.11",
  "strict": true,

  // LSP 展示语言使用标准 BCP 47 tag，例如 zh-CN、ja 或 en。
  "displayLocale": "zh-CN"
}
"#;

fn gitignore_template(out_dir: &str) -> String {
    let header = r#"# uv 管理的环境与编译器缓存，永远不进版本库。
.venv/
__pycache__/
*.pyc
.osiris/
"#;
    // Every rule is present; the ones that encode a choice ship commented, to
    // be opened as needed. `outDir: "."` has no directory to ignore — a `./`
    // line would ignore the whole project — so that layout gets only the
    // per-suffix rules.
    if out_dir == "." {
        format!(
            r#"{header}
# outDir 为 "."：产物与源码同在项目根，没有目录可忽略。需要哪条打开哪条
# 的注释（生成的 .py 与手写 Python 无法按后缀区分）。
# *.osri
# *.py.map
"#
        )
    } else {
        format!(
            r#"{header}
# 生成产物整目录忽略；要提交生成代码（让不运行编译器的消费者直接读）就删掉
# 下面这一行。
{out_dir}/

# outDir 改为 "." 时产物落在项目根，改用按后缀的规则，需要哪条打开哪条的注释。
# *.osri
# *.py.map
"#
        )
    }
}

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
    let setup = match configure_pyproject(&pyproject, arguments.package) {
        Ok(configured) => configured,
        Err(message) => return init_failure(message),
    };
    let (source_root, out_dir) = match configure_osiris_jsonc(&root) {
        Ok(configured) => configured,
        Err(message) => return init_failure(message),
    };
    if let Err(message) = configure_gitignore(&root, &out_dir) {
        return init_failure(message);
    }
    if let Err(error) = create_starter(
        &root,
        &source_root,
        arguments.package.then_some(setup.module.as_str()),
    ) {
        return init_failure(format!("could not create starter source: {error}"));
    }
    if setup.needs_dependency {
        if let Err(message) = uv_add_osiris(&root) {
            return init_failure(message);
        }
    }

    let next = if arguments.package {
        "uv lock && uv build --python 3.11"
    } else {
        "uv run osr run src/main.osr"
    };
    CliOutcome::success(format!(
        "Initialized Osiris project in {}\nRun: cd {} && {next}\n",
        root.display(),
        root.display()
    ))
}

struct InitArguments<'a> {
    path: &'a str,
    existing: bool,
    package: bool,
}

fn parse_init_arguments(arguments: &[String]) -> Result<InitArguments<'_>, String> {
    let mut existing = false;
    let mut package = false;
    let mut path = None;
    for argument in arguments {
        match argument.as_str() {
            "--existing" if existing => {
                return Err("duplicate option '--existing' for 'init'".to_owned());
            }
            "--existing" => existing = true,
            "--package" if package => {
                return Err("duplicate option '--package' for 'init'".to_owned());
            }
            "--package" => package = true,
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
    Ok(InitArguments {
        path,
        existing,
        package,
    })
}

fn uv_init(root: &Path) -> Result<(), String> {
    let output = Command::new("uv")
        .args(["init", "--bare", "--vcs", "none", "--python", "3.11"])
        .arg(root)
        .output()
        .map_err(|error| format!("could not run uv: {error}"))?;
    command_result("uv init", output)
}

fn uv_add_osiris(root: &Path) -> Result<(), String> {
    let requirement = compatible_osiris_requirement();
    let output = Command::new("uv")
        .args(["add", "--dev", &requirement])
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

struct ProjectSetup {
    needs_dependency: bool,
    module: String,
}

fn configure_pyproject(path: &Path, package: bool) -> Result<ProjectSetup, String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let mut document = source
        .parse::<DocumentMut>()
        .map_err(|error| format!("could not parse {}: {error}", path.display()))?;
    let distribution = document
        .get("project")
        .and_then(Item::as_table)
        .and_then(|project| project.get("name"))
        .and_then(Item::as_str)
        .ok_or_else(|| "[project].name is required".to_owned())?;
    let module = package_module_name(distribution);
    let needs_dependency = !has_osiris_dependency(&document);
    if package {
        configure_package_backend(&mut document)?;
        fs::write(path, document.to_string())
            .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    }
    Ok(ProjectSetup {
        needs_dependency,
        module,
    })
}

fn configure_package_backend(document: &mut DocumentMut) -> Result<(), String> {
    if !document.contains_key("build-system") {
        document["build-system"] = Item::Table(Table::new());
    }
    let build = document["build-system"]
        .as_table_mut()
        .ok_or_else(|| "[build-system] conflicts with a non-table value".to_owned())?;
    if let Some(backend) = build.get("build-backend").and_then(Item::as_str)
        && backend != "osiris_build"
    {
        return Err(format!(
            "[build-system].build-backend is `{backend}`; refusing to replace it with `osiris_build`"
        ));
    }
    build["build-backend"] = value("osiris_build");
    let requirement = compatible_osiris_requirement();
    if !build.contains_key("requires") {
        build["requires"] = Item::Value(Value::Array(Array::new()));
    }
    let requires = build["requires"]
        .as_array_mut()
        .ok_or_else(|| "[build-system].requires must be an array".to_owned())?;
    let osiris_requirements = requires
        .iter()
        .filter_map(|item| item.as_str())
        .filter(|item| dependency_name(item) == "osiris-lang")
        .collect::<Vec<_>>();
    if osiris_requirements
        .iter()
        .any(|existing| *existing != requirement)
    {
        return Err(format!(
            "[build-system].requires must pin `{requirement}` for this compiler"
        ));
    }
    if osiris_requirements.is_empty() {
        requires.push(requirement);
    }
    Ok(())
}

fn package_module_name(distribution: &str) -> String {
    let mut module = normalize_distribution_name(distribution).replace('-', "_");
    if module.starts_with(|character: char| character.is_ascii_digit()) {
        module.insert(0, '_');
    }
    module
}

#[derive(Deserialize)]
struct InitJsonc {
    source: Option<Vec<String>>,
    #[serde(rename = "outDir")]
    out_dir: Option<String>,
}

fn configure_osiris_jsonc(root: &Path) -> Result<(PathBuf, String), String> {
    let path = root.join("osiris.jsonc");
    if !path.exists() {
        fs::write(&path, PROJECT_CONFIG)
            .map_err(|error| format!("could not write {}: {error}", path.display()))?;
        return Ok((PathBuf::from("src"), "dist".to_owned()));
    }
    let source = fs::read_to_string(&path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let config: InitJsonc = json5::from_str(&source)
        .map_err(|error| format!("invalid JSONC in {}: {error}", path.display()))?;
    let out_dir = config.out_dir.unwrap_or_else(|| "dist".to_owned());
    let source = config
        .source
        .and_then(|roots| roots.into_iter().next())
        .ok_or_else(|| "osiris.jsonc source must be a non-empty string array".to_owned())?;
    let path = PathBuf::from(source);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::RootDir
            )
        })
    {
        return Err("osiris.jsonc source must stay inside the project".to_owned());
    }
    Ok((path, out_dir))
}

/// Writes `.gitignore` for a fresh project; adds only the compiler cache to an
/// existing one.
///
/// Every rule ships in the file; the ones that encode a choice are commented,
/// to be opened as needed — ignoring the output directory whole, or by suffix
/// where `outDir: "."` leaves no directory to ignore. An existing `.gitignore`
/// records someone's chosen policy, so only the machine-local `.osiris/` cache
/// entry is added active there; the optional rules are appended commented and
/// never uncommented on the author's behalf.
fn configure_gitignore(root: &Path, out_dir: &str) -> Result<(), String> {
    let path = root.join(".gitignore");
    if !path.exists() {
        return fs::write(&path, gitignore_template(out_dir))
            .map_err(|error| format!("could not write {}: {error}", path.display()));
    }
    let existing = fs::read_to_string(&path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let covers_cache = existing.lines().any(|line| {
        matches!(
            line.trim().trim_start_matches('/').trim_end_matches('/'),
            ".osiris"
        )
    });
    if covers_cache {
        return Ok(());
    }
    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str("\n# osr 的编译缓存，机器本地。\n.osiris/\n");
    updated.push_str("# 编译产物按需忽略，需要哪条打开哪条的注释。\n");
    if out_dir != "." {
        updated.push_str(&format!("# {out_dir}/\n"));
    }
    updated.push_str("# *.osri\n# *.py.map\n");
    fs::write(&path, updated)
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}

fn has_osiris_dependency(document: &DocumentMut) -> bool {
    let compatible = compatible_osiris_requirement();
    dependency_items(document).any(|dependency| {
        dependency_name(dependency) == "osiris-lang"
            && (dependency == compatible
                || dependency == format!("osiris-lang=={}", crate::version()))
    })
}

fn compatible_osiris_requirement() -> String {
    let mut parts = crate::version().split('.');
    let major = parts.next().unwrap_or("0");
    let minor = parts.next().unwrap_or("0");
    let next_minor = minor.parse::<u64>().unwrap_or(0) + 1;
    format!("osiris-lang>={major}.{minor},<{major}.{next_minor}")
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

fn create_starter(
    root: &Path,
    source_root: &Path,
    package_module: Option<&str>,
) -> std::io::Result<()> {
    let (relative, contents) = package_module.map_or_else(
        || (PathBuf::from("main.osr"), STARTER_SOURCE.to_owned()),
        |module| {
            (
                PathBuf::from(module).join("core.osr"),
                package_starter_source(module),
            )
        },
    );
    let source = root.join(source_root).join(relative);
    if source.exists() {
        return Ok(());
    }
    fs::create_dir_all(source.parent().expect("starter source has a parent"))?;
    fs::write(source, contents)
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
        assert!(!new.package);
        let existing_arguments = ["--existing".to_owned()];
        let existing = parse_init_arguments(&existing_arguments).unwrap();
        assert_eq!(existing.path, ".");
        assert!(existing.existing);
        assert!(!existing.package);
        let package_arguments = ["--package".to_owned(), "demo-package".to_owned()];
        let package = parse_init_arguments(&package_arguments).unwrap();
        assert_eq!(package.path, "demo-package");
        assert!(package.package);
        assert!(parse_init_arguments(&["--extension".to_owned(), "legacy".to_owned()]).is_err());
    }

    #[test]
    fn gitignore_is_created_whole_but_extended_minimally() {
        let root = std::env::temp_dir().join(format!(
            "osiris-init-gitignore-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("t").len()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        // Fresh project: the output directory is ignored whole and active;
        // the per-suffix rules are present but commented — a choice the
        // author opens, never one made for them.
        configure_gitignore(&root, "generated").unwrap();
        let written = fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(written.lines().any(|line| line == "generated/"));
        assert!(written.lines().any(|line| line == ".osiris/"));
        assert!(written.lines().any(|line| line == "# *.osri"));
        assert!(written.lines().any(|line| line == "# *.py.map"));
        assert!(!written.lines().any(|line| line == "*.osri"));

        // `outDir: "."` has no directory to ignore — a `./` line would
        // swallow the project — so only the commented suffix rules appear.
        fs::remove_file(root.join(".gitignore")).unwrap();
        configure_gitignore(&root, ".").unwrap();
        let in_root = fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(!in_root.lines().any(|line| line == "./" || line == "# ./"));
        assert!(in_root.lines().any(|line| line == "# *.osri"));

        // Existing policy: the cache entry is added active, the optional
        // rules commented, once; the file's own decisions are left alone.
        fs::write(root.join(".gitignore"), ".venv/\n").unwrap();
        configure_gitignore(&root, "dist").unwrap();
        let appended = fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(appended.starts_with(".venv/\n"));
        assert!(appended.lines().any(|line| line == ".osiris/"));
        assert!(appended.lines().any(|line| line == "# dist/"));
        assert!(!appended.lines().any(|line| line == "dist/"));
        configure_gitignore(&root, "dist").unwrap();
        let unchanged = fs::read_to_string(root.join(".gitignore")).unwrap();
        assert_eq!(appended, unchanged);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn recognizes_normalized_requirements() {
        assert_eq!(dependency_name("Osiris_Lang>=0.1.0"), "osiris-lang");
        assert_eq!(dependency_name("osiris-lang[build]"), "osiris-lang");
    }

    #[test]
    fn only_the_current_compiler_requirement_skips_uv_add() {
        let current = format!(
            "[dependency-groups]\ndev = [\"{}\"]\n",
            compatible_osiris_requirement()
        )
        .parse::<DocumentMut>()
        .unwrap();
        let old = "[dependency-groups]\ndev = [\"osiris-lang>=0.2.1\"]\n"
            .parse::<DocumentMut>()
            .unwrap();

        assert!(has_osiris_dependency(&current));
        assert!(!has_osiris_dependency(&old));
    }
}
