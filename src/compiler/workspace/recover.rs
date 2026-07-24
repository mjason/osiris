use super::*;

/// Analyzes every workspace source while preserving semantic models for inputs
/// that contain errors. Provisional interfaces are never trusted build data.
#[must_use]
pub fn analyze_workspace_recovering(
    inputs: &[CompileInput<'_>],
    external_interfaces: &BTreeMap<String, interface::Interface>,
) -> Vec<Analysis> {
    let prepared = inputs
        .iter()
        .enumerate()
        .map(|(input_index, input)| {
            let document = reader::read(input.source);
            let mut lowered = ast::lower_document(&document);
            install_module_identity(&mut lowered.module, input.options, &mut lowered.diagnostics);
            PreparedInput {
                input_index,
                document,
                module_name: lowered
                    .module
                    .name
                    .as_ref()
                    .expect("implicit workspace module name was installed")
                    .canonical
                    .clone(),
                header: lowered.module,
            }
        })
        .collect::<Vec<_>>();

    let mut interfaces = external_interfaces.clone();
    for unit in &prepared {
        if let Ok(model) = interface::build_provisional(&unit.header) {
            interfaces.insert(unit.module_name.clone(), model);
        }
    }

    prepared
        .iter()
        .map(|unit| {
            let imported_phase = imported_phase_modules(&unit.header, &interfaces);
            analyze_document(
                &unit.document,
                inputs[unit.input_index].options,
                &imported_phase,
                Some(&interfaces),
            )
        })
        .collect()
}
