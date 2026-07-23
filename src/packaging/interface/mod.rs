//! Deterministic, data-only `.osri` compilation interfaces.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    ast,
    hir::{self, ItemKind},
    interface_graph::{
        InterfaceBodyHashes, InterfaceHashEdge, InterfaceHashGroup, InterfaceHashMember,
        ResolvedHashDependency, calculate_interface_graph_hashes, verify_interface_hash_group,
    },
    macro_expand,
    name::{BindingId, BindingKind, python_identifier},
    printer::render_document_text,
    reader,
    records::{self, ProjectionKind, StaticDatum, StaticSchema, StaticType, ValidatedRecord},
    source::Span,
    syntax::{
        Document, Form, FormKind, METADATA_DECLARATION_LIMITS, METADATA_INTERFACE_LIMITS,
        METADATA_TARGET_LIMITS, MetadataEntry, MetadataLimitExceeded, MetadataResourceUsage, Name,
        ReaderMacroKind, check_metadata_resources, check_metadata_usage, metadata_aliases,
        metadata_datum_is_serializable,
    },
    types::{
        Alignment, Availability, CallSummaries, DataProperties, Effect, EffectRow, FunctionType,
        OperatorInstance, ScalarOperator, TemporalBound, TemporalSummary, Type, TypeLiteral,
        TypeVarId, nominal_short_name,
    },
};

mod build;
mod common;
mod decode;
mod encode;
mod model;
mod rules;
mod validate;

pub use build::{build, build_with_static_data, from_hir};
pub(crate) use build::{build_provisional, validate_provisional_shape};
use common::*;
use decode::{
    decode_body, decode_graph, decode_hashes, decode_header, normalize_model,
    reject_duplicate_maps, unwrap,
};
pub use encode::install_hash_group;
use encode::{MetadataProjection, calculate_hashes, file_forms, refresh_standalone_hashes};
pub use model::*;
use rules::*;
use validate::{
    metadata_resource_error, validate, validate_interface_metadata_resources,
    validate_metadata_target, validate_model,
};

/// Build and render a deterministic interface for one typed module.
pub fn emit(typed: &hir::Module, surface: &ast::Module) -> InterfaceResult<String> {
    render(&build(typed, surface)?)
}

pub fn render(interface: &Interface) -> InterfaceResult<String> {
    validate(interface)?;
    Ok(render_forms(&file_forms(interface, true)))
}

/// Parses and validates `.osri` without importing or executing Python.
pub fn read(source: &str) -> InterfaceResult<Interface> {
    let document = reader::read(source);
    if let Some(diagnostic) = document.diagnostics.first() {
        if diagnostic.code == "OSR-R0007" {
            return Err(InterfaceError::new(
                "OSR-I0043",
                "duplicate map key in interface",
            ));
        }
        return Err(InterfaceError::new(
            "OSR-I0010",
            format!("invalid S-expression: {}", diagnostic.message),
        ));
    }
    if document.forms.len() != 4 {
        return Err(InterfaceError::new(
            "OSR-I0011",
            "interface requires exactly header, body, graph, and hashes forms",
        ));
    }
    for form in &document.forms {
        reject_duplicate_maps(form)?;
    }
    let header = unwrap(&document.forms[0], "osiris-interface/header")?;
    let body = unwrap(&document.forms[1], "osiris-interface/body")?;
    let graph = unwrap(&document.forms[2], "osiris-interface/graph")?;
    let hashes = unwrap(&document.forms[3], "osiris-interface/hashes")?;
    let (format_version, compiler_abi, language_abi) = decode_header(header)?;
    let (
        module,
        metadata,
        bindings,
        aliases,
        functions,
        structs,
        operator_instances,
        macros,
        phase_helpers,
        static_schemas,
        owned_records,
    ) = decode_body(body)?;
    let mut interface = Interface {
        format_version,
        compiler_abi,
        language_abi,
        module,
        metadata,
        bindings,
        aliases,
        functions,
        structs,
        operator_instances,
        macros,
        phase_helpers,
        static_schemas,
        owned_records,
        graph: decode_graph(graph)?,
        hashes: decode_hashes(hashes)?,
    };
    normalize_model(&mut interface)?;
    validate(&interface)?;
    Ok(interface)
}

pub use read as parse;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
