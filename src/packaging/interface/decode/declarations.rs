use super::super::*;
use super::{support::*, type_data::*};

pub(super) fn decode_binding(form: &Form) -> InterfaceResult<PublicBinding> {
    let values = strict_map(
        form,
        &[
            "id",
            "canonical",
            "python",
            "kind",
            "visibility",
            "type",
            "runtime",
            "metadata",
        ],
    )?;
    require_public(get(&values, "visibility")?)?;
    Ok(PublicBinding {
        id: expect_string(get(&values, "id")?, "binding id")?,
        canonical: expect_string(get(&values, "canonical")?, "canonical")?,
        python: expect_string(get(&values, "python")?, "python")?,
        kind: decode_binding_kind(get(&values, "kind")?)?,
        ty: decode_type(get(&values, "type")?)?,
        runtime: if is_none(get(&values, "runtime")?) {
            None
        } else {
            Some(decode_runtime(get(&values, "runtime")?)?)
        },
        metadata: decode_metadata(get(&values, "metadata")?)?,
    })
}

pub(super) fn decode_runtime(form: &Form) -> InterfaceResult<RuntimeLocator> {
    let values = strict_map(form, &["module", "name", "python-module"])?;
    Ok(RuntimeLocator {
        module: expect_string(get(&values, "module")?, "runtime module")?,
        name: expect_string(get(&values, "name")?, "runtime name")?,
        python_module: expect_bool(get(&values, "python-module")?, "python-module")?,
    })
}

pub(super) fn decode_alias(form: &Form) -> InterfaceResult<PublicAlias> {
    let values = strict_map(form, &["spelling", "canonical", "target", "visibility"])?;
    require_public(get(&values, "visibility")?)?;
    Ok(PublicAlias {
        spelling: expect_string(get(&values, "spelling")?, "alias spelling")?,
        canonical: expect_string(get(&values, "canonical")?, "alias canonical")?,
        target: expect_string(get(&values, "target")?, "alias target")?,
    })
}

pub(super) fn decode_function(form: &Form) -> InterfaceResult<FunctionInterface> {
    let values = strict_map(
        form,
        &[
            "binding",
            "parameters",
            "return",
            "contract-id",
            "summaries",
        ],
    )?;
    Ok(FunctionInterface {
        binding: expect_string(get(&values, "binding")?, "function binding")?,
        parameters: decode_vector(get(&values, "parameters")?, decode_parameter)?,
        return_type: decode_type(get(&values, "return")?)?,
        contract_id: decode_optional_string(get(&values, "contract-id")?, "contract id")?,
        summaries: decode_summaries(get(&values, "summaries")?)?,
    })
}

pub(super) fn decode_parameter(form: &Form) -> InterfaceResult<ParameterInterface> {
    let values = strict_map(
        form,
        &[
            "id",
            "canonical",
            "type",
            "has-default",
            "variadic",
            "aliases",
            "metadata",
        ],
    )?;
    Ok(ParameterInterface {
        id: expect_string(get(&values, "id")?, "parameter id")?,
        canonical: expect_string(get(&values, "canonical")?, "parameter name")?,
        ty: decode_type(get(&values, "type")?)?,
        has_default: expect_bool(get(&values, "has-default")?, "has-default")?,
        variadic: expect_bool(get(&values, "variadic")?, "variadic")?,
        aliases: decode_strings(get(&values, "aliases")?, "parameter aliases")?,
        metadata: decode_metadata(get(&values, "metadata")?)?,
    })
}

pub(super) fn decode_struct(form: &Form) -> InterfaceResult<StructInterface> {
    let values = strict_map(
        form,
        &[
            "binding",
            "type-parameters",
            "fields",
            "invariant-count",
            "doc",
        ],
    )?;
    Ok(StructInterface {
        binding: expect_string(get(&values, "binding")?, "struct binding")?,
        type_parameters: decode_strings(get(&values, "type-parameters")?, "type parameters")?,
        fields: decode_vector(get(&values, "fields")?, decode_field)?,
        invariant_count: expect_usize(get(&values, "invariant-count")?, "invariant count")?,
        doc: decode_optional_string(get(&values, "doc")?, "doc")?,
    })
}

pub(super) fn decode_field(form: &Form) -> InterfaceResult<FieldInterface> {
    let values = strict_map(
        form,
        &[
            "id",
            "canonical",
            "type",
            "has-default",
            "aliases",
            "metadata",
        ],
    )?;
    Ok(FieldInterface {
        id: expect_string(get(&values, "id")?, "field id")?,
        canonical: expect_string(get(&values, "canonical")?, "field name")?,
        ty: decode_type(get(&values, "type")?)?,
        has_default: expect_bool(get(&values, "has-default")?, "has-default")?,
        aliases: decode_strings(get(&values, "aliases")?, "field aliases")?,
        metadata: decode_metadata(get(&values, "metadata")?)?,
    })
}

pub(super) fn decode_operator_instance(form: &Form) -> InterfaceResult<OperatorInstance> {
    let values = strict_map(
        form,
        &[
            "id",
            "binding",
            "owner-binding",
            "operator",
            "operands",
            "result",
            "summaries",
        ],
    )?;
    let operator_name = expect_keyword(get(&values, "operator")?, "operator")?;
    let operator = ScalarOperator::from_stable_name(operator_name).ok_or_else(|| {
        InterfaceError::new(
            "OSR-I0068",
            format!("unknown static operator `{operator_name}`"),
        )
    })?;
    if operator.stable_name() != operator_name {
        return Err(InterfaceError::new(
            "OSR-I0068",
            format!("operator `{operator_name}` is not in canonical wire form"),
        ));
    }
    Ok(OperatorInstance {
        id: expect_string(get(&values, "id")?, "operator instance id")?,
        binding: expect_string(get(&values, "binding")?, "operator binding")?,
        owner_binding: expect_string(get(&values, "owner-binding")?, "operator owner binding")?,
        operator,
        operands: decode_vector(get(&values, "operands")?, decode_type)?,
        result: decode_type(get(&values, "result")?)?,
        summaries: decode_summaries(get(&values, "summaries")?)?,
    })
}

pub(super) fn decode_macro_interface(form: &Form) -> InterfaceResult<MacroInterface> {
    let values = strict_map(
        form,
        &[
            "id",
            "canonical",
            "phase",
            "visibility",
            "parameters",
            "minimum-arity",
            "variadic",
            "helper-bindings",
            "phase-1-ir",
        ],
    )?;
    require_public(get(&values, "visibility")?)?;
    if expect_keyword(get(&values, "phase")?, "macro phase")? != "macro" {
        return Err(InterfaceError::new(
            "OSR-I0059",
            "public macro has an invalid phase",
        ));
    }
    Ok(MacroInterface {
        id: expect_string(get(&values, "id")?, "macro id")?,
        canonical: expect_string(get(&values, "canonical")?, "macro name")?,
        parameters: get(&values, "parameters")?.clone(),
        minimum_arity: expect_usize(get(&values, "minimum-arity")?, "macro minimum arity")?,
        variadic: expect_bool(get(&values, "variadic")?, "macro variadic")?,
        helper_bindings: decode_strings(get(&values, "helper-bindings")?, "helper bindings")?,
        phase_ir: get(&values, "phase-1-ir")?.clone(),
    })
}

pub(super) fn decode_phase_helper(form: &Form) -> InterfaceResult<PhaseHelperInterface> {
    let values = strict_map(
        form,
        &["id", "canonical", "phase", "visibility", "phase-1-ir"],
    )?;
    require_private(get(&values, "visibility")?)?;
    if expect_keyword(get(&values, "phase")?, "helper phase")? != "syntax" {
        return Err(InterfaceError::new(
            "OSR-I0060",
            "phase-1 helper has an invalid phase",
        ));
    }
    Ok(PhaseHelperInterface {
        id: expect_string(get(&values, "id")?, "phase-1 helper id")?,
        canonical: expect_string(get(&values, "canonical")?, "phase-1 helper name")?,
        phase_ir: get(&values, "phase-1-ir")?.clone(),
    })
}
