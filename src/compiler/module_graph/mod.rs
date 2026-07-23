//! Deterministic source/module dependency graphs and read-only interface loading.
//!
//! The graph deliberately operates on the lowered surface AST.  Runtime and
//! phase-1 imports are represented as different edge sets, so callers can
//! schedule ordinary modules independently while enforcing the stronger
//! no-cycle rule for macro/phase-1 dependencies.  Interface loading accepts an
//! explicit module-to-`.osri` path map and only parses the data-only interface;
//! it never imports or executes Python code.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs, io,
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::{
    ast::{self, ImportPhase, ItemKind},
    interface::{self, FunctionInterface, Interface, PublicAlias, PublicBinding, StructInterface},
    name::BindingKind,
    source::Span,
};

mod dependency;
mod error;
#[path = "io.rs"]
mod interface_io;
mod resolution;

pub use dependency::*;
pub use error::*;
pub use interface_io::*;
pub use resolution::*;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
