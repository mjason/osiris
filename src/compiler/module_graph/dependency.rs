use super::*;

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
    pub(super) fn with_nodes(nodes: impl IntoIterator<Item = String>) -> Self {
        let mut graph = Self::default();
        for node in nodes {
            graph.add_node(node);
        }
        graph
    }

    pub(super) fn add_node(&mut self, node: String) {
        self.nodes.insert(node.clone());
        self.adjacency.entry(node).or_default();
    }

    pub(super) fn add_edge(&mut self, edge: ModuleEdge) {
        self.add_node(edge.from.clone());
        self.add_node(edge.to.clone());
        self.adjacency
            .entry(edge.from.clone())
            .or_default()
            .insert(edge.to.clone());
        self.edges.push(edge);
    }

    pub(super) fn finish(&mut self) {
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
