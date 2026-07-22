//! End-to-end compiler orchestration.
//!
//! Individual passes remain usable by tooling, while this module owns pass
//! ordering and the rule that code generation only runs after all semantic
//! gates have succeeded.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use sha2::{Digest, Sha256};

use crate::{
    ast, backend, dependency,
    diagnostic::Diagnostic,
    hir, interface,
    interface_graph::{
        self, InterfaceBodyHashes, InterfaceGraphHashError, InterfaceHashEdge,
        PublishedInterfaceHashes,
    },
    macro_expand::{self, ExpansionOptions, ExpansionTrace, ImportedPhaseModule},
    module_graph::{ModuleGraph, ModuleGraphError},
    name::python_identifier,
    project::PythonVersion,
    reader, records, source_map,
    syntax::{Document, Name},
};

#[derive(Clone, Debug)]
pub struct CompileOptions {
    pub source_name: String,
    pub fallback_module_name: String,
    pub expected_module_name: Option<String>,
    pub distribution: String,
    pub distribution_version: String,
    pub target_python: PythonVersion,
    pub trust_policy: hir::ContractTrustPolicy,
}

impl CompileOptions {
    #[must_use]
    pub fn new(fallback_module_name: impl Into<String>, target_python: PythonVersion) -> Self {
        let fallback_module_name = fallback_module_name.into();
        Self {
            source_name: format!("{fallback_module_name}.osr"),
            distribution: fallback_module_name.clone(),
            distribution_version: "0".to_owned(),
            fallback_module_name,
            expected_module_name: None,
            target_python,
            trust_policy: dependency::contract_trust_policy(&[], &[])
                .expect("empty trust policy is valid"),
        }
    }

    #[must_use]
    pub fn with_source_name(mut self, source_name: impl Into<String>) -> Self {
        self.source_name = source_name.into();
        self
    }

    /// Requires an explicit module declaration to match the source-root path.
    #[must_use]
    pub fn with_expected_module_name(mut self, module_name: impl Into<String>) -> Self {
        self.expected_module_name = Some(module_name.into());
        self
    }

    #[must_use]
    pub fn with_provider(
        mut self,
        distribution: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        self.distribution = distribution.into();
        self.distribution_version = version.into();
        self
    }

    #[must_use]
    pub fn with_trust_policy(mut self, trust_policy: hir::ContractTrustPolicy) -> Self {
        self.trust_policy = trust_policy;
        self
    }
}

#[derive(Clone, Debug)]
pub struct Analysis {
    pub document: Document,
    pub expanded_document: Document,
    pub expansion_traces: Vec<ExpansionTrace>,
    pub surface: ast::Module,
    pub hir: hir::Module,
    pub static_data: records::StaticModuleData,
    pub diagnostics: Vec<Diagnostic>,
    pub source_hash: String,
    pub cache_key: String,
}

impl Analysis {
    #[must_use]
    pub fn has_errors(&self) -> bool {
        !self.diagnostics.is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct CompileResult {
    pub analysis: Analysis,
    pub python: Option<backend::GeneratedPython>,
    pub interface: Option<String>,
    pub source_map: Option<crate::artifact::SourceMap>,
    pub records: Option<records::EncodedSidecar>,
    pub build_hash: String,
}

impl CompileResult {
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.analysis.has_errors()
    }
}

/// One source buffer in a distribution-wide compilation.
#[derive(Clone, Copy, Debug)]
pub struct CompileInput<'source> {
    pub source: &'source str,
    pub options: &'source CompileOptions,
}

impl<'source> CompileInput<'source> {
    #[must_use]
    pub const fn new(source: &'source str, options: &'source CompileOptions) -> Self {
        Self { source, options }
    }
}

/// A diagnostic tied to the input position supplied to [`compile_workspace`].
#[derive(Clone, Debug)]
pub struct LocatedDiagnostic {
    pub input_index: usize,
    pub diagnostic: Diagnostic,
}

/// Result of compiling all source modules as one dependency graph.
#[derive(Clone, Debug, Default)]
pub struct WorkspaceCompileResult {
    /// Successful units are returned in the same order as the inputs.
    pub units: Vec<CompileResult>,
    pub diagnostics: Vec<LocatedDiagnostic>,
}

impl WorkspaceCompileResult {
    #[must_use]
    pub fn has_errors(&self) -> bool {
        !self.diagnostics.is_empty()
    }
}

/// Runs every frontend pass and returns a recoverable semantic model.
#[must_use]
pub fn analyze(source: &str, options: &CompileOptions) -> Analysis {
    let document = reader::read(source);
    analyze_document(&document, options, &[], None)
}

fn analyze_document(
    document: &Document,
    options: &CompileOptions,
    imported_phase_modules: &[ImportedPhaseModule],
    interfaces: Option<&BTreeMap<String, interface::Interface>>,
) -> Analysis {
    let (source_hash, cache_key) = analysis_hashes(document, options);
    let expanded = macro_expand::expand_with_imported_phase_modules_for_module(
        document,
        imported_phase_modules,
        &options.fallback_module_name,
        ExpansionOptions::default(),
    );
    let mut surface_result = ast::lower_document(&expanded.document);
    install_module_identity(
        &mut surface_result.module,
        options,
        &mut surface_result.diagnostics,
    );
    let static_data = interfaces.map_or_else(
        || records::analyze_module(&surface_result.module),
        |interfaces| records::analyze_module_with_interfaces(&surface_result.module, interfaces),
    );
    let hir_result = interfaces.map_or_else(
        || {
            hir::lower_module_with_trust_policy(
                &surface_result.module,
                &options.fallback_module_name,
                &options.trust_policy,
            )
        },
        |interfaces| {
            hir::lower_module_with_interfaces_and_trust_policy(
                &surface_result.module,
                &options.fallback_module_name,
                interfaces,
                &options.trust_policy,
            )
        },
    );

    // Surface lowering carries reader diagnostics forward so standalone AST
    // clients and the end-to-end compiler observe the same recovered errors.
    let mut diagnostics = surface_result.diagnostics;
    diagnostics.extend(static_data.diagnostics.iter().cloned());
    diagnostics.extend(hir_result.diagnostics);
    sort_diagnostics(&mut diagnostics);

    Analysis {
        document: document.clone(),
        expanded_document: expanded.document,
        expansion_traces: expanded.traces,
        surface: surface_result.module,
        hir: hir_result.module,
        static_data,
        diagnostics,
        source_hash,
        cache_key,
    }
}

/// Compiles a source unit to Python after all frontend gates succeed.
#[must_use]
pub fn compile(source: &str, options: &CompileOptions) -> CompileResult {
    let analysis = analyze(source, options);
    finish_compile(analysis, options).0
}

fn finish_compile(
    mut analysis: Analysis,
    options: &CompileOptions,
) -> (CompileResult, Option<interface::Interface>) {
    let interface_model = build_interface_model(&mut analysis);
    finish_compile_with_model(analysis, options, interface_model)
}

fn build_interface_model(analysis: &mut Analysis) -> Option<interface::Interface> {
    if analysis.has_errors() {
        return None;
    }
    match interface::build_with_static_data(&analysis.hir, &analysis.surface, &analysis.static_data)
    {
        Ok(interface) => Some(interface),
        Err(error) => {
            analysis.diagnostics.push(Diagnostic::error(
                error.code,
                error.message,
                analysis.hir.span,
            ));
            None
        }
    }
}

fn finish_compile_with_model(
    mut analysis: Analysis,
    options: &CompileOptions,
    interface_model: Option<interface::Interface>,
) -> (CompileResult, Option<interface::Interface>) {
    let build_hash = build_hash(&analysis, options, interface_model.as_ref());
    let interface = interface_model
        .as_ref()
        .and_then(|model| match interface::render(model) {
            Ok(interface) => Some(interface),
            Err(error) => {
                analysis.diagnostics.push(Diagnostic::error(
                    error.code,
                    error.message,
                    analysis.hir.span,
                ));
                None
            }
        });
    let records = interface_model.as_ref().and_then(|model| {
        let indexed = model
            .owned_records
            .iter()
            .map(|record| records::IndexedRecord {
                occurrence: record.occurrence(
                    &options.distribution,
                    &options.distribution_version,
                    &analysis.hir.name,
                    model.semantic_interface_hash(),
                ),
                record: record.clone(),
                dependency_path: vec![analysis.hir.name.clone()],
            })
            .collect::<Vec<_>>();
        if let Err(diagnostics) = records::merge_unique_indexes(indexed.clone()) {
            analysis.diagnostics.extend(diagnostics);
            return None;
        }
        match records::encode_sidecar([model.semantic_interface_hash().to_owned()], indexed) {
            Ok(sidecar) => Some(sidecar),
            Err(error) => {
                analysis.diagnostics.push(Diagnostic::error(
                    error.code,
                    error.message,
                    error.span.unwrap_or(analysis.hir.span),
                ));
                None
            }
        }
    });
    sort_diagnostics(&mut analysis.diagnostics);
    let (python, generated_map) =
        if analysis.has_errors() || interface.is_none() || records.is_none() {
            (None, None)
        } else {
            match backend::compile_module(&analysis.hir, options.target_python) {
                Ok(generated) => {
                    let generated_name = python_module_path(&analysis.hir.name)
                        .to_string_lossy()
                        .into_owned();
                    let map = source_map::generate(
                        options.source_name.clone(),
                        generated_name,
                        &generated.source,
                        &analysis.hir,
                        &analysis.expansion_traces,
                        &build_hash,
                    );
                    (Some(generated), Some(map))
                }
                Err(error) => {
                    let span = error.span.unwrap_or(analysis.hir.span);
                    analysis
                        .diagnostics
                        .push(Diagnostic::error("OSR-B0001", error.message, span));
                    sort_diagnostics(&mut analysis.diagnostics);
                    (None, None)
                }
            }
        };

    (
        CompileResult {
            analysis,
            python,
            interface,
            source_map: generated_map,
            records,
            build_hash,
        },
        interface_model,
    )
}

struct PreparedInput {
    input_index: usize,
    document: Document,
    header: ast::Module,
    module_name: String,
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

fn locate_interface_graph_error(
    error: &InterfaceGraphHashError,
    prepared: &[PreparedInput],
) -> LocatedDiagnostic {
    let module = match error {
        InterfaceGraphHashError::DuplicateProvider(module)
        | InterfaceGraphHashError::UnknownImporter(module)
        | InterfaceGraphHashError::InvalidHash { owner: module, .. }
        | InterfaceGraphHashError::ComponentCycle(module) => Some(module.as_str()),
        InterfaceGraphHashError::MissingDependency { from, .. } => Some(from.as_str()),
        InterfaceGraphHashError::EmptyModule | InterfaceGraphHashError::InvalidGroup(_) => None,
    };
    let unit = module
        .and_then(|module| prepared.iter().find(|unit| unit.module_name == module))
        .or_else(|| prepared.first());
    LocatedDiagnostic {
        input_index: unit.map_or(0, |unit| unit.input_index),
        diagnostic: Diagnostic::error(
            "OSR-G0012",
            error.to_string(),
            unit.map_or_else(|| crate::source::Span::empty(0), |unit| unit.header.span),
        ),
    }
}

fn analysis_hashes(document: &Document, options: &CompileOptions) -> (String, String) {
    let source = document
        .tokens
        .iter()
        .map(|token| token.text.as_str())
        .collect::<String>();
    let source_hash = hash_fields([source.as_str()]);
    let target_python = options.target_python.to_string();
    let cache_key = hash_fields([
        "osiris-analysis-cache-v1",
        interface::COMPILER_ABI,
        interface::LANGUAGE_ABI,
        &source_hash,
        &options.fallback_module_name,
        &target_python,
        &options.trust_policy.hash,
    ]);
    (source_hash, cache_key)
}

fn build_hash(
    analysis: &Analysis,
    options: &CompileOptions,
    interface: Option<&interface::Interface>,
) -> String {
    let target_python = options.target_python.to_string();
    hash_fields([
        "osiris-build-v1",
        interface::COMPILER_ABI,
        interface::LANGUAGE_ABI,
        &analysis.cache_key,
        &analysis.source_hash,
        &analysis.hir.trust_policy_hash,
        &target_python,
        interface.map_or("none", interface::Interface::semantic_interface_hash),
    ])
}

fn hash_fields<'a>(fields: impl IntoIterator<Item = &'a str>) -> String {
    let mut hasher = Sha256::new();
    for field in fields {
        hasher.update(field.len().to_be_bytes());
        hasher.update(field.as_bytes());
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn install_module_identity(
    module: &mut ast::Module,
    options: &CompileOptions,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(name) = &module.name else {
        module.name = Some(Name {
            spelling: options.fallback_module_name.clone(),
            canonical: options.fallback_module_name.clone(),
        });
        return;
    };
    let Some(expected) = &options.expected_module_name else {
        return;
    };
    if name.canonical != *expected {
        diagnostics.push(Diagnostic::error(
            "OSR-G0011",
            format!(
                "module declaration `{}` does not match source path module `{expected}`",
                name.spelling
            ),
            module.span,
        ));
    }
}

fn imported_phase_modules(
    module: &ast::Module,
    interfaces: &BTreeMap<String, interface::Interface>,
) -> Vec<ImportedPhaseModule> {
    let mut modules = BTreeMap::<String, ImportedPhaseModule>::new();
    for item in &module.items {
        let import = match &item.kind {
            ast::ItemKind::Import(import) | ast::ItemKind::ImportForSyntax(import) => import,
            _ => continue,
        };
        let Some(interface) = interfaces.get(&import.module.canonical) else {
            continue;
        };
        if interface.macros.is_empty() {
            continue;
        }
        let descriptor = modules.entry(interface.module.clone()).or_insert_with(|| {
            ImportedPhaseModule::new(
                interface.module.clone(),
                interface.imported_phase_forms(),
                BTreeMap::new(),
            )
            .with_definition_names(
                interface
                    .bindings
                    .iter()
                    .map(|binding| (binding.canonical.clone(), binding.canonical.clone()))
                    .chain(
                        interface
                            .macros
                            .iter()
                            .map(|macro_| (macro_.canonical.clone(), macro_.canonical.clone())),
                    )
                    .collect(),
            )
        });
        let qualifier = import
            .alias
            .as_ref()
            .map_or(import.module.canonical.as_str(), |alias| {
                alias.canonical.as_str()
            });
        for imported_macro in &interface.macros {
            descriptor.macro_names.insert(
                format!("{qualifier}/{}", imported_macro.canonical),
                imported_macro.canonical.clone(),
            );
            descriptor.macro_names.insert(
                format!("{qualifier}.{}", imported_macro.canonical),
                imported_macro.canonical.clone(),
            );
        }
        for member in &import.members {
            if let Some(imported_macro) = interface
                .macros
                .iter()
                .find(|imported_macro| imported_macro.canonical == member.canonical)
            {
                descriptor
                    .macro_names
                    .insert(member.canonical.clone(), imported_macro.canonical.clone());
                descriptor
                    .macro_names
                    .insert(member.spelling.clone(), imported_macro.canonical.clone());
            }
        }
    }
    modules.into_values().collect()
}

fn locate_graph_error(error: &ModuleGraphError, prepared: &[PreparedInput]) -> LocatedDiagnostic {
    let (code, module, explicit_span) = match error {
        ModuleGraphError::UnnamedModule { span } => ("OSR-G0001", None, Some(*span)),
        ModuleGraphError::DuplicateModule { module, second, .. } => {
            ("OSR-G0002", Some(module.as_str()), *second)
        }
        ModuleGraphError::MissingModule { from, span, .. } => {
            ("OSR-G0003", Some(from.as_str()), Some(*span))
        }
        ModuleGraphError::InterfaceIo { .. } => ("OSR-G0004", None, None),
        ModuleGraphError::InterfaceParse { .. } => ("OSR-G0005", None, None),
        ModuleGraphError::InterfaceModuleMismatch { requested, .. } => {
            ("OSR-G0006", Some(requested.as_str()), None)
        }
        ModuleGraphError::DuplicateInterface { module, .. } => {
            ("OSR-G0007", Some(module.as_str()), None)
        }
        ModuleGraphError::Phase1Cycle { modules } => {
            ("OSR-G0008", modules.first().map(String::as_str), None)
        }
    };
    let unit = module
        .and_then(|module| {
            prepared
                .iter()
                .rev()
                .find(|unit| unit.module_name == module)
        })
        .or_else(|| {
            explicit_span.and_then(|span| {
                prepared.iter().find(|unit| {
                    unit.header.span.start <= span.start && span.end <= unit.header.span.end
                })
            })
        })
        .or_else(|| prepared.first());
    LocatedDiagnostic {
        input_index: unit.map_or(0, |unit| unit.input_index),
        diagnostic: Diagnostic::error(
            code,
            error.to_string(),
            explicit_span
                .or_else(|| unit.map(|unit| unit.header.span))
                .unwrap_or_else(|| crate::source::Span::empty(0)),
        ),
    }
}

fn sort_located_diagnostics(diagnostics: &mut [LocatedDiagnostic]) {
    diagnostics.sort_by(|left, right| {
        (
            left.input_index,
            left.diagnostic.span.start,
            left.diagnostic.span.end,
            left.diagnostic.code,
            &left.diagnostic.message,
        )
            .cmp(&(
                right.input_index,
                right.diagnostic.span.start,
                right.diagnostic.span.end,
                right.diagnostic.code,
                &right.diagnostic.message,
            ))
    });
}

#[must_use]
pub fn python_module_path(module_name: &str) -> PathBuf {
    let mut path = PathBuf::new();
    for component in module_name.split(['/', '.']) {
        path.push(python_identifier(component));
    }
    path.set_extension("py");
    path
}

fn sort_diagnostics(diagnostics: &mut [Diagnostic]) {
    diagnostics.sort_by(|left, right| {
        (left.span.start, left.span.end, left.code, &left.message).cmp(&(
            right.span.start,
            right.span.end,
            right.code,
            &right.message,
        ))
    });
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        CompileInput, CompileOptions, analyze, analyze_workspace_recovering, compile,
        compile_workspace,
    };
    use crate::{hir, interface, project::PythonVersion, types::Type};

    fn options() -> CompileOptions {
        CompileOptions::new("example", PythonVersion::MINIMUM)
    }

    #[test]
    fn trust_policy_hash_partitions_analysis_and_build_artifacts() {
        let source = "(defn value [] -> Int 1)";
        let first_policy =
            hir::ContractTrustPolicy::untrusted(format!("sha256:{}", "1".repeat(64)));
        let second_policy =
            hir::ContractTrustPolicy::untrusted(format!("sha256:{}", "2".repeat(64)));
        let first = compile(
            source,
            &CompileOptions::new("example", PythonVersion::MINIMUM)
                .with_trust_policy(first_policy.clone()),
        );
        let second = compile(
            source,
            &CompileOptions::new("example", PythonVersion::MINIMUM)
                .with_trust_policy(second_policy.clone()),
        );
        assert!(!first.has_errors(), "{:?}", first.analysis.diagnostics);
        assert!(!second.has_errors(), "{:?}", second.analysis.diagnostics);
        assert_ne!(first.analysis.cache_key, second.analysis.cache_key);
        assert_ne!(first.build_hash, second.build_hash);
        assert_eq!(first.interface, second.interface);
        assert_eq!(
            first
                .source_map
                .as_ref()
                .expect("source map")
                .trust_policy_hash,
            first_policy.hash
        );
        assert_eq!(
            second.source_map.as_ref().expect("source map").build_hash,
            second.build_hash
        );
    }

    #[test]
    fn analysis_combines_frontend_diagnostics_in_source_order() {
        let result = analyze("(def first missing)\n(def second [1 2)\n", &options());

        assert!(result.has_errors());
        assert!(
            result
                .diagnostics
                .windows(2)
                .all(|pair| pair[0].span.start <= pair[1].span.start)
        );
    }

    #[test]
    fn frontend_errors_prevent_python_generation() {
        let result = compile("(def value missing)\n", &options());

        assert!(result.has_errors());
        assert!(result.python.is_none());
    }

    #[test]
    fn explicit_module_must_match_the_project_source_identity() {
        let options = CompileOptions::new("nested.expected", PythonVersion::MINIMUM)
            .with_expected_module_name("nested.expected");

        let result = compile("(module nested.other)\n(def value 1)\n", &options);

        assert!(result.has_errors());
        assert!(result.python.is_none());
        assert!(
            result
                .analysis
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "OSR-G0011"
                    && diagnostic.message.contains("nested.expected"))
        );
    }

    #[test]
    fn invalid_static_records_fail_before_codegen() {
        let source = r#"
            (module example)
            (export [owner S])
            (defstatic-schema S
              :schema-id "example/schema"
              :version 1
              :fields {:id {:type Str :required true}})
            (def owner none)
            (static-record S owner {:id 42})
        "#;
        let result = compile(source, &options());

        assert!(result.has_errors());
        assert!(result.python.is_none());
        assert!(
            result
                .analysis
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code.starts_with("OSR-S"))
        );
    }

    #[test]
    fn public_static_records_are_bound_to_the_interface_provider() {
        let source = r#"
            (module example)
            (export [owner S])
            (defstatic-schema S
              :schema-id "example/schema"
              :version 1
              :fields {:id {:type Str :required true}})
            (def owner none)
            (static-record S owner {:id "alpha"})
        "#;
        let options = options().with_provider("example-dist", "1.2.3");
        let result = compile(source, &options);

        assert!(!result.has_errors(), "{:?}", result.analysis.diagnostics);
        let sidecar = result.records.expect("records sidecar should be built");
        assert_eq!(sidecar.sidecar.records.len(), 1);
        let occurrence = &sidecar.sidecar.records[0].occurrence;
        assert_eq!(occurrence.distribution, "example-dist");
        assert_eq!(occurrence.version, "1.2.3");
        assert_eq!(occurrence.interface_member_id, "example");
        assert_eq!(
            occurrence.semantic_interface_hash,
            sidecar.sidecar.interface_semantic_hashes[0]
        );
        let rendered = result
            .interface
            .as_ref()
            .expect("interface should be rendered");
        let decoded = interface::read(rendered).expect("rendered interface should parse");
        assert_eq!(
            occurrence.semantic_interface_hash,
            decoded.semantic_interface_hash()
        );
        assert_ne!(
            occurrence.semantic_interface_hash, decoded.hashes.semantic_body,
            "record occurrence must use the published graph hash, not the local body hash"
        );
    }

    #[test]
    fn workspace_compiles_typed_dependencies_before_importers() {
        let app = r#"
            (module app)
            (import dep.core :as dep)
            (export [call])
            (defn call [] -> Int (dep/add 1 :value 2))
        "#;
        let dependency = r#"
            (module dep.core)
            (export [add])
            (defn add [[x Int] [value Int]] -> Int (+ x value))
        "#;
        let app_options = CompileOptions::new("app", PythonVersion::MINIMUM);
        let dependency_options = CompileOptions::new("dep.core", PythonVersion::MINIMUM);
        let inputs = [
            CompileInput::new(app, &app_options),
            CompileInput::new(dependency, &dependency_options),
        ];

        let result = compile_workspace(&inputs, &BTreeMap::new());

        assert!(!result.has_errors(), "{:?}", result.diagnostics);
        assert_eq!(result.units.len(), 2);
        assert_eq!(result.units[0].analysis.hir.name, "app");
        assert_eq!(result.units[1].analysis.hir.name, "dep.core");
        let python = &result.units[0]
            .python
            .as_ref()
            .expect("app should generate Python")
            .source;
        assert!(python.contains("from dep.core import add"), "{python}");
        assert!(python.contains("return add(1, value=2)"), "{python}");

        let app_interface = interface::read(
            result.units[0]
                .interface
                .as_ref()
                .expect("app interface should be rendered"),
        )
        .expect("app interface should parse");
        let dependency_interface = interface::read(
            result.units[1]
                .interface
                .as_ref()
                .expect("dependency interface should be rendered"),
        )
        .expect("dependency interface should parse");
        assert_eq!(
            result.units[0]
                .records
                .as_ref()
                .expect("app records should be rendered")
                .sidecar
                .interface_semantic_hashes,
            vec![app_interface.semantic_interface_hash().to_owned()]
        );
        assert_ne!(
            app_interface.semantic_interface_hash(),
            app_interface.hashes.semantic_body,
            "workspace interfaces must publish an SCC/group hash"
        );
        assert_ne!(
            dependency_interface.semantic_interface_hash(),
            dependency_interface.hashes.semantic_body,
            "even a dependency-only singleton retains a standalone group hash"
        );
        let dependency = app_interface
            .graph
            .external_dependencies
            .iter()
            .find(|dependency| dependency.to == "dep.core")
            .expect("app graph should retain its dependency hash");
        assert_eq!(
            dependency.semantic_interface_hash,
            dependency_interface.semantic_interface_hash()
        );
    }

    #[test]
    fn recovering_workspace_keeps_healthy_imports_when_another_module_is_invalid() {
        let dependency = r#"
            (module dep.core)
            (export [add-one])
            (defn add-one [[x Int]] -> Int (+ x 1))
        "#;
        let app = r#"
            (module app)
            (import dep.core :as dep)
            (def answer (dep/add-one 41))
        "#;
        let broken = r#"
            (module broken)
            (defn invalid [[x Int]] -> Int)
        "#;
        let dependency_options = CompileOptions::new("dep.core", PythonVersion::MINIMUM);
        let app_options = CompileOptions::new("app", PythonVersion::MINIMUM);
        let broken_options = CompileOptions::new("broken", PythonVersion::MINIMUM);
        let inputs = [
            CompileInput::new(dependency, &dependency_options),
            CompileInput::new(app, &app_options),
            CompileInput::new(broken, &broken_options),
        ];

        let strict = compile_workspace(&inputs, &BTreeMap::new());
        assert!(strict.has_errors());
        assert!(strict.units.is_empty());

        let recovered = analyze_workspace_recovering(&inputs, &BTreeMap::new());
        assert_eq!(recovered.len(), inputs.len());
        assert!(
            recovered[1].diagnostics.is_empty(),
            "{:?}",
            recovered[1].diagnostics
        );
        assert_eq!(recovered[1].hir.name, "app");
        let imported = recovered[1]
            .hir
            .bindings
            .iter()
            .find(|binding| binding.name.id.as_str() == "dep.core::function::add-one")
            .expect("dependency function should remain available to the app");
        let Type::Fn(signature) = &imported.ty else {
            panic!("imported dependency binding should retain its function signature");
        };
        assert_eq!(signature.parameters, vec![Type::Int]);
        assert_eq!(*signature.return_type, Type::Int);
        assert!(!recovered[2].diagnostics.is_empty());
        assert_eq!(recovered[2].hir.name, "broken");
    }

    #[test]
    fn workspace_graph_hashes_are_stable_when_input_order_changes() {
        let app = r#"
            (module app)
            (import dep.core :as dep)
            (export [call])
            (defn call [] -> Int (dep/add 1 :value 2))
        "#;
        let dependency = r#"
            (module dep.core)
            (export [add])
            (defn add [[x Int] [value Int]] -> Int (+ x value))
        "#;
        let app_options = CompileOptions::new("app", PythonVersion::MINIMUM);
        let dependency_options = CompileOptions::new("dep.core", PythonVersion::MINIMUM);
        let forward = compile_workspace(
            &[
                CompileInput::new(app, &app_options),
                CompileInput::new(dependency, &dependency_options),
            ],
            &BTreeMap::new(),
        );
        let reverse = compile_workspace(
            &[
                CompileInput::new(dependency, &dependency_options),
                CompileInput::new(app, &app_options),
            ],
            &BTreeMap::new(),
        );
        assert!(!forward.has_errors(), "{:?}", forward.diagnostics);
        assert!(!reverse.has_errors(), "{:?}", reverse.diagnostics);

        let forward_app = interface::read(
            forward.units[0]
                .interface
                .as_ref()
                .expect("forward app interface"),
        )
        .expect("forward app interface should parse");
        let forward_dep = interface::read(
            forward.units[1]
                .interface
                .as_ref()
                .expect("forward dependency interface"),
        )
        .expect("forward dependency interface should parse");
        let reverse_app = interface::read(
            reverse.units[1]
                .interface
                .as_ref()
                .expect("reverse app interface"),
        )
        .expect("reverse app interface should parse");
        let reverse_dep = interface::read(
            reverse.units[0]
                .interface
                .as_ref()
                .expect("reverse dependency interface"),
        )
        .expect("reverse dependency interface should parse");

        assert_eq!(
            forward_app.semantic_interface_hash(),
            reverse_app.semantic_interface_hash()
        );
        assert_eq!(
            forward_app.tooling_metadata_hash(),
            reverse_app.tooling_metadata_hash()
        );
        assert_eq!(
            forward_dep.semantic_interface_hash(),
            reverse_dep.semantic_interface_hash()
        );
        assert_eq!(
            forward_app.graph.external_dependencies,
            reverse_app.graph.external_dependencies
        );
    }

    #[test]
    fn workspace_replays_exported_macro_and_private_helper() {
        let macros = r#"
            (module sample.macros)
            (defn-for-syntax make-add [value]
              (list '+ value 1))
            (defmacro add-one [value]
              (make-add value))
            (export [add-one])
        "#;
        let app = r#"
            (module sample.app)
            (import sample.macros :as macros)
            (export [increment])
            (defn increment [[value Int]] -> Int
              (macros/add-one value))
        "#;
        let app_options = CompileOptions::new("sample.app", PythonVersion::MINIMUM);
        let macro_options = CompileOptions::new("sample.macros", PythonVersion::MINIMUM);
        let inputs = [
            CompileInput::new(app, &app_options),
            CompileInput::new(macros, &macro_options),
        ];

        let result = compile_workspace(&inputs, &BTreeMap::new());

        assert!(!result.has_errors(), "{:?}", result.diagnostics);
        let python = &result.units[0]
            .python
            .as_ref()
            .expect("macro consumer should generate Python")
            .source;
        assert!(python.contains("return value + 1"), "{python}");
        assert!(
            result.units[0]
                .analysis
                .expansion_traces
                .iter()
                .any(|trace| trace.macro_name == "add-one")
        );
    }

    #[test]
    fn workspace_validates_records_against_an_imported_schema() {
        let producer = r#"
            (module sample.producer)
            (import sample.schema :as schema)
            (export [owner])
            (def owner none)
            (static-record schema/Descriptor owner {:id "example.normalize"})
        "#;
        let schema = r#"
            (module sample.schema)
            (export [Descriptor])
            (defstatic-schema Descriptor
              :schema-id "sample/descriptor"
              :version 1
              :fields {:id {:type Str :required true}})
        "#;
        let producer_options = CompileOptions::new("sample.producer", PythonVersion::MINIMUM);
        let schema_options = CompileOptions::new("sample.schema", PythonVersion::MINIMUM);
        let inputs = [
            CompileInput::new(producer, &producer_options),
            CompileInput::new(schema, &schema_options),
        ];

        let result = compile_workspace(&inputs, &BTreeMap::new());

        assert!(!result.has_errors(), "{:?}", result.diagnostics);
        let records = &result.units[0]
            .records
            .as_ref()
            .expect("producer should emit a records projection")
            .sidecar
            .records;
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].record.schema.binding_id,
            "sample.schema::type::Descriptor"
        );
    }

    #[test]
    fn workspace_isolates_same_named_macros_and_private_helpers() {
        let app = r#"
            (module sample.app)
            (import sample.alpha :as alpha)
            (import sample.beta :as beta)
            (defn calculate [[value Int]] -> Int
              (+ (alpha/wrap value) (beta/wrap value)))
        "#;
        let alpha = r#"
            (module sample.alpha)
            (defn-for-syntax helper [value] (list '+ value 1))
            (defmacro wrap [value] (helper value))
            (export [wrap])
        "#;
        let beta = r#"
            (module sample.beta)
            (defn-for-syntax helper [value] (list '* value 2))
            (defmacro wrap [value] (helper value))
            (export [wrap])
        "#;
        let app_options = CompileOptions::new("sample.app", PythonVersion::MINIMUM);
        let alpha_options = CompileOptions::new("sample.alpha", PythonVersion::MINIMUM);
        let beta_options = CompileOptions::new("sample.beta", PythonVersion::MINIMUM);
        let inputs = [
            CompileInput::new(app, &app_options),
            CompileInput::new(alpha, &alpha_options),
            CompileInput::new(beta, &beta_options),
        ];

        let result = compile_workspace(&inputs, &BTreeMap::new());

        assert!(!result.has_errors(), "{:?}", result.diagnostics);
        let python = &result.units[0]
            .python
            .as_ref()
            .expect("macro consumer should generate Python")
            .source;
        assert!(python.contains("value + 1"), "{python}");
        assert!(python.contains("value * 2"), "{python}");
        assert_eq!(
            result.units[0]
                .analysis
                .expansion_traces
                .iter()
                .filter(|trace| trace.macro_name == "wrap")
                .count(),
            2
        );
    }

    #[test]
    fn workspace_compiles_a_two_module_runtime_cycle_with_provisional_interfaces() {
        let left = r#"
            (module cycle.left)
            (import cycle.right :as right)
            (export [left])
            (defn left [[value Int]] -> Int (right/right value))
        "#;
        let right = r#"
            (module cycle.right)
            (import cycle.left :as left)
            (export [right])
            (defn right [[value Int]] -> Int (left/left value))
        "#;
        let left_options = CompileOptions::new("cycle.left", PythonVersion::MINIMUM);
        let right_options = CompileOptions::new("cycle.right", PythonVersion::MINIMUM);
        let result = compile_workspace(
            &[
                CompileInput::new(left, &left_options),
                CompileInput::new(right, &right_options),
            ],
            &BTreeMap::new(),
        );
        assert!(!result.has_errors(), "{:?}", result.diagnostics);
        assert_eq!(result.units.len(), 2);
        assert!(result.units[0].python.is_some());
        assert!(result.units[1].python.is_some());
    }

    #[test]
    fn runtime_scc_provisional_interfaces_preserve_struct_and_operator_shape() {
        let left = r#"
            (module capability.left)
            (import capability.right :as right)
            (defstruct Series [value Float])
            ^{:osiris/operator :multiply}
            (defn multiply-series
              [[series Series] [multiplier Float]]
              -> Series series)
            (export [Series multiply-series])
            (defn dispatch [[series Series] [multiplier Float]]
              -> Series (right/scale series multiplier))
            (export [dispatch])
        "#;
        let right = r#"
            (module capability.right)
            (import capability.left :refer [Series])
            (export [scale])
            (defn scale [[series Series] [multiplier Float]]
              -> Series series)
        "#;
        let left_options = CompileOptions::new("capability.left", PythonVersion::MINIMUM);
        let right_options = CompileOptions::new("capability.right", PythonVersion::MINIMUM);
        let result = compile_workspace(
            &[
                CompileInput::new(left, &left_options),
                CompileInput::new(right, &right_options),
            ],
            &BTreeMap::new(),
        );
        assert!(!result.has_errors(), "{:?}", result.diagnostics);
    }

    #[test]
    fn runtime_scc_rebuilds_provisional_shape_after_declaration_macro_expansion() {
        let provider = r#"
            (module macro.cycle-provider)
            (import macro.cycle-consumer :as consumer)
            (defmacro emit-generated [] '(def generated 1))
            (emit-generated)
            (export [generated run])
            (defn run [[value Int]] -> Int (consumer/identity value))
        "#;
        let consumer = r#"
            (module macro.cycle-consumer)
            (import macro.cycle-provider :as provider)
            (export [identity])
            (defn identity [[value Int]] -> Int value)
        "#;
        let provider_options = CompileOptions::new("macro.cycle-provider", PythonVersion::MINIMUM);
        let consumer_options = CompileOptions::new("macro.cycle-consumer", PythonVersion::MINIMUM);
        let result = compile_workspace(
            &[
                CompileInput::new(provider, &provider_options),
                CompileInput::new(consumer, &consumer_options),
            ],
            &BTreeMap::new(),
        );
        assert!(!result.has_errors(), "{:?}", result.diagnostics);
        let provider_python = result.units[0]
            .python
            .as_ref()
            .expect("provider should generate Python")
            .source
            .as_str();
        assert!(provider_python.contains("generated"), "{provider_python}");
    }

    #[test]
    fn workspace_still_rejects_a_phase1_cycle_before_runtime_scc_lowering() {
        let left = r#"
            (module cycle.phase-left)
            (import-for-syntax cycle.phase-right)
        "#;
        let right = r#"
            (module cycle.phase-right)
            (import-for-syntax cycle.phase-left)
        "#;
        let left_options = CompileOptions::new("cycle.phase-left", PythonVersion::MINIMUM);
        let right_options = CompileOptions::new("cycle.phase-right", PythonVersion::MINIMUM);
        let result = compile_workspace(
            &[
                CompileInput::new(left, &left_options),
                CompileInput::new(right, &right_options),
            ],
            &BTreeMap::new(),
        );
        assert!(result.has_errors());
        assert_eq!(result.diagnostics[0].diagnostic.code, "OSR-G0008");
    }

    #[test]
    fn workspace_breaks_a_mixed_runtime_and_phase1_cycle_with_a_provisional_batch() {
        let runtime_importer = r#"
            (module mixed.runtime)
            (import mixed.syntax :as syntax)
            (export [run])
            (defn run [[value Int]] -> Int (syntax/emit value))
        "#;
        let syntax_importer = r#"
            (module mixed.syntax)
            (import-for-syntax mixed.runtime)
            (export [emit])
            (defn emit [[value Int]] -> Int value)
        "#;
        let runtime_options = CompileOptions::new("mixed.runtime", PythonVersion::MINIMUM);
        let syntax_options = CompileOptions::new("mixed.syntax", PythonVersion::MINIMUM);
        let result = compile_workspace(
            &[
                CompileInput::new(runtime_importer, &runtime_options),
                CompileInput::new(syntax_importer, &syntax_options),
            ],
            &BTreeMap::new(),
        );
        assert!(!result.has_errors(), "{:?}", result.diagnostics);
        assert_eq!(result.units.len(), 2);
    }

    #[test]
    fn runtime_cycle_interface_hashes_are_stable_when_input_order_changes() {
        let left = r#"
            (module stable.left)
            (import stable.right :as right)
            (export [left])
            (defn left [[value Int]] -> Int (right/right value))
        "#;
        let right = r#"
            (module stable.right)
            (import stable.left :as left)
            (export [right])
            (defn right [[value Int]] -> Int (left/left value))
        "#;
        let left_options = CompileOptions::new("stable.left", PythonVersion::MINIMUM);
        let right_options = CompileOptions::new("stable.right", PythonVersion::MINIMUM);
        let forward = compile_workspace(
            &[
                CompileInput::new(left, &left_options),
                CompileInput::new(right, &right_options),
            ],
            &BTreeMap::new(),
        );
        let reverse = compile_workspace(
            &[
                CompileInput::new(right, &right_options),
                CompileInput::new(left, &left_options),
            ],
            &BTreeMap::new(),
        );
        assert!(!forward.has_errors(), "{:?}", forward.diagnostics);
        assert!(!reverse.has_errors(), "{:?}", reverse.diagnostics);
        let forward_left =
            interface::read(forward.units[0].interface.as_ref().expect("left interface"))
                .expect("left interface should parse");
        let reverse_left =
            interface::read(reverse.units[1].interface.as_ref().expect("left interface"))
                .expect("left interface should parse");
        assert_eq!(
            forward_left.semantic_interface_hash(),
            reverse_left.semantic_interface_hash()
        );
        assert_eq!(
            forward_left.tooling_metadata_hash(),
            reverse_left.tooling_metadata_hash()
        );
    }

    #[test]
    fn runtime_sccs_are_scheduled_before_their_cross_scc_importers() {
        let app = r#"
            (module ordered.app)
            (import ordered.left :as left)
            (export [run])
            (defn run [[value Int]] -> Int (left/left value))
        "#;
        let left = r#"
            (module ordered.left)
            (import ordered.right :as right)
            (export [left])
            (defn left [[value Int]] -> Int (right/right value))
        "#;
        let right = r#"
            (module ordered.right)
            (import ordered.left :as left)
            (export [right])
            (defn right [[value Int]] -> Int (left/left value))
        "#;
        let app_options = CompileOptions::new("ordered.app", PythonVersion::MINIMUM);
        let left_options = CompileOptions::new("ordered.left", PythonVersion::MINIMUM);
        let right_options = CompileOptions::new("ordered.right", PythonVersion::MINIMUM);
        let result = compile_workspace(
            &[
                CompileInput::new(app, &app_options),
                CompileInput::new(left, &left_options),
                CompileInput::new(right, &right_options),
            ],
            &BTreeMap::new(),
        );
        assert!(!result.has_errors(), "{:?}", result.diagnostics);
        assert_eq!(result.units.len(), 3);
        assert!(result.units.iter().all(|unit| unit.python.is_some()));
    }
}
