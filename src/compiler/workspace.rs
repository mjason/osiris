use super::*;
use rayon::prelude::*;

mod recover;

pub use recover::analyze_workspace_recovering;

pub(super) struct PreparedInput {
    pub(super) input_index: usize,
    pub(super) document: Document,
    pub(super) header: ast::Module,
    pub(super) module_name: String,
}

/// Per-module analyses reused across workspace runs.
///
/// An interactive caller re-analyzes one workspace on every keystroke while
/// almost every module is byte-identical to the previous run. A module's
/// analysis is a pure function of its own source, its compile options, and the
/// interfaces visible to it, so an entry stays valid until one of those
/// changes. Batch callers pass no memo and always analyze from scratch.
///
/// Entries are keyed by content, never invalidated: a changed input produces a
/// different key. Unused entries are dropped after two runs so an editing
/// session does not accumulate every revision it has seen.
#[derive(Debug, Default)]
pub struct WorkspaceMemo {
    entries: std::sync::Mutex<BTreeMap<String, MemoEntry>>,
    generation: std::sync::atomic::AtomicU64,
    hits: std::sync::atomic::AtomicUsize,
    misses: std::sync::atomic::AtomicUsize,
}

/// Module analyses reused and recomputed by the most recent run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkspaceMemoStats {
    pub reused: usize,
    pub analyzed: usize,
}

#[derive(Clone, Debug)]
struct MemoEntry {
    used: u64,
    value: std::sync::Arc<(Analysis, interface::Interface)>,
}

/// Runs kept before an unused entry is dropped.
const MEMO_RETAINED_GENERATIONS: u64 = 2;

impl WorkspaceMemo {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of retained module analyses. Intended for tests and diagnostics.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.lock().map_or(0, |entries| entries.len())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Modules reused and recomputed by the most recent run.
    #[must_use]
    pub fn stats(&self) -> WorkspaceMemoStats {
        WorkspaceMemoStats {
            reused: self.hits.load(std::sync::atomic::Ordering::Relaxed),
            analyzed: self.misses.load(std::sync::atomic::Ordering::Relaxed),
        }
    }

    fn begin(&self) -> u64 {
        self.hits.store(0, std::sync::atomic::Ordering::Relaxed);
        self.misses.store(0, std::sync::atomic::Ordering::Relaxed);
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1
    }

    fn get(
        &self,
        key: &str,
        generation: u64,
    ) -> Option<std::sync::Arc<(Analysis, interface::Interface)>> {
        let reused = {
            let Ok(mut entries) = self.entries.lock() else {
                return None;
            };
            entries.get_mut(key).map(|entry| {
                entry.used = generation;
                std::sync::Arc::clone(&entry.value)
            })
        };
        let counter = if reused.is_some() {
            &self.hits
        } else {
            &self.misses
        };
        counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        reused
    }

    fn insert(&self, key: String, generation: u64, value: (Analysis, interface::Interface)) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(
                key,
                MemoEntry {
                    used: generation,
                    value: std::sync::Arc::new(value),
                },
            );
        }
    }

    fn evict(&self, generation: u64) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|_, entry| {
                entry.used + MEMO_RETAINED_GENERATIONS > generation
            });
        }
    }
}

/// Identity of one compilation input: everything about a module that is not
/// carried by the interfaces it sees.
fn module_digest(input: &CompileInput<'_>) -> String {
    let options = input.options;
    hash_fields([
        "osiris-workspace-module-v1",
        interface::COMPILER_ABI,
        interface::LANGUAGE_ABI,
        input.source,
        &options.source_name,
        &options.fallback_module_name,
        options.expected_module_name.as_deref().unwrap_or(""),
        &options.distribution,
        &options.distribution_version,
        &options.target_python.to_string(),
        if options.strict { "strict" } else { "permissive" },
        &options.trust_policy.hash,
    ])
}

/// Modules whose interfaces each local module can resolve, following imports
/// transitively.
///
/// A module can only name what it imports, and a re-export is itself an import
/// edge, so this closure bounds the interfaces its analysis can depend on. All
/// members of one scheduling batch share a provisional-interface map, but
/// keying on that whole map would invalidate every member whenever any one of
/// them changed.
fn import_closures(
    prepared: &[PreparedInput],
    graph: &ModuleGraph,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut imports = BTreeMap::<&str, Vec<&str>>::new();
    for edge in graph.runtime_edges().iter().chain(graph.phase1_edges()) {
        imports
            .entry(edge.from.as_str())
            .or_default()
            .push(edge.to.as_str());
    }
    prepared
        .par_iter()
        .map(|unit| {
            let mut reached = BTreeSet::new();
            let mut frontier = vec![unit.module_name.as_str()];
            while let Some(module) = frontier.pop() {
                for target in imports.get(module).into_iter().flatten() {
                    if reached.insert((*target).to_owned()) {
                        frontier.push(target);
                    }
                }
            }
            (unit.module_name.clone(), reached)
        })
        .collect()
}

/// Identity of the interfaces one module is analyzed against.
///
/// Local interfaces are identified by the source that produces them rather
/// than by the interface model, because a model built during analysis does not
/// yet carry its published hashes. That is coarser than comparing interfaces —
/// editing a function body invalidates the modules that import it even though
/// its interface is unchanged — but it cannot go stale.
///
/// Each entry also records whether it is provisional. A module that joins the
/// analyzed module's cycle switches from a final to a provisional interface
/// without its own source changing, and that must not be reused across.
fn environment_digest(
    module_name: &str,
    closure: &BTreeSet<String>,
    provisional: &BTreeMap<String, interface::Interface>,
    scc_interfaces: &BTreeMap<String, interface::Interface>,
    module_digests: &BTreeMap<String, String>,
    external_interfaces: &BTreeMap<String, interface::Interface>,
) -> String {
    let mut material = String::from("osiris-workspace-environment-v2");
    for name in std::iter::once(module_name)
        .chain(closure.iter().map(String::as_str))
        .collect::<BTreeSet<_>>()
    {
        if !scc_interfaces.contains_key(name) {
            continue;
        }
        material.push('\u{1f}');
        material.push_str(name);
        material.push('=');
        material.push(if provisional.contains_key(name) {
            'P'
        } else {
            'F'
        });
        if let Some(digest) = module_digests.get(name) {
            material.push_str(digest);
        } else if let Some(external) = external_interfaces.get(name) {
            material.push_str(external.semantic_interface_hash());
            material.push('/');
            material.push_str(external.tooling_metadata_hash());
        } else {
            // An interface with neither a local source nor an external model
            // has no stable identity; refuse to reuse anything against it.
            material.push_str("unknown");
        }
    }
    crate::hash::sha256(material.as_bytes())
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
    compile_workspace_with_emission(inputs, external_interfaces, true, None)
}

/// Analyze a workspace without generating Python or serialized artifacts.
#[must_use]
pub fn analyze_workspace(
    inputs: &[CompileInput<'_>],
    external_interfaces: &BTreeMap<String, interface::Interface>,
) -> WorkspaceCompileResult {
    compile_workspace_with_emission(inputs, external_interfaces, false, None)
}

/// Analyze a workspace, reusing per-module analyses from `memo`.
///
/// Equivalent to [`analyze_workspace`] except that unchanged modules are not
/// re-analyzed. Intended for callers that analyze the same workspace
/// repeatedly, such as a language server responding to edits.
#[must_use]
pub fn analyze_workspace_with_memo(
    inputs: &[CompileInput<'_>],
    external_interfaces: &BTreeMap<String, interface::Interface>,
    memo: &WorkspaceMemo,
) -> WorkspaceCompileResult {
    compile_workspace_with_emission(inputs, external_interfaces, false, Some(memo))
}

fn compile_workspace_with_emission(
    inputs: &[CompileInput<'_>],
    external_interfaces: &BTreeMap<String, interface::Interface>,
    emit: bool,
    memo: Option<&WorkspaceMemo>,
) -> WorkspaceCompileResult {
    // The run's generation is established here so that every early return
    // below still leaves the memo bounded.
    let generation = memo.map(WorkspaceMemo::begin);
    let result = compile_workspace_scheduled(inputs, external_interfaces, emit, memo, generation);
    if let (Some(memo), Some(generation)) = (memo, generation) {
        memo.evict(generation);
    }
    result
}

fn compile_workspace_scheduled(
    inputs: &[CompileInput<'_>],
    external_interfaces: &BTreeMap<String, interface::Interface>,
    emit: bool,
    memo: Option<&WorkspaceMemo>,
    memo_generation: Option<u64>,
) -> WorkspaceCompileResult {
    let timing_started = std::time::Instant::now();
    let timings = std::env::var_os("OSIRIS_TIMINGS").is_some();
    if inputs.is_empty() {
        return WorkspaceCompileResult::default();
    }

    let target = inputs[0].options.target_python;
    if let Some((input_index, input)) = inputs
        .iter()
        .enumerate()
        .find(|(_, input)| input.options.target_python != target)
    {
        return WorkspaceCompileResult {
            units: Vec::new(),
            diagnostics: vec![LocatedDiagnostic {
                input_index,
                diagnostic: Diagnostic::error(
                    "OSR-I0018",
                    format!(
                        "workspace Python target `{}` differs from `{target}`",
                        input.options.target_python
                    ),
                    crate::source::Span::empty(0),
                ),
            }],
        };
    }
    if let Some((module, interface)) = external_interfaces
        .iter()
        .find(|(_, interface)| interface.python_target != target)
    {
        return WorkspaceCompileResult {
            units: Vec::new(),
            diagnostics: vec![LocatedDiagnostic {
                input_index: 0,
                diagnostic: Diagnostic::error(
                    "OSR-I0018",
                    format!(
                        "interface `{module}` targets Python {}, expected {target}",
                        interface.python_target
                    ),
                    crate::source::Span::empty(0),
                ),
            }],
        };
    }

    let prepared_results = inputs
        .par_iter()
        .enumerate()
        .map(|(input_index, input)| {
            let document = reader::read(input.source);
            let mut lowered = ast::lower_document(&document);
            install_module_identity(&mut lowered.module, input.options, &mut lowered.diagnostics);
            let diagnostics = lowered
                .diagnostics
                .drain(..)
                .map(|diagnostic| LocatedDiagnostic {
                    input_index,
                    diagnostic,
                })
                .collect::<Vec<_>>();
            let module_name = lowered
                .module
                .name
                .as_ref()
                .expect("implicit workspace module name was installed")
                .canonical
                .clone();
            (
                PreparedInput {
                    input_index,
                    document,
                    header: lowered.module,
                    module_name,
                },
                diagnostics,
            )
        })
        .collect::<Vec<_>>();
    let mut prepared = Vec::with_capacity(inputs.len());
    let mut diagnostics = Vec::new();
    for (unit, mut unit_diagnostics) in prepared_results {
        prepared.push(unit);
        diagnostics.append(&mut unit_diagnostics);
    }
    if timings {
        eprintln!(
            "osr timing: workspace read+lower {:?}",
            timing_started.elapsed()
        );
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
    let analysis_started = std::time::Instant::now();

    // Reuse identity is only computed when a caller supplied a memo; batch
    // compilation pays nothing for it.
    let module_digests = memo
        .map(|_| {
            prepared
                .par_iter()
                .map(|unit| {
                    (
                        unit.module_name.clone(),
                        module_digest(&inputs[unit.input_index]),
                    )
                })
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let import_closures = memo
        .map(|_| import_closures(&prepared, &graph))
        .unwrap_or_default();

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
    let mut provisional_elapsed = std::time::Duration::ZERO;
    let mut expansion_elapsed = std::time::Duration::ZERO;
    let mut frontend_elapsed = std::time::Duration::ZERO;
    let mut interface_elapsed = std::time::Duration::ZERO;
    let mut validation_elapsed = std::time::Duration::ZERO;

    while completed_components.len() < runtime_components.len() {
        let ready = (0..runtime_components.len())
            .filter(|component| {
                !completed_components.contains(component)
                    && component_dependencies[*component].is_subset(&completed_components)
            })
            .collect::<Vec<_>>();
        // A phase-1 graph is checked for cycles before this loop.  If no
        // condensed runtime component is ready, the only remaining shape is
        // therefore a cross-component cycle that mixes runtime and phase-1
        // edges.  Runtime provisional interfaces break that cycle as well;
        // compile the remaining components as one deterministic batch while
        // retaining the phase-1 dependency order below.
        let batch_components = if ready.is_empty() {
            (0..runtime_components.len())
                .filter(|component| !completed_components.contains(component))
                .collect::<Vec<_>>()
        } else {
            ready
        };
        let mut modules = batch_components
            .iter()
            .flat_map(|component| runtime_components[*component].iter().cloned())
            .collect::<Vec<_>>();
        modules.sort();
        let mut provisional = BTreeMap::<String, interface::Interface>::new();
        let provisional_results = modules
            .par_iter()
            .map(|module_name| {
                let unit = &prepared[*by_name
                    .get(module_name)
                    .expect("workspace source module has an input")];
                let started = std::time::Instant::now();
                (
                    module_name.clone(),
                    unit.input_index,
                    unit.header.span,
                    interface::build_provisional(&unit.header),
                    started.elapsed(),
                )
            })
            .collect::<Vec<_>>();
        for (module_name, input_index, span, result, elapsed) in provisional_results {
            provisional_elapsed += elapsed;
            let model = match result {
                Ok(model) => model,
                Err(error) => {
                    return WorkspaceCompileResult {
                        units: Vec::new(),
                        diagnostics: vec![LocatedDiagnostic {
                            input_index,
                            diagnostic: Diagnostic::error(error.code, error.message, span),
                        }],
                    };
                }
            };
            provisional.insert(module_name, model);
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
        let expanded_results = modules
            .par_iter()
            .map(|module_name| {
                let unit = &prepared[*by_name
                    .get(module_name)
                    .expect("workspace source module has an input")];
                let imported_phase = imported_phase_modules(&unit.header, &raw_interfaces);
                let started = std::time::Instant::now();
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
                (
                    module_name.clone(),
                    interface::build_provisional(&lowered.module).ok(),
                    started.elapsed(),
                )
            })
            .collect::<Vec<_>>();
        let mut expanded_provisional = BTreeMap::new();
        for (module_name, model, elapsed) in expanded_results {
            expansion_elapsed += elapsed;
            if let Some(model) = model {
                expanded_provisional.insert(module_name, model);
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

        let analysis_results = member_order
            .par_iter()
            .map(|module_name| {
                let unit = &prepared[*by_name
                    .get(module_name)
                    .expect("workspace source module has an input")];
                let memo_key = memo.and_then(|_| {
                    let environment = environment_digest(
                        module_name,
                        import_closures.get(module_name)?,
                        &provisional,
                        &scc_interfaces,
                        &module_digests,
                        external_interfaces,
                    );
                    Some(hash_fields([
                        "osiris-workspace-analysis-v1",
                        environment.as_str(),
                        module_name.as_str(),
                        module_digests.get(module_name)?.as_str(),
                    ]))
                });
                if let (Some(memo), Some(key), Some(generation)) =
                    (memo, memo_key.as_ref(), memo_generation)
                    && let Some(reused) = memo.get(key, generation)
                {
                    let (analysis, interface_model) = (*reused).clone();
                    return (
                        module_name.clone(),
                        Ok((analysis, interface_model)),
                        std::time::Duration::ZERO,
                        std::time::Duration::ZERO,
                        std::time::Duration::ZERO,
                    );
                }
                let imported_phase = imported_phase_modules(&unit.header, &scc_interfaces);
                let started = std::time::Instant::now();
                let mut analysis = analyze_document(
                    &unit.document,
                    inputs[unit.input_index].options,
                    &imported_phase,
                    Some(&scc_interfaces),
                );
                let frontend_elapsed = started.elapsed();
                let started = std::time::Instant::now();
                let Some(interface_model) = build_interface_model(
                    &mut analysis,
                    inputs[unit.input_index].options.target_python,
                ) else {
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
                    return (
                        module_name.clone(),
                        Err(diagnostics),
                        frontend_elapsed,
                        started.elapsed(),
                        std::time::Duration::ZERO,
                    );
                };
                let interface_elapsed = started.elapsed();
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
                    return (
                        module_name.clone(),
                        Err(diagnostics),
                        frontend_elapsed,
                        interface_elapsed,
                        std::time::Duration::ZERO,
                    );
                }
                let Some(provisional_model) = provisional.get(module_name) else {
                    unreachable!("every SCC member has a provisional interface")
                };
                let started = std::time::Instant::now();
                if let Err(error) =
                    interface::validate_provisional_shape(provisional_model, &interface_model)
                {
                    return (
                        module_name.clone(),
                        Err(vec![LocatedDiagnostic {
                            input_index: unit.input_index,
                            diagnostic: Diagnostic::error(
                                error.code,
                                error.message,
                                unit.header.span,
                            ),
                        }]),
                        frontend_elapsed,
                        interface_elapsed,
                        started.elapsed(),
                    );
                }
                // Retain only fully validated modules. A batch that fails later
                // still leaves its successful members reusable, which is the
                // common case while an edit in progress breaks one module.
                if let (Some(memo), Some(key), Some(generation)) =
                    (memo, memo_key, memo_generation)
                {
                    memo.insert(key, generation, (analysis.clone(), interface_model.clone()));
                }
                (
                    module_name.clone(),
                    Ok((analysis, interface_model)),
                    frontend_elapsed,
                    interface_elapsed,
                    started.elapsed(),
                )
            })
            .collect::<Vec<_>>();

        let mut staged = Vec::<(String, Analysis, interface::Interface)>::new();
        for (module_name, result, frontend, interface, validation) in analysis_results {
            frontend_elapsed += frontend;
            interface_elapsed += interface;
            validation_elapsed += validation;
            match result {
                Ok((analysis, interface_model)) => {
                    staged.push((module_name, analysis, interface_model));
                }
                Err(diagnostics) => {
                    return WorkspaceCompileResult {
                        units: Vec::new(),
                        diagnostics,
                    };
                }
            }
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
    if timings {
        eprintln!(
            "osr timing: workspace analysis {:?} (provisional {:?}, expansion {:?}, frontend {:?}, interface {:?}, validation {:?})",
            analysis_started.elapsed(),
            provisional_elapsed,
            expansion_elapsed,
            frontend_elapsed,
            interface_elapsed,
            validation_elapsed,
        );
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

    let mut jobs = Vec::with_capacity(prepared.len());
    for unit in &prepared {
        let analysis = analyses[unit.input_index]
            .take()
            .expect("every workspace module has an analysis");
        let model = interface_models
            .remove(&unit.module_name)
            .expect("every workspace module has an interface model");
        jobs.push((unit.input_index, analysis, model));
    }
    let results = if emit {
        jobs.into_par_iter()
            .map(|(input_index, analysis, model)| {
                (
                    input_index,
                    finish_compile_with_model(analysis, inputs[input_index].options, Some(model)).0,
                )
            })
            .collect::<Vec<_>>()
    } else {
        jobs.into_iter()
            .map(|(input_index, analysis, _)| {
                (
                    input_index,
                    CompileResult {
                        build_hash: analysis.cache_key.clone(),
                        analysis,
                        python: None,
                        interface: None,
                        source_map: None,
                        records: None,
                    },
                )
            })
            .collect::<Vec<_>>()
    };
    let mut compiled = (0..inputs.len()).map(|_| None).collect::<Vec<_>>();
    for (input_index, result) in results {
        if result.has_errors() {
            let mut diagnostics = result
                .analysis
                .diagnostics
                .iter()
                .cloned()
                .map(|diagnostic| LocatedDiagnostic {
                    input_index,
                    diagnostic,
                })
                .collect::<Vec<_>>();
            sort_located_diagnostics(&mut diagnostics);
            return WorkspaceCompileResult {
                units: Vec::new(),
                diagnostics,
            };
        }
        compiled[input_index] = Some(result);
    }

    WorkspaceCompileResult {
        units: compiled
            .into_iter()
            .map(|unit| unit.expect("every workspace module was compiled"))
            .collect(),
        diagnostics: Vec::new(),
    }
}
