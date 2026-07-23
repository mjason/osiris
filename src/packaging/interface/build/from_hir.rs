use super::super::*;

pub fn from_hir(typed: &hir::Module) -> InterfaceResult<Interface> {
    let exports = typed
        .exports
        .iter()
        .map(|id| id.as_str().to_owned())
        .collect::<BTreeSet<_>>();
    let by_id = typed
        .bindings
        .iter()
        .map(|binding| (binding.name.id.as_str(), binding))
        .collect::<BTreeMap<_, _>>();

    let mut bindings = Vec::new();
    for id in &exports {
        let binding = by_id.get(id.as_str()).ok_or_else(|| {
            InterfaceError::new("OSR-I0001", format!("export `{id}` has no typed binding"))
        })?;
        if !binding.public {
            return Err(InterfaceError::new(
                "OSR-I0002",
                format!("export `{id}` is private in typed HIR"),
            ));
        }
        bindings.push(PublicBinding {
            id: id.clone(),
            canonical: binding.name.canonical.clone(),
            python: binding.name.python.clone(),
            kind: binding.name.kind,
            ty: binding.ty.clone(),
            runtime: binding.runtime.as_ref().map(|runtime| RuntimeLocator {
                module: runtime.module.clone(),
                name: runtime.name.clone(),
                python_module: runtime.python_module,
            }),
            metadata: normalize_metadata(&binding.metadata)?,
        });
    }
    bindings.sort_by(|left, right| left.id.cmp(&right.id));

    if let Some(binding) = typed
        .bindings
        .iter()
        .find(|binding| binding.public && !exports.contains(binding.name.id.as_str()))
    {
        return Err(InterfaceError::new(
            "OSR-I0003",
            format!(
                "public binding `{}` is absent from export table",
                binding.name.id.as_str()
            ),
        ));
    }

    let mut aliases = typed
        .aliases
        .iter()
        .filter(|alias| alias.public)
        .map(|alias| {
            if !exports.contains(alias.target.as_str()) {
                return Err(InterfaceError::new(
                    "OSR-I0004",
                    format!(
                        "public alias `{}` targets private binding `{}`",
                        alias.spelling,
                        alias.target.as_str()
                    ),
                ));
            }
            Ok(PublicAlias {
                spelling: alias.spelling.clone(),
                canonical: alias.canonical.clone(),
                target: alias.target.as_str().to_owned(),
            })
        })
        .collect::<InterfaceResult<Vec<_>>>()?;
    for binding in &bindings {
        for alias in metadata_aliases(&binding.metadata, &binding.canonical) {
            aliases.push(PublicAlias {
                spelling: alias.clone(),
                canonical: alias,
                target: binding.id.clone(),
            });
        }
    }
    aliases.sort_by(|left, right| {
        (&left.canonical, &left.target).cmp(&(&right.canonical, &right.target))
    });
    aliases
        .dedup_by(|left, right| left.canonical == right.canonical && left.target == right.target);

    let mut functions = Vec::new();
    let mut structs = Vec::new();
    for item in &typed.items {
        match &item.kind {
            ItemKind::Function(function) if exports.contains(function.binding.as_str()) => {
                let parameters = function
                    .parameters
                    .iter()
                    .map(|parameter| {
                        let binding = by_id.get(parameter.binding.as_str()).ok_or_else(|| {
                            InterfaceError::new(
                                "OSR-I0005",
                                format!(
                                    "parameter `{}` has no typed binding",
                                    parameter.binding.as_str()
                                ),
                            )
                        })?;
                        let metadata = normalize_metadata(&binding.metadata)?;
                        Ok(ParameterInterface {
                            id: parameter.binding.as_str().to_owned(),
                            canonical: binding.name.canonical.clone(),
                            ty: parameter.ty.clone(),
                            has_default: parameter.default.is_some(),
                            variadic: parameter.variadic,
                            aliases: metadata_aliases(&metadata, &binding.name.canonical),
                            metadata,
                        })
                    })
                    .collect::<InterfaceResult<Vec<_>>>()?;
                functions.push(FunctionInterface {
                    binding: function.binding.as_str().to_owned(),
                    parameters,
                    return_type: function.return_type.clone(),
                    contract_id: None,
                    summaries: function.summaries.clone(),
                });
            }
            ItemKind::Struct(structure) if exports.contains(structure.binding.as_str()) => {
                let fields = structure
                    .fields
                    .iter()
                    .map(|field| {
                        let binding = by_id.get(field.binding.as_str()).ok_or_else(|| {
                            InterfaceError::new(
                                "OSR-I0006",
                                format!("field `{}` has no typed binding", field.binding.as_str()),
                            )
                        })?;
                        let metadata = normalize_metadata(&binding.metadata)?;
                        Ok(FieldInterface {
                            id: field.binding.as_str().to_owned(),
                            canonical: binding.name.canonical.clone(),
                            ty: field.ty.clone(),
                            has_default: field.default.is_some(),
                            aliases: metadata_aliases(&metadata, &binding.name.canonical),
                            metadata,
                        })
                    })
                    .collect::<InterfaceResult<Vec<_>>>()?;
                structs.push(StructInterface {
                    binding: structure.binding.as_str().to_owned(),
                    type_parameters: structure.type_parameters.clone(),
                    fields,
                    invariant_count: structure.checks.len(),
                    doc: structure.doc.clone(),
                });
            }
            _ => {}
        }
    }
    for function in &typed.extern_functions {
        if !exports.contains(function.binding.as_str()) {
            continue;
        }
        let parameters = function
            .parameters
            .iter()
            .map(|parameter| {
                let binding = by_id.get(parameter.binding.as_str()).ok_or_else(|| {
                    InterfaceError::new(
                        "OSR-I0005",
                        format!(
                            "extern parameter `{}` has no typed binding",
                            parameter.binding.as_str()
                        ),
                    )
                })?;
                let metadata = normalize_metadata(&binding.metadata)?;
                Ok(ParameterInterface {
                    id: parameter.binding.as_str().to_owned(),
                    canonical: binding.name.canonical.clone(),
                    ty: parameter.ty.clone(),
                    has_default: parameter.default.is_some(),
                    variadic: parameter.variadic,
                    aliases: metadata_aliases(&metadata, &binding.name.canonical),
                    metadata,
                })
            })
            .collect::<InterfaceResult<Vec<_>>>()?;
        functions.push(FunctionInterface {
            binding: function.binding.as_str().to_owned(),
            parameters,
            return_type: function.return_type.clone(),
            contract_id: function.contract_id.clone(),
            summaries: function.summaries.clone(),
        });
    }
    functions.sort_by(|left, right| left.binding.cmp(&right.binding));
    structs.sort_by(|left, right| left.binding.cmp(&right.binding));

    let mut interface = Interface {
        format_version: FORMAT_VERSION,
        compiler_abi: COMPILER_ABI.to_owned(),
        language_abi: LANGUAGE_ABI.to_owned(),
        module: typed.name.clone(),
        metadata: normalize_metadata(&typed.metadata)?,
        bindings,
        aliases,
        functions,
        structs,
        operator_instances: Vec::new(),
        macros: Vec::new(),
        phase_helpers: Vec::new(),
        static_schemas: Vec::new(),
        owned_records: Vec::new(),
        graph: empty_hash_group(&typed.name),
        hashes: InterfaceHashes {
            interface_body: String::new(),
            semantic_body: String::new(),
            tooling_body: String::new(),
            content_integrity: String::new(),
        },
    };
    validate_model(&interface)?;
    refresh_standalone_hashes(&mut interface)?;
    Ok(interface)
}
