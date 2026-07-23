use super::super::*;
use super::type_data::*;

pub(super) fn binding_form(binding: &PublicBinding, projection: MetadataProjection) -> Form {
    map(vec![
        ("id", string(&binding.id)),
        ("canonical", string(&binding.canonical)),
        ("python", string(&binding.python)),
        ("kind", keyword(binding_kind_name(binding.kind))),
        ("visibility", keyword("public")),
        ("type", type_form(&binding.ty)),
        (
            "runtime",
            binding.runtime.as_ref().map_or_else(none, |runtime| {
                map(vec![
                    ("module", string(&runtime.module)),
                    ("name", string(&runtime.name)),
                    ("python-module", boolean(runtime.python_module)),
                ])
            }),
        ),
        (
            "metadata",
            metadata_form(&project_metadata(&binding.metadata, projection)),
        ),
    ])
}

pub(super) fn alias_form(alias: &PublicAlias) -> Form {
    map(vec![
        ("spelling", string(&alias.spelling)),
        ("canonical", string(&alias.canonical)),
        ("target", string(&alias.target)),
        ("visibility", keyword("public")),
    ])
}

pub(super) fn function_form(function: &FunctionInterface, projection: MetadataProjection) -> Form {
    map(vec![
        ("binding", string(&function.binding)),
        (
            "parameters",
            vector(
                function
                    .parameters
                    .iter()
                    .map(|parameter| parameter_form(parameter, projection))
                    .collect(),
            ),
        ),
        ("return", type_form(&function.return_type)),
        (
            "contract-id",
            optional_string(function.contract_id.as_deref()),
        ),
        ("summaries", summaries_form(&function.summaries)),
    ])
}

pub(super) fn parameter_form(
    parameter: &ParameterInterface,
    projection: MetadataProjection,
) -> Form {
    map(vec![
        ("id", string(&parameter.id)),
        ("canonical", string(&parameter.canonical)),
        ("type", type_form(&parameter.ty)),
        ("has-default", boolean(parameter.has_default)),
        ("variadic", boolean(parameter.variadic)),
        ("aliases", strings_form(&parameter.aliases)),
        (
            "metadata",
            metadata_form(&project_metadata(&parameter.metadata, projection)),
        ),
    ])
}

pub(super) fn struct_form(structure: &StructInterface, projection: MetadataProjection) -> Form {
    map(vec![
        ("binding", string(&structure.binding)),
        ("type-parameters", strings_form(&structure.type_parameters)),
        (
            "fields",
            vector(
                structure
                    .fields
                    .iter()
                    .map(|field| field_form(field, projection))
                    .collect(),
            ),
        ),
        ("invariant-count", integer_usize(structure.invariant_count)),
        ("doc", optional_string(structure.doc.as_deref())),
    ])
}

pub(super) fn operator_instance_form(instance: &OperatorInstance) -> Form {
    map(vec![
        ("id", string(&instance.id)),
        ("binding", string(&instance.binding)),
        ("owner-binding", string(&instance.owner_binding)),
        ("operator", keyword(instance.operator.stable_name())),
        (
            "operands",
            vector(instance.operands.iter().map(type_form).collect()),
        ),
        ("result", type_form(&instance.result)),
        ("summaries", summaries_form(&instance.summaries)),
    ])
}

pub(super) fn field_form(field: &FieldInterface, projection: MetadataProjection) -> Form {
    map(vec![
        ("id", string(&field.id)),
        ("canonical", string(&field.canonical)),
        ("type", type_form(&field.ty)),
        ("has-default", boolean(field.has_default)),
        ("aliases", strings_form(&field.aliases)),
        (
            "metadata",
            metadata_form(&project_metadata(&field.metadata, projection)),
        ),
    ])
}

pub(super) fn macro_interface_form(macro_: &MacroInterface) -> Form {
    map(vec![
        ("id", string(&macro_.id)),
        ("canonical", string(&macro_.canonical)),
        ("phase", keyword("macro")),
        ("visibility", keyword("public")),
        ("parameters", macro_.parameters.clone()),
        ("minimum-arity", integer_usize(macro_.minimum_arity)),
        ("variadic", boolean(macro_.variadic)),
        ("helper-bindings", strings_form(&macro_.helper_bindings)),
        ("phase-1-ir", macro_.phase_ir.clone()),
    ])
}

pub(super) fn phase_helper_form(helper: &PhaseHelperInterface) -> Form {
    map(vec![
        ("id", string(&helper.id)),
        ("canonical", string(&helper.canonical)),
        ("phase", keyword("syntax")),
        ("visibility", keyword("private")),
        ("phase-1-ir", helper.phase_ir.clone()),
    ])
}
