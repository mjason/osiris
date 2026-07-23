use super::super::*;

pub(super) fn validate_operator_instances(
    interface: &Interface,
    bindings: &BTreeMap<&str, &PublicBinding>,
) -> InterfaceResult<()> {
    // Operator capabilities are deliberately closed data.  Validate every
    // reference and signature against the public function/type surface so a
    // hand-edited `.osri` cannot smuggle in an implementation or overload.
    let function_interfaces = interface
        .functions
        .iter()
        .map(|function| (function.binding.as_str(), function))
        .collect::<BTreeMap<_, _>>();
    let mut operator_signatures = BTreeSet::new();
    for instance in &interface.operator_instances {
        let binding = bindings.get(instance.binding.as_str()).ok_or_else(|| {
            InterfaceError::new(
                "OSR-I0066",
                format!(
                    "operator instance `{}` references a missing/private function `{}`",
                    instance.id, instance.binding
                ),
            )
        })?;
        if binding.kind != BindingKind::Function {
            return Err(InterfaceError::new(
                "OSR-I0066",
                format!(
                    "operator instance `{}` binding is not a function",
                    instance.id
                ),
            ));
        }
        let owner = bindings
            .get(instance.owner_binding.as_str())
            .ok_or_else(|| {
                InterfaceError::new(
                    "OSR-I0067",
                    format!(
                        "operator instance `{}` references a missing/private owner type `{}`",
                        instance.id, instance.owner_binding
                    ),
                )
            })?;
        if owner.kind != BindingKind::Type {
            return Err(InterfaceError::new(
                "OSR-I0067",
                format!(
                    "operator instance `{}` owner binding is not a type",
                    instance.id
                ),
            ));
        }
        let expected_id = format!(
            "{}::operator::{}",
            instance.binding,
            instance.operator.stable_name()
        );
        if instance.id != expected_id {
            return Err(InterfaceError::new(
                "OSR-I0068",
                format!(
                    "operator instance id `{}` does not match its binding/operator",
                    instance.id
                ),
            ));
        }
        if !is_publishable_operator(instance.operator) {
            return Err(InterfaceError::new(
                "OSR-I0068",
                format!(
                    "operator `{}` is not publishable in the v0 capability set",
                    instance.operator.stable_name()
                ),
            ));
        }
        let expected_arity = operator_arity(instance.operator);
        if instance.operands.len() != expected_arity {
            return Err(InterfaceError::new(
                "OSR-I0069",
                format!(
                    "operator instance `{}` expects {expected_arity} operands, found {}",
                    instance.id,
                    instance.operands.len()
                ),
            ));
        }
        if instance.operands.iter().any(contains_dynamic_operator_type)
            || contains_dynamic_operator_type(&instance.result)
        {
            return Err(InterfaceError::new(
                "OSR-I0069",
                format!(
                    "operator instance `{}` contains Any, Unknown, or Error",
                    instance.id
                ),
            ));
        }
        if !instance.operands.iter().any(|operand| {
            matches!(
                operand,
                Type::Nominal { binding, .. } if binding == &instance.owner_binding
            )
        }) {
            return Err(InterfaceError::new(
                "OSR-I0070",
                format!(
                    "operator instance `{}` violates the orphan rule for `{}`",
                    instance.id, owner.canonical
                ),
            ));
        }

        let expected_type = Type::Fn(
            FunctionType::new(instance.operands.clone(), instance.result.clone())
                .with_summaries(instance.summaries.clone()),
        );
        if let Some(function) = function_interfaces.get(instance.binding.as_str()) {
            let function_type = Type::Fn(
                FunctionType::new(
                    function
                        .parameters
                        .iter()
                        .map(|parameter| parameter.ty.clone())
                        .collect(),
                    function.return_type.clone(),
                )
                .with_summaries(function.summaries.clone()),
            );
            if function_type != expected_type {
                return Err(InterfaceError::new(
                    "OSR-I0071",
                    format!(
                        "operator instance `{}` signature differs from its function interface",
                        instance.id
                    ),
                ));
            }
        }
        let binding_type = match &binding.ty {
            Type::Fn(function) => Type::Fn(function.clone()),
            _ => {
                return Err(InterfaceError::new(
                    "OSR-I0071",
                    format!(
                        "operator instance `{}` binding has no function type",
                        instance.id
                    ),
                ));
            }
        };
        if binding_type != expected_type {
            return Err(InterfaceError::new(
                "OSR-I0071",
                format!(
                    "operator instance `{}` signature differs from its function binding",
                    instance.id
                ),
            ));
        }
        if !operator_signatures.insert((instance.operator, instance.operands.clone())) {
            return Err(InterfaceError::new(
                "OSR-I0072",
                format!(
                    "duplicate operator instance signature for `{}`",
                    instance.operator.stable_name()
                ),
            ));
        }
    }
    Ok(())
}
