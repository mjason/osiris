//! Deterministic, read-only dependency projection for Osiris projects.
//!
//! uv remains the resolver and installer. This module only validates and
//! projects `uv.lock`, extension markers, and `.osri` hashes. It performs no
//! network access and never imports Python.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
};

use crate::{
    extension::{self, ExtensionDistribution, ExtensionError},
    hash::{push_field, sha256},
    hir::{ContractTrustPolicy, InterfaceTrustPolicy},
    interface,
    project::{ProjectConfig, TrustContract},
    types::PythonVersion,
};

mod error;
mod lockfile;
mod model;
mod requirement;
mod resolve;

pub use error::DependencyError;
use lockfile::*;
pub use model::*;
use requirement::*;
pub use resolve::*;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
