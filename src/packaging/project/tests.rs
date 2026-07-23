use std::{
    fs,
    sync::atomic::{AtomicUsize, Ordering},
};

use super::{ProjectConfig, PythonVersion};

static NEXT_TEST: AtomicUsize = AtomicUsize::new(0);

fn fixture(contents: &str) -> std::path::PathBuf {
    let id = NEXT_TEST.fetch_add(1, Ordering::Relaxed);
    let root =
        std::env::temp_dir().join(format!("osiris-project-config-{}-{id}", std::process::id()));
    fs::create_dir(&root).expect("fixture root should be created");
    let path = root.join("pyproject.toml");
    fs::write(&path, contents).expect("fixture TOML should be written");
    path
}

#[test]
fn loads_tool_configuration_and_trust_contracts() {
    let path = fixture(
        r#"
[project]
name = "sample"

[tool.osiris]
source = ["osr-src"]
target-python = "3.11"
strict = true
extensions = ["osiris-data-ext"]
display-locale = "zh-CN"

[[tool.osiris.trust.contract]]
distribution = "osiris-data-ext"
semantic-interface-hash = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
ids = ["osiris.data.mean"]
"#,
    );
    let config = ProjectConfig::load(&path).expect("configuration should load");

    assert_eq!(
        config.target_python,
        PythonVersion {
            major: 3,
            minor: 11
        }
    );
    assert_eq!(config.extensions, ["osiris-data-ext"]);
    assert_eq!(config.distribution, "sample");
    assert_eq!(config.distribution_version, "0");
    assert_eq!(config.display_locale.as_deref(), Some("zh-CN"));
    assert_eq!(config.trust_contracts.len(), 1);
    let root = path.parent().expect("fixture has parent");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn loads_bare_relative_pyproject_path_from_project_root() {
    let id = NEXT_TEST.fetch_add(1, Ordering::Relaxed);
    let fixture_name = format!(
        ".osiris-relative-project-config-{}-{id}.toml",
        std::process::id()
    );
    let source_root_name = format!(
        ".osiris-relative-project-source-{}-{id}",
        std::process::id()
    );
    let pyproject = std::path::PathBuf::from(&fixture_name);
    let source_root = std::path::PathBuf::from(&source_root_name);
    let source = source_root.join("value.osr");
    fs::create_dir(&source_root).expect("relative source root should be created");
    fs::write(&source, "(module value)\n").expect("relative source should be written");
    fs::write(
        &pyproject,
        format!("[tool.osiris]\nsource = [\"{source_root_name}\"]\n"),
    )
    .expect("relative configuration should be written");

    let config = ProjectConfig::load(&pyproject).expect("relative configuration should load");
    let current_dir = std::env::current_dir().expect("current directory should be available");
    assert_eq!(config.root, current_dir);
    let absolute_source = config.root.join(&source);
    assert_eq!(
        config.module_name_for_source(&absolute_source).unwrap(),
        "value"
    );

    let _ = fs::remove_file(pyproject);
    let _ = fs::remove_dir_all(source_root);
}

#[test]
fn maps_source_paths_to_unique_module_names() {
    let path = fixture("[tool.osiris]\nsource = [\"src\"]\n");
    let root = path.parent().expect("fixture has parent");
    let source = root.join("src/数据/归一化.osr");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, "(module 数据.归一化)\n").unwrap();
    let config = ProjectConfig::load(&path).unwrap();
    assert_eq!(
        config.module_name_for_source(&source).unwrap(),
        "数据.归一化"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn rejects_ambiguous_nested_source_roots() {
    let path = fixture("[tool.osiris]\nsource = [\"src\", \"src/pkg\"]\n");
    let root = path.parent().expect("fixture has parent");
    let source = root.join("src/pkg/value.osr");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, "(module value)\n").unwrap();
    let config = ProjectConfig::load(&path).unwrap();
    let error = config.module_name_for_source(&source).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("multiple configured source roots")
    );
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn rejects_symlinked_source_identity() {
    use std::os::unix::fs::symlink;

    let path = fixture("[tool.osiris]\nsource = [\"src\"]\n");
    let root = path.parent().expect("fixture has parent");
    let real = root.join("src/real.osr");
    let linked = root.join("src/linked.osr");
    fs::create_dir_all(real.parent().unwrap()).unwrap();
    fs::write(&real, "(module real)\n").unwrap();
    symlink(&real, &linked).unwrap();
    let config = ProjectConfig::load(&path).unwrap();
    let error = config.module_name_for_source(&linked).unwrap_err();
    assert!(error.to_string().contains("must not contain symlinks"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn rejects_source_paths_that_escape_the_project() {
    let path = fixture("[tool.osiris]\nsource = [\"../outside\"]\n");
    let error = ProjectConfig::load(&path).expect_err("escaping source root must fail");
    assert!(error.to_string().contains("normalized relative path"));
    let root = path.parent().expect("fixture has parent");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn rejects_unsupported_python_versions() {
    let error = "3.8"
        .parse::<PythonVersion>()
        .expect_err("Python 3.8 must be rejected");
    assert!(error.to_string().contains("supported minimum"));
}

#[test]
fn rejects_empty_and_duplicate_build_groups() {
    for (groups, expected) in [
        (r#"["osiris", ""]"#, "entries must not be empty"),
        (r#"["osiris", "osiris"]"#, "must not contain duplicates"),
    ] {
        let path = fixture(&format!("[tool.osiris]\nbuild-groups = {groups}\n"));
        let error = ProjectConfig::load(&path).expect_err("invalid groups must be rejected");
        assert!(error.to_string().contains(expected), "{error}");
        let root = path.parent().expect("fixture has parent");
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn omitted_build_groups_do_not_select_an_implicit_dependency_group() {
    let path = fixture("[tool.osiris]\n");
    let config = ProjectConfig::load(&path).expect("minimal configuration should load");
    assert!(config.build_groups.is_empty());
    let root = path.parent().expect("fixture has parent");
    let _ = fs::remove_dir_all(root);
}
