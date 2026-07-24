use std::{thread, time::Duration};

use super::*;

fn wait_for(mut predicate: impl FnMut() -> bool) -> bool {
    for _ in 0..100 {
        if predicate() {
            return true;
        }
        thread::sleep(Duration::from_millis(25));
    }
    false
}

#[test]
fn watch_builds_initially_and_rebuilds_changed_sources() {
    let fixture = SourceFixture::new("none\n");
    let source = fixture.write("src/main.osr", "(module main)\n(def value 1)\n");
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"watch-project\"\nversion = \"1.0\"\n",
    )
    .unwrap();
    fs::write(
        fixture.directory.join("osiris.jsonc"),
        r#"{"source":["src"],"outDir":"dist","targetPython":"3.11"}"#,
    )
    .unwrap();
    let generated = fixture.directory.join("dist/main.py");
    let mut child = Command::new(env!("CARGO_BIN_EXE_osr"))
        .args(["watch", path_argument(&fixture.directory)])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let initial = wait_for(|| generated.is_file());
    let initial_python = fs::read_to_string(&generated).unwrap_or_default();
    if initial {
        fs::write(&source, "(module main)\n(def value 2)\n").unwrap();
    }
    let rebuilt = initial
        && wait_for(|| {
            fs::read_to_string(&generated)
                .is_ok_and(|generated| generated != initial_python && generated.contains('2'))
        });

    let _ = child.kill();
    let output = child.wait_with_output().expect("watch process should stop");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        initial,
        "watch did not complete its initial build ({}): stdout={stdout:?}, stderr={stderr:?}",
        output.status
    );
    assert!(rebuilt, "watch did not rebuild the changed source");
}

#[test]
fn watch_refreshes_source_roots_after_configuration_changes() {
    let fixture = SourceFixture::new("none\n");
    let old_source = fixture.write("src/main.osr", "(module main)\n(def value 1)\n");
    let next_source = fixture.write("next/main.osr", "(module main)\n(def value 2)\n");
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"watch-refresh\"\nversion = \"1.0\"\n",
    )
    .unwrap();
    let config = fixture.directory.join("osiris.jsonc");
    fs::write(
        &config,
        r#"{"source":["src"],"outDir":"dist","targetPython":"3.11"}"#,
    )
    .unwrap();
    let generated = fixture.directory.join("dist/main.py");
    let mut child = Command::new(env!("CARGO_BIN_EXE_osr"))
        .args(["watch", path_argument(&fixture.directory)])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let initial = wait_for(|| {
        fs::read_to_string(&generated).is_ok_and(|python| python.contains("value: int = 1"))
    });
    if initial {
        fs::write(
            &config,
            r#"{"source":["next"],"outDir":"dist","targetPython":"3.11"}"#,
        )
        .unwrap();
    }
    let switched = initial
        && wait_for(|| {
            fs::read_to_string(&generated).is_ok_and(|python| python.contains("value: int = 2"))
        });
    if switched {
        fs::write(&next_source, "(module main)\n(def value 3)\n").unwrap();
    }
    let rebuilt_new_root = switched
        && wait_for(|| {
            fs::read_to_string(&generated).is_ok_and(|python| python.contains("value: int = 3"))
        });
    if rebuilt_new_root {
        fs::write(&old_source, "(module main)\n(def value 99)\n").unwrap();
        thread::sleep(Duration::from_millis(250));
    }
    let ignored_old_root = rebuilt_new_root
        && fs::read_to_string(&generated).is_ok_and(|python| python.contains("value: int = 3"));

    let _ = child.kill();
    let output = child.wait_with_output().expect("watch process should stop");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        initial,
        "watch did not complete initial build: stdout={stdout:?}, stderr={stderr:?}"
    );
    assert!(
        switched,
        "watch did not build the configured source root: stdout={stdout:?}, stderr={stderr:?}"
    );
    assert!(
        rebuilt_new_root,
        "watch did not observe the new source root"
    );
    assert!(
        ignored_old_root,
        "watch still observed the removed source root"
    );
}

#[test]
fn watch_does_not_start_after_a_failed_initial_build() {
    let fixture = SourceFixture::new("none\n");
    fixture.write("src/main.osr", "(module main\n");
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"broken-watch\"\nversion = \"1.0\"\n",
    )
    .unwrap();
    fs::write(
        fixture.directory.join("osiris.jsonc"),
        r#"{"source":["src"],"outDir":"dist"}"#,
    )
    .unwrap();

    let output = osr(&["watch", path_argument(&fixture.directory)]);

    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Watching"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("initial build failed"));
}

#[cfg(unix)]
#[test]
fn native_watch_exits_on_sigint() {
    use std::os::unix::process::ExitStatusExt;

    let fixture = SourceFixture::new("none\n");
    fixture.write("src/main.osr", "(module main)\n(def value 1)\n");
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"signal-project\"\nversion = \"1.0\"\n",
    )
    .unwrap();
    fs::write(
        fixture.directory.join("osiris.jsonc"),
        r#"{"source":["src"],"outDir":"dist"}"#,
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_osr"))
        .args(["watch", path_argument(&fixture.directory)])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    assert!(
        wait_for(|| fixture.directory.join("dist/main.py").is_file()),
        "watch did not complete its initial build"
    );

    let signal = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("SIGINT should be sent");
    assert!(signal.success());
    let mut status = None;
    for _ in 0..100 {
        if let Some(completed) = child.try_wait().expect("watch status should be readable") {
            status = Some(completed);
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let status = status.expect("native watch did not exit after SIGINT");
    assert_eq!(status.signal(), Some(2));
}
