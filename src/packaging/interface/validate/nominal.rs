use super::super::*;

pub(super) fn validate_nominal_type_identities(
    interface: &Interface,
    public_bindings: &BTreeMap<&str, &PublicBinding>,
) -> InterfaceResult<()> {
    for binding in &interface.bindings {
        let expected = BindingId::new(&interface.module, &binding.canonical, binding.kind);
        if binding.id != expected.as_str() {
            return Err(InterfaceError::new(
                "OSR-I0084",
                format!(
                    "public binding `{}` has non-canonical identity `{}`",
                    binding.canonical, binding.id
                ),
            ));
        }
        validate_type_nominal_identities(&binding.ty, interface, public_bindings)?;
        if binding.kind == BindingKind::Type
            && !matches!(
                &binding.ty,
                Type::Nominal { binding: nominal, .. } if nominal == &binding.id
            )
        {
            return Err(InterfaceError::new(
                "OSR-I0084",
                format!(
                    "public type `{}` does not carry its own binding identity",
                    binding.canonical
                ),
            ));
        }
    }
    for function in &interface.functions {
        for parameter in &function.parameters {
            validate_type_nominal_identities(&parameter.ty, interface, public_bindings)?;
        }
        validate_type_nominal_identities(&function.return_type, interface, public_bindings)?;
    }
    for structure in &interface.structs {
        for field in &structure.fields {
            validate_type_nominal_identities(&field.ty, interface, public_bindings)?;
        }
    }
    for instance in &interface.operator_instances {
        for operand in &instance.operands {
            validate_type_nominal_identities(operand, interface, public_bindings)?;
        }
        validate_type_nominal_identities(&instance.result, interface, public_bindings)?;
    }
    Ok(())
}

pub(super) fn validate_type_nominal_identities(
    ty: &Type,
    interface: &Interface,
    public_bindings: &BTreeMap<&str, &PublicBinding>,
) -> InterfaceResult<()> {
    match ty {
        Type::Option(inner) | Type::List(inner) | Type::Vector(inner) | Type::Set(inner) => {
            validate_type_nominal_identities(inner, interface, public_bindings)?;
        }
        Type::Union(members) | Type::Tuple(members) => {
            for member in members {
                validate_type_nominal_identities(member, interface, public_bindings)?;
            }
        }
        Type::Map(key, value) => {
            validate_type_nominal_identities(key, interface, public_bindings)?;
            validate_type_nominal_identities(value, interface, public_bindings)?;
        }
        Type::Fn(function) => {
            for parameter in &function.parameters {
                validate_type_nominal_identities(parameter, interface, public_bindings)?;
            }
            validate_type_nominal_identities(&function.return_type, interface, public_bindings)?;
        }
        Type::Nominal { binding, args } => {
            let Some((owner_module, name)) = binding.rsplit_once("::type::") else {
                return Err(InterfaceError::new(
                    "OSR-I0084",
                    format!("nominal type has unresolved binding identity `{binding}`"),
                ));
            };
            if owner_module.is_empty()
                || name.is_empty()
                || BindingId::new(owner_module, name, BindingKind::Type).as_str() != binding
            {
                return Err(InterfaceError::new(
                    "OSR-I0084",
                    format!("nominal type has non-canonical binding identity `{binding}`"),
                ));
            }
            if owner_module == interface.module {
                let owner = public_bindings.get(binding.as_str()).ok_or_else(|| {
                    InterfaceError::new(
                        "OSR-I0084",
                        format!("nominal type leaks private or missing local type `{binding}`"),
                    )
                })?;
                if owner.kind != BindingKind::Type || owner.canonical != name {
                    return Err(InterfaceError::new(
                        "OSR-I0084",
                        format!("nominal type identity `{binding}` is not a public type"),
                    ));
                }
            }
            for argument in args {
                validate_type_nominal_identities(argument, interface, public_bindings)?;
            }
        }
        Type::Bool
        | Type::Int
        | Type::Float
        | Type::Str
        | Type::Bytes
        | Type::None
        | Type::Any
        | Type::Never
        | Type::Unknown
        | Type::Error
        | Type::Literal(_)
        | Type::TypeVar(_) => {}
    }
    Ok(())
}
