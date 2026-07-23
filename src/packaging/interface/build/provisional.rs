use super::super::*;
use super::{collect_phase_interface, provisional_declarations::*};

/// Build the non-executable public shape used while compiling a runtime SCC.
/// It must be replaced by a final interface before publishing an artifact.
pub(crate) fn build_provisional(surface: &ast::Module) -> InterfaceResult<Interface> {
    let module = surface
        .name
        .as_ref()
        .map(|name| name.canonical.clone())
        .ok_or_else(|| {
            InterfaceError::new("OSR-I0080", "provisional interface has no module name")
        })?;

    let exports = surface
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ast::ItemKind::Export(export) => Some(export.names.iter()),
            _ => None,
        })
        .flatten()
        .map(|name| name.canonical.clone())
        .collect::<BTreeSet<_>>();

    let mut declarations = BTreeMap::<String, ProvisionalDeclaration>::new();
    let mut type_variable = 0u32;
    for item in &surface.items {
        collect_provisional_item(&module, &item.kind, &mut declarations, &mut type_variable)?;
    }
    let type_resolutions = declarations
        .iter()
        .filter(|(_, declaration)| declaration.binding.kind == BindingKind::Type)
        .map(|(name, declaration)| (name.clone(), declaration.binding.id.clone()))
        .collect::<BTreeMap<_, _>>();
    for declaration in declarations.values_mut() {
        declaration.binding.ty =
            hir::resolve_nominal_bindings(&declaration.binding.ty, &type_resolutions, "");
        if let Some(function) = &mut declaration.function {
            for parameter in &mut function.parameters {
                parameter.ty = hir::resolve_nominal_bindings(&parameter.ty, &type_resolutions, "");
            }
            function.return_type =
                hir::resolve_nominal_bindings(&function.return_type, &type_resolutions, "");
        }
        if let Some(structure) = &mut declaration.structure {
            for field in &mut structure.fields {
                field.ty = hir::resolve_nominal_bindings(&field.ty, &type_resolutions, "");
            }
        }
    }
    // Operator ownership may refer to a struct declared later in the source;
    // resolve those declarations only after the complete local shape exists.
    for item in &surface.items {
        let mut functions = Vec::<&ast::Function>::new();
        match &item.kind {
            ast::ItemKind::Defn(function) => functions.push(function),
            ast::ItemKind::Extern(external) => {
                functions.extend(
                    external
                        .items
                        .iter()
                        .filter_map(|nested| match &nested.kind {
                            ast::ItemKind::Defn(function) => Some(function),
                            _ => None,
                        }),
                );
            }
            _ => {}
        }
        for function in functions {
            let Some(name) = &function.name else {
                continue;
            };
            let Some(declaration) = declarations.get(&name.canonical) else {
                continue;
            };
            let Some(signature) = declaration.function.clone() else {
                continue;
            };
            let binding = declaration.binding.clone();
            let operator = provisional_operator(function, &binding, &signature, &declarations);
            if let Some(declaration) = declarations.get_mut(&name.canonical) {
                declaration.operator = operator;
            }
        }
    }

    let mut bindings = declarations
        .iter()
        .filter(|(name, _)| exports.contains(*name))
        .map(|(_, declaration)| declaration.binding.clone())
        .collect::<Vec<_>>();
    bindings.sort_by(|left, right| left.id.cmp(&right.id));

    let exported_ids = bindings
        .iter()
        .map(|binding| binding.id.clone())
        .collect::<BTreeSet<_>>();
    let mut aliases = Vec::new();
    for item in &surface.items {
        let ast::ItemKind::Alias(alias) = &item.kind else {
            continue;
        };
        if !exports.contains(&alias.local.canonical) {
            continue;
        }
        let Some(target) = declarations.get(&alias.target.canonical) else {
            continue;
        };
        // Match HIR's boundary rule: a public alias cannot expose a private
        // canonical target.  The final lowering pass remains authoritative;
        // omission here merely makes an invalid provisional import fail closed.
        if !exports.contains(&alias.target.canonical) || !exported_ids.contains(&target.binding.id)
        {
            continue;
        }
        aliases.push(PublicAlias {
            spelling: alias.local.spelling.clone(),
            canonical: alias.local.canonical.clone(),
            target: target.binding.id.clone(),
        });
    }
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

    let mut functions = declarations
        .iter()
        .filter(|(name, _)| exports.contains(*name))
        .filter_map(|(_, declaration)| declaration.function.clone())
        .collect::<Vec<_>>();
    functions.sort_by(|left, right| left.binding.cmp(&right.binding));
    let mut structs = declarations
        .iter()
        .filter(|(name, _)| exports.contains(*name))
        .filter_map(|(_, declaration)| declaration.structure.clone())
        .collect::<Vec<_>>();
    structs.sort_by(|left, right| left.binding.cmp(&right.binding));

    let mut operator_instances = declarations
        .iter()
        .filter(|(name, _)| exports.contains(*name))
        .filter_map(|(_, declaration)| declaration.operator.clone())
        .collect::<Vec<_>>();
    operator_instances.sort_by(|left, right| left.id.cmp(&right.id));

    // Static schemas are data-only and do not depend on function bodies.  A
    // best-effort projection here lets cyclic modules refer to a schema while
    // records are checked again against final interfaces later.
    let static_data = records::analyze_module(surface);
    let static_schemas = static_data
        .schemas
        .into_iter()
        .filter(|schema| exports.contains(&schema.name))
        .collect::<Vec<_>>();

    let (macros, phase_helpers) = collect_phase_interface(surface, &module)?;

    Ok(Interface {
        format_version: FORMAT_VERSION,
        compiler_abi: COMPILER_ABI.to_owned(),
        language_abi: LANGUAGE_ABI.to_owned(),
        module: module.clone(),
        metadata: surface.metadata.clone(),
        bindings,
        aliases,
        functions,
        structs,
        operator_instances,
        macros,
        phase_helpers,
        static_schemas,
        owned_records: Vec::new(),
        graph: empty_hash_group(&module),
        hashes: InterfaceHashes {
            interface_body: String::new(),
            semantic_body: String::new(),
            tooling_body: String::new(),
            content_integrity: String::new(),
        },
    })
}
