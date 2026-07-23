use std::{
    env, fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

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

#[path = "control_compile/assertions.rs"]
mod assertions;
#[path = "control_compile/collections.rs"]
mod collections;
#[path = "control_compile/recursion_and_types.rs"]
mod recursion_and_types;
