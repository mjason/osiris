use std::{
    fs,
    sync::atomic::{AtomicUsize, Ordering},
};

use super::{ProjectConfig, PythonVersion};

static NEXT_TEST: AtomicUsize = AtomicUsize::new(0);

fn fixture(config: &str) -> std::path::PathBuf {
    let id = NEXT_TEST.fetch_add(1, Ordering::Relaxed);
    let root =
        std::env::temp_dir().join(format!("osiris-project-config-{}-{id}", std::process::id()));
    fs::create_dir(&root).expect("fixture root should be created");
    let path = root.join("pyproject.toml");
    fs::write(&path, "[project]\nname = \"sample\"\nversion = \"0\"\n")
        .expect("fixture TOML should be written");
    fs::write(root.join("osiris.jsonc"), config).expect("fixture JSONC should be written");
    path
}

#[test]
fn loads_jsonc_configuration() {
    let path = fixture(
        r#"{
          // JSONC comments and trailing commas are accepted.
          "source": ["osr-src"],
          "targetPython": "3.11",
          "strict": true,
          "displayLocale": "zh-CN",
        }"#,
    );
    let config = ProjectConfig::load(&path).expect("configuration should load");

    assert_eq!(
        config.target_python,
        PythonVersion {
            major: 3,
            minor: 11
        }
    );
    assert_eq!(config.distribution, "sample");
    assert_eq!(config.distribution_version, "0");
    assert_eq!(config.display_locale.as_deref(), Some("zh-CN"));
    let root = path.parent().expect("fixture has parent");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn strict_configuration_controls_unknown_fields() {
    let path = fixture(r#"{"strict": false, "futureOption": {"enabled": true}}"#);
    let config = ProjectConfig::load(&path).expect("non-strict config accepts future fields");
    assert!(!config.strict);
    let root = path.parent().expect("fixture has parent");
    let _ = fs::remove_dir_all(root);

    let path = fixture(r#"{"strict": true, "futureOption": true}"#);
    let error = ProjectConfig::load(&path).expect_err("strict config rejects future fields");
    assert!(error.to_string().contains("unknown osiris.jsonc field"));
    let root = path.parent().expect("fixture has parent");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn jsonc_rejects_duplicate_unknown_fields() {
    let path = fixture(r#"{"strict": false, "futureOption": 1, "futureOption": 2}"#);
    let error = ProjectConfig::load(&path).expect_err("duplicate fields are never valid JSONC");
    assert!(error.to_string().contains("duplicate JSONC field"));
    let root = path.parent().expect("fixture has parent");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn loads_bare_relative_pyproject_path_from_project_root() {
    let id = NEXT_TEST.fetch_add(1, Ordering::Relaxed);
    let fixture_name = format!(".osiris-relative-project-config-{}-{id}", std::process::id());
    let source_root_name = format!(
        ".osiris-relative-project-source-{}-{id}",
        std::process::id()
    );
    let fixture_root = std::path::PathBuf::from(&fixture_name);
    let pyproject = fixture_root.join("pyproject.toml");
    let source_root = fixture_root.join(&source_root_name);
    let source = source_root.join("value.osr");
    fs::create_dir(&fixture_root).expect("relative project root should be created");
    fs::create_dir(&source_root).expect("relative source root should be created");
    fs::write(&source, "(module value)\n").expect("relative source should be written");
    fs::write(&pyproject, "[project]\nname = \"relative\"\nversion = \"0\"\n")
    .expect("relative configuration should be written");
    fs::write(
        fixture_root.join("osiris.jsonc"),
        format!("{{\"source\": [\"{source_root_name}\"]}}"),
    )
    .expect("relative JSONC should be written");

    let config = ProjectConfig::load(&pyproject).expect("relative configuration should load");
    let current_dir = std::env::current_dir().expect("current directory should be available");
    assert_eq!(config.root, current_dir.join(&fixture_root));
    let absolute_source = config.root.join(&source_root_name).join("value.osr");
    assert_eq!(
        config.module_name_for_source(&absolute_source).unwrap(),
        "value"
    );

    let _ = fs::remove_dir_all(fixture_root);
}

#[test]
fn maps_source_paths_to_unique_module_names() {
    let path = fixture(r#"{"source": ["src"]}"#);
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
    let path = fixture(r#"{"source": ["src", "src/pkg"]}"#);
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

    let path = fixture(r#"{"source": ["src"]}"#);
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
    let path = fixture(r#"{"source": ["../outside"]}"#);
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
fn accepts_well_formed_display_locale_and_rejects_malformed_tags() {
    let path = fixture("{}");
    let config = ProjectConfig::load(&path).expect("default project locale");
    assert_eq!(config.display_locale.as_deref(), Some("zh-CN"));
    let root = path.parent().expect("fixture has parent");
    let _ = fs::remove_dir_all(root);

    let path = fixture(r#"{"displayLocale": "zh"}"#);
    let config = ProjectConfig::load(&path).expect("a well-formed BCP 47 tag");
    assert_eq!(config.display_locale.as_deref(), Some("zh"));
    let root = path.parent().expect("fixture has parent");
    let _ = fs::remove_dir_all(root);

    let path = fixture(r#"{"displayLocale": "not_a_locale"}"#);
    let error = ProjectConfig::load(&path).expect_err("malformed locale must fail");
    assert!(error.to_string().contains("BCP 47"));
    let root = path.parent().expect("fixture has parent");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn rejects_removed_and_unknown_configuration_fields() {
    for field in [
        "watch",
        "emit",
        "extensions",
        "buildGroups",
        "trust",
        "unknown",
    ] {
        let path = fixture(&format!(r#"{{"{field}": {{}}}}"#));
        let error = ProjectConfig::load(&path).expect_err("unknown fields must fail closed");
        assert!(
            error.to_string().contains("unknown osiris.jsonc field"),
            "{error}"
        );
        let root = path.parent().expect("fixture has parent");
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn minimal_configuration_uses_defaults() {
    let path = fixture("{}");
    let config = ProjectConfig::load(&path).expect("minimal configuration should load");
    assert_eq!(config.target_python, PythonVersion::DEFAULT_TARGET);
    assert_eq!(config.default_output_dir(), config.root.join("dist"));
    let root = path.parent().expect("fixture has parent");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn rejects_empty_duplicate_and_output_owned_source_roots() {
    for (config, expected) in [
        (r#"{"source": []}"#, "at least one source root"),
        (
            r#"{"source": ["src", "src"]}"#,
            "duplicate normalized source root",
        ),
        (
            r#"{"source": ["dist/source"], "outDir": "dist"}"#,
            "must not be inside output directory",
        ),
    ] {
        let path = fixture(config);
        let error = ProjectConfig::load(&path).expect_err("invalid source roots must fail");
        assert!(error.to_string().contains(expected), "{error}");
        let root = path.parent().expect("fixture has parent");
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn output_directory_is_always_excluded_from_broad_source_scope() {
    let path = fixture(r#"{"source": ["src"], "outDir": "src/generated"}"#);
    let config = ProjectConfig::load(&path).expect("output may be nested below a source root");
    assert!(config.is_excluded(&config.root.join("src/generated/module.osr")));
    assert!(!config.is_excluded(&config.root.join("src/module.osr")));
    let root = path.parent().expect("fixture has parent");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn discovers_project_virtual_environment_without_running_python() {
    let path = fixture("{}");
    let config = ProjectConfig::load(&path).expect("minimal configuration should load");
    let site_packages = config.root.join(".venv/lib/python3.11/site-packages");
    fs::create_dir_all(&site_packages).expect("site-packages fixture should be created");

    assert!(config.installed_package_roots().contains(&site_packages));

    let _ = fs::remove_dir_all(&config.root);
}

#[test]
fn loads_output_and_glob_exclusions() {
    let path = fixture(
        r#"{
          "outDir": "generated/python",
          "exclude": ["src/generated", "src/**/cache/**", "src/**/*_test.osr"]
        }"#,
    );
    let config = ProjectConfig::load(&path).unwrap();
    assert_eq!(config.output_dir, config.root.join("generated/python"));
    assert!(config.is_excluded(&config.root.join("src/generated/value.osr")));
    assert!(config.is_excluded(&config.root.join("src/pkg/cache/value.osr")));
    assert!(config.is_excluded(&config.root.join("src/pkg/value_test.osr")));
    assert!(!config.is_excluded(&config.root.join("src/pkg/value.osr")));
    let root = path.parent().unwrap();
    let _ = fs::remove_dir_all(root);
}
