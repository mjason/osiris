use super::super::super::*;

impl<'a> Lowerer<'a> {
    pub(in crate::hir) fn declare(
        &mut self,
        source_name: &Name,
        kind: BindingKind,
        ty: Type,
        metadata: Vec<MetadataEntry>,
        span: Span,
        runtime: Option<RuntimeBinding>,
    ) -> Option<BindingId> {
        if let Some(existing) = self.globals.get(&source_name.canonical) {
            self.error(
                "OSR-N0001",
                format!(
                    "name `{}` conflicts with existing binding `{}`",
                    source_name.spelling,
                    existing.as_str()
                ),
                span,
            );
            return None;
        }
        match self
            .allocator
            .declare(&self.module_name, &source_name.spelling, kind, span)
        {
            Ok(name) => {
                let id = name.id.clone();
                self.globals.insert(name.canonical.clone(), id.clone());
                self.bindings.insert(
                    id.clone(),
                    Binding {
                        name,
                        source_spelling: source_name.spelling.clone(),
                        ty,
                        runtime,
                        public: false,
                        metadata,
                    },
                );
                Some(id)
            }
            Err(diagnostic) => {
                self.diagnostics.push(diagnostic);
                None
            }
        }
    }

    pub(in crate::hir) fn register_callable(
        &mut self,
        id: BindingId,
        signature: FunctionType,
        parameters: Vec<CallableParameter>,
        generic_variables: Vec<TypeVarId>,
        contract_evidence: ContractEvidence,
    ) {
        let mut claimed_names = BTreeMap::<String, (usize, String)>::new();
        let mut conflicts = Vec::new();
        for (index, parameter) in parameters.iter().enumerate() {
            for name in &parameter.accepted_names {
                if let Some((existing_index, existing_name)) = claimed_names.get(name) {
                    if *existing_index != index {
                        conflicts.push((
                            name.clone(),
                            existing_name.clone(),
                            parameter.canonical.clone(),
                            parameter.span,
                        ));
                    }
                } else {
                    claimed_names.insert(name.clone(), (index, parameter.canonical.clone()));
                }
            }
        }
        for (name, first, second, span) in conflicts {
            self.error(
                "OSR-N0014",
                format!("parameter name or alias `{name}` refers to both `{first}` and `{second}`"),
                span,
            );
        }
        self.callables.insert(
            id,
            CallableInfo {
                signature,
                parameters,
                generic_variables,
                contract_evidence,
            },
        );
    }

    pub(in crate::hir) fn resolve_aliases(&mut self, module: &ast::Module) {
        for item in &module.items {
            let AstItemKind::Alias(alias) = &item.kind else {
                continue;
            };
            // An alias target may be a qualified member of an imported
            // interface (for example `series/rolling-mean`).  Such members
            // intentionally do not enter `globals`: they retain the
            // provider's stable imported BindingId in `qualified_imports`.
            // Resolve that table here so a local alias is only a spelling
            // change, never a wrapper or a second binding.
            let Some(target) = self.resolve_alias_target(&alias.target.canonical) else {
                if self.phase_one_names.contains(&alias.target.canonical) {
                    self.phase_one_names.insert(alias.local.canonical.clone());
                    continue;
                }
                self.error(
                    "OSR-N0010",
                    format!("unknown alias target `{}`", alias.target.spelling),
                    alias.span,
                );
                continue;
            };
            let Some(binding) = self.bindings.get(&target).cloned() else {
                continue;
            };
            match self
                .allocator
                .alias(&alias.local.spelling, &binding.name, alias.span)
            {
                Ok(()) => {
                    self.globals
                        .insert(alias.local.canonical.clone(), target.clone());
                    self.aliases.push(Alias {
                        spelling: alias.local.spelling.clone(),
                        canonical: alias.local.canonical.clone(),
                        target,
                        span: alias.span,
                        public: false,
                    });
                }
                Err(diagnostic) => self.diagnostics.push(diagnostic),
            }
        }
    }

    pub(in crate::hir) fn resolve_nominal_types(&mut self, span: Span) {
        let resolutions = self.nominal_type_resolutions();
        let mut unknown = BTreeSet::new();
        for binding in self.bindings.values() {
            collect_unresolved_nominal_bindings(&binding.ty, &resolutions, &mut unknown);
        }
        for callable in self.callables.values() {
            collect_unresolved_nominal_bindings(
                &Type::Fn(callable.signature.clone()),
                &resolutions,
                &mut unknown,
            );
            for parameter in &callable.parameters {
                collect_unresolved_nominal_bindings(&parameter.ty, &resolutions, &mut unknown);
            }
        }
        for name in unknown {
            self.report_unknown_nominal_type(&name, span);
        }
        let module = self.module_name.as_str();
        for binding in self.bindings.values_mut() {
            binding.ty = resolve_nominal_bindings(&binding.ty, &resolutions, module);
        }
        for callable in self.callables.values_mut() {
            callable.signature =
                resolve_function_nominal_bindings(&callable.signature, &resolutions, module);
            for parameter in &mut callable.parameters {
                parameter.ty = resolve_nominal_bindings(&parameter.ty, &resolutions, module);
            }
        }
        for table in self.struct_fields.values_mut() {
            for field in table.fields.values_mut() {
                field.ty = resolve_nominal_bindings(&field.ty, &resolutions, module);
            }
        }
        for instance in self.operator_instances.values_mut() {
            instance.operands = instance
                .operands
                .iter()
                .map(|operand| resolve_nominal_bindings(operand, &resolutions, module))
                .collect();
            instance.result = resolve_nominal_bindings(&instance.result, &resolutions, module);
        }
    }

    pub(in crate::hir) fn nominal_type_resolutions(&self) -> BTreeMap<String, String> {
        let mut resolutions = BTreeMap::new();
        for (spelling, id) in self.globals.iter().chain(&self.qualified_imports) {
            if self
                .bindings
                .get(id)
                .is_some_and(|binding| binding.name.kind == BindingKind::Type)
            {
                resolutions.insert(spelling.clone(), id.as_str().to_owned());
            }
        }

        // Preserve the existing convenient unqualified spelling when exactly
        // one imported or local type has that canonical name. Ambiguous short
        // names deliberately remain unresolved so the caller diagnoses them
        // as unknown instead of collapsing two provider types together.
        let mut candidates = BTreeMap::<String, BTreeSet<String>>::new();
        for (id, binding) in &self.bindings {
            if binding.name.kind == BindingKind::Type {
                candidates
                    .entry(binding.name.canonical.clone())
                    .or_default()
                    .insert(id.as_str().to_owned());
            }
        }
        for (name, bindings) in candidates {
            if bindings.len() == 1 {
                resolutions
                    .entry(name)
                    .or_insert_with(|| bindings.into_iter().next().expect("one type binding"));
            }
        }

        // A closed set of Python exception classes is available to `catch`
        // without declaring a nominal Osiris type or importing a runtime
        // module.  Local/imported declarations win on spelling conflicts;
        // unknown nominal names remain rejected below.
        for name in python_builtin_exception_names() {
            if let Some(binding) = python_builtin_exception_binding(name) {
                resolutions
                    .entry((*name).to_owned())
                    .or_insert_with(|| binding.clone());
                resolutions
                    .entry(format!("builtins/{name}"))
                    .or_insert_with(|| binding.clone());
                resolutions
                    .entry(format!("builtins.{name}"))
                    .or_insert(binding);
            }
        }
        resolutions
    }

    pub(in crate::hir) fn resolve_type_expr(&mut self, expression: &ast::TypeExpr) -> Type {
        self.resolve_type_expr_with_generics(expression, &BTreeMap::new())
    }

    pub(in crate::hir) fn resolve_type_expr_with_generics(
        &mut self,
        expression: &ast::TypeExpr,
        generic_parameters: &BTreeMap<String, Type>,
    ) -> Type {
        let ty = type_from_ast_with_generics(expression, generic_parameters);
        let resolutions = self.nominal_type_resolutions();
        let mut unknown = BTreeSet::new();
        collect_unresolved_nominal_bindings(&ty, &resolutions, &mut unknown);
        for name in unknown {
            self.report_unknown_nominal_type(&name, expression.span);
        }
        resolve_nominal_bindings(&ty, &resolutions, &self.module_name)
    }

    pub(in crate::hir) fn report_unknown_nominal_type(&mut self, name: &str, span: Span) {
        if self.unknown_nominal_types.insert(name.to_owned()) {
            self.error("OSR-T0021", format!("unknown nominal type `{name}`"), span);
        }
    }
}
