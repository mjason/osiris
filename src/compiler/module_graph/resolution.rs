use super::*;

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
                if crate::stdlib::is_standard_namespace(&import.module.canonical)
                    && !known.contains(&import.module.canonical)
                {
                    continue;
                }
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
