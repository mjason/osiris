use super::*;

mod from_hir;
mod provisional;
mod provisional_declarations;
mod provisional_validation;

pub use from_hir::from_hir;
pub(crate) use provisional::build_provisional;
pub(crate) use provisional_validation::validate_provisional_shape;

pub fn build(typed: &hir::Module, surface: &ast::Module) -> InterfaceResult<Interface> {
    let static_data = records::analyze_module(surface);
    build_with_static_data(typed, surface, &static_data)
}

/// Build an interface using static declarations already validated in the
/// caller's complete interface environment.
pub fn build_with_static_data(
    typed: &hir::Module,
    surface: &ast::Module,
    static_data: &records::StaticModuleData,
) -> InterfaceResult<Interface> {
    let mut interface = from_hir(typed)?;
    if let Some(diagnostic) = static_data.diagnostics.first() {
        return Err(InterfaceError::new(
            "OSR-I0055",
            format!("invalid static declaration: {}", diagnostic.message),
        ));
    }

    let public_bindings = interface
        .bindings
        .iter()
        .map(|binding| binding.id.as_str())
        .collect::<BTreeSet<_>>();
    interface.static_schemas = static_data
        .schemas
        .iter()
        .filter(|schema| {
            let binding = BindingId::new(&typed.name, &schema.name, BindingKind::Type);
            public_bindings.contains(binding.as_str())
        })
        .cloned()
        .collect();
    interface.owned_records = static_data
        .records
        .iter()
        .filter(|record| record.public && record.module == typed.name)
        .cloned()
        .collect();
    let (macros, phase_helpers) = collect_phase_interface(surface, &typed.name)?;
    interface.macros = macros;
    interface.phase_helpers = phase_helpers;
    interface.operator_instances = collect_operator_instances(typed, surface, &interface)?;
    interface
        .static_schemas
        .sort_by(|left, right| left.name.cmp(&right.name));
    interface
        .owned_records
        .sort_by(|left, right| left.stable_record_id.cmp(&right.stable_record_id));
    validate_model(&interface)?;
    refresh_standalone_hashes(&mut interface)?;
    Ok(interface)
}

fn collect_operator_instances(
    typed: &hir::Module,
    surface: &ast::Module,
    interface: &Interface,
) -> InterfaceResult<Vec<OperatorInstance>> {
    let public_bindings = interface
        .bindings
        .iter()
        .map(|binding| (binding.canonical.as_str(), binding))
        .collect::<BTreeMap<_, _>>();
    let public_types = interface
        .bindings
        .iter()
        .filter(|binding| binding.kind == BindingKind::Type)
        .map(|binding| binding.id.as_str())
        .collect::<BTreeSet<_>>();
    let typed_bindings = typed
        .bindings
        .iter()
        .map(|binding| (binding.name.id.as_str(), binding))
        .collect::<BTreeMap<_, _>>();
    let function_interfaces = interface
        .functions
        .iter()
        .map(|function| (function.binding.as_str(), function))
        .collect::<BTreeMap<_, _>>();

    let mut declarations = Vec::new();
    for item in &surface.items {
        match &item.kind {
            ast::ItemKind::Defn(function) => declarations.push(function),
            ast::ItemKind::Extern(external) => {
                declarations.extend(external.items.iter().filter_map(|item| match &item.kind {
                    ast::ItemKind::Defn(function) => Some(function),
                    _ => None,
                }));
            }
            _ => {}
        }
    }

    let mut instances = Vec::new();
    let mut signatures = BTreeMap::<(ScalarOperator, Vec<Type>), String>::new();
    for function in declarations {
        let declared = ast::operator_declaration(&function.metadata).map_err(|error| {
            InterfaceError::new(
                "OSR-I0061",
                match error {
                    ast::OperatorMetadataError::Duplicate => {
                        "operator declaration metadata is duplicated"
                    }
                    ast::OperatorMetadataError::ExpectedName => {
                        "`:osiris/operator` must contain a keyword or symbol"
                    }
                },
            )
        })?;
        let Some(declared) = declared else {
            continue;
        };
        let operator = ScalarOperator::from_stable_name(&declared).ok_or_else(|| {
            InterfaceError::new("OSR-I0061", format!("unknown static operator `{declared}`"))
        })?;
        if !matches!(
            operator,
            ScalarOperator::Add
                | ScalarOperator::Subtract
                | ScalarOperator::Multiply
                | ScalarOperator::TrueDivide
                | ScalarOperator::Less
                | ScalarOperator::LessEqual
                | ScalarOperator::Greater
                | ScalarOperator::GreaterEqual
                | ScalarOperator::Equal
                | ScalarOperator::NotEqual
                | ScalarOperator::Negate
                | ScalarOperator::Positive
                | ScalarOperator::Abs
        ) {
            return Err(InterfaceError::new(
                "OSR-I0061",
                format!(
                    "operator `{}` is not publishable in the v0 capability set",
                    operator.stable_name()
                ),
            ));
        }
        let name = function.name.as_ref().ok_or_else(|| {
            InterfaceError::new("OSR-I0061", "operator implementation requires a name")
        })?;
        if function.return_type.is_none()
            || function
                .params
                .iter()
                .any(|parameter| parameter.type_annotation.is_none())
        {
            return Err(InterfaceError::new(
                "OSR-I0062",
                format!(
                    "operator implementation `{}` requires explicit parameter and return types",
                    name.canonical
                ),
            ));
        }
        let public = public_bindings
            .get(name.canonical.as_str())
            .ok_or_else(|| {
                InterfaceError::new(
                    "OSR-I0062",
                    format!(
                        "operator implementation `{}` must be a public exported function",
                        name.canonical
                    ),
                )
            })?;
        if public.kind != BindingKind::Function {
            return Err(InterfaceError::new(
                "OSR-I0062",
                format!(
                    "operator implementation `{}` is not a function",
                    name.canonical
                ),
            ));
        }
        let (operands, result, summaries) =
            if let Some(function) = function_interfaces.get(public.id.as_str()) {
                (
                    function
                        .parameters
                        .iter()
                        .map(|parameter| parameter.ty.clone())
                        .collect::<Vec<_>>(),
                    function.return_type.clone(),
                    function.summaries.clone(),
                )
            } else {
                let typed_binding = typed_bindings.get(public.id.as_str()).ok_or_else(|| {
                    InterfaceError::new(
                        "OSR-I0062",
                        format!("operator binding `{}` is absent from typed HIR", public.id),
                    )
                })?;
                let Type::Fn(signature) = &typed_binding.ty else {
                    return Err(InterfaceError::new(
                        "OSR-I0062",
                        format!("operator binding `{}` has no function signature", public.id),
                    ));
                };
                (
                    signature.parameters.clone(),
                    (*signature.return_type).clone(),
                    signature.summaries.clone(),
                )
            };
        let expected_arity = usize::from(matches!(
            operator,
            ScalarOperator::Add
                | ScalarOperator::Subtract
                | ScalarOperator::Multiply
                | ScalarOperator::TrueDivide
                | ScalarOperator::Less
                | ScalarOperator::LessEqual
                | ScalarOperator::Greater
                | ScalarOperator::GreaterEqual
                | ScalarOperator::Equal
                | ScalarOperator::NotEqual
        )) + 1;
        if operands.len() != expected_arity {
            return Err(InterfaceError::new(
                "OSR-I0063",
                format!(
                    "operator `{}` expects {expected_arity} operands, found {}",
                    operator.stable_name(),
                    operands.len()
                ),
            ));
        }
        if operands.iter().any(contains_dynamic_operator_type)
            || contains_dynamic_operator_type(&result)
        {
            return Err(InterfaceError::new(
                "OSR-I0063",
                "operator signatures cannot contain Any, Unknown, or Error",
            ));
        }
        let owner_binding = operands
            .iter()
            .find_map(|operand| match operand {
                Type::Nominal { binding, .. } if public_types.contains(binding.as_str()) => {
                    Some(binding.as_str())
                }
                _ => None,
            })
            .ok_or_else(|| {
                InterfaceError::new(
                    "OSR-I0064",
                    format!(
                        "operator `{}` violates the orphan rule: no operand is a public nominal type owned by `{}`",
                        operator.stable_name(),
                        interface.module
                    ),
                )
            })?;
        let instance = OperatorInstance::new(
            public.id.clone(),
            owner_binding,
            operator,
            operands.clone(),
            result,
            summaries,
        );
        if let Some(previous) = signatures.insert((operator, operands), instance.id.clone()) {
            return Err(InterfaceError::new(
                "OSR-I0065",
                format!(
                    "operator `{}` has conflicting instances `{previous}` and `{}`",
                    operator.stable_name(),
                    instance.id
                ),
            ));
        }
        instances.push(instance);
    }
    instances.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(instances)
}

fn collect_phase_interface(
    surface: &ast::Module,
    module: &str,
) -> InterfaceResult<(Vec<MacroInterface>, Vec<PhaseHelperInterface>)> {
    let exports = surface
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ast::ItemKind::Export(export) => Some(export.names.as_slice()),
            _ => None,
        })
        .flatten()
        .map(|name| name.canonical.clone())
        .collect::<BTreeSet<_>>();

    let mut macro_forms = BTreeMap::<String, Form>::new();
    let mut helper_forms = BTreeMap::<String, Form>::new();
    let mut all_phase_forms = Vec::new();
    for item in &surface.items {
        match &item.kind {
            ast::ItemKind::Defmacro(declaration) => {
                let form = normalize_form(&declaration.phase_form);
                all_phase_forms.push(form.clone());
                macro_forms.insert(declaration.name.canonical.clone(), form);
            }
            ast::ItemKind::DefnForSyntax(declaration) => {
                let Some(name) = declaration.name.as_ref() else {
                    continue;
                };
                let Some(phase_form) = declaration.phase_form.as_ref() else {
                    return Err(InterfaceError::new(
                        "OSR-I0060",
                        format!(
                            "phase-1 helper `{}` lost its declaration form",
                            name.canonical
                        ),
                    ));
                };
                let form = normalize_form(phase_form);
                all_phase_forms.push(form.clone());
                helper_forms.insert(name.canonical.clone(), form);
            }
            _ => {}
        }
    }
    if let Some(diagnostic) = macro_expand::validate_phase_forms(&all_phase_forms).first() {
        return Err(InterfaceError::new(
            "OSR-I0059",
            format!("invalid phase-1 declaration: {}", diagnostic.message),
        ));
    }

    let helper_names = helper_forms.keys().cloned().collect::<BTreeSet<_>>();
    let mut macros = Vec::new();
    let mut required_helpers = BTreeSet::new();
    for (name, phase_ir) in macro_forms
        .into_iter()
        .filter(|(name, _)| exports.contains(name))
    {
        let (declaration_name, parameters, _) = phase_declaration_parts(&phase_ir, "defmacro")?;
        if declaration_name != name {
            return Err(InterfaceError::new(
                "OSR-I0059",
                format!("macro declaration name differs from `{name}`"),
            ));
        }
        let (minimum_arity, variadic) = phase_parameter_arity(parameters)?;
        let closure = phase_helper_closure(&phase_ir, &helper_forms, &helper_names)?;
        required_helpers.extend(closure.iter().cloned());
        macros.push(MacroInterface {
            id: BindingId::new(module, &name, BindingKind::Macro)
                .as_str()
                .to_owned(),
            canonical: name,
            parameters: normalize_form(parameters),
            minimum_arity,
            variadic,
            helper_bindings: closure
                .into_iter()
                .map(|helper| {
                    BindingId::new(module, &helper, BindingKind::Macro)
                        .as_str()
                        .to_owned()
                })
                .collect(),
            phase_ir,
        });
    }
    macros.sort_by(|left, right| left.id.cmp(&right.id));

    let mut phase_helpers = required_helpers
        .into_iter()
        .map(|name| {
            let phase_ir = helper_forms.get(&name).cloned().ok_or_else(|| {
                InterfaceError::new(
                    "OSR-I0060",
                    format!("macro helper closure is missing `{name}`"),
                )
            })?;
            Ok(PhaseHelperInterface {
                id: BindingId::new(module, &name, BindingKind::Macro)
                    .as_str()
                    .to_owned(),
                canonical: name,
                phase_ir,
            })
        })
        .collect::<InterfaceResult<Vec<_>>>()?;
    phase_helpers.sort_by(|left, right| left.id.cmp(&right.id));
    Ok((macros, phase_helpers))
}
