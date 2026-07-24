use super::super::super::*;

impl<'a> Lowerer<'a> {
    /// Install one validated interface and synthesize stable imported member
    /// bindings for qualified and referred calls.
    pub(in crate::hir) fn predeclare_interface_import(
        &mut self,
        import: &ast::Import,
        metadata: &[MetadataEntry],
    ) {
        let Some(provider) = self.interfaces else {
            return;
        };
        let module_name = import.module.canonical.clone();
        let Some(interface) = provider.interface(&module_name).cloned() else {
            self.error(
                "OSR-H0010",
                format!("imported module `{module_name}` has no validated interface"),
                import.span,
            );
            return;
        };

        self.merge_imported_operator_instances(&interface, import.span);

        let exported = interface
            .bindings
            .iter()
            .map(|binding| binding.canonical.clone())
            .chain(
                interface
                    .aliases
                    .iter()
                    .map(|alias| alias.canonical.clone()),
            )
            .chain(
                interface
                    .macros
                    .iter()
                    .map(|macro_| macro_.canonical.clone()),
            )
            .collect::<BTreeSet<_>>();
        let excluded = self.validate_interface_exclusions(import, &exported);
        let renamed = self.validate_interface_renames(import, &exported, &excluded);
        let referred = self.validate_interface_refers(import, &exported, &excluded);
        for canonical in &referred {
            if interface
                .macros
                .iter()
                .any(|macro_| macro_.canonical == *canonical)
            {
                self.phase_one_names
                    .insert(renamed.get(canonical).unwrap_or(canonical).clone());
            }
        }

        let mut bindings = BTreeMap::<String, BindingId>::new();
        for public in &interface.bindings {
            if excluded.contains(&public.canonical) {
                continue;
            }
            if let Some(id) =
                self.install_imported_binding(public, &interface, None, metadata, import.span)
            {
                bindings.insert(public.canonical.clone(), id.clone());
                let base = import
                    .alias
                    .as_ref()
                    .map_or(module_name.as_str(), |alias| alias.canonical.as_str());
                for qualifier in [base, module_name.as_str()] {
                    self.qualified_imports
                        .insert(format!("{qualifier}/{}", public.canonical), id.clone());
                    self.qualified_imports
                        .insert(format!("{qualifier}.{}", public.canonical), id.clone());
                }
            }
        }

        let base = import
            .alias
            .as_ref()
            .map_or(module_name.as_str(), |alias| alias.canonical.as_str())
            .to_owned();
        for alias in &interface.aliases {
            let Some(target) = bindings.get(&alias_target_canonical(&interface, alias)) else {
                continue;
            };
            for qualifier in [base.as_str(), module_name.as_str()] {
                self.qualified_imports
                    .insert(format!("{qualifier}/{}", alias.canonical), target.clone());
                self.qualified_imports
                    .insert(format!("{qualifier}.{}", alias.canonical), target.clone());
                self.qualified_imports
                    .insert(format!("{qualifier}/{}", alias.spelling), target.clone());
                self.qualified_imports
                    .insert(format!("{qualifier}.{}", alias.spelling), target.clone());
            }
        }

        for canonical in &referred {
            let Some(public) = find_imported_binding(&interface, canonical) else {
                continue;
            };
            let local = renamed.get(canonical).unwrap_or(canonical);
            let Some(id) = self.install_imported_binding(
                public,
                &interface,
                Some(local),
                metadata,
                import.span,
            ) else {
                continue;
            };
            self.globals.insert(local.clone(), id.clone());
            if local != canonical {
                self.aliases.push(Alias {
                    spelling: local.clone(),
                    canonical: local.clone(),
                    target: id,
                    span: import.span,
                    public: false,
                });
            }
        }

        // Preserve authored spellings for explicitly referred public aliases.
        for alias in &interface.aliases {
            let explicitly_requested = import.members.iter().any(|member| {
                member.canonical == alias.canonical || member.canonical == alias.spelling
            });
            if explicitly_requested && !excluded.contains(&alias.canonical) {
                if let Some(id) = bindings.get(&alias_target_canonical(&interface, alias)) {
                    let requested = import
                        .members
                        .iter()
                        .map(|member| member.canonical.clone())
                        .collect::<BTreeSet<_>>();
                    let canonical = requested_alias_key(&requested, alias);
                    let local = renamed.get(&canonical).unwrap_or(&canonical).clone();
                    self.globals.insert(local.clone(), id.clone());
                    if !self
                        .aliases
                        .iter()
                        .any(|existing| existing.canonical == local && existing.target == *id)
                    {
                        self.aliases.push(Alias {
                            spelling: local.clone(),
                            canonical: local,
                            target: id.clone(),
                            span: import.span,
                            public: false,
                        });
                    }
                }
            }
        }
    }

    pub(in crate::hir) fn merge_imported_operator_instances(
        &mut self,
        interface: &Interface,
        span: Span,
    ) {
        for instance in &interface.operator_instances {
            if let Some(existing) = self.operator_instances.get(&instance.id) {
                if existing != instance {
                    self.error(
                        "OSR-H0020",
                        format!(
                            "imported operator instance id `{}` has conflicting declarations",
                            instance.id
                        ),
                        span,
                    );
                }
                continue;
            }
            if self.operator_instances.values().any(|existing| {
                existing.operator == instance.operator
                    && existing.operands == instance.operands
                    && existing.id != instance.id
            }) {
                self.error(
                    "OSR-H0021",
                    format!(
                        "operator `{}` has conflicting imported operand tuple",
                        instance.operator.stable_name()
                    ),
                    span,
                );
                continue;
            }
            if let Some(public) = interface
                .bindings
                .iter()
                .find(|binding| binding.id == instance.binding)
            {
                let evidence =
                    self.imported_contract_evidence(interface, public, Some(&instance.id));
                self.operator_contract_evidence
                    .insert(instance.id.clone(), evidence);
            }
            self.operator_instances
                .insert(instance.id.clone(), instance.clone());
        }
    }

    pub(in crate::hir) fn install_imported_binding(
        &mut self,
        public: &PublicBinding,
        interface: &Interface,
        local_name: Option<&str>,
        _metadata: &[MetadataEntry],
        span: Span,
    ) -> Option<BindingId> {
        let id = BindingId::from_interface(public.id.clone());
        if !self.bindings.contains_key(&id) {
            let runtime = public.runtime.as_ref().map_or_else(
                || RuntimeBinding {
                    module: interface.module.clone(),
                    name: public.python.clone(),
                    python_module: false,
                },
                |runtime| RuntimeBinding {
                    module: if runtime.module.is_empty() {
                        interface.module.clone()
                    } else {
                        runtime.module.clone()
                    },
                    name: runtime.name.clone(),
                    python_module: runtime.python_module,
                },
            );
            let name = BindingName {
                id: id.clone(),
                canonical: public.canonical.clone(),
                python: if public.python.is_empty() {
                    python_identifier(&public.canonical)
                } else {
                    public.python.clone()
                },
                kind: public.kind,
                span,
            };
            self.bindings.insert(
                id.clone(),
                Binding {
                    name,
                    source_spelling: local_name.unwrap_or(&public.canonical).to_owned(),
                    ty: public.ty.clone(),
                    runtime: Some(runtime),
                    public: false,
                    metadata: public.metadata.clone(),
                },
            );
            self.register_imported_callable(&id, public, interface);
        }
        if let Some(local_name) = local_name {
            if let Some(existing) = self.globals.get(local_name)
                && existing != &id
            {
                self.error(
                    "OSR-N0003",
                    format!("imported name `{local_name}` conflicts with another binding"),
                    span,
                );
                return None;
            }
        }
        Some(id)
    }

    pub(in crate::hir) fn register_imported_callable(
        &mut self,
        id: &BindingId,
        public: &PublicBinding,
        interface: &Interface,
    ) {
        match public.kind {
            BindingKind::Function => {
                let Some(function) = interface
                    .functions
                    .iter()
                    .find(|function| function.binding == id.as_str())
                else {
                    self.error(
                        "OSR-H0012",
                        format!(
                            "interface `{}` has no function signature for `{}`",
                            interface.module,
                            id.as_str()
                        ),
                        Span::default(),
                    );
                    return;
                };
                let mut variables = BTreeMap::new();
                let parameters = function
                    .parameters
                    .iter()
                    .map(|parameter| {
                        import_type_with_variables(&mut self.types, &parameter.ty, &mut variables)
                    })
                    .collect::<Vec<_>>();
                let return_type = import_type_with_variables(
                    &mut self.types,
                    &function.return_type,
                    &mut variables,
                );
                let signature = FunctionType::new(parameters.clone(), return_type)
                    .with_summaries(function.summaries.clone());
                self.set_binding_type(id, Type::Fn(signature.clone()));
                let callable_parameters = function
                    .parameters
                    .iter()
                    .zip(parameters)
                    .map(|(parameter, ty)| CallableParameter {
                        canonical: parameter.canonical.clone(),
                        accepted_names: interface_parameter_names(parameter),
                        ty,
                        required: !parameter.has_default && !parameter.variadic,
                        variadic: parameter.variadic,
                        span: Span::default(),
                    })
                    .collect();
                let generic_variables = variables
                    .values()
                    .filter_map(|ty| match ty {
                        Type::TypeVar(variable) => Some(*variable),
                        _ => None,
                    })
                    .collect();
                let contract_evidence = self.imported_contract_evidence(
                    interface,
                    public,
                    function.contract_id.as_deref(),
                );
                self.register_callable(
                    id.clone(),
                    signature,
                    callable_parameters,
                    generic_variables,
                    contract_evidence,
                );
            }
            BindingKind::Type => {
                let Some(structure) = interface
                    .structs
                    .iter()
                    .find(|structure| structure.binding == id.as_str())
                else {
                    return;
                };
                let mut variables = BTreeMap::new();
                let fields = structure
                    .fields
                    .iter()
                    .map(|field| {
                        import_type_with_variables(&mut self.types, &field.ty, &mut variables)
                    })
                    .collect::<Vec<_>>();
                let return_type =
                    import_type_with_variables(&mut self.types, &public.ty, &mut variables);
                let generic_variables = match &return_type {
                    Type::Nominal { args, .. } => args
                        .iter()
                        .filter_map(|argument| match argument {
                            Type::TypeVar(variable) => Some(*variable),
                            _ => None,
                        })
                        .collect::<Vec<_>>(),
                    _ => Vec::new(),
                };
                let mut field_table = StructFieldTable {
                    generic_variables: generic_variables.clone(),
                    fields: BTreeMap::new(),
                };
                for (field, ty) in structure.fields.iter().zip(&fields) {
                    let info = StructFieldInfo {
                        canonical: field.canonical.clone(),
                        ty: ty.clone(),
                    };
                    for name in interface_field_names(field) {
                        field_table.fields.insert(name, info.clone());
                    }
                }
                self.struct_fields.insert(public.id.clone(), field_table);
                let mut summaries = CallSummaries::pure_scalar();
                if structure.invariant_count > 0 {
                    summaries.effects = EffectRow::singleton(Effect::Throw);
                }
                let signature =
                    FunctionType::new(fields.clone(), return_type).with_summaries(summaries);
                let callable_parameters = structure
                    .fields
                    .iter()
                    .zip(fields)
                    .map(|(field, ty)| CallableParameter {
                        canonical: field.canonical.clone(),
                        accepted_names: interface_field_names(field),
                        ty,
                        required: !field.has_default,
                        variadic: false,
                        span: Span::default(),
                    })
                    .collect();
                self.register_callable(
                    id.clone(),
                    signature,
                    callable_parameters,
                    generic_variables,
                    ContractEvidence::default(),
                );
            }
            _ => {}
        }
    }
}
