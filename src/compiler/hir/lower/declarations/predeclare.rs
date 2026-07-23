use super::super::super::*;

impl<'a> Lowerer<'a> {
    pub(in crate::hir) fn predeclare_function(
        &mut self,
        function: &ast::Function,
        external: bool,
        runtime_module: Option<&str>,
    ) {
        let Some(name) = &function.name else {
            return;
        };
        let parameter_types = function
            .params
            .iter()
            .map(|parameter| {
                parameter
                    .type_annotation
                    .as_ref()
                    .map_or_else(|| self.types.fresh_var(), type_from_ast)
            })
            .collect::<Vec<_>>();
        let return_type = function
            .return_type
            .as_ref()
            .map_or_else(|| self.types.fresh_var(), type_from_ast);
        let mut signature = FunctionType::new(parameter_types, return_type);
        let contract_evidence = if external {
            signature.summaries = function
                .contract
                .as_ref()
                .map_or_else(CallSummaries::unknown, |contract| {
                    contract.summaries.clone()
                });
            self.local_extern_contract_evidence(function)
        } else {
            signature.summaries = CallSummaries::unknown();
            ContractEvidence::default()
        };
        let callable_parameters = function
            .params
            .iter()
            .zip(&signature.parameters)
            .map(|(parameter, ty)| CallableParameter {
                canonical: parameter.name.canonical.clone(),
                accepted_names: parameter_names(&parameter.name, &parameter.metadata),
                ty: ty.clone(),
                required: parameter.default.is_none() && !parameter.variadic,
                variadic: parameter.variadic,
                span: parameter.span,
            })
            .collect::<Vec<_>>();
        if let Some(id) = self.declare(
            name,
            BindingKind::Function,
            Type::Fn(signature.clone()),
            function.metadata.clone(),
            function.span,
            runtime_module.map(|module| RuntimeBinding {
                module: module.to_owned(),
                name: python_identifier(&name.canonical),
                python_module: true,
            }),
        ) {
            self.register_callable(
                id,
                signature,
                callable_parameters,
                Vec::new(),
                contract_evidence,
            );
        }
        if external && runtime_module.is_none() {
            self.error(
                "OSR-H0002",
                "external function has no Python runtime module",
                function.span,
            );
        }
    }

    pub(in crate::hir) fn imported_contract_evidence(
        &self,
        interface: &Interface,
        public: &PublicBinding,
        contract_id: Option<&str>,
    ) -> ContractEvidence {
        let policy = self
            .trust_policy
            .interfaces
            .get(&interface.module)
            .filter(|policy| policy.semantic_interface_hash == interface.semantic_interface_hash());
        let fact = ContractFact {
            distribution: policy.map(|policy| policy.distribution.clone()),
            provider_module: interface.module.clone(),
            semantic_interface_hash: Some(interface.semantic_interface_hash().to_owned()),
            binding: public.id.clone(),
            contract_id: contract_id.map(str::to_owned),
        };
        let verified = policy
            .zip(contract_id)
            .is_some_and(|(policy, id)| policy.trusted_contract_ids.contains(id));
        ContractEvidence {
            declared: BTreeSet::from([fact.clone()]),
            verified: if verified {
                BTreeSet::from([fact])
            } else {
                BTreeSet::new()
            },
        }
    }

    pub(in crate::hir) fn local_extern_contract_evidence(
        &self,
        function: &ast::Function,
    ) -> ContractEvidence {
        let Some(name) = &function.name else {
            return ContractEvidence::default();
        };
        let fact = ContractFact {
            distribution: None,
            provider_module: self.module_name.clone(),
            semantic_interface_hash: None,
            binding: BindingId::new(&self.module_name, &name.canonical, BindingKind::Function)
                .as_str()
                .to_owned(),
            contract_id: function
                .contract
                .as_ref()
                .map(|contract| contract.id.clone()),
        };
        ContractEvidence {
            declared: BTreeSet::from([fact]),
            verified: BTreeSet::new(),
        }
    }

    pub(in crate::hir) fn predeclare_struct(&mut self, structure: &ast::Defstruct) {
        let type_binding = BindingId::new(
            &self.module_name,
            &structure.name.canonical,
            BindingKind::Type,
        );
        let generic_variables = structure
            .type_params
            .iter()
            .map(|_| match self.types.fresh_var() {
                Type::TypeVar(variable) => variable,
                _ => unreachable!("fresh_var always returns a type variable"),
            })
            .collect::<Vec<_>>();
        let generic_parameters = structure
            .type_params
            .iter()
            .map(|name| name.canonical.clone())
            .zip(generic_variables.iter().copied().map(Type::TypeVar))
            .collect::<BTreeMap<_, _>>();
        let nominal = Type::Nominal {
            binding: type_binding.as_str().to_owned(),
            args: generic_variables
                .iter()
                .copied()
                .map(Type::TypeVar)
                .collect(),
        };
        let parameter_types = structure
            .fields
            .iter()
            .map(|field| {
                field
                    .type_annotation
                    .as_ref()
                    .map_or(Type::Unknown, |expression| {
                        type_from_ast_with_generics(expression, &generic_parameters)
                    })
            })
            .collect::<Vec<_>>();
        let mut field_table = StructFieldTable {
            generic_variables: generic_variables.clone(),
            fields: BTreeMap::new(),
        };
        for (field, ty) in structure.fields.iter().zip(&parameter_types) {
            let info = StructFieldInfo {
                canonical: field.name.canonical.clone(),
                ty: ty.clone(),
            };
            for name in parameter_names(&field.name, &field.metadata) {
                field_table.fields.insert(name, info.clone());
            }
        }
        self.struct_fields
            .insert(type_binding.as_str().to_owned(), field_table);
        let callable_parameters = structure
            .fields
            .iter()
            .zip(&parameter_types)
            .map(|(field, ty)| CallableParameter {
                canonical: field.name.canonical.clone(),
                accepted_names: parameter_names(&field.name, &field.metadata),
                ty: ty.clone(),
                required: field.default.is_none(),
                variadic: false,
                span: field.span,
            })
            .collect();
        let mut signature = FunctionType::new(parameter_types, nominal.clone());
        if !structure.checks.is_empty() {
            signature.summaries.effects = EffectRow::singleton(Effect::Throw);
        }
        if let Some(id) = self.declare(
            &structure.name,
            BindingKind::Type,
            nominal,
            structure.metadata.clone(),
            structure.span,
            None,
        ) {
            self.struct_type_parameters
                .insert(id.clone(), generic_parameters);
            self.register_callable(
                id,
                signature,
                callable_parameters,
                generic_variables,
                ContractEvidence::default(),
            );
        }
    }
}
