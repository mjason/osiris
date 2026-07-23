use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use serde::Serialize;

use crate::{
    artifact::{Artifact, ArtifactKind, publish_artifacts},
    compiler::{self, CompileOptions, python_module_path},
    dependency, diagnostic,
    extension::{self, normalize_distribution_name},
    interface,
    macro_expand::{self, ExpansionOptions},
    printer::{render_document_json, render_document_text},
    project::{ConfigError, ProjectConfig, PythonVersion},
    reader, records,
    semantic::SemanticDocument,
    source::Span,
};

pub const USAGE: &str = "Usage: osr [OPTIONS]\n       osr init PROJECT\n       osr init --extension PROJECT\n       osr init --existing [--extension] [DIR]\n       osr check FILE [--site-root DIR]\n       osr build [DIR] [--site-root DIR]\n       osr compile FILE... [--out-dir DIR] [--emit py,osri,map,records] [--site-root DIR]\n       osr watch [DIR] [--site-root DIR]\n       osr run FILE [--site-root DIR] [-- ARGS...]\n       osr expand [--once] FILE\n       osr inspect [--syntax|--semantic] FILE [--format text|json]\n       osr lsp\n\nCommands:\n  init          Create a project or add Osiris to an existing uv project\n  check FILE    Analyze an Osiris project or standalone source file\n  build         Compile the project configured by osiris.jsonc\n  compile FILE  Compile explicit source files or a containing project\n  watch         Rebuild a project when configured inputs change\n  run FILE      Compile and run an Osiris project entry module\n  expand FILE   Print macro-expanded Osiris forms\n  inspect FILE  Inspect syntax or the semantic model\n  lsp           Run the Language Server Protocol server\n\nOptions:\n  --site-root DIR  Search this installed-package root for locked static extensions\n  -V, --version    Print version\n  -h, --help       Print help";

static NEXT_RUN_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CliOutcome {
    pub exit_code: u8,
    pub stdout: String,
    pub stderr: String,
}

impl CliOutcome {
    fn success(stdout: String) -> Self {
        Self {
            exit_code: 0,
            stdout,
            stderr: String::new(),
        }
    }

    fn failure(exit_code: u8, stdout: String, stderr: String) -> Self {
        Self {
            exit_code,
            stdout,
            stderr,
        }
    }

    fn usage_error(message: impl AsRef<str>) -> Self {
        Self::failure(
            2,
            String::new(),
            format!(
                "osr: {}\nTry 'osr --help' for more information.\n",
                message.as_ref()
            ),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InspectFormat {
    Text,
    Json,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InspectView {
    Syntax,
    Semantic,
}

/// Runs the command-line interface without writing to process streams or exiting.
#[must_use]
pub fn run_cli(arguments: &[String]) -> CliOutcome {
    match arguments {
        [] => CliOutcome::success(format!("{USAGE}\n")),
        [argument] if argument == "-h" || argument == "--help" => {
            CliOutcome::success(format!("{USAGE}\n"))
        }
        [argument] if argument == "-V" || argument == "--version" => {
            CliOutcome::success(format!("osr {}\n", crate::version()))
        }
        [command, rest @ ..] if command == "init" => run_init(rest),
        [command, rest @ ..] if command == "check" => run_check(rest),
        [command, rest @ ..] if command == "build" => run_build(rest),
        [command, rest @ ..] if command == "compile" => run_compile(rest),
        [command, rest @ ..] if command == "run" => run_program(rest),
        [command, rest @ ..] if command == "expand" => run_expand(rest),
        [command, rest @ ..] if command == "inspect" => run_inspect(rest),
        _ => CliOutcome::usage_error("unexpected arguments"),
    }
}

mod build;
mod check;
mod compile;
mod extensions;
mod init;
mod inspect;
mod run;
#[path = "io.rs"]
mod source_io;
mod watch;
mod workspace;

use build::*;
use check::*;
use compile::*;
use extensions::*;
use init::*;
use inspect::*;
use run::*;
use source_io::*;
use workspace::*;

pub use watch::run_watch_stdio;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
