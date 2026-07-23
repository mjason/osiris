use super::*;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[cfg(unix)]
#[test]
fn init_new_delegates_project_and_dependency_management_to_uv() {
    let fixture = SourceFixture::new("none\n");
    let bin = fixture.directory.join("bin");
    fs::create_dir(&bin).unwrap();
    let uv = bin.join("uv");
    fs::write(
        &uv,
        r#"#!/bin/sh
if [ "$1" = "init" ]; then
  for target do :; done
  mkdir -p "$target"
  printf '[project]\nname = "new-app"\nversion = "0.1.0"\nrequires-python = ">=3.9"\ndependencies = []\n' > "$target/pyproject.toml"
  exit 0
fi
if [ "$1" = "add" ]; then
  printf '%s\n' "$*" > .uv-add-invocation
  exit 0
fi
exit 9
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&uv).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&uv, permissions).unwrap();
    let project = fixture.directory.join("new-app");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = Command::new(env!("CARGO_BIN_EXE_osr"))
        .args(["init", path_argument(&project)])
        .env("PATH", path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let configured = fs::read_to_string(project.join("pyproject.toml")).unwrap();
    assert!(configured.contains("[tool.osiris]"));
    assert!(configured.contains("source = [\"src\"]"));
    assert_eq!(
        fs::read_to_string(project.join(".uv-add-invocation")).unwrap(),
        "add --dev osiris-lang\n"
    );
    let starter = project.join("src/main.osr");
    assert!(starter.is_file());
    let check = osr(&["check", path_argument(&starter)]);
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
}

#[test]
fn init_existing_preserves_uv_project_and_is_idempotent() {
    let fixture = SourceFixture::new("none\n");
    let pyproject = fixture.directory.join("pyproject.toml");
    fs::write(
        &pyproject,
        r#"# This comment belongs to the application.
[project]
name = "existing-app"
version = "2.3.4"
dependencies = ["requests>=2"]

[dependency-groups]
dev = ["osiris-lang>=0.1.0", "pytest>=8"]

[tool.example]
keep = "unchanged"
"#,
    )
    .unwrap();

    for _ in 0..2 {
        let output = osr(&["init", "--existing", path_argument(&fixture.directory)]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let configured = fs::read_to_string(pyproject).unwrap();
    assert!(configured.starts_with("# This comment belongs to the application."));
    assert!(configured.contains("requests>=2"));
    assert!(configured.contains("keep = \"unchanged\""));
    assert_eq!(configured.matches("[tool.osiris]").count(), 1);
    assert!(configured.contains("source = [\"src\"]"));
    assert!(fixture.directory.join("src/main.osr").is_file());
}

#[test]
fn init_existing_does_not_replace_an_existing_starter() {
    let fixture = SourceFixture::new("none\n");
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"demo\"\nversion = \"1\"\n\n[dependency-groups]\ndev = [\"osiris-lang\"]\n",
    )
    .unwrap();
    let starter = fixture.write("src/main.osr", "(module main)\n(def answer 42)\n");

    let output = osr(&["init", "--existing", path_argument(&fixture.directory)]);

    assert!(output.status.success());
    assert_eq!(
        fs::read_to_string(starter).unwrap(),
        "(module main)\n(def answer 42)\n"
    );
}

#[test]
fn init_existing_uses_the_configured_source_root() {
    let fixture = SourceFixture::new("none\n");
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"demo\"\nversion = \"1\"\n\n[dependency-groups]\ndev = [\"osiris-lang\"]\n\n[tool.osiris]\nsource = [\"lisp\"]\n",
    )
    .unwrap();

    let output = osr(&["init", "--existing", path_argument(&fixture.directory)]);

    assert!(output.status.success());
    assert!(fixture.directory.join("lisp/main.osr").is_file());
    assert!(!fixture.directory.join("src/main.osr").exists());
}
