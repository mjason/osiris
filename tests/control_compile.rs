use std::{
    env, fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use osiris::backend::GeneratedPython;
use osiris::{compiler::CompileOptions, compiler::compile, project::PythonVersion};

fn options() -> CompileOptions {
    CompileOptions::new("control_compile", PythonVersion::default())
}

fn temporary_directory() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let path = env::temp_dir().join(format!("osiris-control-{nonce}"));
    fs::create_dir_all(&path).expect("create temporary directory");
    path
}

fn write_generated_module(root: &std::path::Path, filename: &str, generated: &GeneratedPython) {
    fs::write(root.join(filename), &generated.source).expect("write generated module");
    let Some(support) = &generated.runtime_support else {
        return;
    };
    for (path, source) in osiris::backend::runtime_distribution_files(
        support,
        osiris::project::PythonVersion::default(),
    )
    .expect("link runtime distribution")
    {
        let destination = root.join(path);
        fs::create_dir_all(destination.parent().expect("support parent"))
            .expect("create support directory");
        fs::write(destination, source).expect("write support file");
    }
}

#[path = "control_compile/assertions.rs"]
mod assertions;
#[path = "control_compile/collections.rs"]
mod collections;
#[path = "control_compile/recursion_and_types.rs"]
mod recursion_and_types;
