use super::super::*;

/// Check that a final interface retains the public shape advertised by a
/// provisional SCC interface while ignoring body-derived call summaries.
pub(crate) fn validate_provisional_shape(
    provisional: &Interface,
    final_interface: &Interface,
) -> InterfaceResult<()> {
    let provisional_bindings = provisional
        .bindings
        .iter()
        .map(|binding| (binding.canonical.as_str(), binding))
        .collect::<BTreeMap<_, _>>();
    let final_bindings = final_interface
        .bindings
        .iter()
        .map(|binding| (binding.canonical.as_str(), binding))
        .collect::<BTreeMap<_, _>>();
    if provisional_bindings.len() != final_bindings.len()
        || provisional_bindings
            .keys()
            .any(|name| !final_bindings.contains_key(name))
    {
        return Err(InterfaceError::new(
            "OSR-I0081",
            format!(
                "final interface `{}` changed its exported binding set",
                provisional.module
            ),
        ));
    }
    for (name, expected) in &provisional_bindings {
        let actual = final_bindings
            .get(name)
            .expect("binding set was checked above");
        if expected.kind != actual.kind || !provisional_type_matches(&expected.ty, &actual.ty) {
            return Err(InterfaceError::new(
                "OSR-I0081",
                format!(
                    "final interface `{}` changed binding `{name}`",
                    provisional.module
                ),
            ));
        }
    }

    let provisional_functions = provisional
        .functions
        .iter()
        .filter_map(|function| {
            provisional_bindings
                .values()
                .find(|binding| binding.id == function.binding)
                .map(|binding| (binding.canonical.as_str(), function))
        })
        .collect::<BTreeMap<_, _>>();
    let final_functions = final_interface
        .functions
        .iter()
        .filter_map(|function| {
            final_bindings
                .values()
                .find(|binding| binding.id == function.binding)
                .map(|binding| (binding.canonical.as_str(), function))
        })
        .collect::<BTreeMap<_, _>>();
    if provisional_functions.len() != final_functions.len() {
        return Err(InterfaceError::new(
            "OSR-I0081",
            format!(
                "final interface `{}` changed function declarations",
                provisional.module
            ),
        ));
    }
    for (name, expected) in provisional_functions {
        let Some(actual) = final_functions.get(name) else {
            return Err(InterfaceError::new(
                "OSR-I0081",
                format!(
                    "final interface `{}` removed function `{name}`",
                    provisional.module
                ),
            ));
        };
        if expected.parameters.len() != actual.parameters.len()
            || expected
                .parameters
                .iter()
                .zip(&actual.parameters)
                .any(|(left, right)| {
                    left.canonical != right.canonical
                        || left.has_default != right.has_default
                        || left.variadic != right.variadic
                        || !provisional_type_matches(&left.ty, &right.ty)
                })
            || !provisional_type_matches(&expected.return_type, &actual.return_type)
        {
            return Err(InterfaceError::new(
                "OSR-I0081",
                format!(
                    "final interface `{}` changed function `{name}`",
                    provisional.module
                ),
            ));
        }
    }

    let provisional_structs = provisional
        .structs
        .iter()
        .filter_map(|structure| {
            provisional_bindings
                .values()
                .find(|binding| binding.id == structure.binding)
                .map(|binding| (binding.canonical.as_str(), structure))
        })
        .collect::<BTreeMap<_, _>>();
    let final_structs = final_interface
        .structs
        .iter()
        .filter_map(|structure| {
            final_bindings
                .values()
                .find(|binding| binding.id == structure.binding)
                .map(|binding| (binding.canonical.as_str(), structure))
        })
        .collect::<BTreeMap<_, _>>();
    if provisional_structs.len() != final_structs.len() {
        return Err(InterfaceError::new(
            "OSR-I0081",
            format!(
                "final interface `{}` changed struct declarations",
                provisional.module
            ),
        ));
    }
    for (name, expected) in provisional_structs {
        let Some(actual) = final_structs.get(name) else {
            return Err(InterfaceError::new(
                "OSR-I0081",
                format!(
                    "final interface `{}` removed struct `{name}`",
                    provisional.module
                ),
            ));
        };
        if expected.type_parameters != actual.type_parameters
            || expected.fields.len() != actual.fields.len()
            || expected
                .fields
                .iter()
                .zip(&actual.fields)
                .any(|(left, right)| {
                    left.canonical != right.canonical
                        || left.has_default != right.has_default
                        || !provisional_type_matches(&left.ty, &right.ty)
                })
        {
            return Err(InterfaceError::new(
                "OSR-I0081",
                format!(
                    "final interface `{}` changed struct `{name}`",
                    provisional.module
                ),
            ));
        }
    }

    let aliases = |interface: &Interface| {
        interface
            .aliases
            .iter()
            .filter_map(|alias| {
                interface
                    .bindings
                    .iter()
                    .find(|binding| binding.id == alias.target)
                    .map(|binding| (alias.canonical.clone(), binding.canonical.clone()))
            })
            .collect::<BTreeSet<_>>()
    };
    if aliases(provisional) != aliases(final_interface) {
        return Err(InterfaceError::new(
            "OSR-I0081",
            format!("final interface `{}` changed aliases", provisional.module),
        ));
    }

    let operators = |interface: &Interface| {
        interface
            .operator_instances
            .iter()
            .map(|instance| {
                (
                    instance.operator,
                    instance
                        .binding
                        .split("::function::")
                        .nth(1)
                        .unwrap_or(instance.binding.as_str())
                        .to_owned(),
                    instance.owner_binding.clone(),
                    instance.operands.clone(),
                    instance.result.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    let provisional_operators = operators(provisional);
    let final_operators = operators(final_interface);
    if provisional_operators.len() != final_operators.len()
        || provisional_operators
            .iter()
            .zip(&final_operators)
            .any(|(left, right)| {
                left.0 != right.0
                    || left.1 != right.1
                    || left.2 != right.2
                    || left.3.len() != right.3.len()
                    || left
                        .3
                        .iter()
                        .zip(&right.3)
                        .any(|(a, b)| !provisional_type_matches(a, b))
                    || !provisional_type_matches(&left.4, &right.4)
            })
    {
        return Err(InterfaceError::new(
            "OSR-I0081",
            format!(
                "final interface `{}` changed operator declarations",
                provisional.module
            ),
        ));
    }
    Ok(())
}

pub(super) fn provisional_type_matches(expected: &Type, actual: &Type) -> bool {
    match (expected, actual) {
        (Type::Unknown, _) | (_, Type::Unknown) => true,
        (Type::TypeVar(_), Type::TypeVar(_)) => true,
        (Type::Option(left), Type::Option(right))
        | (Type::List(left), Type::List(right))
        | (Type::Vector(left), Type::Vector(right))
        | (Type::Set(left), Type::Set(right)) => provisional_type_matches(left, right),
        (Type::Map(left_key, left_value), Type::Map(right_key, right_value)) => {
            provisional_type_matches(left_key, right_key)
                && provisional_type_matches(left_value, right_value)
        }
        (Type::Union(left), Type::Union(right)) | (Type::Tuple(left), Type::Tuple(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| provisional_type_matches(left, right))
        }
        (Type::Fn(left), Type::Fn(right)) => {
            left.parameters.len() == right.parameters.len()
                && left
                    .parameters
                    .iter()
                    .zip(&right.parameters)
                    .all(|(left, right)| provisional_type_matches(left, right))
                && provisional_type_matches(&left.return_type, &right.return_type)
        }
        (
            Type::Nominal {
                binding: left_binding,
                args: left_args,
            },
            Type::Nominal {
                binding: right_binding,
                args: right_args,
            },
        ) => {
            (left_binding == right_binding
                || (!left_binding.contains("::type::")
                    && nominal_short_name(right_binding) == left_binding))
                && left_args.len() == right_args.len()
                && left_args
                    .iter()
                    .zip(right_args)
                    .all(|(left, right)| provisional_type_matches(left, right))
        }
        (left, right) => left == right,
    }
}
