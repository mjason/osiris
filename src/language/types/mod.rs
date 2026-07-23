//! Core types and local type inference primitives.
//!
//! This module deliberately has no dependency on name resolution or HIR.  It
//! can therefore be reused by the compiler, interface reader, and LSP.
//! Nominal types carry a stable type-binding identity separately from their
//! short display name, so equal spellings exported by different modules never
//! become the same semantic type.

use std::{collections::BTreeMap, error::Error, fmt};

use serde::Serialize;

use crate::{
    source::Span,
    syntax::{Form, FormKind},
};

mod context;
mod model;
mod operators;
mod parse;
mod python;

pub use context::*;
pub use model::*;
pub use operators::*;
pub use parse::*;
pub use python::*;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
