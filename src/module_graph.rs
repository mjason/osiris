//! Deterministic source/module dependency graphs and read-only interface loading.
//!
//! The graph deliberately operates on the lowered surface AST.  Runtime and
//! phase-1 imports are represented as different edge sets, so callers can
//! schedule ordinary modules independently while enforcing the stronger
//! no-cycle rule for macro/phase-1 dependencies.  Interface loading accepts an
//! explicit module-to-`.osri` path map and only parses the data-only interface;
//! it never imports or executes Python code.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs, io,
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::{
    ast::{self, ImportPhase, ItemKind},
    interface::{self, FunctionInterface, Interface, PublicAlias, PublicBinding, StructInterface},
    name::BindingKind,
    source::Span,
};

/// The two compile-time dependency edge classes understood by the graph.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EdgeKind {
    Runtime,
    Phase1,
}

impl EdgeKind {
    #[must_use]
    pub const fn is_phase1(self) -> bool {
        matches!(self, Self::Phase1)
    }
}

impl fmt::Display for EdgeKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Runtime => "runtime",
            Self::Phase1 => "phase-1",
        })
    }
}

impl From<ImportPhase> for EdgeKind {
    fn from(phase: ImportPhase) -> Self {
        match phase {
            ImportPhase::Runtime => Self::Runtime,
            ImportPhase::Syntax => Self::Phase1,
        }
    }
}

/// One source import declaration in a dependency graph.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ModuleEdge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
    pub span: Span,
}

/// A deterministic strongly-connected component projection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StronglyConnectedComponent {
    /// Component ids are assigned by the lexicographically smallest module.
    pub id: usize,
    pub modules: Vec<String>,
    pub cyclic: bool,
}

pub type Scc = StronglyConnectedComponent;

/// A graph over module names.  Edges point from an importer to its dependency.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct DependencyGraph {
    nodes: BTreeSet<String>,
    adjacency: BTreeMap<String, BTreeSet<String>>,
    edges: Vec<ModuleEdge>,
}

impl DependencyGraph {
    fn with_nodes(nodes: impl IntoIterator<Item = String>) -> Self {
        let mut graph = Self::default();
        for node in nodes {
            graph.add_node(node);
        }
        graph
    }

    fn add_node(&mut self, node: String) {
        self.nodes.insert(node.clone());
        self.adjacency.entry(node).or_default();
    }

    fn add_edge(&mut self, edge: ModuleEdge) {
        self.add_node(edge.from.clone());
        self.add_node(edge.to.clone());
        self.adjacency
            .entry(edge.from.clone())
            .or_default()
            .insert(edge.to.clone());
        self.edges.push(edge);
    }

    fn finish(&mut self) {
        self.edges.sort_by(|left, right| {
            (
                &left.from,
                &left.to,
                left.kind,
                left.span.start,
                left.span.end,
                &left.alias,
                &left.members,
            )
                .cmp(&(
                    &right.from,
                    &right.to,
                    right.kind,
                    right.span.start,
                    right.span.end,
                    &right.alias,
                    &right.members,
                ))
        });
    }

    /// Returns all graph nodes in stable lexical order.
    #[must_use]
    pub fn nodes(&self) -> Vec<String> {
        self.nodes.iter().cloned().collect()
    }

    /// Returns all import declarations in stable source-independent order.
    #[must_use]
    pub fn edges(&self) -> &[ModuleEdge] {
        &self.edges
    }

    /// Returns the distinct direct dependencies of `module`.
    #[must_use]
    pub fn successors(&self, module: &str) -> Vec<String> {
        self.adjacency
            .get(module)
            .map(|values| values.iter().cloned().collect())
            .unwrap_or_default()
    }

    #[must_use]
    pub fn contains(&self, module: &str) -> bool {
        self.nodes.contains(module)
    }

    /// Compute SCCs with deterministic ids and member ordering.
    #[must_use]
    pub fn sccs(&self) -> Vec<StronglyConnectedComponent> {
        let mut visited = BTreeSet::new();
        let mut finish_order = Vec::with_capacity(self.nodes.len());
        for node in &self.nodes {
            dfs_finish(node, &self.adjacency, &mut visited, &mut finish_order);
        }

        let mut reverse = BTreeMap::<String, BTreeSet<String>>::new();
        for node in &self.nodes {
            reverse.entry(node.clone()).or_default();
        }
        for (from, targets) in &self.adjacency {
            for target in targets {
                reverse
                    .entry(target.clone())
                    .or_default()
                    .insert(from.clone());
            }
        }

        visited.clear();
        let mut components = Vec::<Vec<String>>::new();
        for node in finish_order.into_iter().rev() {
            if visited.contains(&node) {
                continue;
            }
            let mut component = Vec::new();
            dfs_collect(&node, &reverse, &mut visited, &mut component);
            component.sort();
            components.push(component);
        }
        components.sort_by(|left, right| left.first().cmp(&right.first()));

        components
            .into_iter()
            .enumerate()
            .map(|(id, modules)| {
                let cyclic = modules.len() > 1
                    || modules.first().is_some_and(|module| {
                        self.adjacency
                            .get(module)
                            .is_some_and(|targets| targets.contains(module))
                    });
                StronglyConnectedComponent {
                    id,
                    modules,
                    cyclic,
                }
            })
            .collect()
    }

    /// Topological order with importers before their dependencies.
    ///
    /// This is the conventional direction for the stored edges.  Use
    /// [`Self::dependency_order`] when scheduling compilation (dependencies
    /// first) is more convenient.
    pub fn topological_order(&self) -> Result<Vec<String>, TopologyError> {
        self.topological(false)
    }

    /// Topological order with dependencies before their importers.
    pub fn dependency_order(&self) -> Result<Vec<String>, TopologyError> {
        self.topological(true)
    }

    /// Return strongly-connected components in deterministic dependency-first
    /// order.  Edges in this graph point from an importer to its dependency,
    /// so a component is ready once all of the components it imports have
    /// already been emitted.  Unlike [`Self::dependency_order`], this method
    /// intentionally accepts cycles and schedules each cyclic component as a
    /// single unit.
    #[must_use]
    pub fn scc_dependency_order(&self) -> Vec<StronglyConnectedComponent> {
        let components = self.sccs();
        if components.len() <= 1 {
            return components;
        }

        let component_by_module = components
            .iter()
            .enumerate()
            .flat_map(|(component, value)| {
                value
                    .modules
                    .iter()
                    .cloned()
                    .map(move |module| (module, component))
            })
            .collect::<BTreeMap<_, _>>();
        // `remaining` counts distinct dependencies of each component.  The
        // reverse relation lets us release importers as dependencies finish.
        let mut remaining = vec![0usize; components.len()];
        let mut importers = vec![BTreeSet::<usize>::new(); components.len()];
        for (from, targets) in &self.adjacency {
            let Some(&from_component) = component_by_module.get(from) else {
                continue;
            };
            for target in targets {
                let Some(&target_component) = component_by_module.get(target) else {
                    continue;
                };
                if from_component == target_component {
                    continue;
                }
                if importers[target_component].insert(from_component) {
                    remaining[from_component] += 1;
                }
            }
        }

        // Component ids are assigned by lexical first member, which gives a
        // stable tie-break whenever multiple SCCs become ready together.
        let mut ready = components
            .iter()
            .enumerate()
            .filter(|(component, _)| remaining[*component] == 0)
            .map(|(component, value)| (value.id, component))
            .collect::<BTreeSet<_>>();
        let mut order = Vec::with_capacity(components.len());
        while let Some((_, component)) = ready.pop_first() {
            order.push(components[component].clone());
            // `importers[component]` contains components which depend on the
            // one just emitted.
            for importer in importers[component].iter().copied() {
                remaining[importer] = remaining[importer].saturating_sub(1);
                if remaining[importer] == 0 {
                    ready.insert((components[importer].id, importer));
                }
            }
        }

        // The condensation graph is acyclic by construction.  Keep a total,
        // deterministic result even if a future graph implementation violates
        // that invariant rather than silently dropping components.
        if order.len() != components.len() {
            let seen = order
                .iter()
                .flat_map(|component| component.modules.iter().cloned())
                .collect::<BTreeSet<_>>();
            let mut missing = components
                .into_iter()
                .filter(|component| {
                    component
                        .modules
                        .iter()
                        .any(|module| !seen.contains(module))
                })
                .collect::<Vec<_>>();
            missing.sort_by_key(|component| component.id);
            order.extend(missing);
        }
        order
    }

    fn topological(&self, dependencies_first: bool) -> Result<Vec<String>, TopologyError> {
        let mut remaining = BTreeMap::<String, usize>::new();
        let mut reverse = BTreeMap::<String, BTreeSet<String>>::new();
        for node in &self.nodes {
            remaining.insert(
                node.clone(),
                if dependencies_first {
                    self.adjacency.get(node).map_or(0, BTreeSet::len)
                } else {
                    self.nodes
                        .iter()
                        .filter(|candidate| {
                            self.adjacency
                                .get(*candidate)
                                .is_some_and(|targets| targets.contains(node))
                        })
                        .count()
                },
            );
            reverse.entry(node.clone()).or_default();
        }
        for (from, targets) in &self.adjacency {
            for target in targets {
                reverse
                    .entry(target.clone())
                    .or_default()
                    .insert(from.clone());
            }
        }

        let mut ready = self
            .nodes
            .iter()
            .filter(|node| remaining.get(*node) == Some(&0))
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut order = Vec::with_capacity(self.nodes.len());
        while let Some(node) = ready.pop_first() {
            order.push(node.clone());
            if dependencies_first {
                // Removing a dependency makes its importers eligible.
                for importer in reverse.get(&node).into_iter().flatten() {
                    let count = remaining
                        .get_mut(importer)
                        .expect("reverse graph node must exist");
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        ready.insert(importer.clone());
                    }
                }
            } else {
                // Removing an importer makes each dependency's incoming
                // importer count smaller.
                for dependency in self.successors(&node) {
                    let count = remaining
                        .get_mut(&dependency)
                        .expect("graph node must exist");
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        ready.insert(dependency);
                    }
                }
            }
        }
        if order.len() != self.nodes.len() {
            return Err(TopologyError::Cycle {
                components: self
                    .sccs()
                    .into_iter()
                    .filter(|component| component.cyclic)
                    .collect(),
            });
        }
        Ok(order)
    }
}

fn dfs_finish(
    node: &str,
    adjacency: &BTreeMap<String, BTreeSet<String>>,
    visited: &mut BTreeSet<String>,
    order: &mut Vec<String>,
) {
    if !visited.insert(node.to_owned()) {
        return;
    }
    if let Some(targets) = adjacency.get(node) {
        for target in targets {
            dfs_finish(target, adjacency, visited, order);
        }
    }
    order.push(node.to_owned());
}

fn dfs_collect(
    node: &str,
    reverse: &BTreeMap<String, BTreeSet<String>>,
    visited: &mut BTreeSet<String>,
    component: &mut Vec<String>,
) {
    if !visited.insert(node.to_owned()) {
        return;
    }
    component.push(node.to_owned());
    if let Some(targets) = reverse.get(node) {
        for target in targets {
            dfs_collect(target, reverse, visited, component);
        }
    }
}

/// Errors produced by graph construction and interface loading.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModuleGraphError {
    UnnamedModule {
        span: Span,
    },
    DuplicateModule {
        module: String,
        first: Option<Span>,
        second: Option<Span>,
    },
    MissingModule {
        from: String,
        module: String,
        kind: EdgeKind,
        span: Span,
    },
    InterfaceIo {
        requested: String,
        path: PathBuf,
        message: String,
    },
    InterfaceParse {
        requested: String,
        path: PathBuf,
        message: String,
    },
    InterfaceModuleMismatch {
        requested: String,
        declared: String,
        path: PathBuf,
    },
    DuplicateInterface {
        module: String,
        first: PathBuf,
        second: PathBuf,
    },
    Phase1Cycle {
        modules: Vec<String>,
    },
}

impl fmt::Display for ModuleGraphError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnnamedModule { .. } => formatter.write_str("module declaration requires a name"),
            Self::DuplicateModule { module, .. } => {
                write!(formatter, "duplicate module `{module}`")
            }
            Self::MissingModule {
                from, module, kind, ..
            } => write!(
                formatter,
                "{kind} import from `{from}` references missing module `{module}`"
            ),
            Self::InterfaceIo {
                requested,
                path,
                message,
            } => write!(
                formatter,
                "could not read interface for `{requested}` at {}: {message}",
                path.display()
            ),
            Self::InterfaceParse {
                requested,
                path,
                message,
            } => write!(
                formatter,
                "invalid interface for `{requested}` at {}: {message}",
                path.display()
            ),
            Self::InterfaceModuleMismatch {
                requested,
                declared,
                path,
            } => write!(
                formatter,
                "interface path {} requested as `{requested}` declares `{declared}`",
                path.display()
            ),
            Self::DuplicateInterface {
                module,
                first,
                second,
            } => write!(
                formatter,
                "module `{module}` is provided by both {} and {}",
                first.display(),
                second.display()
            ),
            Self::Phase1Cycle { modules } => {
                write!(formatter, "phase-1 import cycle: {}", modules.join(" -> "))
            }
        }
    }
}

impl std::error::Error for ModuleGraphError {}

/// A cycle error returned by a topological ordering request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TopologyError {
    Cycle {
        components: Vec<StronglyConnectedComponent>,
    },
}

impl fmt::Display for TopologyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cycle { components } => {
                let names = components
                    .iter()
                    .map(|component| component.modules.join(" <-> "))
                    .collect::<Vec<_>>();
                write!(
                    formatter,
                    "dependency graph contains cycle(s): {}",
                    names.join(", ")
                )
            }
        }
    }
}

impl std::error::Error for TopologyError {}

/// Read-only lookup failures for public interface members.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExportLookupError {
    MissingModule {
        module: String,
    },
    MissingExport {
        module: String,
        name: String,
    },
    WrongKind {
        module: String,
        name: String,
        expected: BindingKind,
        actual: BindingKind,
    },
    MissingInterfaceMember {
        module: String,
        binding: String,
    },
}

impl fmt::Display for ExportLookupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingModule { module } => {
                write!(formatter, "module `{module}` has no loaded interface")
            }
            Self::MissingExport { module, name } => {
                write!(formatter, "module `{module}` does not export `{name}`")
            }
            Self::WrongKind {
                module,
                name,
                expected,
                actual,
            } => write!(
                formatter,
                "export `{module}/{name}` has kind {actual:?}, expected {expected:?}"
            ),
            Self::MissingInterfaceMember { module, binding } => {
                write!(
                    formatter,
                    "interface `{module}` has no member description for `{binding}`"
                )
            }
        }
    }
}

impl std::error::Error for ExportLookupError {}

/// A resolved member from an imported interface.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ResolvedMember {
    pub requested: String,
    pub canonical: String,
    pub binding: String,
    pub kind: BindingKind,
}

/// A resolved import declaration.  This owns only stable strings, so it can
/// safely cross an API boundary without borrowing an AST or interface.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ResolvedImport {
    pub from: String,
    pub module: String,
    pub kind: EdgeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<ResolvedMember>,
}

/// A source module graph plus explicitly loaded `.osri` interfaces.
#[derive(Clone, Debug, Default)]
pub struct ModuleGraph {
    modules: BTreeMap<String, ast::Module>,
    interfaces: BTreeMap<String, Interface>,
    runtime: DependencyGraph,
    phase1: DependencyGraph,
}

impl ModuleGraph {
    /// Alias for [`Self::build`] for callers that prefer constructor syntax.
    pub fn new<I>(modules: I) -> Result<Self, ModuleGraphError>
    where
        I: IntoIterator<Item = ast::Module>,
    {
        Self::build(modules)
    }

    /// Start an incremental graph builder.
    #[must_use]
    pub fn builder() -> ModuleGraphBuilder {
        ModuleGraphBuilder::new()
    }

    /// Build a graph from source modules without external interfaces.
    pub fn build<I>(modules: I) -> Result<Self, ModuleGraphError>
    where
        I: IntoIterator<Item = ast::Module>,
    {
        Self::build_with_interface_paths(modules, &BTreeMap::new())
    }

    /// Convenience form for callers holding borrowed AST modules.
    pub fn from_modules(modules: &[ast::Module]) -> Result<Self, ModuleGraphError> {
        Self::build(modules.iter().cloned())
    }

    /// Build a graph and load interfaces from an explicit module/path map.
    pub fn build_with_interface_paths<I>(
        modules: I,
        paths: &BTreeMap<String, PathBuf>,
    ) -> Result<Self, ModuleGraphError>
    where
        I: IntoIterator<Item = ast::Module>,
    {
        let interfaces = read_interface_paths(paths)?;
        Self::build_with_interfaces(modules, interfaces)
    }

    /// Build a graph from already parsed, data-only interfaces.
    pub fn build_with_interfaces<I>(
        modules: I,
        interfaces: BTreeMap<String, Interface>,
    ) -> Result<Self, ModuleGraphError>
    where
        I: IntoIterator<Item = ast::Module>,
    {
        let mut source_records = modules
            .into_iter()
            .map(|module| {
                let name = module
                    .name
                    .as_ref()
                    .map(|name| name.canonical.clone())
                    .ok_or(ModuleGraphError::UnnamedModule { span: module.span })?;
                if name.is_empty() {
                    return Err(ModuleGraphError::UnnamedModule { span: module.span });
                }
                Ok((name, module))
            })
            .collect::<Result<Vec<_>, ModuleGraphError>>()?;
        source_records.sort_by(|left, right| {
            (&left.0, left.1.span.start, left.1.span.end).cmp(&(
                &right.0,
                right.1.span.start,
                right.1.span.end,
            ))
        });

        let mut source_modules: BTreeMap<String, ast::Module> = BTreeMap::new();
        for (name, module) in source_records {
            if let Some(previous) = source_modules.get(&name) {
                return Err(ModuleGraphError::DuplicateModule {
                    module: name,
                    first: Some(previous.span),
                    second: Some(module.span),
                });
            }
            source_modules.insert(name, module);
        }

        let mut declared_interfaces = BTreeMap::<String, String>::new();
        for (requested, interface) in &interfaces {
            if interface.module != *requested {
                return Err(ModuleGraphError::InterfaceModuleMismatch {
                    requested: requested.clone(),
                    declared: interface.module.clone(),
                    path: PathBuf::new(),
                });
            }
            if let Some(previous) =
                declared_interfaces.insert(interface.module.clone(), requested.clone())
            {
                return Err(ModuleGraphError::DuplicateInterface {
                    module: interface.module.clone(),
                    first: PathBuf::from(previous),
                    second: PathBuf::from(requested),
                });
            }
        }

        for name in interfaces.keys() {
            if let Some(source) = source_modules.get(name) {
                return Err(ModuleGraphError::DuplicateModule {
                    module: name.clone(),
                    first: Some(source.span),
                    second: None,
                });
            }
        }

        let mut known = source_modules.keys().cloned().collect::<BTreeSet<_>>();
        known.extend(interfaces.keys().cloned());
        let mut runtime = DependencyGraph::with_nodes(known.iter().cloned());
        let mut phase1 = DependencyGraph::with_nodes(known.iter().cloned());
        let mut imports: Vec<ModuleEdge> = Vec::new();
        for (from, module) in &source_modules {
            for item in &module.items {
                let (import, kind) = match &item.kind {
                    ItemKind::Import(import) => (import, EdgeKind::Runtime),
                    ItemKind::ImportForSyntax(import) => (import, EdgeKind::Phase1),
                    _ => continue,
                };
                let edge = ModuleEdge {
                    from: from.clone(),
                    to: import.module.canonical.clone(),
                    kind,
                    alias: import.alias.as_ref().map(|name| name.canonical.clone()),
                    members: import
                        .members
                        .iter()
                        .map(|name| name.canonical.clone())
                        .collect(),
                    span: import.span,
                };
                imports.push(edge);
            }
        }
        imports.sort_by(|left, right| {
            (
                &left.from,
                &left.to,
                left.kind,
                left.span.start,
                left.span.end,
            )
                .cmp(&(
                    &right.from,
                    &right.to,
                    right.kind,
                    right.span.start,
                    right.span.end,
                ))
        });

        for edge in imports {
            if !known.contains(&edge.to) {
                return Err(ModuleGraphError::MissingModule {
                    from: edge.from,
                    module: edge.to,
                    kind: edge.kind,
                    span: edge.span,
                });
            }
            if edge.kind.is_phase1() {
                phase1.add_edge(edge);
            } else {
                runtime.add_edge(edge);
            }
        }
        runtime.finish();
        phase1.finish();

        if let Some(component) = phase1.sccs().into_iter().find(|component| component.cyclic) {
            return Err(ModuleGraphError::Phase1Cycle {
                modules: component.modules,
            });
        }

        Ok(Self {
            modules: source_modules,
            interfaces,
            runtime,
            phase1,
        })
    }

    /// Read and validate interfaces from a module/path map, adding them to an
    /// existing graph.  This method is useful when source discovery and
    /// interface discovery happen in separate phases.
    pub fn load_interfaces(
        &mut self,
        paths: &BTreeMap<String, PathBuf>,
    ) -> Result<(), ModuleGraphError> {
        let interfaces = read_interface_paths(paths)?;
        for (module, interface) in interfaces {
            if self.modules.contains_key(&module) || self.interfaces.contains_key(&module) {
                return Err(ModuleGraphError::DuplicateModule {
                    module: module.clone(),
                    first: self.modules.get(&module).map(|source| source.span),
                    second: None,
                });
            }
            self.runtime.add_node(module.clone());
            self.phase1.add_node(module.clone());
            self.interfaces.insert(module, interface);
        }
        Ok(())
    }

    #[must_use]
    pub fn source_modules(&self) -> &BTreeMap<String, ast::Module> {
        &self.modules
    }

    #[must_use]
    pub fn interfaces(&self) -> &BTreeMap<String, Interface> {
        &self.interfaces
    }

    #[must_use]
    pub fn interface(&self, module: &str) -> Option<&Interface> {
        self.interfaces.get(module)
    }

    #[must_use]
    pub fn runtime(&self) -> &DependencyGraph {
        &self.runtime
    }

    #[must_use]
    pub fn runtime_edges(&self) -> &[ModuleEdge] {
        self.runtime.edges()
    }

    #[must_use]
    pub fn phase1(&self) -> &DependencyGraph {
        &self.phase1
    }

    #[must_use]
    pub fn phase_one(&self) -> &DependencyGraph {
        self.phase1()
    }

    #[must_use]
    pub fn phase1_edges(&self) -> &[ModuleEdge] {
        self.phase1.edges()
    }

    #[must_use]
    pub fn sccs(&self, kind: EdgeKind) -> Vec<StronglyConnectedComponent> {
        self.graph(kind).sccs()
    }

    pub fn topological_order(&self, kind: EdgeKind) -> Result<Vec<String>, TopologyError> {
        self.graph(kind).topological_order()
    }

    pub fn dependency_order(&self, kind: EdgeKind) -> Result<Vec<String>, TopologyError> {
        self.graph(kind).dependency_order()
    }

    /// Dependency-first order of SCCs for one edge class.  Runtime cycles are
    /// legal and therefore appear as one component; phase-1 cycles are still
    /// rejected during [`ModuleGraph::build_with_interfaces`].
    #[must_use]
    pub fn scc_dependency_order(&self, kind: EdgeKind) -> Vec<StronglyConnectedComponent> {
        self.graph(kind).scc_dependency_order()
    }

    #[must_use]
    pub fn graph(&self, kind: EdgeKind) -> &DependencyGraph {
        if kind.is_phase1() {
            &self.phase1
        } else {
            &self.runtime
        }
    }

    /// All source and interface module names in deterministic order.
    #[must_use]
    pub fn modules(&self) -> Vec<String> {
        self.runtime.nodes()
    }

    pub fn exported_binding(
        &self,
        module: &str,
        name: &str,
    ) -> Result<&PublicBinding, ExportLookupError> {
        let interface = self.loaded_interface(module)?;
        find_binding(interface, name).ok_or_else(|| ExportLookupError::MissingExport {
            module: module.to_owned(),
            name: name.to_owned(),
        })
    }

    pub fn binding(&self, module: &str, name: &str) -> Result<&PublicBinding, ExportLookupError> {
        self.exported_binding(module, name)
    }

    pub fn exported_alias(
        &self,
        module: &str,
        name: &str,
    ) -> Result<&PublicAlias, ExportLookupError> {
        let interface = self.loaded_interface(module)?;
        find_alias(interface, name).ok_or_else(|| ExportLookupError::MissingExport {
            module: module.to_owned(),
            name: name.to_owned(),
        })
    }

    pub fn alias(&self, module: &str, name: &str) -> Result<&PublicAlias, ExportLookupError> {
        self.exported_alias(module, name)
    }

    pub fn exported_function(
        &self,
        module: &str,
        name: &str,
    ) -> Result<&FunctionInterface, ExportLookupError> {
        let binding = self.exported_binding(module, name)?;
        if binding.kind != BindingKind::Function {
            return Err(ExportLookupError::WrongKind {
                module: module.to_owned(),
                name: name.to_owned(),
                expected: BindingKind::Function,
                actual: binding.kind,
            });
        }
        let interface = self.loaded_interface(module)?;
        interface
            .functions
            .iter()
            .find(|function| function.binding == binding.id)
            .ok_or_else(|| ExportLookupError::MissingInterfaceMember {
                module: module.to_owned(),
                binding: binding.id.clone(),
            })
    }

    pub fn function(
        &self,
        module: &str,
        name: &str,
    ) -> Result<&FunctionInterface, ExportLookupError> {
        self.exported_function(module, name)
    }

    /// Return a public type binding.  Struct details, when present, are
    /// available through [`Self::exported_struct`].
    pub fn exported_type(
        &self,
        module: &str,
        name: &str,
    ) -> Result<&PublicBinding, ExportLookupError> {
        let binding = self.exported_binding(module, name)?;
        if binding.kind != BindingKind::Type {
            return Err(ExportLookupError::WrongKind {
                module: module.to_owned(),
                name: name.to_owned(),
                expected: BindingKind::Type,
                actual: binding.kind,
            });
        }
        Ok(binding)
    }

    pub fn type_binding(
        &self,
        module: &str,
        name: &str,
    ) -> Result<&PublicBinding, ExportLookupError> {
        self.exported_type(module, name)
    }

    pub fn exported_struct(
        &self,
        module: &str,
        name: &str,
    ) -> Result<&StructInterface, ExportLookupError> {
        let binding = self.exported_type(module, name)?;
        let interface = self.loaded_interface(module)?;
        interface
            .structs
            .iter()
            .find(|structure| structure.binding == binding.id)
            .ok_or_else(|| ExportLookupError::MissingInterfaceMember {
                module: module.to_owned(),
                binding: binding.id.clone(),
            })
    }

    pub fn resolve_import(
        &self,
        from: &str,
        import: &ast::Import,
    ) -> Result<ResolvedImport, ExportLookupError> {
        let module = import.module.canonical.clone();
        let mut members = Vec::with_capacity(import.members.len());
        for requested_name in &import.members {
            let binding = self.exported_binding(&module, &requested_name.canonical)?;
            members.push(ResolvedMember {
                requested: requested_name.spelling.clone(),
                canonical: binding.canonical.clone(),
                binding: binding.id.clone(),
                kind: binding.kind,
            });
        }
        Ok(ResolvedImport {
            from: from.to_owned(),
            module,
            kind: import.phase.into(),
            alias: import.alias.as_ref().map(|name| name.canonical.clone()),
            members,
        })
    }

    fn loaded_interface(&self, module: &str) -> Result<&Interface, ExportLookupError> {
        self.interfaces
            .get(module)
            .ok_or_else(|| ExportLookupError::MissingModule {
                module: module.to_owned(),
            })
    }
}

fn find_binding<'a>(interface: &'a Interface, name: &str) -> Option<&'a PublicBinding> {
    if let Some(binding) = interface
        .bindings
        .iter()
        .find(|binding| binding.canonical == name || binding.id == name)
    {
        return Some(binding);
    }
    let alias = find_alias(interface, name)?;
    interface
        .bindings
        .iter()
        .find(|binding| binding.id == alias.target)
}

fn find_alias<'a>(interface: &'a Interface, name: &str) -> Option<&'a PublicAlias> {
    interface
        .aliases
        .iter()
        .find(|alias| alias.canonical == name || alias.spelling == name)
}

/// Read all interfaces in a deterministic explicit path map.
pub fn read_interface_paths(
    paths: &BTreeMap<String, PathBuf>,
) -> Result<BTreeMap<String, Interface>, ModuleGraphError> {
    let mut output = BTreeMap::new();
    let mut origins = BTreeMap::<String, PathBuf>::new();
    for (requested, path) in paths {
        let text = fs::read_to_string(path).map_err(|error| ModuleGraphError::InterfaceIo {
            requested: requested.clone(),
            path: path.clone(),
            message: error.to_string(),
        })?;
        let parsed = interface::read(&text).map_err(|error| ModuleGraphError::InterfaceParse {
            requested: requested.clone(),
            path: path.clone(),
            message: error.to_string(),
        })?;
        if parsed.module != *requested {
            return Err(ModuleGraphError::InterfaceModuleMismatch {
                requested: requested.clone(),
                declared: parsed.module.clone(),
                path: path.clone(),
            });
        }
        if let Some(first) = origins.insert(parsed.module.clone(), path.clone()) {
            return Err(ModuleGraphError::DuplicateInterface {
                module: parsed.module,
                first,
                second: path.clone(),
            });
        }
        output.insert(requested.clone(), parsed);
    }
    Ok(output)
}

/// Read one interface and verify that its declared module equals `requested`.
pub fn read_interface_file(requested: &str, path: &Path) -> Result<Interface, ModuleGraphError> {
    let mut paths = BTreeMap::new();
    paths.insert(requested.to_owned(), path.to_path_buf());
    read_interface_paths(&paths).and_then(|mut interfaces| {
        interfaces
            .remove(requested)
            .ok_or_else(|| ModuleGraphError::InterfaceParse {
                requested: requested.to_owned(),
                path: path.to_path_buf(),
                message: "interface was not returned by reader".to_owned(),
            })
    })
}

/// A small builder for callers that discover sources and interface paths
/// incrementally.
#[derive(Clone, Debug, Default)]
pub struct ModuleGraphBuilder {
    modules: Vec<ast::Module>,
    interface_paths: BTreeMap<String, PathBuf>,
}

impl ModuleGraphBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_module(&mut self, module: ast::Module) -> &mut Self {
        self.modules.push(module);
        self
    }

    pub fn add_interface_path(
        &mut self,
        module: impl Into<String>,
        path: impl Into<PathBuf>,
    ) -> &mut Self {
        self.interface_paths.insert(module.into(), path.into());
        self
    }

    pub fn build(self) -> Result<ModuleGraph, ModuleGraphError> {
        ModuleGraph::build_with_interface_paths(self.modules, &self.interface_paths)
    }
}

impl From<io::Error> for ModuleGraphError {
    fn from(error: io::Error) -> Self {
        Self::InterfaceIo {
            requested: String::new(),
            path: PathBuf::new(),
            message: error.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use super::{EdgeKind, ModuleGraph, ModuleGraphError, TopologyError, read_interface_file};
    use crate::{ast, compiler, project::PythonVersion, reader};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn source(text: &str) -> ast::Module {
        let lowered = ast::lower_document(&reader::read(text));
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
        lowered.module
    }

    fn fixture_interface() -> (PathBuf, String) {
        let source_text = r#"
            (module dep.core)
            (defn run [[x Int]] -> Int x)
            (defstruct Point [x Int])
            (alias 执行 run)
            (export [run Point 执行])
        "#;
        let options = compiler::CompileOptions::new("dep.core", PythonVersion::MINIMUM);
        let result = compiler::compile(source_text, &options);
        assert!(!result.has_errors(), "{:?}", result.analysis.diagnostics);
        let encoded = result.interface.expect("fixture should have interface");
        let path = std::env::temp_dir().join(format!(
            "osiris-module-graph-{}-{}.osri",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, &encoded).expect("interface fixture should be written");
        (path, encoded)
    }

    #[test]
    fn separates_runtime_and_phase1_edges_and_orders_deterministically() {
        let graph = ModuleGraph::build([
            source("(module a) (import b) (import-for-syntax c)"),
            source("(module b) (import c)"),
            source("(module c)"),
        ])
        .expect("graph should build");
        assert_eq!(graph.runtime().edges().len(), 2);
        assert_eq!(graph.phase1().edges().len(), 1);
        assert_eq!(graph.runtime().edges()[0].kind, EdgeKind::Runtime);
        assert_eq!(graph.phase1().edges()[0].kind, EdgeKind::Phase1);
        assert_eq!(graph.runtime().dependency_order().unwrap(), ["c", "b", "a"]);
        assert_eq!(
            graph
                .runtime()
                .scc_dependency_order()
                .into_iter()
                .map(|component| component.modules)
                .collect::<Vec<_>>(),
            vec![
                vec!["c".to_owned()],
                vec!["b".to_owned()],
                vec!["a".to_owned()]
            ]
        );
        assert_eq!(
            graph.runtime().topological_order().unwrap(),
            ["a", "b", "c"]
        );
        assert_eq!(graph.phase1().sccs().len(), 3);
    }

    #[test]
    fn detects_duplicate_missing_and_phase1_cycles() {
        let duplicate = ModuleGraph::build([source("(module same)"), source("(module same)")])
            .expect_err("duplicate should fail");
        assert!(matches!(
            duplicate,
            ModuleGraphError::DuplicateModule { .. }
        ));

        let missing = ModuleGraph::build([source("(module root) (import absent)")])
            .expect_err("missing import should fail");
        assert!(matches!(missing, ModuleGraphError::MissingModule { .. }));

        let cycle = ModuleGraph::build([
            source("(module a) (import-for-syntax b)"),
            source("(module b) (import-for-syntax a)"),
        ])
        .expect_err("phase1 cycle should fail");
        assert_eq!(
            cycle,
            ModuleGraphError::Phase1Cycle {
                modules: vec!["a".to_owned(), "b".to_owned()]
            }
        );
    }

    #[test]
    fn runtime_cycle_is_reported_by_topology_but_allowed_in_graph() {
        let graph = ModuleGraph::build([
            source("(module a) (import b)"),
            source("(module b) (import a)"),
        ])
        .expect("runtime cycles are allowed");
        let error = graph
            .runtime()
            .dependency_order()
            .expect_err("cycle expected");
        assert!(matches!(error, TopologyError::Cycle { .. }));
        assert_eq!(graph.runtime().sccs()[0].modules, ["a", "b"]);
        assert_eq!(
            graph.runtime().scc_dependency_order()[0].modules,
            ["a", "b"]
        );
    }

    #[test]
    fn runtime_scc_order_is_dependency_first_across_components() {
        let graph = ModuleGraph::build([
            source("(module app) (import left) (import right)"),
            source("(module left) (import shared) (import cycle.one)"),
            source("(module right) (import shared)"),
            source("(module shared)"),
            source("(module cycle.one) (import cycle.two)"),
            source("(module cycle.two) (import cycle.one) (import shared)"),
        ])
        .expect("runtime graph should build");
        let order = graph
            .runtime()
            .scc_dependency_order()
            .into_iter()
            .map(|component| component.modules)
            .collect::<Vec<_>>();
        assert_eq!(
            order,
            vec![
                vec!["shared".to_owned()],
                vec!["cycle.one".to_owned(), "cycle.two".to_owned()],
                vec!["left".to_owned()],
                vec!["right".to_owned()],
                vec!["app".to_owned()],
            ]
        );
    }

    #[test]
    fn loads_interface_without_executing_python_and_resolves_aliases() {
        let (path, encoded) = fixture_interface();
        let interface = read_interface_file("dep.core", &path).expect("interface should load");
        assert_eq!(interface.module, "dep.core");
        assert!(encoded.contains("osiris-interface"));

        let mut paths = std::collections::BTreeMap::new();
        paths.insert("dep.core".to_owned(), path.clone());
        let graph = ModuleGraph::build_with_interface_paths(
            [source("(module app) (import dep.core :refer [执行 Point])")],
            &paths,
        )
        .expect("external interface should satisfy import");
        assert_eq!(
            graph.exported_function("dep.core", "执行").unwrap().binding,
            "dep.core::function::run"
        );
        assert_eq!(
            graph.exported_alias("dep.core", "执行").unwrap().target,
            "dep.core::function::run"
        );
        assert_eq!(
            graph.exported_struct("dep.core", "Point").unwrap().binding,
            "dep.core::type::Point"
        );
        let import = match &graph.source_modules()["app"].items[0].kind {
            ast::ItemKind::Import(import) => import,
            _ => panic!("expected import"),
        };
        let resolved = graph.resolve_import("app", import).unwrap();
        assert_eq!(resolved.members.len(), 2);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_interface_module_mismatch() {
        let (path, _) = fixture_interface();
        let error = read_interface_file("other", &path).expect_err("mismatch should fail");
        assert!(matches!(
            error,
            ModuleGraphError::InterfaceModuleMismatch { .. }
        ));
        let _ = fs::remove_file(path);
    }
}
