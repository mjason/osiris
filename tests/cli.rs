use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};

use _core::records;
use sha2::Digest;

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct SourceFixture {
    directory: PathBuf,
    path: PathBuf,
}

impl SourceFixture {
    fn new(source: &str) -> Self {
        let sequence = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let directory =
            std::env::temp_dir().join(format!("osiris-cli-test-{}-{sequence}", std::process::id()));
        fs::create_dir(&directory).expect("fixture directory should be created");
        let path = directory.join("示例.osr");
        fs::write(&path, source).expect("fixture source should be written");
        Self { directory, path }
    }

    fn write(&self, relative: &str, source: &str) -> PathBuf {
        let path = self.directory.join(relative);
        fs::create_dir_all(path.parent().expect("fixture file should have a parent"))
            .expect("fixture parent should be created");
        fs::write(&path, source).expect("fixture source should be written");
        path
    }
}

impl Drop for SourceFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn osr(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_osr"))
        .args(arguments)
        .output()
        .expect("osr should run")
}

fn path_argument(path: &Path) -> &str {
    path.to_str().expect("fixture path should be UTF-8")
}

#[path = "cli/compilation.rs"]
mod compilation;
#[path = "cli/execution.rs"]
mod execution;
#[path = "cli/inspection.rs"]
mod inspection;
#[path = "cli/protocol.rs"]
mod protocol;
