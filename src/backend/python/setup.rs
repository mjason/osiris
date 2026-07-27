use super::*;

impl<'hir> Backend<'hir> {
    pub(super) fn standard_positional_keywords(
        &self,
        callee: &hir::Expr,
        positional_count: usize,
    ) -> BTreeMap<usize, String> {
        let hir::ExprKind::Binding(id) = &callee.kind else {
            return BTreeMap::new();
        };
        let Some(standard) = crate::stdlib::find_by_id(id) else {
            return BTreeMap::new();
        };
        let Ok(interface) = crate::stdlib::interface_artifact(standard.namespace) else {
            return BTreeMap::new();
        };
        let Some(function) = interface
            .functions
            .iter()
            .find(|function| function.binding == id.as_str())
        else {
            return BTreeMap::new();
        };
        let Some(optional_start) =
            function
                .parameters
                .iter()
                .enumerate()
                .find_map(|(index, parameter)| {
                    (parameter.has_default
                        && function.parameters[index + 1..]
                            .iter()
                            .any(|later| !later.has_default && !later.variadic))
                    .then_some(index)
                })
        else {
            return BTreeMap::new();
        };
        if positional_count <= optional_start {
            return BTreeMap::new();
        }
        let suffix = &function.parameters[optional_start..];
        let required = suffix
            .iter()
            .filter(|parameter| !parameter.has_default && !parameter.variadic)
            .count();
        let supplied_suffix = positional_count - optional_start;
        if supplied_suffix < required {
            return BTreeMap::new();
        }
        let supplied_optional = supplied_suffix - required;
        let selected = suffix
            .iter()
            .filter(|parameter| parameter.has_default)
            .take(supplied_optional)
            .chain(
                suffix
                    .iter()
                    .filter(|parameter| !parameter.has_default && !parameter.variadic),
            )
            .collect::<Vec<_>>();
        selected
            .into_iter()
            .enumerate()
            .map(|(offset, parameter)| (optional_start + offset, parameter.canonical.clone()))
            .collect()
    }

    pub(super) fn new(hir: &'hir hir::Module, target: PythonVersion) -> Self {
        Self::with_runtime_module(hir, target, runtime_module_for(&hir.name))
    }

    pub(super) fn with_runtime_module(
        hir: &'hir hir::Module,
        target: PythonVersion,
        runtime_module: String,
    ) -> Self {
        let bindings = hir
            .bindings
            .iter()
            .map(|binding| (binding.name.id.clone(), binding))
            .collect::<BTreeMap<_, _>>();
        let mut reserved_names = BTreeSet::new();
        let mut names = BTreeMap::new();
        // HIR has already checked global Python collisions. Module-level names
        // are assigned first so that `LocalNames` below can treat them as fixed.
        let local_binding_prefix = format!("{}::local-", hir.name);
        let mut global_bindings = hir
            .bindings
            .iter()
            .filter(|binding| {
                !binding.name.id.as_str().starts_with(&local_binding_prefix)
                    && matches!(
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
                binding.name.canonical.starts_with('\0'),
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
        // Local bindings keep their authored spelling, but only while it is
        // free along the enclosing Python scope chain. Python has one namespace
        // per function, so a local that reuses the spelling of an outer local
        // or of a module-level name would otherwise overwrite it and silently
        // change what later references mean.
        let globals = reserved_names.clone();
        let mut locals = LocalNames {
            bindings: &bindings,
            names: &mut names,
            reserved: &mut reserved_names,
            globals: globals.clone(),
            active: BTreeSet::new(),
            frame: Vec::new(),
            // A module-level `let` lowers to a module-level assignment, so it
            // must never reuse the name of any module-level binding.
            referenced: globals,
        };
        for item in &hir.items {
            locals.item(item);
        }
        // Anything the item walk cannot reach — struct fields, extern
        // parameters — keeps the plain projection of its canonical name.
        for binding in &hir.bindings {
            if names.contains_key(&binding.name.id) {
                continue;
            }
            reserved_names.insert(binding.name.python.clone());
            names.insert(binding.name.id.clone(), binding.name.python.clone());
        }
        // A facade binding and a compiler-generated intrinsic binding may
        // intentionally target the same linked helper. They must share one
        // Python name; otherwise the grouped import can replace one alias
        // while expressions still reference the other.
        let mut runtime_groups = BTreeMap::<(String, String), Vec<crate::name::BindingId>>::new();
        for binding in &hir.bindings {
            let Some(runtime) = &binding.runtime else {
                continue;
            };
            if matches!(
                binding.name.kind,
                crate::name::BindingKind::Module | crate::name::BindingKind::PythonModule
            ) {
                continue;
            }
            runtime_groups
                .entry((runtime.module.clone(), runtime.name.clone()))
                .or_default()
                .push(binding.name.id.clone());
        }
        for ids in runtime_groups.values_mut() {
            if ids.len() < 2 {
                continue;
            }
            ids.sort();
            let preferred = ids
                .iter()
                .find(|id| crate::stdlib::find_by_id(id).is_some())
                .unwrap_or(&ids[0]);
            let shared = names
                .get(preferred)
                .cloned()
                .expect("runtime binding was assigned a Python name");
            for id in ids {
                names.insert(id.clone(), shared.clone());
            }
        }
        Self {
            target,
            bindings,
            names,
            reserved_names,
            temporary_counter: 0,
            direct_imports: BTreeMap::new(),
            from_imports: BTreeMap::new(),
            used_module_bindings: BTreeSet::new(),
            typing: BTreeSet::new(),
            need_dataclass: false,
            need_dataclass_field: false,
            typevars: BTreeMap::new(),
            typevar_names: BTreeMap::new(),
            active_type_parameters: BTreeMap::new(),
            binding_overrides: Vec::new(),
            runtime_module,
            runtime_helpers: BTreeSet::new(),
            linked_runtime_helpers: BTreeMap::new(),
            reachable_standard_bindings: BTreeSet::new(),
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
        let standard = crate::stdlib::find_by_id(id);
        let module = if standard.is_some() && binding.name.kind == crate::name::BindingKind::Type {
            self.runtime_helpers.insert(runtime.name.clone());
            self.reachable_standard_bindings
                .insert(id.as_str().to_owned());
            self.runtime_module.clone()
        } else if let Some(standard) = standard {
            self.reachable_standard_bindings
                .insert(id.as_str().to_owned());
            format!(
                "{}.stdlib.{}",
                self.runtime_module,
                standard.namespace.rsplit('.').next().unwrap_or("standard")
            )
        } else if runtime.module == "osiris.kernel" {
            self.runtime_helpers.insert(runtime.name.clone());
            if standard.is_some() {
                self.reachable_standard_bindings
                    .insert(id.as_str().to_owned());
            }
            self.runtime_module.clone()
        } else if runtime.python_module {
            runtime.module.replace('/', ".")
        } else {
            crate::name::python_module_identifier(&runtime.module)
        };
        self.from_imports
            .entry(module)
            .or_default()
            .insert(runtime.name.clone(), Some(local));
    }

    pub(super) fn linked_runtime_helper(&mut self, helper: &str) -> py::Expr {
        if let Some(local) = self.linked_runtime_helpers.get(helper) {
            return py::Expr::name(local.clone());
        }

        let local = self.fresh_helper(&format!("_osiris_{}", python_identifier(helper)));
        self.runtime_helpers.insert(helper.to_owned());
        self.from_imports
            .entry(self.runtime_module.clone())
            .or_default()
            .insert(helper.to_owned(), Some(local.clone()));
        self.linked_runtime_helpers
            .insert(helper.to_owned(), local.clone());
        py::Expr::name(local)
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
        let module = if import.python {
            import.module.replace('/', ".")
        } else {
            crate::name::python_module_identifier(&import.module)
        };
        let default_local = module.rsplit('.').next().unwrap_or(&module);
        let alias = (local != default_local).then_some(local);
        self.direct_imports.insert(
            module,
            DirectImport {
                alias,
                binding: import.binding.clone(),
                python: import.python,
            },
        );
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
        for (module, import) in &self.direct_imports {
            if !import.python
                && !self.used_module_bindings.contains(&import.binding)
                && self.from_imports.contains_key(module)
            {
                continue;
            }
            result.push(py::Stmt::Import(py::Import::Direct(vec![
                match &import.alias {
                    Some(alias) => py::ImportAlias::renamed(module, alias),
                    None => py::ImportAlias::new(module),
                },
            ])))
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

    pub(super) fn runtime_support(&self) -> Option<RuntimeSupport> {
        (!self.runtime_helpers.is_empty() || !self.reachable_standard_bindings.is_empty()).then(
            || RuntimeSupport {
                package: self.runtime_module.clone(),
                helpers: self.runtime_helpers.clone(),
                binding_ids: self.reachable_standard_bindings.clone(),
            },
        )
    }
}

/// Assigns Python names to local bindings so that no local can hide a
/// module-level name or another local that is still readable where it appears.
///
/// Python scopes are per function, and lowering lifts `let` bindings into
/// statements ahead of the enclosing expression, so two sibling `let` bindings
/// in one function do share a scope. Names are therefore reserved for the whole
/// function and only released when a nested `fn` scope ends.
struct LocalNames<'a, 'hir> {
    bindings: &'a BTreeMap<crate::name::BindingId, &'hir hir::Binding>,
    names: &'a mut BTreeMap<crate::name::BindingId, String>,
    reserved: &'a mut BTreeSet<String>,
    globals: BTreeSet<String>,
    active: BTreeSet<String>,
    /// Names declared by the innermost function scope.
    frame: Vec<String>,
    /// Module-level names the innermost function scope reads. Shadowing any
    /// other module-level name is harmless, so only these force a rename and
    /// ordinary shadowing keeps the authored spelling.
    referenced: BTreeSet<String>,
}

impl LocalNames<'_, '_> {
    fn item(&mut self, item: &hir::Item) {
        match &item.kind {
            ItemKind::Function(function) => {
                for decorator in &function.decorators {
                    self.expr(decorator);
                }
                let enclosing = std::mem::take(&mut self.frame);
                let referenced = self.referenced_globals(&function.body);
                let outer = std::mem::replace(&mut self.referenced, referenced);
                self.parameters(&function.parameters);
                self.expr(&function.body);
                self.referenced = outer;
                self.close_frame(enclosing);
            }
            ItemKind::Value(value) => {
                if let Some(expression) = &value.value {
                    self.expr(expression);
                }
            }
            ItemKind::Expr(expression) => self.expr(expression),
            ItemKind::Struct(structure) => {
                for decorator in &structure.decorators {
                    self.expr(decorator);
                }
                for field in &structure.fields {
                    if let Some(default) = &field.default {
                        self.expr(default);
                    }
                }
                for check in &structure.checks {
                    self.expr(&check.condition);
                    if let Some(message) = &check.message {
                        self.expr(message);
                    }
                }
            }
            ItemKind::Import(_) | ItemKind::StaticSchema(_) | ItemKind::StaticRecord(_) => {}
        }
    }

    fn parameters(&mut self, parameters: &[hir::Parameter]) {
        for parameter in parameters {
            if let Some(default) = &parameter.default {
                self.expr(default);
            }
            self.declare(&parameter.binding);
        }
    }

    fn expr(&mut self, expression: &hir::Expr) {
        match &expression.kind {
            ExprKind::Let { bindings, body } => {
                for binding in bindings {
                    self.expr(&binding.value);
                    self.declare(&binding.binding);
                }
                self.expr(body);
            }
            ExprKind::Lambda { parameters, body } => {
                let enclosing = std::mem::take(&mut self.frame);
                let referenced = self.referenced_globals(body);
                let outer = std::mem::replace(&mut self.referenced, referenced);
                self.parameters(parameters);
                self.expr(body);
                self.referenced = outer;
                self.close_frame(enclosing);
            }
            ExprKind::Try {
                body,
                catches,
                finally_body,
            } => {
                self.expr(body);
                for catch in catches {
                    if let Some(binding) = &catch.binding {
                        self.declare(binding);
                    }
                    self.expr(&catch.body);
                }
                if let Some(finally_body) = finally_body {
                    self.expr(finally_body);
                }
            }
            _ => {
                let mut children = Vec::new();
                visit_children(expression, &mut |child| children.push(child));
                for child in children {
                    self.expr(child);
                }
            }
        }
    }

    fn declare(&mut self, id: &crate::name::BindingId) {
        if self.names.contains_key(id) {
            return;
        }
        let Some(binding) = self.bindings.get(id) else {
            return;
        };
        let base = binding.name.python.clone();
        let mut python = base.clone();
        let mut suffix = 2_usize;
        while self.active.contains(&python) || self.referenced.contains(&python) {
            python = format!("{base}_{suffix}");
            suffix += 1;
        }
        self.active.insert(python.clone());
        self.reserved.insert(python.clone());
        self.frame.push(python.clone());
        self.names.insert(id.clone(), python);
    }

    /// Module-level Python names read anywhere inside one function scope,
    /// including its nested `fn` scopes: a local here would shadow the
    /// module-level name for those too.
    fn referenced_globals(&self, body: &hir::Expr) -> BTreeSet<String> {
        let mut referenced = BTreeSet::new();
        self.collect_referenced(body, &mut referenced);
        referenced
    }

    fn collect_referenced(&self, expression: &hir::Expr, referenced: &mut BTreeSet<String>) {
        if let ExprKind::Binding(id) = &expression.kind
            && let Some(python) = self.names.get(id)
            && self.globals.contains(python)
        {
            referenced.insert(python.clone());
        }
        visit_children(expression, &mut |child| {
            self.collect_referenced(child, referenced)
        });
    }

    fn close_frame(&mut self, enclosing: Vec<String>) {
        for python in std::mem::replace(&mut self.frame, enclosing) {
            self.active.remove(&python);
        }
    }
}

/// Applies `visit` to every direct sub-expression, including the bodies of
/// nested binding forms.
fn visit_children<'hir>(expression: &'hir hir::Expr, visit: &mut impl FnMut(&'hir hir::Expr)) {
    match &expression.kind {
        ExprKind::List(items)
        | ExprKind::Vector(items)
        | ExprKind::Set(items)
        | ExprKind::Do(items) => items.iter().for_each(visit),
        ExprKind::Map(entries) => {
            for (key, value) in entries {
                visit(key);
                visit(value);
            }
        }
        ExprKind::Call { callee, arguments } => {
            visit(callee);
            for argument in arguments {
                match argument {
                    hir::CallArgument::Positional(value)
                    | hir::CallArgument::Keyword { value, .. } => visit(value),
                }
            }
        }
        ExprKind::Operator { operands, .. } => operands.iter().for_each(visit),
        ExprKind::Attribute { value, .. } => visit(value),
        ExprKind::Index { value, index } => {
            visit(value);
            visit(index);
        }
        ExprKind::Let { bindings, body } => {
            for binding in bindings {
                visit(&binding.value);
            }
            visit(body);
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            visit(condition);
            visit(then_branch);
            visit(else_branch);
        }
        ExprKind::Lambda { parameters, body } => {
            for parameter in parameters {
                if let Some(default) = &parameter.default {
                    visit(default);
                }
            }
            visit(body);
        }
        ExprKind::Try {
            body,
            catches,
            finally_body,
        } => {
            visit(body);
            for catch in catches {
                visit(&catch.body);
            }
            if let Some(finally_body) = finally_body {
                visit(finally_body);
            }
        }
        ExprKind::Raise(value) => {
            if let Some(value) = value {
                visit(value);
            }
        }
        ExprKind::None
        | ExprKind::Bool(_)
        | ExprKind::Integer(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Binding(_)
        | ExprKind::Error => {}
    }
}

fn runtime_module_for(module: &str) -> String {
    module.split_once('.').map_or_else(
        || "__osiris_runtime__".to_owned(),
        |(package, _)| {
            format!(
                "{}.__osiris_runtime__",
                crate::name::python_identifier(package)
            )
        },
    )
}
