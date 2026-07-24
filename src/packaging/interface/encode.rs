use super::*;

mod declarations;
mod graph;
mod static_data;
mod type_data;

use declarations::*;
use graph::*;
use static_data::*;

pub(in crate::interface) fn calculate_hashes(interface: &Interface) -> InterfaceHashes {
    let full = body_form(interface, MetadataProjection::Full);
    let semantic = body_form(interface, MetadataProjection::Semantic);
    let tooling = tooling_body_form(interface);
    let interface_body = hash_form(&full);
    let semantic_body = hash_form(&semantic);
    let tooling_body = hash_form(&tooling);
    let header = header_form(interface);
    let hash_section = hashes_form(&interface_body, &semantic_body, &tooling_body, None);
    let content_integrity = hash_text(&render_forms(&[
        header,
        wrap("osiris-interface/body", full),
        wrap("osiris-interface/graph", graph_form(&interface.graph)),
        hash_section,
    ]));
    InterfaceHashes {
        interface_body,
        semantic_body,
        tooling_body,
        content_integrity,
    }
}

pub(crate) fn refresh_standalone_hashes(interface: &mut Interface) -> InterfaceResult<()> {
    interface.hashes = calculate_hashes(interface);
    let local = BTreeMap::from([(
        interface.module.clone(),
        InterfaceBodyHashes {
            semantic_body: interface.hashes.semantic_body.clone(),
            tooling_body: interface.hashes.tooling_body.clone(),
        },
    )]);
    let graph = calculate_interface_graph_hashes(&local, [], &BTreeMap::new())
        .map_err(|error| InterfaceError::new("OSR-I0073", error.to_string()))?;
    interface.graph = graph
        .groups
        .into_iter()
        .next()
        .expect("one local interface produces one hash group");
    interface.hashes = calculate_hashes(interface);
    Ok(())
}

pub fn install_hash_group(
    interface: &mut Interface,
    group: InterfaceHashGroup,
) -> InterfaceResult<()> {
    interface.graph = group;
    interface.hashes = calculate_hashes(interface);
    validate(interface)
}

pub(in crate::interface) fn file_forms(interface: &Interface, integrity: bool) -> Vec<Form> {
    vec![
        header_form(interface),
        wrap(
            "osiris-interface/body",
            body_form(interface, MetadataProjection::Full),
        ),
        wrap("osiris-interface/graph", graph_form(&interface.graph)),
        hashes_form(
            &interface.hashes.interface_body,
            &interface.hashes.semantic_body,
            &interface.hashes.tooling_body,
            integrity.then_some(interface.hashes.content_integrity.as_str()),
        ),
    ]
}

fn header_form(interface: &Interface) -> Form {
    wrap(
        "osiris-interface/header",
        map(vec![
            ("format", string(FORMAT_NAME)),
            ("format-version", integer(interface.format_version)),
            ("compiler-abi", string(&interface.compiler_abi)),
            ("language-version", string(&interface.language_version)),
            ("language-abi", string(&interface.language_abi)),
            (
                "standard-library-abi",
                integer(interface.standard_library_abi),
            ),
            (
                "linkable-helper-format",
                integer(interface.linkable_helper_format),
            ),
            (
                "python-target",
                string(&interface.python_target.to_string()),
            ),
        ]),
    )
}

#[derive(Clone, Copy)]
pub(in crate::interface) enum MetadataProjection {
    Full,
    Semantic,
}

fn body_form(interface: &Interface, projection: MetadataProjection) -> Form {
    map(vec![
        ("module", string(&interface.module)),
        (
            "metadata",
            metadata_form(&project_metadata(&interface.metadata, projection)),
        ),
        (
            "bindings",
            vector(
                interface
                    .bindings
                    .iter()
                    .map(|binding| binding_form(binding, projection))
                    .collect(),
            ),
        ),
        (
            "aliases",
            vector(interface.aliases.iter().map(alias_form).collect()),
        ),
        (
            "functions",
            vector(
                interface
                    .functions
                    .iter()
                    .map(|function| function_form(function, projection))
                    .collect(),
            ),
        ),
        (
            "structs",
            vector(
                interface
                    .structs
                    .iter()
                    .map(|structure| struct_form(structure, projection))
                    .collect(),
            ),
        ),
        (
            "operator-instances",
            vector(
                interface
                    .operator_instances
                    .iter()
                    .map(operator_instance_form)
                    .collect(),
            ),
        ),
        (
            "macros",
            vector(interface.macros.iter().map(macro_interface_form).collect()),
        ),
        (
            "phase-helpers",
            vector(
                interface
                    .phase_helpers
                    .iter()
                    .map(phase_helper_form)
                    .collect(),
            ),
        ),
        (
            "static-schemas",
            vector(
                interface
                    .static_schemas
                    .iter()
                    .map(|schema| static_schema_form(&interface.module, schema))
                    .collect(),
            ),
        ),
        (
            "owned-records",
            vector(
                interface
                    .owned_records
                    .iter()
                    .map(|record| static_record_form(record, projection))
                    .collect(),
            ),
        ),
    ])
}

fn tooling_body_form(interface: &Interface) -> Form {
    map(vec![
        ("module", string(&interface.module)),
        ("metadata", metadata_form(&interface.metadata)),
        (
            "bindings",
            vector(
                interface
                    .bindings
                    .iter()
                    .map(|binding| {
                        map(vec![
                            ("id", string(&binding.id)),
                            ("canonical", string(&binding.canonical)),
                            ("metadata", metadata_form(&binding.metadata)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "aliases",
            vector(interface.aliases.iter().map(alias_form).collect()),
        ),
        (
            "functions",
            vector(
                interface
                    .functions
                    .iter()
                    .map(|function| {
                        map(vec![
                            ("binding", string(&function.binding)),
                            (
                                "parameters",
                                vector(
                                    function
                                        .parameters
                                        .iter()
                                        .map(|parameter| {
                                            map(vec![
                                                ("id", string(&parameter.id)),
                                                ("canonical", string(&parameter.canonical)),
                                                ("aliases", strings_form(&parameter.aliases)),
                                                ("metadata", metadata_form(&parameter.metadata)),
                                            ])
                                        })
                                        .collect(),
                                ),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "structs",
            vector(
                interface
                    .structs
                    .iter()
                    .map(|structure| {
                        map(vec![
                            ("binding", string(&structure.binding)),
                            ("doc", optional_string(structure.doc.as_deref())),
                            (
                                "fields",
                                vector(
                                    structure
                                        .fields
                                        .iter()
                                        .map(|field| {
                                            map(vec![
                                                ("id", string(&field.id)),
                                                ("canonical", string(&field.canonical)),
                                                ("aliases", strings_form(&field.aliases)),
                                                ("metadata", metadata_form(&field.metadata)),
                                            ])
                                        })
                                        .collect(),
                                ),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "operator-instances",
            vector(
                interface
                    .operator_instances
                    .iter()
                    .map(|instance| {
                        map(vec![
                            ("id", string(&instance.id)),
                            ("binding", string(&instance.binding)),
                            ("owner-binding", string(&instance.owner_binding)),
                            ("operator", keyword(instance.operator.stable_name())),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "macros",
            vector(
                interface
                    .macros
                    .iter()
                    .map(|macro_| {
                        map(vec![
                            ("id", string(&macro_.id)),
                            ("canonical", string(&macro_.canonical)),
                            ("parameters", macro_.parameters.clone()),
                            ("minimum-arity", integer_usize(macro_.minimum_arity)),
                            ("variadic", boolean(macro_.variadic)),
                            ("metadata", metadata_form(&macro_.phase_ir.metadata)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "phase-helpers",
            vector(
                interface
                    .phase_helpers
                    .iter()
                    .map(|helper| {
                        map(vec![
                            ("id", string(&helper.id)),
                            ("canonical", string(&helper.canonical)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "static-schemas",
            vector(
                interface
                    .static_schemas
                    .iter()
                    .map(|schema| {
                        map(vec![
                            (
                                "binding",
                                string(
                                    BindingId::new(
                                        &interface.module,
                                        &schema.name,
                                        BindingKind::Type,
                                    )
                                    .as_str(),
                                ),
                            ),
                            ("name", string(&schema.name)),
                            ("schema-id", string(&schema.schema_id)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "owned-records",
            vector(
                interface
                    .owned_records
                    .iter()
                    .map(|record| {
                        map(vec![
                            ("stable-record-id", string(&record.stable_record_id)),
                            ("owner-binding-id", string(&record.owner_binding_id)),
                            ("owner-name", string(&record.owner_name)),
                            ("origin", record_origin_form(&record.origin)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}
