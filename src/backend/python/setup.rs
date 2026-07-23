use super::*;

impl<'hir> Backend<'hir> {
    pub(super) fn new(hir: &'hir hir::Module, target: PythonVersion) -> Self {
        let bindings = hir
            .bindings
            .iter()
            .map(|binding| (binding.name.id.clone(), binding))
            .collect::<BTreeMap<_, _>>();
        let mut reserved_names = BTreeSet::new();
        let mut names = BTreeMap::new();
        // HIR has already checked global Python collisions.  Local bindings
        // can repeat across lexical scopes, which is legal in separate Python
        // functions, so retain their canonical spelling here for readability.
        let mut global_bindings = hir
            .bindings
            .iter()
            .filter(|binding| {
                matches!(
                    binding.name.kind,
                    crate::name::BindingKind::Module
                        | crate::name::BindingKind::PythonModule
                        | crate::name::BindingKind::Value
                        | crate::name::BindingKind::Function
                        | crate::name::BindingKind::Type
                )
            })
            .collect::<Vec<_>>();
        global_bindings.sort_by_key(|binding| {
            (
                !binding
                    .name
                    .id
                    .as_str()
                    .starts_with(&format!("{}::", hir.name)),
                binding.name.id.as_str(),
            )
        });
        for binding in global_bindings {
            let base = binding.name.python.clone();
            let mut python = base.clone();
            let mut suffix = 2_usize;
            while reserved_names.contains(&python) {
                python = format!("{base}_{suffix}");
                suffix += 1;
            }
            reserved_names.insert(python.clone());
            names.insert(binding.name.id.clone(), python);
        }
        for binding in &hir.bindings {
            reserved_names.insert(binding.name.python.clone());
            names
                .entry(binding.name.id.clone())
                .or_insert_with(|| binding.name.python.clone());
        }
        Self {
            target,
            bindings,
            names,
            reserved_names,
            temporary_counter: 0,
            helper_counter: 0,
            direct_imports: BTreeMap::new(),
            from_imports: BTreeMap::new(),
            typing: BTreeSet::new(),
            need_dataclass: false,
            need_dataclass_field: false,
            typevars: BTreeMap::new(),
            typevar_names: BTreeMap::new(),
            active_type_parameters: BTreeMap::new(),
            binding_overrides: Vec::new(),
        }
    }

    pub(super) fn lower_items(
        &mut self,
        module: &hir::Module,
    ) -> Result<Vec<py::Stmt>, BackendError> {
        let mut body = Vec::new();
        for item in &module.items {
            match &item.kind {
                ItemKind::Import(import) => self.register_item_import(import),
                ItemKind::Value(value) => {
                    let target = self.binding_target(&value.binding)?;
                    let binding = self.binding(&value.binding)?;
                    let annotation = self.annotation(&binding.ty, Some(item.span))?;
                    match &value.value {
                        Some(expression) => {
                            let lowered = self.lower_value(expression)?;
                            if !lowered.prefix.is_empty() {
                                body.push(py::Stmt::AnnAssign(py::AnnAssign {
                                    target: target.clone(),
                                    annotation,
                                    value: None,
                                }));
                                body.extend(lowered.prefix);
                                let result = lowered.value.ok_or_else(|| {
                                    self.error(
                                        "value definition terminates before producing a value",
                                        Some(expression.span),
                                    )
                                })?;
                                body.push(py::Stmt::Assign(py::Assign {
                                    targets: vec![target],
                                    value: result,
                                }));
                            } else {
                                body.push(py::Stmt::AnnAssign(py::AnnAssign {
                                    target,
                                    annotation,
                                    value: Some(lowered.value.ok_or_else(|| {
                                        self.error(
                                            "value definition does not produce a value",
                                            Some(expression.span),
                                        )
                                    })?),
                                }));
                            }
                        }
                        None => body.push(py::Stmt::AnnAssign(py::AnnAssign {
                            target,
                            annotation,
                            value: None,
                        })),
                    }
                }
                ItemKind::Function(function) => {
                    let lowered = self.lower_function(function)?;
                    body.push(lowered);
                }
                ItemKind::Struct(structure) => {
                    body.extend(self.lower_struct(structure)?);
                }
                ItemKind::Expr(expression) => {
                    let lowered = self.lower_value(expression)?;
                    body.extend(lowered.prefix);
                    if let Some(value) = lowered.value {
                        body.push(py::Stmt::Expr(value));
                    }
                }
                ItemKind::StaticSchema(_) | ItemKind::StaticRecord(_) => {
                    // Static interface data belongs in .osri/records artifacts;
                    // it has no runtime Python statement.
                }
            }
        }
        Ok(body)
    }

    pub(super) fn register_runtime_binding(&mut self, id: &crate::name::BindingId) {
        let Some(binding) = self.bindings.get(id).copied() else {
            return;
        };
        let Some(runtime) = &binding.runtime else {
            return;
        };
        if binding.name.kind == crate::name::BindingKind::PythonModule
            || binding.name.kind == crate::name::BindingKind::Module
        {
            return;
        }
        let local = self.python_name(&binding.name.id).to_owned();
        self.from_imports
            .entry(runtime.module.replace('/', "."))
            .or_default()
            .insert(runtime.name.clone(), Some(local));
    }

    pub(super) fn register_runtime_type(&mut self, nominal_binding: &str) {
        // Nominal types in HIR use the stable defining type BindingId. A
        // static schema has no corresponding runtime struct and therefore is
        // never registered here; it remains a records/.osri-only declaration.
        let binding_id = self.bindings.iter().find_map(|(id, binding)| {
            (binding.name.kind == crate::name::BindingKind::Type && id.as_str() == nominal_binding)
                .then_some(id.clone())
        });
        if let Some(id) = binding_id {
            self.register_runtime_binding(&id);
        }
    }

    pub(super) fn register_item_import(&mut self, import: &hir::Import) {
        let local = self
            .names
            .get(&import.binding)
            .cloned()
            .unwrap_or_else(|| python_identifier(&import.module));
        let module = import.module.replace('/', ".");
        let default_local = module.rsplit('.').next().unwrap_or(&module);
        let alias = (local != default_local).then_some(local);
        self.direct_imports.insert(module, alias);
    }

    pub(super) fn imports(&self) -> Vec<py::Stmt> {
        let mut result = Vec::new();
        if self.need_dataclass {
            let mut names = vec![py::ImportAlias::new("dataclass")];
            if self.need_dataclass_field {
                names.push(py::ImportAlias::new("field"));
            }
            result.push(py::Stmt::Import(py::Import::From {
                module: Some("dataclasses".to_owned()),
                names,
                level: 0,
            }));
        }
        for (module, names) in &self.from_imports {
            let aliases = names
                .iter()
                .map(|(name, alias)| match alias {
                    Some(alias) if alias != name => py::ImportAlias::renamed(name, alias),
                    _ => py::ImportAlias::new(name),
                })
                .collect();
            result.push(py::Stmt::Import(py::Import::From {
                module: Some(module.clone()),
                names: aliases,
                level: 0,
            }));
        }
        for (module, alias) in &self.direct_imports {
            result.push(py::Stmt::Import(py::Import::Direct(vec![match alias {
                Some(alias) => py::ImportAlias::renamed(module, alias),
                None => py::ImportAlias::new(module),
            }])))
        }
        for name in &self.typing {
            // Added below as one grouped statement; this loop only keeps
            // deterministic ordering obvious to readers of the backend.
            let _ = name;
        }
        if !self.typing.is_empty() {
            result.insert(
                usize::from(self.need_dataclass),
                py::Stmt::Import(py::Import::From {
                    module: Some("typing".to_owned()),
                    names: self.typing.iter().map(py::ImportAlias::new).collect(),
                    level: 0,
                }),
            );
        }
        result
    }

    pub(super) fn typing_imports(&self) -> Vec<py::Stmt> {
        // Imports are emitted in `imports`; this hook is kept separate so the
        // final assembly can remain stable if more typing backends are added.
        Vec::new()
    }

    pub(super) fn typevar_declarations(&self) -> Vec<py::Stmt> {
        self.typevars
            .iter()
            .map(|(source, python)| {
                py::Stmt::Assign(py::Assign {
                    targets: vec![py::Expr::name(python.clone())],
                    value: py::Expr::call(
                        py::Expr::name("TypeVar"),
                        vec![py::CallArgument::Positional(py::Expr::string(
                            source.clone(),
                        ))],
                    ),
                })
            })
            .collect()
    }
}
