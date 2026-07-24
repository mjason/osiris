use super::*;

mod declarations;
mod static_data;
mod support;
mod type_data;

use declarations::*;
use static_data::*;
use support::*;
pub(in crate::interface) use support::{reject_duplicate_maps, unwrap};
pub(in crate::interface) use type_data::normalize_model;

pub(in crate::interface) fn decode_header(
    form: &Form,
) -> InterfaceResult<(
    u32,
    String,
    String,
    String,
    u32,
    u32,
    crate::types::PythonVersion,
)> {
    let values = strict_map(
        form,
        &[
            "format",
            "format-version",
            "compiler-abi",
            "language-version",
            "language-abi",
            "standard-library-abi",
            "linkable-helper-format",
            "python-target",
        ],
    )?;
    if expect_string(get(&values, "format")?, "format")? != FORMAT_NAME {
        return Err(InterfaceError::new("OSR-I0030", "unknown interface format"));
    }
    Ok((
        expect_u32(get(&values, "format-version")?, "format-version")?,
        expect_string(get(&values, "compiler-abi")?, "compiler-abi")?,
        expect_string(get(&values, "language-version")?, "language-version")?,
        expect_string(get(&values, "language-abi")?, "language-abi")?,
        expect_u32(
            get(&values, "standard-library-abi")?,
            "standard-library-abi",
        )?,
        expect_u32(
            get(&values, "linkable-helper-format")?,
            "linkable-helper-format",
        )?,
        expect_string(get(&values, "python-target")?, "python-target")?
            .parse()
            .map_err(|error: crate::project::ConfigError| {
                InterfaceError::new("OSR-I0030", error.to_string())
            })?,
    ))
}

#[allow(clippy::type_complexity)]
pub(in crate::interface) fn decode_body(
    form: &Form,
) -> InterfaceResult<(
    String,
    Vec<MetadataEntry>,
    Vec<PublicBinding>,
    Vec<PublicAlias>,
    Vec<FunctionInterface>,
    Vec<StructInterface>,
    Vec<OperatorInstance>,
    Vec<MacroInterface>,
    Vec<PhaseHelperInterface>,
    Vec<StaticSchema>,
    Vec<ValidatedRecord>,
)> {
    let values = strict_map(
        form,
        &[
            "module",
            "metadata",
            "bindings",
            "aliases",
            "functions",
            "structs",
            "operator-instances",
            "macros",
            "phase-helpers",
            "static-schemas",
            "owned-records",
        ],
    )?;
    let module = expect_string(get(&values, "module")?, "module")?;
    let static_schemas = decode_vector(get(&values, "static-schemas")?, |form| {
        decode_static_schema(form, &module)
    })?;
    Ok((
        module,
        decode_metadata(get(&values, "metadata")?)?,
        decode_vector(get(&values, "bindings")?, decode_binding)?,
        decode_vector(get(&values, "aliases")?, decode_alias)?,
        decode_vector(get(&values, "functions")?, decode_function)?,
        decode_vector(get(&values, "structs")?, decode_struct)?,
        decode_vector(
            get(&values, "operator-instances")?,
            decode_operator_instance,
        )?,
        decode_vector(get(&values, "macros")?, decode_macro_interface)?,
        decode_vector(get(&values, "phase-helpers")?, decode_phase_helper)?,
        static_schemas,
        decode_vector(get(&values, "owned-records")?, decode_static_record)?,
    ))
}

pub(in crate::interface) fn decode_graph(form: &Form) -> InterfaceResult<InterfaceHashGroup> {
    let values = strict_map(
        form,
        &[
            "group-id",
            "members",
            "internal-edges",
            "external-dependencies",
            "semantic-interface-hash",
            "tooling-metadata-hash",
        ],
    )?;
    Ok(InterfaceHashGroup {
        id: expect_string(get(&values, "group-id")?, "interface hash group id")?,
        members: decode_vector(get(&values, "members")?, decode_graph_member)?,
        internal_edges: decode_vector(get(&values, "internal-edges")?, decode_graph_edge)?,
        external_dependencies: decode_vector(
            get(&values, "external-dependencies")?,
            decode_graph_dependency,
        )?,
        semantic_interface_hash: expect_hash(get(&values, "semantic-interface-hash")?)?,
        tooling_metadata_hash: expect_hash(get(&values, "tooling-metadata-hash")?)?,
    })
}

fn decode_graph_member(form: &Form) -> InterfaceResult<InterfaceHashMember> {
    let values = strict_map(form, &["module", "semantic-body", "tooling-body"])?;
    Ok(InterfaceHashMember {
        module: expect_string(get(&values, "module")?, "interface hash group member")?,
        semantic_body_hash: expect_hash(get(&values, "semantic-body")?)?,
        tooling_body_hash: expect_hash(get(&values, "tooling-body")?)?,
    })
}

fn decode_graph_edge(form: &Form) -> InterfaceResult<InterfaceHashEdge> {
    let values = strict_map(form, &["from", "to", "kind"])?;
    Ok(InterfaceHashEdge {
        from: expect_string(get(&values, "from")?, "interface edge source")?,
        to: expect_string(get(&values, "to")?, "interface edge target")?,
        kind: decode_edge_kind(get(&values, "kind")?)?,
    })
}

fn decode_graph_dependency(form: &Form) -> InterfaceResult<ResolvedHashDependency> {
    let values = strict_map(
        form,
        &[
            "from",
            "to",
            "kind",
            "semantic-interface-hash",
            "tooling-metadata-hash",
        ],
    )?;
    Ok(ResolvedHashDependency {
        from: expect_string(get(&values, "from")?, "interface dependency source")?,
        to: expect_string(get(&values, "to")?, "interface dependency target")?,
        kind: decode_edge_kind(get(&values, "kind")?)?,
        semantic_interface_hash: expect_hash(get(&values, "semantic-interface-hash")?)?,
        tooling_metadata_hash: expect_hash(get(&values, "tooling-metadata-hash")?)?,
    })
}

fn decode_edge_kind(form: &Form) -> InterfaceResult<crate::module_graph::EdgeKind> {
    match expect_keyword(form, "interface edge kind")? {
        "runtime" => Ok(crate::module_graph::EdgeKind::Runtime),
        "phase-1" => Ok(crate::module_graph::EdgeKind::Phase1),
        value => Err(InterfaceError::new(
            "OSR-I0073",
            format!("unknown interface edge kind `:{value}`"),
        )),
    }
}

pub(in crate::interface) fn decode_hashes(form: &Form) -> InterfaceResult<InterfaceHashes> {
    let values = strict_map(
        form,
        &[
            "interface-body",
            "semantic-body",
            "tooling-body",
            "content-integrity",
        ],
    )?;
    Ok(InterfaceHashes {
        interface_body: expect_hash(get(&values, "interface-body")?)?,
        semantic_body: expect_hash(get(&values, "semantic-body")?)?,
        tooling_body: expect_hash(get(&values, "tooling-body")?)?,
        content_integrity: expect_hash(get(&values, "content-integrity")?)?,
    })
}
