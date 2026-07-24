use super::super::*;

#[derive(Clone)]
pub(super) struct ProvisionalDeclaration {
    pub(super) binding: PublicBinding,
    pub(super) function: Option<FunctionInterface>,
    pub(super) structure: Option<StructInterface>,
    pub(super) operator: Option<OperatorInstance>,
}

pub(super) fn collect_provisional_item(
    module: &str,
    item: &ast::ItemKind,
    declarations: &mut BTreeMap<String, ProvisionalDeclaration>,
    next_type_variable: &mut u32,
) -> InterfaceResult<()> {
    match item {
        ast::ItemKind::Def(definition) => {
            let binding = provisional_value_binding(
                module,
                &definition.name,
                definition
                    .type_annotation
                    .as_ref()
                    .map_or(Type::Unknown, hir::type_from_ast),
                definition.metadata.clone(),
                None,
            );
            declarations.insert(
                definition.name.canonical.clone(),
                ProvisionalDeclaration {
                    binding,
                    function: None,
                    structure: None,
                    operator: None,
                },
            );
        }
        ast::ItemKind::Defn(function) => {
            collect_provisional_function(module, function, None, declarations, next_type_variable)?;
        }
        ast::ItemKind::Defstruct(structure) => {
            collect_provisional_struct(module, structure, declarations, next_type_variable)?;
        }
        ast::ItemKind::DefstaticSchema(schema) => {
            let binding_id = BindingId::new(module, &schema.name.canonical, BindingKind::Type);
            let binding = PublicBinding {
                id: binding_id.as_str().to_owned(),
                canonical: schema.name.canonical.clone(),
                python: python_identifier(&schema.name.canonical),
                kind: BindingKind::Type,
                ty: Type::Nominal {
                    binding: binding_id.as_str().to_owned(),
                    args: Vec::new(),
                },
                runtime: None,
                metadata: normalize_metadata(&schema.metadata)?,
            };
            declarations.insert(
                schema.name.canonical.clone(),
                ProvisionalDeclaration {
                    binding,
                    function: None,
                    structure: None,
                    operator: None,
                },
            );
        }
        ast::ItemKind::Extern(external) => {
            for nested in &external.items {
                match &nested.kind {
                    ast::ItemKind::Def(definition) => {
                        let binding = provisional_value_binding(
                            module,
                            &definition.name,
                            definition
                                .type_annotation
                                .as_ref()
                                .map_or(Type::Any, hir::type_from_ast),
                            definition.metadata.clone(),
                            Some(RuntimeLocator {
                                module: external.module.clone(),
                                name: python_identifier(&definition.name.canonical),
                                python_module: true,
                            }),
                        );
                        declarations.insert(
                            definition.name.canonical.clone(),
                            ProvisionalDeclaration {
                                binding,
                                function: None,
                                structure: None,
                                operator: None,
                            },
                        );
                    }
                    ast::ItemKind::Defn(function) => {
                        collect_provisional_function(
                            module,
                            function,
                            Some(external.module.as_str()),
                            declarations,
                            next_type_variable,
                        )?;
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn provisional_value_binding(
    module: &str,
    name: &Name,
    ty: Type,
    metadata: Vec<MetadataEntry>,
    runtime: Option<RuntimeLocator>,
) -> PublicBinding {
    PublicBinding {
        id: BindingId::new(module, &name.canonical, BindingKind::Value)
            .as_str()
            .to_owned(),
        canonical: name.canonical.clone(),
        python: python_identifier(&name.canonical),
        kind: BindingKind::Value,
        ty,
        runtime,
        metadata,
    }
}

pub(super) fn collect_provisional_function(
    module: &str,
    function: &ast::Function,
    runtime_module: Option<&str>,
    declarations: &mut BTreeMap<String, ProvisionalDeclaration>,
    next_type_variable: &mut u32,
) -> InterfaceResult<()> {
    let Some(name) = &function.name else {
        return Ok(());
    };
    let generic_parameters = function
        .type_params
        .iter()
        .map(|parameter| {
            let variable = Type::TypeVar(TypeVarId(*next_type_variable));
            *next_type_variable = (*next_type_variable).saturating_add(1);
            (parameter.canonical.clone(), variable)
        })
        .collect::<BTreeMap<_, _>>();
    let parameters = function
        .params
        .iter()
        .enumerate()
        .map(|(index, parameter)| {
            let metadata = normalize_metadata(&parameter.metadata)?;
            Ok(ParameterInterface {
                id: format!(
                    "{}::provisional-parameter-{index}",
                    BindingId::new(module, &name.canonical, BindingKind::Function).as_str()
                ),
                canonical: parameter.name.canonical.clone(),
                ty: parameter
                    .type_annotation
                    .as_ref()
                    .map_or(Type::Unknown, |annotation| {
                        hir::type_from_ast_with_generics(annotation, &generic_parameters)
                    }),
                has_default: parameter.default.is_some(),
                variadic: parameter.variadic,
                aliases: metadata_aliases(&metadata, &parameter.name.canonical),
                metadata,
            })
        })
        .collect::<InterfaceResult<Vec<_>>>()?;
    let return_type = function
        .return_type
        .as_ref()
        .map_or(Type::Unknown, |annotation| {
            hir::type_from_ast_with_generics(annotation, &generic_parameters)
        });
    let summaries = function
        .contract
        .as_ref()
        .map_or_else(CallSummaries::unknown, |contract| {
            contract.summaries.clone()
        });
    let binding_id = BindingId::new(module, &name.canonical, BindingKind::Function);
    let signature = FunctionType::new(
        parameters
            .iter()
            .map(|parameter| parameter.ty.clone())
            .collect(),
        return_type.clone(),
    )
    .with_summaries(summaries.clone());
    let binding = PublicBinding {
        id: binding_id.as_str().to_owned(),
        canonical: name.canonical.clone(),
        python: python_identifier(&name.canonical),
        kind: BindingKind::Function,
        ty: Type::Fn(signature),
        runtime: runtime_module.map(|runtime_module| RuntimeLocator {
            module: runtime_module.to_owned(),
            name: python_identifier(&name.canonical),
            python_module: true,
        }),
        metadata: normalize_metadata(&function.metadata)?,
    };
    let function_interface = FunctionInterface {
        binding: binding_id.as_str().to_owned(),
        parameters,
        return_type,
        contract_id: function
            .contract
            .as_ref()
            .map(|contract| contract.id.clone()),
        summaries,
    };
    declarations.insert(
        name.canonical.clone(),
        ProvisionalDeclaration {
            binding,
            function: Some(function_interface),
            structure: None,
            operator: None,
        },
    );
    Ok(())
}

pub(super) fn collect_provisional_struct(
    module: &str,
    structure: &ast::Defstruct,
    declarations: &mut BTreeMap<String, ProvisionalDeclaration>,
    next_type_variable: &mut u32,
) -> InterfaceResult<()> {
    let mut generic_parameters = BTreeMap::new();
    let mut type_parameters = Vec::new();
    for parameter in &structure.type_params {
        let variable = Type::TypeVar(TypeVarId(*next_type_variable));
        *next_type_variable = (*next_type_variable).saturating_add(1);
        generic_parameters.insert(parameter.canonical.clone(), variable);
        type_parameters.push(parameter.canonical.clone());
    }
    let binding_id = BindingId::new(module, &structure.name.canonical, BindingKind::Type);
    let nominal = Type::Nominal {
        binding: binding_id.as_str().to_owned(),
        args: type_parameters
            .iter()
            .filter_map(|name| generic_parameters.get(name).cloned())
            .collect(),
    };
    let binding = PublicBinding {
        id: binding_id.as_str().to_owned(),
        canonical: structure.name.canonical.clone(),
        python: python_identifier(&structure.name.canonical),
        kind: BindingKind::Type,
        ty: nominal,
        runtime: None,
        metadata: normalize_metadata(&structure.metadata)?,
    };
    let fields = structure
        .fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            Ok(FieldInterface {
                id: format!("{}::provisional-field-{index}", binding_id.as_str()),
                canonical: field.name.canonical.clone(),
                ty: field.type_annotation.as_ref().map_or(Type::Unknown, |ty| {
                    hir::type_from_ast_with_generics(ty, &generic_parameters)
                }),
                has_default: field.default.is_some(),
                aliases: metadata_aliases(
                    &normalize_metadata(&field.metadata)?,
                    &field.name.canonical,
                ),
                metadata: normalize_metadata(&field.metadata)?,
            })
        })
        .collect::<InterfaceResult<Vec<_>>>()?;
    let structure_interface = StructInterface {
        binding: binding_id.as_str().to_owned(),
        type_parameters,
        fields,
        invariant_count: structure.checks.len(),
        doc: structure.doc.clone(),
    };
    declarations.insert(
        structure.name.canonical.clone(),
        ProvisionalDeclaration {
            binding,
            function: None,
            structure: Some(structure_interface),
            operator: None,
        },
    );
    Ok(())
}

pub(super) fn provisional_operator(
    function: &ast::Function,
    binding: &PublicBinding,
    signature: &FunctionInterface,
    declarations: &BTreeMap<String, ProvisionalDeclaration>,
) -> Option<OperatorInstance> {
    let declared = ast::operator_declaration(&function.metadata)
        .ok()
        .flatten()?;
    let operator = ScalarOperator::from_stable_name(&declared)?;
    let operands = signature
        .parameters
        .iter()
        .map(|parameter| parameter.ty.clone())
        .collect::<Vec<_>>();
    let owner_binding = operands.iter().find_map(|operand| {
        let Type::Nominal {
            binding: nominal_binding,
            ..
        } = operand
        else {
            return None;
        };
        declarations.values().find_map(|declaration| {
            (declaration.binding.kind == BindingKind::Type
                && declaration.binding.id == *nominal_binding)
                .then(|| declaration.binding.id.clone())
        })
    })?;
    Some(OperatorInstance::new(
        binding.id.clone(),
        owner_binding,
        operator,
        operands,
        signature.return_type.clone(),
        CallSummaries::unknown(),
    ))
}
