use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};

use osiris::records;
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

struct ExtensionMarkerMember<'a> {
    id: &'a str,
    interface: &'a str,
    source: &'a str,
}

fn write_extension_marker(
    site_root: &Path,
    dist_info: &Path,
    distribution: &str,
    version: &str,
    dependencies: &[&str],
    members: &[ExtensionMarkerMember<'_>],
    records: Option<(&str, &[u8])>,
) {
    let mut marker = format!(
        "schema = 2\ncompiler_abi = 1\nlanguage_abi = 2\nlanguage_version = {:?}\nstandard_library_abi = {}\nlinkable_helper_format = {}\ndistribution = {:?}\nversion = {:?}\npython_target = \"3.11\"\ndependencies = {}\n",
        osiris::LANGUAGE_VERSION,
        osiris::STANDARD_LIBRARY_ABI,
        osiris::LINKABLE_HELPER_FORMAT,
        distribution,
        version,
        serde_json::to_string(dependencies).unwrap(),
    );
    if let Some((path, bytes)) = records {
        marker.push_str(&format!(
            "records = {:?}\nrecords_hash = {:?}\n",
            path,
            format!("sha256:{:x}", sha2::Sha256::digest(bytes))
        ));
    }
    for member in members {
        let interface_text = fs::read_to_string(site_root.join(member.interface))
            .expect("extension interface should be readable");
        let interface =
            osiris::interface::read(&interface_text).expect("extension interface should be valid");
        let source_bytes = fs::read(site_root.join(member.source))
            .expect("packaged extension source should be readable");
        let source_hash = format!("sha256:{:x}", sha2::Sha256::digest(&source_bytes));
        let source_map = format!("{}.py.map", member.source.trim_end_matches(".osr"));
        let generated = format!("{}.py", member.source.trim_end_matches(".osr"));
        let map_bytes = serde_json::to_vec(&serde_json::json!({
            "version": 3,
            "language_version": osiris::LANGUAGE_VERSION,
            "python_target": "3.11",
            "source": member.source,
            "source_hash": source_hash,
            "generated": generated,
            "trust_policy_hash": format!("sha256:{}", "0".repeat(64)),
            "build_hash": format!("sha256:{}", "0".repeat(64)),
            "mappings": [],
        }))
        .unwrap();
        fs::write(site_root.join(&source_map), &map_bytes)
            .expect("extension source map should be written");
        marker.push_str(&format!(
            "\n[[extension]]\nid = {:?}\ninterface = {:?}\ninterface_hash = {:?}\nsource = {:?}\nsource_hash = {:?}\nsource_map = {:?}\nsource_map_hash = {:?}\n",
            member.id,
            member.interface,
            interface.semantic_interface_hash(),
            member.source,
            source_hash,
            source_map,
            format!("sha256:{:x}", sha2::Sha256::digest(&map_bytes)),
        ));
    }
    fs::write(dist_info.join("osiris.toml"), marker).expect("extension marker should be written");
}

#[path = "cli/compilation.rs"]
mod compilation;
#[path = "cli/execution.rs"]
mod execution;
#[path = "cli/formatting.rs"]
mod formatting;
#[path = "cli/initialization.rs"]
mod initialization;
#[path = "cli/inspection.rs"]
mod inspection;
#[path = "cli/protocol.rs"]
mod protocol;
#[path = "cli/watching.rs"]
mod watching;
