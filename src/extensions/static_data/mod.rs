//! Static schemas, records, and their deterministic runtime sidecar.
//!
//! This module is deliberately independent from the Python backend.  A static
//! record is data validated at compile time; it is never evaluated and it is
//! never represented by a Python object while compiling.  The public API is
//! also useful to interface/build consumers which only have an AST or an
//! already validated record set.

use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser};
use unicode_normalization::UnicodeNormalization;

use crate::{
    ast::{self, Expr, ExprKind, ItemKind, Module},
    diagnostic::Diagnostic,
    hash::sha256,
    name::{BindingId, BindingKind},
    source::Span,
    syntax::Name,
};

mod analysis;
mod datum;
mod index;
mod record;
mod schema;
mod sidecar;

pub use analysis::{StaticModuleData, analyze_module, analyze_module_with_interfaces};
pub use datum::{
    RECORD_INDEX_CONFLICT, RECORD_INVALID_DATUM, RECORD_RECORD_INDEX, RECORD_RECORD_SHAPE,
    RECORD_RECORD_TYPE, RECORD_SCHEMA_FIELD, RECORD_SCHEMA_INDEX, RECORD_SCHEMA_SHAPE,
    RECORD_SCHEMA_TYPE, RECORD_SIDECAR, RecordError, StaticDatum,
};
pub use index::{IndexedRecord, MergedIndexClaim, MergedIndexes, merge_unique_indexes};
pub use record::{
    IndexClaim, RecordOccurrenceId, RecordOrigin, SchemaIdentity, ValidatedRecord, validate_record,
    validate_record_with_schema_binding, verify_record_against_schema,
};
pub use schema::{
    IndexProjection, ProjectionKind, SchemaField, SchemaIndex, StaticSchema, StaticType,
    parse_schema,
};
pub use sidecar::{
    EncodedSidecar, RECORD_SIDECAR_FORMAT_VERSION, RecordSidecar, SidecarRecord, decode_sidecar,
    encode_sidecar, verify_sidecar_against_records,
};

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
