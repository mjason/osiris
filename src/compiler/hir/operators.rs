use super::*;

pub(in crate::hir) fn operator_from_name(name: &str) -> Option<Operator> {
    Some(match name {
        "+" => Operator::Add,
        "-" => Operator::Subtract,
        "*" => Operator::Multiply,
        "/" => Operator::Divide,
        "//" => Operator::FloorDivide,
        "%" => Operator::Remainder,
        "=" | "==" => Operator::Equal,
        "!=" | "not=" => Operator::NotEqual,
        "<" => Operator::Less,
        "<=" => Operator::LessEqual,
        ">" => Operator::Greater,
        ">=" => Operator::GreaterEqual,
        "and" => Operator::And,
        "or" => Operator::Or,
        "not" => Operator::Not,
        _ => return None,
    })
}

pub(in crate::hir) fn select_operator_signature<'a>(
    context: &TypeContext,
    signatures: &'a [OperatorSignature],
    operands: &[Type],
) -> Option<&'a OperatorSignature> {
    signatures.iter().find(|signature| {
        signature.operands.len() == operands.len()
            && operands
                .iter()
                .zip(&signature.operands)
                .all(|(actual, expected)| context.is_assignable(actual, expected))
    })
}

pub(in crate::hir) fn is_dynamic_operator_type(ty: &Type) -> bool {
    match ty {
        Type::Any | Type::Unknown | Type::Error => true,
        Type::Option(inner) | Type::List(inner) | Type::Vector(inner) | Type::Set(inner) => {
            is_dynamic_operator_type(inner)
        }
        Type::Union(members) | Type::Tuple(members) => members.iter().any(is_dynamic_operator_type),
        Type::Map(key, value) => is_dynamic_operator_type(key) || is_dynamic_operator_type(value),
        Type::Fn(function) => {
            function.parameters.iter().any(is_dynamic_operator_type)
                || is_dynamic_operator_type(&function.return_type)
        }
        Type::Nominal { args, .. } => args.iter().any(is_dynamic_operator_type),
        _ => false,
    }
}

pub(in crate::hir) fn operator_type_matches(
    context: &TypeContext,
    actual: &Type,
    expected: &Type,
    variables: &mut BTreeMap<TypeVarId, Type>,
) -> bool {
    if is_dynamic_operator_type(actual) || is_dynamic_operator_type(expected) {
        return false;
    }
    match expected {
        Type::TypeVar(variable) => {
            if let Some(previous) = variables.get(variable) {
                return context.is_assignable(actual, previous)
                    || context.is_assignable(previous, actual);
            }
            if matches!(actual, Type::TypeVar(_)) {
                return false;
            }
            variables.insert(*variable, actual.clone());
            true
        }
        Type::Option(expected_inner) => match actual {
            Type::None => true,
            Type::Option(actual_inner) => {
                operator_type_matches(context, actual_inner, expected_inner, variables)
            }
            _ => operator_type_matches(context, actual, expected_inner, variables),
        },
        Type::Union(expected_members) => expected_members.iter().any(|member| {
            let mut trial = variables.clone();
            if operator_type_matches(context, actual, member, &mut trial) {
                *variables = trial;
                true
            } else {
                false
            }
        }),
        Type::Tuple(expected_members) => match actual {
            Type::Tuple(actual_members) if actual_members.len() == expected_members.len() => {
                actual_members
                    .iter()
                    .zip(expected_members)
                    .all(|(actual, expected)| {
                        operator_type_matches(context, actual, expected, variables)
                    })
            }
            _ => false,
        },
        Type::List(expected_inner) => match actual {
            Type::List(actual_inner) => {
                operator_type_matches(context, actual_inner, expected_inner, variables)
            }
            _ => false,
        },
        Type::Vector(expected_inner) => match actual {
            Type::Vector(actual_inner) => {
                operator_type_matches(context, actual_inner, expected_inner, variables)
            }
            _ => false,
        },
        Type::Set(expected_inner) => match actual {
            Type::Set(actual_inner) => {
                operator_type_matches(context, actual_inner, expected_inner, variables)
            }
            _ => false,
        },
        Type::Map(expected_key, expected_value) => match actual {
            Type::Map(actual_key, actual_value) => {
                operator_type_matches(context, actual_key, expected_key, variables)
                    && operator_type_matches(context, actual_value, expected_value, variables)
            }
            _ => false,
        },
        Type::Nominal {
            binding: expected_binding,
            args: expected_args,
        } => match actual {
            Type::Nominal {
                binding: actual_binding,
                args: actual_args,
            } if actual_binding == expected_binding && actual_args.len() == expected_args.len() => {
                actual_args
                    .iter()
                    .zip(expected_args)
                    .all(|(actual, expected)| {
                        operator_type_matches(context, actual, expected, variables)
                    })
            }
            _ => false,
        },
        _ => context.is_assignable(actual, expected),
    }
}

pub(in crate::hir) fn contains_unresolved_operator_variable(
    ty: &Type,
    variables: &BTreeMap<TypeVarId, Type>,
) -> bool {
    match ty {
        Type::TypeVar(variable) => !variables.contains_key(variable),
        Type::Option(inner) | Type::List(inner) | Type::Vector(inner) | Type::Set(inner) => {
            contains_unresolved_operator_variable(inner, variables)
        }
        Type::Union(members) | Type::Tuple(members) => members
            .iter()
            .any(|member| contains_unresolved_operator_variable(member, variables)),
        Type::Map(key, value) => {
            contains_unresolved_operator_variable(key, variables)
                || contains_unresolved_operator_variable(value, variables)
        }
        Type::Fn(function) => {
            function
                .parameters
                .iter()
                .any(|parameter| contains_unresolved_operator_variable(parameter, variables))
                || contains_unresolved_operator_variable(&function.return_type, variables)
        }
        Type::Nominal { args, .. } => args
            .iter()
            .any(|argument| contains_unresolved_operator_variable(argument, variables)),
        _ => false,
    }
}
