use super::super::*;

pub(in crate::interface) fn validate_interface_metadata_resources(
    interface: &Interface,
) -> InterfaceResult<()> {
    let bindings = interface
        .bindings
        .iter()
        .map(|binding| (binding.id.as_str(), binding))
        .collect::<BTreeMap<_, _>>();
    let mut counted_bindings = BTreeSet::new();
    let mut interface_usage = MetadataResourceUsage::default();

    let mut declaration = MetadataResourceUsage::default();
    include_metadata_target(
        &interface.metadata,
        "module declaration",
        &mut declaration,
        &mut interface_usage,
    )?;

    for function in &interface.functions {
        let mut declaration = MetadataResourceUsage::default();
        if counted_bindings.insert(function.binding.as_str()) {
            if let Some(binding) = bindings.get(function.binding.as_str()) {
                include_metadata_target(
                    &binding.metadata,
                    &format!("declaration `{}`", binding.canonical),
                    &mut declaration,
                    &mut interface_usage,
                )?;
            }
        }
        for parameter in &function.parameters {
            include_metadata_target(
                &parameter.metadata,
                &format!("parameter `{}`", parameter.canonical),
                &mut declaration,
                &mut interface_usage,
            )?;
        }
    }

    for structure in &interface.structs {
        let mut declaration = MetadataResourceUsage::default();
        if counted_bindings.insert(structure.binding.as_str()) {
            if let Some(binding) = bindings.get(structure.binding.as_str()) {
                include_metadata_target(
                    &binding.metadata,
                    &format!("declaration `{}`", binding.canonical),
                    &mut declaration,
                    &mut interface_usage,
                )?;
            }
        }
        for field in &structure.fields {
            include_metadata_target(
                &field.metadata,
                &format!("field `{}`", field.canonical),
                &mut declaration,
                &mut interface_usage,
            )?;
        }
    }

    for binding in &interface.bindings {
        if counted_bindings.insert(binding.id.as_str()) {
            let mut declaration = MetadataResourceUsage::default();
            include_metadata_target(
                &binding.metadata,
                &format!("declaration `{}`", binding.canonical),
                &mut declaration,
                &mut interface_usage,
            )?;
        }
    }

    for macro_ in &interface.macros {
        let context = format!("macro declaration `{}`", macro_.canonical);
        let mut declaration = MetadataResourceUsage::default();
        include_form_metadata(
            &macro_.parameters,
            &context,
            &mut declaration,
            &mut interface_usage,
        )?;
        include_form_metadata(
            &macro_.phase_ir,
            &context,
            &mut declaration,
            &mut interface_usage,
        )?;
    }

    for helper in &interface.phase_helpers {
        let context = format!("phase-1 declaration `{}`", helper.canonical);
        let mut declaration = MetadataResourceUsage::default();
        include_form_metadata(
            &helper.phase_ir,
            &context,
            &mut declaration,
            &mut interface_usage,
        )?;
    }
    Ok(())
}

pub(super) fn include_form_metadata(
    root: &Form,
    context: &str,
    declaration: &mut MetadataResourceUsage,
    interface: &mut MetadataResourceUsage,
) -> InterfaceResult<()> {
    let mut pending = vec![root];
    while let Some(form) = pending.pop() {
        include_metadata_target(&form.metadata, context, declaration, interface)?;
        match &form.kind {
            FormKind::List(items)
            | FormKind::Vector(items)
            | FormKind::Map(items)
            | FormKind::Set(items) => pending.extend(items),
            FormKind::ReaderMacro { form, .. } => pending.push(form),
            _ => {}
        }
    }
    Ok(())
}

pub(super) fn include_metadata_target(
    metadata: &[MetadataEntry],
    context: &str,
    declaration: &mut MetadataResourceUsage,
    interface: &mut MetadataResourceUsage,
) -> InterfaceResult<()> {
    if metadata.is_empty() {
        return Ok(());
    }
    let usage = validate_metadata_target(metadata, context)?;
    *declaration = declaration.saturating_add(usage);
    check_metadata_usage(*declaration, METADATA_DECLARATION_LIMITS)
        .map_err(|exceeded| metadata_resource_error(context, "declaration", exceeded))?;
    *interface = interface.saturating_add(usage);
    check_metadata_usage(*interface, METADATA_INTERFACE_LIMITS)
        .map_err(|exceeded| metadata_resource_error(context, "interface", exceeded))?;
    Ok(())
}

pub(in crate::interface) fn validate_metadata_target(
    metadata: &[MetadataEntry],
    context: &str,
) -> InterfaceResult<MetadataResourceUsage> {
    if metadata.iter().any(|entry| {
        !metadata_datum_is_serializable(&entry.key) || !metadata_datum_is_serializable(&entry.value)
    }) {
        return Err(InterfaceError::new(
            "OSR-I0083",
            format!("{context} contains non-serializable metadata data"),
        ));
    }
    check_metadata_resources(metadata, METADATA_TARGET_LIMITS)
        .map_err(|exceeded| metadata_resource_error(context, "syntax target", exceeded))
}

pub(in crate::interface) fn metadata_resource_error(
    context: &str,
    scope: &str,
    exceeded: MetadataLimitExceeded,
) -> InterfaceError {
    InterfaceError::new(
        "OSR-I0082",
        format!(
            "{context} exceeds the metadata {scope} {} limit of {} (found {})",
            exceeded.resource, exceeded.limit, exceeded.actual
        ),
    )
}

pub(super) fn unique<'a>(
    values: impl IntoIterator<Item = &'a String>,
    kind: &str,
) -> InterfaceResult<()> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(InterfaceError::new(
                "OSR-I0024",
                format!("duplicate {kind} `{value}`"),
            ));
        }
    }
    Ok(())
}
