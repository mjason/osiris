use super::*;

pub(super) struct PreparedInput {
    pub(super) input_index: usize,
    pub(super) document: Document,
    pub(super) header: ast::Module,
    pub(super) module_name: String,
}

/// Compile a set of source modules as one distribution-wide dependency graph.
///
/// External interfaces must already have passed `.osri` integrity validation.
/// This function never discovers packages or executes Python; discovery is a
/// project/build-layer responsibility.
#[must_use]
pub fn compile_workspace(
    inputs: &[CompileInput<'_>],
    external_interfaces: &BTreeMap<String, interface::Interface>,
) -> WorkspaceCompileResult {
    if inputs.is_empty() {
        return WorkspaceCompileResult::default();
    }

    let mut prepared = Vec::with_capacity(inputs.len());
    let mut diagnostics = Vec::new();
    for (input_index, input) in inputs.iter().enumerate() {
        let document = reader::read(input.source);
        let mut lowered = ast::lower_document(&document);
        install_module_identity(&mut lowered.module, input.options, &mut lowered.diagnostics);
        diagnostics.extend(
            lowered
                .diagnostics
                .drain(..)
                .map(|diagnostic| LocatedDiagnostic {
                    input_index,
                    diagnostic,
                }),
        );
        let module_name = lowered
            .module
            .name
            .as_ref()
            .expect("implicit workspace module name was installed")
            .canonical
            .clone();
        prepared.push(PreparedInput {
            input_index,
            document,
            header: lowered.module,
            module_name,
        });
    }
    if !diagnostics.is_empty() {
        sort_located_diagnostics(&mut diagnostics);
        return WorkspaceCompileResult {
            units: Vec::new(),
            diagnostics,
        };
    }

    let graph = match ModuleGraph::build_with_interfaces(
        prepared.iter().map(|unit| unit.header.clone()),
        external_interfaces.clone(),
    ) {
        Ok(graph) => graph,
        Err(error) => {
            return WorkspaceCompileResult {
                units: Vec::new(),
                diagnostics: vec![locate_graph_error(&error, &prepared)],
            };
        }
    };

    let source_names = prepared
        .iter()
        .map(|unit| unit.module_name.clone())
        .collect::<BTreeSet<_>>();
    let by_name = prepared
        .iter()
        .enumerate()
        .map(|(position, unit)| (unit.module_name.clone(), position))
        .collect::<BTreeMap<_, _>>();

    // Runtime cycles are legal.  Condense them into deterministic SCCs and
    // schedule the resulting DAG dependency-first.  Interfaces for every
    // member of one SCC are provisioned before any member is lowered, so
    // source order cannot leak into type resolution or call summaries.
    let runtime_components = graph
        .runtime()
        .scc_dependency_order()
        .into_iter()
        .filter_map(|component| {
            let modules = component
                .modules
                .into_iter()
                .filter(|module| source_names.contains(module))
                .collect::<Vec<_>>();
            (!modules.is_empty()).then_some(modules)
        })
        .collect::<Vec<_>>();
    let component_by_module = runtime_components
        .iter()
        .enumerate()
        .flat_map(|(component, modules)| {
            modules
                .iter()
                .cloned()
                .map(move |module| (module, component))
        })
        .collect::<BTreeMap<_, _>>();
    let mut component_dependencies = (0..runtime_components.len())
        .map(|_| BTreeSet::new())
        .collect::<Vec<BTreeSet<usize>>>();
    for edge in graph.runtime_edges().iter().chain(graph.phase1_edges()) {
        let (Some(&from), Some(&to)) = (
            component_by_module.get(&edge.from),
            component_by_module.get(&edge.to),
        ) else {
            continue;
        };
        if from != to {
            component_dependencies[from].insert(to);
        }
    }

    let mut available_interfaces = external_interfaces.clone();
    let mut completed_components = BTreeSet::new();
    let mut analyses = (0..inputs.len()).map(|_| None).collect::<Vec<_>>();
    let mut interface_models = BTreeMap::<String, interface::Interface>::new();

    while completed_components.len() < runtime_components.len() {
        let ready = (0..runtime_components.len()).find(|component| {
            !completed_components.contains(component)
                && component_dependencies[*component].is_subset(&completed_components)
        });
        // A phase-1 graph is checked for cycles before this loop.  If no
        // condensed runtime component is ready, the only remaining shape is
        // therefore a cross-component cycle that mixes runtime and phase-1
        // edges.  Runtime provisional interfaces break that cycle as well;
        // compile the remaining components as one deterministic batch while
        // retaining the phase-1 dependency order below.
        let batch_components = ready.map_or_else(
            || {
                (0..runtime_components.len())
                    .filter(|component| !completed_components.contains(component))
                    .collect::<Vec<_>>()
            },
            |component| vec![component],
        );
        let mut modules = batch_components
            .iter()
            .flat_map(|component| runtime_components[*component].iter().cloned())
            .collect::<Vec<_>>();
        modules.sort();
        let mut provisional = BTreeMap::<String, interface::Interface>::new();
        for module_name in &modules {
            let unit = &prepared[*by_name
                .get(module_name)
                .expect("workspace source module has an input")];
            let model = match interface::build_provisional(&unit.header) {
                Ok(model) => model,
                Err(error) => {
                    return WorkspaceCompileResult {
                        units: Vec::new(),
                        diagnostics: vec![LocatedDiagnostic {
                            input_index: unit.input_index,
                            diagnostic: Diagnostic::error(
                                error.code,
                                error.message,
                                unit.header.span,
                            ),
                        }],
                    };
                }
            };
            provisional.insert(module_name.clone(), model);
        }

        // Keep all provisional members visible for the complete SCC.  Final
        // interfaces are staged separately and become visible only after all
        // members have been analyzed.  The first pass also supplies phase-1
        // macro IR so declaration macros can contribute the public runtime
        // shape; the second pass rebuilds provisional interfaces from those
        // expanded surfaces before typed HIR lowering begins.
        let mut raw_interfaces = available_interfaces.clone();
        raw_interfaces.extend(
            provisional
                .iter()
                .map(|(name, model)| (name.clone(), model.clone())),
        );
        let mut expanded_provisional = BTreeMap::new();
        for module_name in &modules {
            let unit = &prepared[*by_name
                .get(module_name)
                .expect("workspace source module has an input")];
            let imported_phase = imported_phase_modules(&unit.header, &raw_interfaces);
            let expanded = macro_expand::expand_with_imported_phase_modules_for_module(
                &unit.document,
                &imported_phase,
                &unit.module_name,
                ExpansionOptions::default(),
            );
            let mut lowered = ast::lower_document(&expanded.document);
            install_module_identity(
                &mut lowered.module,
                inputs[unit.input_index].options,
                &mut lowered.diagnostics,
            );
            if let Ok(model) = interface::build_provisional(&lowered.module) {
                expanded_provisional.insert(module_name.clone(), model);
            }
        }
        if expanded_provisional.len() == modules.len() {
            provisional = expanded_provisional;
        }
        let mut scc_interfaces = available_interfaces.clone();
        scc_interfaces.extend(
            provisional
                .iter()
                .map(|(name, model)| (name.clone(), model.clone())),
        );

        // Phase-1 imports are acyclic and can impose an order inside a runtime
        // SCC.  Runtime imports continue to resolve against the complete
        // provisional map above.
        let phase_order = graph
            .phase1()
            .dependency_order()
            .unwrap_or_default()
            .into_iter()
            .filter(|module| modules.binary_search(module).is_ok())
            .collect::<Vec<_>>();
        let mut member_order = phase_order;
        let missing_members = modules
            .iter()
            .filter(|module| !member_order.contains(module))
            .cloned()
            .collect::<Vec<_>>();
        member_order.extend(missing_members);

        let mut staged = Vec::<(String, Analysis, interface::Interface)>::new();
        for module_name in member_order {
            let unit = &prepared[*by_name
                .get(&module_name)
                .expect("workspace source module has an input")];
            let imported_phase = imported_phase_modules(&unit.header, &scc_interfaces);
            let mut analysis = analyze_document(
                &unit.document,
                inputs[unit.input_index].options,
                &imported_phase,
                Some(&scc_interfaces),
            );
            let Some(interface_model) = build_interface_model(&mut analysis) else {
                let mut diagnostics = analysis
                    .diagnostics
                    .iter()
                    .cloned()
                    .map(|diagnostic| LocatedDiagnostic {
                        input_index: unit.input_index,
                        diagnostic,
                    })
                    .collect::<Vec<_>>();
                sort_located_diagnostics(&mut diagnostics);
                return WorkspaceCompileResult {
                    units: Vec::new(),
                    diagnostics,
                };
            };
            if analysis.has_errors() {
                let mut diagnostics = analysis
                    .diagnostics
                    .iter()
                    .cloned()
                    .map(|diagnostic| LocatedDiagnostic {
                        input_index: unit.input_index,
                        diagnostic,
                    })
                    .collect::<Vec<_>>();
                sort_located_diagnostics(&mut diagnostics);
                return WorkspaceCompileResult {
                    units: Vec::new(),
                    diagnostics,
                };
            }
            let Some(provisional_model) = provisional.get(&module_name) else {
                unreachable!("every SCC member has a provisional interface")
            };
            if let Err(error) =
                interface::validate_provisional_shape(provisional_model, &interface_model)
            {
                return WorkspaceCompileResult {
                    units: Vec::new(),
                    diagnostics: vec![LocatedDiagnostic {
                        input_index: unit.input_index,
                        diagnostic: Diagnostic::error(error.code, error.message, unit.header.span),
                    }],
                };
            }
            staged.push((module_name, analysis, interface_model));
        }

        for (module_name, analysis, model) in staged {
            let input_index = by_name
                .get(&module_name)
                .and_then(|position| prepared.get(*position))
                .map_or(0, |unit| unit.input_index);
            analyses[input_index] = Some(analysis);
            interface_models.insert(module_name.clone(), model.clone());
            available_interfaces.insert(module_name, model);
        }
        completed_components.extend(batch_components);
    }

    let local_bodies = interface_models
        .iter()
        .map(|(module, model)| (module.clone(), InterfaceBodyHashes::from_interface(model)))
        .collect::<BTreeMap<_, _>>();
    let external_hashes = external_interfaces
        .iter()
        .map(|(module, model)| {
            (
                module.clone(),
                PublishedInterfaceHashes {
                    semantic_interface: model.semantic_interface_hash().to_owned(),
                    tooling_metadata: model.tooling_metadata_hash().to_owned(),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let graph_edges = graph
        .runtime_edges()
        .iter()
        .chain(graph.phase1_edges())
        .map(InterfaceHashEdge::from)
        .collect::<Vec<_>>();
    let graph_hashes = match interface_graph::calculate_interface_graph_hashes(
        &local_bodies,
        graph_edges,
        &external_hashes,
    ) {
        Ok(hashes) => hashes,
        Err(error) => {
            return WorkspaceCompileResult {
                units: Vec::new(),
                diagnostics: vec![locate_interface_graph_error(&error, &prepared)],
            };
        }
    };

    for group in &graph_hashes.groups {
        for member in &group.members {
            let Some(model) = interface_models.get_mut(&member.module) else {
                return WorkspaceCompileResult {
                    units: Vec::new(),
                    diagnostics: vec![LocatedDiagnostic {
                        input_index: 0,
                        diagnostic: Diagnostic::error(
                            "OSR-G0012",
                            format!(
                                "interface hash group `{}` references unknown local module `{}`",
                                group.id, member.module
                            ),
                            crate::source::Span::empty(0),
                        ),
                    }],
                };
            };
            if let Err(error) = interface::install_hash_group(model, group.clone()) {
                let unit = prepared
                    .iter()
                    .find(|unit| unit.module_name == member.module)
                    .or_else(|| prepared.first());
                return WorkspaceCompileResult {
                    units: Vec::new(),
                    diagnostics: vec![LocatedDiagnostic {
                        input_index: unit.map_or(0, |unit| unit.input_index),
                        diagnostic: Diagnostic::error(
                            error.code,
                            error.message,
                            unit.map_or_else(
                                || crate::source::Span::empty(0),
                                |unit| unit.header.span,
                            ),
                        ),
                    }],
                };
            }
        }
    }

    let mut compiled = (0..inputs.len()).map(|_| None).collect::<Vec<_>>();
    for unit in &prepared {
        let analysis = analyses[unit.input_index]
            .take()
            .expect("every workspace module has an analysis");
        let model = interface_models
            .remove(&unit.module_name)
            .expect("every workspace module has an interface model");
        let (result, _) =
            finish_compile_with_model(analysis, inputs[unit.input_index].options, Some(model));
        if result.has_errors() {
            let mut diagnostics = result
                .analysis
                .diagnostics
                .iter()
                .cloned()
                .map(|diagnostic| LocatedDiagnostic {
                    input_index: unit.input_index,
                    diagnostic,
                })
                .collect::<Vec<_>>();
            sort_located_diagnostics(&mut diagnostics);
            return WorkspaceCompileResult {
                units: Vec::new(),
                diagnostics,
            };
        }
        compiled[unit.input_index] = Some(result);
    }

    WorkspaceCompileResult {
        units: compiled
            .into_iter()
            .map(|unit| unit.expect("every workspace module was compiled"))
            .collect(),
        diagnostics: Vec::new(),
    }
}

/// Analyzes every workspace source while preserving a semantic model for
/// inputs that contain errors.
///
/// This entry point is intended for editor tooling. It uses provisional local
/// interfaces so healthy modules retain workspace imports when another module
/// is incomplete. Those interfaces are never rendered, hashed, or used as
/// trusted build artifacts; [`compile_workspace`] remains the fail-closed API
/// for builds.
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
