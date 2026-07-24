use super::super::super::*;

impl<'a> Lowerer<'a> {
    pub(in crate::hir) fn new(
        module_name: String,
        module: &ast::Module,
        interfaces: Option<&'a dyn InterfaceProvider>,
        trust_policy: &'a ContractTrustPolicy,
        strict: bool,
    ) -> Self {
        let _ = module;
        let mut lowerer = Self {
            module_name,
            allocator: NameAllocator::default(),
            bindings: BTreeMap::new(),
            local_value_summaries: BTreeMap::new(),
            globals: BTreeMap::new(),
            callables: BTreeMap::new(),
            struct_type_parameters: BTreeMap::new(),
            phase_one_names: BTreeSet::new(),
            aliases: Vec::new(),
            exports: BTreeSet::new(),
            extern_functions: Vec::new(),
            items: Vec::new(),
            diagnostics: Vec::new(),
            types: TypeContext::new(),
            next_scope: 0,
            interfaces,
            qualified_imports: BTreeMap::new(),
            operator_instances: BTreeMap::new(),
            operator_contract_evidence: BTreeMap::new(),
            core_abs_binding: None,
            core_mapv_binding: None,
            core_collection_bindings: BTreeMap::new(),
            core_loop_binding: None,
            core_recur_binding: None,
            loop_arities: Vec::new(),
            loop_state_types: Vec::new(),
            function_depth: 0,
            loop_callback_depths: Vec::new(),
            function_recur_contexts: Vec::new(),
            struct_fields: BTreeMap::new(),
            trust_policy,
            strict,
            contract_evidence_stack: Vec::new(),
            unknown_nominal_types: BTreeSet::new(),
        };
        lowerer.install_core_reduced_type(module.span);
        lowerer.install_core_delay_type(module.span);
        lowerer.install_core_future_type(module.span);
        lowerer.install_core_promise_type(module.span);
        lowerer
    }

    pub(in crate::hir) fn finish(self, source: &ast::Module) -> LowerResult {
        LowerResult {
            module: Module {
                name: self.module_name,
                trust_policy_hash: self.trust_policy.hash.clone(),
                span: source.span,
                metadata: source.metadata.clone(),
                bindings: self.bindings.into_values().collect(),
                aliases: self.aliases,
                exports: self.exports.into_iter().collect(),
                extern_functions: self.extern_functions,
                items: self.items,
            },
            diagnostics: self.diagnostics,
        }
    }

    pub(in crate::hir) fn predeclare(&mut self, module: &ast::Module) {
        for item in &module.items {
            match &item.kind {
                AstItemKind::Import(import) => {
                    let embedded_standard =
                        crate::stdlib::is_standard_namespace(&import.module.canonical)
                            && self.interfaces.is_none_or(|interfaces| {
                                interfaces.interface(&import.module.canonical).is_none()
                            });
                    if embedded_standard {
                        self.predeclare_standard_import(import);
                        continue;
                    }
                    let name = import.alias.as_ref().unwrap_or(&import.module);
                    self.declare(
                        name,
                        BindingKind::Module,
                        Type::Any,
                        item.metadata.clone(),
                        import.span,
                        Some(RuntimeBinding {
                            module: import.module.canonical.replace('/', "."),
                            name: name.canonical.clone(),
                            python_module: false,
                        }),
                    );
                    self.predeclare_interface_import(import, item.metadata.as_slice());
                }
                AstItemKind::PyImport(import) => {
                    let default_name = import.module.rsplit('.').next().unwrap_or(&import.module);
                    let name = import.alias.clone().unwrap_or_else(|| Name {
                        spelling: default_name.to_owned(),
                        canonical: default_name.to_owned(),
                    });
                    self.declare(
                        &name,
                        BindingKind::PythonModule,
                        Type::Any,
                        item.metadata.clone(),
                        import.span,
                        Some(RuntimeBinding {
                            module: import.module.clone(),
                            name: name.canonical.clone(),
                            python_module: true,
                        }),
                    );
                }
                AstItemKind::Def(definition) => {
                    let ty = definition
                        .type_annotation
                        .as_ref()
                        .map_or_else(|| self.types.fresh_var(), type_from_ast);
                    self.declare(
                        &definition.name,
                        BindingKind::Value,
                        ty,
                        definition.metadata.clone(),
                        definition.span,
                        None,
                    );
                }
                AstItemKind::Defn(function) => self.predeclare_function(function, false, None),
                AstItemKind::Defstruct(structure) => self.predeclare_struct(structure),
                AstItemKind::DefstaticSchema(schema) => {
                    let type_binding = BindingId::new(
                        &self.module_name,
                        &schema.name.canonical,
                        BindingKind::Type,
                    );
                    self.declare(
                        &schema.name,
                        BindingKind::Type,
                        Type::Nominal {
                            binding: type_binding.as_str().to_owned(),
                            args: Vec::new(),
                        },
                        schema.metadata.clone(),
                        schema.span,
                        None,
                    );
                }
                AstItemKind::Extern(extern_block) => {
                    for nested in &extern_block.items {
                        match &nested.kind {
                            AstItemKind::Defn(function) => self.predeclare_function(
                                function,
                                true,
                                Some(extern_block.module.as_str()),
                            ),
                            AstItemKind::Def(definition) => {
                                let ty = definition
                                    .type_annotation
                                    .as_ref()
                                    .map_or(Type::Any, type_from_ast);
                                self.declare(
                                    &definition.name,
                                    BindingKind::Value,
                                    ty,
                                    definition.metadata.clone(),
                                    definition.span,
                                    Some(RuntimeBinding {
                                        module: extern_block.module.clone(),
                                        name: python_identifier(&definition.name.canonical),
                                        python_module: true,
                                    }),
                                );
                            }
                            _ => self.error(
                                "OSR-H0001",
                                "extern currently accepts defn and def declarations",
                                nested.span,
                            ),
                        }
                    }
                }
                AstItemKind::Defmacro(macro_definition) => {
                    self.phase_one_names
                        .insert(macro_definition.name.canonical.clone());
                }
                AstItemKind::DefnForSyntax(function) => {
                    if let Some(name) = &function.name {
                        self.phase_one_names.insert(name.canonical.clone());
                    }
                }
                AstItemKind::ImportForSyntax(import) => {
                    let name = import.alias.as_ref().unwrap_or(&import.module);
                    self.phase_one_names.insert(name.canonical.clone());
                }
                AstItemKind::Export(_)
                | AstItemKind::Alias(_)
                | AstItemKind::PyDecorate(_)
                | AstItemKind::StaticRecord(_)
                | AstItemKind::Expr(_)
                | AstItemKind::Error(_) => {}
            }
        }
    }
}
