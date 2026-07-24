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
    printer::render_document_text,
    project::{ConfigError, ProjectConfig, PythonVersion},
    reader, records,
    semantic::SemanticDocument,
    source::Span,
};

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

/// Runs the command-line interface without writing to process streams or exiting.
#[must_use]
pub fn run_cli(arguments: &[String]) -> CliOutcome {
    if let Some(outcome) = registry::help_request(arguments) {
        return outcome;
    }
    if arguments.first().is_some_and(|command| {
        matches!(
            command.as_str(),
            "check" | "build" | "compile" | "run" | "expand" | "lsc"
        )
    }) {
        if let Err(error) = crate::stdlib::validate_resources() {
            return CliOutcome::failure(
                1,
                String::new(),
                format!("osr: invalid compiler installation: {error}\n"),
            );
        }
    }
    match arguments {
        [] => CliOutcome::success(registry::root_help()),
        [argument] if argument == "-V" || argument == "--version" => {
            CliOutcome::success(format!("osr {}\n", crate::version()))
        }
        [command, rest @ ..] if command == "init" => run_init(rest),
        [command, rest @ ..] if command == "check" => run_check(rest),
        [command, rest @ ..] if command == "build" => run_build(rest),
        [command, rest @ ..] if command == "compile" => run_compile(rest),
        [command, rest @ ..] if command == "run" => run_program(rest),
        [command, rest @ ..] if command == "expand" => run_expand(rest),
        [command, rest @ ..] if command == "fmt" => run_fmt(rest),
        [command, rest @ ..] if command == "lsc" => run_lsc(rest),
        [command, rest @ ..] if command == "syntax" => run_syntax(rest),
        [command, rest @ ..] if command == "doc" => run_doc(rest),
        _ => CliOutcome::usage_error("unexpected arguments"),
    }
}

mod build;
mod check;
mod compile;
mod docs;
mod expand;
mod extensions;
mod fmt;
mod init;
mod lsc;
mod registry;
mod run;
#[path = "io.rs"]
mod source_io;
mod watch;
mod workspace;

use build::*;
use check::*;
use compile::*;
use docs::*;
use expand::*;
use extensions::*;
use fmt::*;
use init::*;
use lsc::*;
use run::*;
use source_io::*;
use workspace::*;

pub use docs::run_doc_stdio;
pub use fmt::run_fmt_stdio;
pub use watch::run_watch_stdio;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
