use super::*;

pub(in crate::hir) fn import_type_with_variables(
    context: &mut TypeContext,
    ty: &Type,
    variables: &mut BTreeMap<TypeVarId, Type>,
) -> Type {
    match ty {
        Type::TypeVar(variable) => variables
            .entry(*variable)
            .or_insert_with(|| context.fresh_var())
            .clone(),
        Type::Option(inner) => Type::option(import_type_with_variables(context, inner, variables)),
        Type::Union(members) => Type::union(
            members
                .iter()
                .map(|member| import_type_with_variables(context, member, variables)),
        ),
        Type::Tuple(members) => Type::Tuple(
            members
                .iter()
                .map(|member| import_type_with_variables(context, member, variables))
                .collect(),
        ),
        Type::List(item) => Type::List(Box::new(import_type_with_variables(
            context, item, variables,
        ))),
        Type::Vector(item) => Type::Vector(Box::new(import_type_with_variables(
            context, item, variables,
        ))),
        Type::Map(key, value) => Type::Map(
            Box::new(import_type_with_variables(context, key, variables)),
            Box::new(import_type_with_variables(context, value, variables)),
        ),
        Type::Set(item) => Type::Set(Box::new(import_type_with_variables(
            context, item, variables,
        ))),
        Type::Fn(function) => Type::Fn(
            FunctionType::new(
                function
                    .parameters
                    .iter()
                    .map(|parameter| import_type_with_variables(context, parameter, variables))
                    .collect(),
                import_type_with_variables(context, &function.return_type, variables),
            )
            .with_summaries(function.summaries.clone()),
        ),
        Type::Nominal { binding, args } => Type::Nominal {
            binding: binding.clone(),
            args: args
                .iter()
                .map(|argument| import_type_with_variables(context, argument, variables))
                .collect(),
        },
        other => other.clone(),
    }
}

pub(in crate::hir) fn resolve_function_nominal_bindings(
    function: &FunctionType,
    resolutions: &BTreeMap<String, String>,
    fallback_module: &str,
) -> FunctionType {
    FunctionType::new(
        function
            .parameters
            .iter()
            .map(|parameter| resolve_nominal_bindings(parameter, resolutions, fallback_module))
            .collect(),
        resolve_nominal_bindings(&function.return_type, resolutions, fallback_module),
    )
    .with_summaries(function.summaries.clone())
}

pub(in crate::hir) fn collect_unresolved_nominal_bindings(
    ty: &Type,
    resolutions: &BTreeMap<String, String>,
    unknown: &mut BTreeSet<String>,
) {
    match ty {
        Type::Option(inner) | Type::List(inner) | Type::Vector(inner) | Type::Set(inner) => {
            collect_unresolved_nominal_bindings(inner, resolutions, unknown);
        }
        Type::Union(members) | Type::Tuple(members) => {
            for member in members {
                collect_unresolved_nominal_bindings(member, resolutions, unknown);
            }
        }
        Type::Map(key, value) => {
            collect_unresolved_nominal_bindings(key, resolutions, unknown);
            collect_unresolved_nominal_bindings(value, resolutions, unknown);
        }
        Type::Fn(function) => {
            for parameter in &function.parameters {
                collect_unresolved_nominal_bindings(parameter, resolutions, unknown);
            }
            collect_unresolved_nominal_bindings(&function.return_type, resolutions, unknown);
        }
        Type::Nominal { binding, args } => {
            if !binding.contains("::type::") && !resolutions.contains_key(binding) {
                unknown.insert(binding.clone());
            }
            for argument in args {
                collect_unresolved_nominal_bindings(argument, resolutions, unknown);
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
}

pub(crate) fn resolve_nominal_bindings(
    ty: &Type,
    resolutions: &BTreeMap<String, String>,
    fallback_module: &str,
) -> Type {
    match ty {
        Type::Option(inner) => Type::option(resolve_nominal_bindings(
            inner,
            resolutions,
            fallback_module,
        )),
        Type::Union(members) => Type::union(
            members
                .iter()
                .map(|member| resolve_nominal_bindings(member, resolutions, fallback_module)),
        ),
        Type::Tuple(members) => Type::Tuple(
            members
                .iter()
                .map(|member| resolve_nominal_bindings(member, resolutions, fallback_module))
                .collect(),
        ),
        Type::List(item) => Type::List(Box::new(resolve_nominal_bindings(
            item,
            resolutions,
            fallback_module,
        ))),
        Type::Vector(item) => Type::Vector(Box::new(resolve_nominal_bindings(
            item,
            resolutions,
            fallback_module,
        ))),
        Type::Map(key, value) => Type::Map(
            Box::new(resolve_nominal_bindings(key, resolutions, fallback_module)),
            Box::new(resolve_nominal_bindings(
                value,
                resolutions,
                fallback_module,
            )),
        ),
        Type::Set(item) => Type::Set(Box::new(resolve_nominal_bindings(
            item,
            resolutions,
            fallback_module,
        ))),
        Type::Fn(function) => Type::Fn(resolve_function_nominal_bindings(
            function,
            resolutions,
            fallback_module,
        )),
        Type::Nominal { binding, args } => {
            let binding = if binding.contains("::type::") {
                binding.clone()
            } else {
                match resolutions.get(binding) {
                    Some(resolved) => resolved.clone(),
                    None if fallback_module.is_empty() => binding.clone(),
                    None => return Type::Error,
                }
            };
            Type::Nominal {
                binding,
                args: args
                    .iter()
                    .map(|argument| {
                        resolve_nominal_bindings(argument, resolutions, fallback_module)
                    })
                    .collect(),
            }
        }
        other => other.clone(),
    }
}

pub(in crate::hir) fn collect_parameter_names(form: &Form, names: &mut BTreeSet<String>) {
    let FormKind::Map(entries) = &form.kind else {
        return;
    };
    for pair in entries.chunks_exact(2) {
        let Some(key) = pair.first().and_then(form_keyword_or_symbol) else {
            if let Some(value) = pair.get(1) {
                collect_parameter_names(value, names);
            }
            continue;
        };
        match key.trim_start_matches(':') {
            "preferred" => {
                if let Some(name) = form_name_value(&pair[1]) {
                    names.insert(name);
                }
            }
            "aliases" => {
                if let FormKind::Vector(values) = &pair[1].kind {
                    for value in values {
                        if let Some(name) = form_name_value(value) {
                            names.insert(name);
                        }
                    }
                }
            }
            // Locale keys contain a nested name descriptor.  Recurse so the
            // same parser handles both a single locale and a future map of
            // locale descriptors without hard-coding locale names.
            _ => collect_parameter_names(&pair[1], names),
        }
    }
}

pub(in crate::hir) fn form_keyword_or_symbol(form: &Form) -> Option<&str> {
    match &form.kind {
        FormKind::Keyword(name) | FormKind::Symbol(name) => Some(name.canonical.as_str()),
        _ => None,
    }
}

pub(in crate::hir) fn form_name_value(form: &Form) -> Option<String> {
    match &form.kind {
        FormKind::Symbol(name) => Some(name.canonical.clone()),
        _ => None,
    }
}

pub(crate) fn type_from_ast(expression: &ast::TypeExpr) -> Type {
    type_from_ast_with_generics(expression, &BTreeMap::new())
}

pub(in crate::hir) fn replace_type_variables(
    ty: &Type,
    substitutions: &BTreeMap<TypeVarId, Type>,
) -> Type {
    match ty {
        Type::TypeVar(variable) => substitutions.get(variable).map_or_else(
            || ty.clone(),
            |replacement| replace_type_variables(replacement, substitutions),
        ),
        Type::Option(inner) => Type::option(replace_type_variables(inner, substitutions)),
        Type::Union(members) => Type::union(
            members
                .iter()
                .map(|member| replace_type_variables(member, substitutions)),
        ),
        Type::Tuple(members) => Type::Tuple(
            members
                .iter()
                .map(|member| replace_type_variables(member, substitutions))
                .collect(),
        ),
        Type::List(item) => Type::List(Box::new(replace_type_variables(item, substitutions))),
        Type::Vector(item) => Type::Vector(Box::new(replace_type_variables(item, substitutions))),
        Type::Map(key, value) => Type::Map(
            Box::new(replace_type_variables(key, substitutions)),
            Box::new(replace_type_variables(value, substitutions)),
        ),
        Type::Set(item) => Type::Set(Box::new(replace_type_variables(item, substitutions))),
        Type::Fn(function) => Type::Fn(
            FunctionType::new(
                function
                    .parameters
                    .iter()
                    .map(|parameter| replace_type_variables(parameter, substitutions))
                    .collect(),
                replace_type_variables(&function.return_type, substitutions),
            )
            .with_summaries(function.summaries.clone()),
        ),
        Type::Nominal { binding, args } => Type::Nominal {
            binding: binding.clone(),
            args: args
                .iter()
                .map(|argument| replace_type_variables(argument, substitutions))
                .collect(),
        },
        _ => ty.clone(),
    }
}

pub(in crate::hir) fn contains_type_variable(ty: &Type) -> bool {
    match ty {
        Type::TypeVar(_) => true,
        Type::Option(inner) | Type::List(inner) | Type::Vector(inner) | Type::Set(inner) => {
            contains_type_variable(inner)
        }
        Type::Union(members) | Type::Tuple(members) => members.iter().any(contains_type_variable),
        Type::Map(key, value) => contains_type_variable(key) || contains_type_variable(value),
        Type::Fn(function) => {
            function.parameters.iter().any(contains_type_variable)
                || contains_type_variable(&function.return_type)
        }
        Type::Nominal { args, .. } => args.iter().any(contains_type_variable),
        _ => false,
    }
}

pub(crate) fn type_from_ast_with_generics(
    expression: &ast::TypeExpr,
    generic_parameters: &BTreeMap<String, Type>,
) -> Type {
    match &expression.kind {
        TypeExprKind::Name(name) => generic_parameters
            .get(&name.canonical)
            .cloned()
            .unwrap_or_else(|| type_name(&name.canonical)),
        TypeExprKind::Apply { constructor, args } => {
            let name = match &constructor.kind {
                TypeExprKind::Name(name) => name.canonical.as_str(),
                _ => return Type::Error,
            };
            let args = args
                .iter()
                .map(|argument| type_from_ast_with_generics(argument, generic_parameters))
                .collect::<Vec<_>>();
            match (name, args.as_slice()) {
                ("Option", [item]) => Type::option(item.clone()),
                ("Union", items) => Type::union(items.iter().cloned()),
                ("Tuple", items) => Type::Tuple(items.to_vec()),
                ("List", [item]) => Type::List(Box::new(item.clone())),
                ("Vector", [item]) => Type::Vector(Box::new(item.clone())),
                ("Map", [key, value]) => Type::Map(Box::new(key.clone()), Box::new(value.clone())),
                ("Set", [item]) => Type::Set(Box::new(item.clone())),
                (name, args) => Type::Nominal {
                    binding: name.to_owned(),
                    args: args.to_vec(),
                },
            }
        }
        TypeExprKind::Function {
            parameters,
            return_type,
        } => Type::Fn(
            FunctionType::new(
                parameters
                    .iter()
                    .map(|parameter| type_from_ast_with_generics(parameter, generic_parameters))
                    .collect(),
                type_from_ast_with_generics(return_type, generic_parameters),
            )
            .with_summaries(CallSummaries::unknown()),
        ),
        TypeExprKind::Tuple(items) => Type::Tuple(
            items
                .iter()
                .map(|item| type_from_ast_with_generics(item, generic_parameters))
                .collect(),
        ),
        TypeExprKind::Union(items) => Type::union(
            items
                .iter()
                .map(|item| type_from_ast_with_generics(item, generic_parameters)),
        ),
        TypeExprKind::Literal(form) => TypeLiteral::from_form(form)
            .map(Type::Literal)
            .unwrap_or(Type::Error),
        TypeExprKind::Error(_) => Type::Error,
    }
}

pub(in crate::hir) fn type_name(name: &str) -> Type {
    match name {
        "Bool" => Type::Bool,
        "Int" => Type::Int,
        "Float" => Type::Float,
        "Str" => Type::Str,
        "Bytes" => Type::Bytes,
        "None" => Type::None,
        "Any" => Type::Any,
        "Never" => Type::Never,
        "Unknown" => Type::Unknown,
        "Error" => Type::Error,
        name => Type::Nominal {
            binding: name.to_owned(),
            args: Vec::new(),
        },
    }
}
