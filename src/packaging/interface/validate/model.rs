use super::super::*;
use super::{metadata::*, nominal::*, operators::*};

pub(in crate::interface) fn validate_model(interface: &Interface) -> InterfaceResult<()> {
    validate_interface_metadata_resources(interface)?;
    if interface.module.is_empty() {
        return Err(InterfaceError::new("OSR-I0016", "empty module name"));
    }
    unique(
        interface.bindings.iter().map(|binding| &binding.id),
        "binding id",
    )?;
    unique(
        interface.bindings.iter().map(|binding| &binding.canonical),
        "binding name",
    )?;
    unique(
        interface.aliases.iter().map(|alias| &alias.canonical),
        "alias",
    )?;
    unique(
        interface.functions.iter().map(|function| &function.binding),
        "function",
    )?;
    unique(
        interface.structs.iter().map(|structure| &structure.binding),
        "struct",
    )?;
    unique(
        interface
            .operator_instances
            .iter()
            .map(|instance| &instance.id),
        "operator instance id",
    )?;
    unique(
        interface.static_schemas.iter().map(|schema| &schema.name),
        "static schema name",
    )?;
    let mut schema_versions = BTreeSet::new();
    for schema in &interface.static_schemas {
        if !schema_versions.insert((&schema.schema_id, schema.version)) {
            return Err(InterfaceError::new(
                "OSR-I0024",
                format!(
                    "duplicate static schema id/version `{}@{}`",
                    schema.schema_id, schema.version
                ),
            ));
        }
    }
    unique(
        interface
            .owned_records
            .iter()
            .map(|record| &record.stable_record_id),
        "owned static record",
    )?;
    let bindings = interface
        .bindings
        .iter()
        .map(|binding| (binding.id.as_str(), binding))
        .collect::<BTreeMap<_, _>>();
    validate_nominal_type_identities(interface, &bindings)?;
    let mut names = interface
        .bindings
        .iter()
        .map(|binding| binding.canonical.as_str())
        .collect::<BTreeSet<_>>();
    for alias in &interface.aliases {
        if !bindings.contains_key(alias.target.as_str()) {
            return Err(InterfaceError::new(
                "OSR-I0017",
                format!(
                    "alias `{}` leaks missing/private target `{}`",
                    alias.spelling, alias.target
                ),
            ));
        }
        if !names.insert(&alias.canonical) {
            return Err(InterfaceError::new(
                "OSR-I0018",
                format!("public name `{}` is duplicated", alias.canonical),
            ));
        }
    }
    let mut contract_ids = BTreeSet::new();
    for function in &interface.functions {
        let binding = bindings.get(function.binding.as_str()).ok_or_else(|| {
            InterfaceError::new(
                "OSR-I0019",
                format!("function `{}` leaks a private binding", function.binding),
            )
        })?;
        if binding.kind != BindingKind::Function {
            return Err(InterfaceError::new(
                "OSR-I0020",
                format!("function `{}` references a non-function", function.binding),
            ));
        }
        if let Some(contract_id) = &function.contract_id {
            if contract_id.is_empty()
                || contract_id.trim() != contract_id
                || contract_id.chars().any(char::is_control)
            {
                return Err(InterfaceError::new(
                    "OSR-I0074",
                    format!("function `{}` has an invalid contract id", function.binding),
                ));
            }
            if !contract_ids.insert(contract_id) {
                return Err(InterfaceError::new(
                    "OSR-I0074",
                    format!("duplicate declared contract id `{contract_id}`"),
                ));
            }
        }
        unique(
            function.parameters.iter().map(|parameter| &parameter.id),
            "parameter id",
        )?;
        let mut parameter_names = BTreeSet::new();
        for parameter in &function.parameters {
            for name in std::iter::once(&parameter.canonical).chain(&parameter.aliases) {
                if !parameter_names.insert(name) {
                    return Err(InterfaceError::new(
                        "OSR-I0021",
                        format!("duplicate parameter name `{name}`"),
                    ));
                }
            }
        }
        let Type::Fn(signature) = &binding.ty else {
            return Err(InterfaceError::new(
                "OSR-I0074",
                format!(
                    "function `{}` binding has no function type",
                    function.binding
                ),
            ));
        };
        let parameters = function
            .parameters
            .iter()
            .map(|parameter| parameter.ty.clone())
            .collect::<Vec<_>>();
        if signature.parameters != parameters
            || signature.return_type.as_ref() != &function.return_type
            || signature.summaries != function.summaries
        {
            return Err(InterfaceError::new(
                "OSR-I0074",
                format!(
                    "function `{}` interface differs from its binding signature",
                    function.binding
                ),
            ));
        }
    }
    for structure in &interface.structs {
        let binding = bindings.get(structure.binding.as_str()).ok_or_else(|| {
            InterfaceError::new(
                "OSR-I0022",
                format!("struct `{}` leaks a private binding", structure.binding),
            )
        })?;
        if binding.kind != BindingKind::Type {
            return Err(InterfaceError::new(
                "OSR-I0023",
                format!("struct `{}` references a non-type", structure.binding),
            ));
        }
        unique(structure.fields.iter().map(|field| &field.id), "field id")?;
        unique(
            structure.fields.iter().map(|field| &field.canonical),
            "field name",
        )?;
        unique(structure.type_parameters.iter(), "type parameter")?;
        let Type::Nominal { args, .. } = &binding.ty else {
            return Err(InterfaceError::new(
                "OSR-I0084",
                format!("struct `{}` binding has no nominal type", structure.binding),
            ));
        };
        if args.len() != structure.type_parameters.len() {
            return Err(InterfaceError::new(
                "OSR-I0084",
                format!(
                    "struct `{}` declares {} type parameters but its nominal type has {} arguments",
                    structure.binding,
                    structure.type_parameters.len(),
                    args.len()
                ),
            ));
        }
    }

    validate_operator_instances(interface, &bindings)?;

    unique(interface.macros.iter().map(|macro_| &macro_.id), "macro id")?;
    unique(
        interface.macros.iter().map(|macro_| &macro_.canonical),
        "macro name",
    )?;
    unique(
        interface.phase_helpers.iter().map(|helper| &helper.id),
        "phase helper id",
    )?;
    unique(
        interface
            .phase_helpers
            .iter()
            .map(|helper| &helper.canonical),
        "phase helper name",
    )?;
    let phase_forms = interface
        .phase_helpers
        .iter()
        .map(|helper| helper.phase_ir.clone())
        .chain(
            interface
                .macros
                .iter()
                .map(|macro_| macro_.phase_ir.clone()),
        )
        .collect::<Vec<_>>();
    if let Some(diagnostic) = macro_expand::validate_phase_forms(&phase_forms).first() {
        return Err(InterfaceError::new(
            "OSR-I0059",
            format!(
                "invalid replayable phase-1 declaration: {}",
                diagnostic.message
            ),
        ));
    }
    let helper_forms = interface
        .phase_helpers
        .iter()
        .map(|helper| (helper.canonical.clone(), helper.phase_ir.clone()))
        .collect::<BTreeMap<_, _>>();
    let helper_names = helper_forms.keys().cloned().collect::<BTreeSet<_>>();
    let mut required_helpers = BTreeSet::new();
    for macro_ in &interface.macros {
        let expected_id = BindingId::new(&interface.module, &macro_.canonical, BindingKind::Macro);
        if macro_.id != expected_id.as_str() {
            return Err(InterfaceError::new(
                "OSR-I0059",
                format!("macro `{}` has an invalid binding id", macro_.canonical),
            ));
        }
        let (name, parameters, _) = phase_declaration_parts(&macro_.phase_ir, "defmacro")?;
        if name != macro_.canonical || normalize_form(parameters) != macro_.parameters {
            return Err(InterfaceError::new(
                "OSR-I0059",
                format!(
                    "macro `{}` signature does not match its phase IR",
                    macro_.canonical
                ),
            ));
        }
        let arity = phase_parameter_arity(parameters)?;
        if arity != (macro_.minimum_arity, macro_.variadic) {
            return Err(InterfaceError::new(
                "OSR-I0059",
                format!(
                    "macro `{}` has an inconsistent arity signature",
                    macro_.canonical
                ),
            ));
        }
        let closure = phase_helper_closure(&macro_.phase_ir, &helper_forms, &helper_names)?;
        required_helpers.extend(closure.iter().cloned());
        let expected_bindings = closure
            .iter()
            .map(|name| {
                BindingId::new(&interface.module, name, BindingKind::Macro)
                    .as_str()
                    .to_owned()
            })
            .collect::<Vec<_>>();
        if macro_.helper_bindings != expected_bindings {
            return Err(InterfaceError::new(
                "OSR-I0060",
                format!(
                    "macro `{}` helper closure is inconsistent",
                    macro_.canonical
                ),
            ));
        }
    }
    if required_helpers.len() != interface.phase_helpers.len()
        || interface
            .phase_helpers
            .iter()
            .any(|helper| !required_helpers.contains(&helper.canonical))
    {
        return Err(InterfaceError::new(
            "OSR-I0060",
            "interface contains a phase helper outside exported macro closures",
        ));
    }
    for helper in &interface.phase_helpers {
        let expected_id = BindingId::new(&interface.module, &helper.canonical, BindingKind::Macro);
        if helper.id != expected_id.as_str() {
            return Err(InterfaceError::new(
                "OSR-I0060",
                format!(
                    "phase helper `{}` has an invalid binding id",
                    helper.canonical
                ),
            ));
        }
        let (name, _, _) = phase_declaration_parts(&helper.phase_ir, "defn-for-syntax")?;
        if name != helper.canonical {
            return Err(InterfaceError::new(
                "OSR-I0060",
                format!(
                    "phase helper `{}` name differs from its phase IR",
                    helper.canonical
                ),
            ));
        }
    }

    let schemas = interface
        .static_schemas
        .iter()
        .map(|schema| {
            let binding = BindingId::new(&interface.module, &schema.name, BindingKind::Type)
                .as_str()
                .to_owned();
            (binding, schema)
        })
        .collect::<BTreeMap<_, _>>();
    for (binding, schema) in &schemas {
        let public_binding = bindings.get(binding.as_str()).ok_or_else(|| {
            InterfaceError::new(
                "OSR-I0056",
                format!("static schema `{}` has no public type binding", schema.name),
            )
        })?;
        if public_binding.kind != BindingKind::Type || public_binding.canonical != schema.name {
            return Err(InterfaceError::new(
                "OSR-I0056",
                format!(
                    "static schema `{}` has an inconsistent public binding",
                    schema.name
                ),
            ));
        }
        schema.verify_integrity().map_err(|error| {
            InterfaceError::new(
                "OSR-I0056",
                format!("invalid static schema `{}`: {}", schema.name, error.message),
            )
        })?;
    }

    for record in &interface.owned_records {
        if !record.public || record.module != interface.module {
            return Err(InterfaceError::new(
                "OSR-I0057",
                "private or non-owned static record leaked into interface",
            ));
        }
        let owner_name = bindings
            .get(record.owner_binding_id.as_str())
            .map(|binding| binding.canonical.as_str())
            .or_else(|| {
                schemas
                    .get(record.owner_binding_id.as_str())
                    .map(|schema| schema.name.as_str())
            });
        if owner_name.is_none() {
            return Err(InterfaceError::new(
                "OSR-I0057",
                format!(
                    "static record `{}` has a missing/private owner `{}`",
                    record.stable_record_id, record.owner_binding_id
                ),
            ));
        }
        if owner_name != Some(record.owner_name.as_str()) {
            return Err(InterfaceError::new(
                "OSR-I0057",
                format!(
                    "static record `{}` owner name does not match `{}`",
                    record.stable_record_id, record.owner_binding_id
                ),
            ));
        }
        if let Some(schema) = schemas.get(record.schema.binding_id.as_str()) {
            records::verify_record_against_schema(record, schema, &record.schema.binding_id)
                .map_err(|error| {
                    InterfaceError::new(
                        "OSR-I0057",
                        format!(
                            "invalid static record `{}`: {}",
                            record.stable_record_id, error.message
                        ),
                    )
                })?;
        } else if record
            .schema
            .binding_id
            .split_once("::")
            .is_some_and(|(module, suffix)| {
                module != interface.module && suffix.starts_with("type::")
            })
        {
            // Imported schemas are validated against the dependency interface
            // during graph compilation. The owning interface retains the
            // exact schema binding/body hash and can still validate the
            // record's canonical identity without pretending to re-export it.
            record.verify_integrity().map_err(|error| {
                InterfaceError::new(
                    "OSR-I0057",
                    format!(
                        "invalid static record `{}`: {}",
                        record.stable_record_id, error.message
                    ),
                )
            })?;
        } else {
            return Err(InterfaceError::new(
                "OSR-I0057",
                format!(
                    "static record `{}` references a missing/private schema `{}`",
                    record.stable_record_id, record.schema.binding_id
                ),
            ));
        }
    }
    Ok(())
}
