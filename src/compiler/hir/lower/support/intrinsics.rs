use super::super::super::*;

impl<'a> Lowerer<'a> {
    pub(in crate::hir) fn ensure_core_loop_binding(&mut self, span: Span) -> BindingId {
        if let Some(binding) = &self.core_loop_binding {
            return binding.clone();
        }
        let id = BindingId::new("osiris.prelude", "loop", BindingKind::Function);
        let signature =
            FunctionType::new(Vec::new(), Type::Any).with_summaries(CallSummaries::unknown());
        self.bindings.insert(
            id.clone(),
            Binding {
                name: BindingName {
                    id: id.clone(),
                    canonical: "osiris.prelude/loop*".to_owned(),
                    python: "_u0_osiris_loop".to_owned(),
                    kind: BindingKind::Function,
                    span,
                },
                source_spelling: "osiris.prelude/loop*".to_owned(),
                ty: Type::Fn(signature),
                runtime: Some(RuntimeBinding {
                    module: "osiris.prelude".to_owned(),
                    name: "loop".to_owned(),
                    python_module: false,
                }),
                public: false,
                metadata: Vec::new(),
            },
        );
        self.core_loop_binding = Some(id.clone());
        id
    }

    pub(in crate::hir) fn ensure_core_recur_binding(
        &mut self,
        span: Span,
        arity: usize,
    ) -> BindingId {
        if let Some(binding) = &self.core_recur_binding {
            return binding.clone();
        }
        let id = BindingId::new("osiris.prelude", "recur", BindingKind::Function);
        let signature = FunctionType::new(vec![Type::Any; arity], Type::Never)
            .with_summaries(CallSummaries::pure_scalar());
        self.bindings.insert(
            id.clone(),
            Binding {
                name: BindingName {
                    id: id.clone(),
                    canonical: "osiris.prelude/recur*".to_owned(),
                    python: "_u0_osiris_recur".to_owned(),
                    kind: BindingKind::Function,
                    span,
                },
                source_spelling: "osiris.prelude/recur*".to_owned(),
                ty: Type::Fn(signature),
                runtime: Some(RuntimeBinding {
                    module: "osiris.prelude".to_owned(),
                    name: "recur".to_owned(),
                    python_module: false,
                }),
                public: false,
                metadata: Vec::new(),
            },
        );
        self.core_recur_binding = Some(id.clone());
        id
    }

    pub(in crate::hir) fn ensure_core_collection_binding(
        &mut self,
        name: &str,
        span: Span,
    ) -> BindingId {
        if let Some(binding) = self.core_collection_bindings.get(name) {
            return binding.clone();
        }
        let id = BindingId::new("osiris.prelude", name, BindingKind::Function);
        let signature =
            FunctionType::new(Vec::new(), Type::Any).with_summaries(CallSummaries::unknown());
        self.bindings.insert(
            id.clone(),
            Binding {
                name: BindingName {
                    id: id.clone(),
                    canonical: format!("osiris.prelude/{name}"),
                    python: format!("_u0_osiris_{name}"),
                    kind: BindingKind::Function,
                    span,
                },
                source_spelling: format!("osiris.prelude/{name}"),
                ty: Type::Fn(signature),
                runtime: Some(RuntimeBinding {
                    module: "osiris.prelude".to_owned(),
                    name: name.to_owned(),
                    python_module: false,
                }),
                public: false,
                metadata: Vec::new(),
            },
        );
        self.core_collection_bindings
            .insert(name.to_owned(), id.clone());
        id
    }

    pub(in crate::hir) fn install_core_reduced_type(&mut self, span: Span) {
        let id = core_reduced_type_binding();
        self.bindings.insert(
            id.clone(),
            Binding {
                name: BindingName {
                    id: id.clone(),
                    canonical: "Reduced".to_owned(),
                    python: "_u0_osiris_Reduced".to_owned(),
                    kind: BindingKind::Type,
                    span,
                },
                source_spelling: "Reduced".to_owned(),
                ty: Type::Nominal {
                    binding: id.as_str().to_owned(),
                    args: Vec::new(),
                },
                runtime: Some(RuntimeBinding {
                    module: "osiris.prelude".to_owned(),
                    name: "Reduced".to_owned(),
                    python_module: false,
                }),
                public: false,
                metadata: Vec::new(),
            },
        );
    }

    pub(in crate::hir) fn install_core_delay_type(&mut self, span: Span) {
        let id = core_delay_type_binding();
        self.bindings.insert(
            id.clone(),
            Binding {
                name: BindingName {
                    id: id.clone(),
                    canonical: "Delay".to_owned(),
                    python: "_u0_osiris_Delay".to_owned(),
                    kind: BindingKind::Type,
                    span,
                },
                source_spelling: "Delay".to_owned(),
                ty: Type::Nominal {
                    binding: id.as_str().to_owned(),
                    args: Vec::new(),
                },
                runtime: Some(RuntimeBinding {
                    module: "osiris.prelude".to_owned(),
                    name: "Delay".to_owned(),
                    python_module: false,
                }),
                public: false,
                metadata: Vec::new(),
            },
        );
    }

    pub(in crate::hir) fn install_core_future_type(&mut self, span: Span) {
        let id = core_future_type_binding();
        self.bindings.insert(
            id.clone(),
            Binding {
                name: BindingName {
                    id: id.clone(),
                    canonical: "Future".to_owned(),
                    python: "_u0_osiris_Future".to_owned(),
                    kind: BindingKind::Type,
                    span,
                },
                source_spelling: "Future".to_owned(),
                ty: Type::Nominal {
                    binding: id.as_str().to_owned(),
                    args: Vec::new(),
                },
                runtime: Some(RuntimeBinding {
                    module: "osiris.prelude".to_owned(),
                    name: "Future".to_owned(),
                    python_module: false,
                }),
                public: false,
                metadata: Vec::new(),
            },
        );
    }

    pub(in crate::hir) fn install_core_promise_type(&mut self, span: Span) {
        let id = core_promise_type_binding();
        self.bindings.insert(
            id.clone(),
            Binding {
                name: BindingName {
                    id: id.clone(),
                    canonical: "Promise".to_owned(),
                    python: "_u0_osiris_Promise".to_owned(),
                    kind: BindingKind::Type,
                    span,
                },
                source_spelling: "Promise".to_owned(),
                ty: Type::Nominal {
                    binding: id.as_str().to_owned(),
                    args: Vec::new(),
                },
                runtime: Some(RuntimeBinding {
                    module: "osiris.prelude".to_owned(),
                    name: "Promise".to_owned(),
                    python_module: false,
                }),
                public: false,
                metadata: Vec::new(),
            },
        );
    }

    pub(in crate::hir) fn ensure_core_mapv_binding(&mut self, span: Span) -> BindingId {
        if let Some(binding) = &self.core_mapv_binding {
            return binding.clone();
        }
        let id = BindingId::new("osiris.prelude", "mapv", BindingKind::Function);
        let signature = FunctionType::new(vec![Type::Any, Type::Any], Type::Any)
            .with_summaries(CallSummaries::unknown());
        self.bindings.insert(
            id.clone(),
            Binding {
                name: BindingName {
                    id: id.clone(),
                    canonical: "osiris.prelude/mapv".to_owned(),
                    // This internal name cannot be produced by authored source.
                    python: "_u0_osiris_mapv".to_owned(),
                    kind: BindingKind::Function,
                    span,
                },
                source_spelling: "osiris.prelude/mapv".to_owned(),
                ty: Type::Fn(signature),
                runtime: Some(RuntimeBinding {
                    module: "osiris.prelude".to_owned(),
                    name: "mapv".to_owned(),
                    python_module: false,
                }),
                public: false,
                metadata: Vec::new(),
            },
        );
        self.core_mapv_binding = Some(id.clone());
        id
    }

    pub(in crate::hir) fn lower_abs(
        &mut self,
        operand: &ast::Expr,
        span: Span,
        scope: &mut Scope,
    ) -> Expr {
        let operand = self.lower_expr(operand, scope);
        let selection =
            self.select_operator(ScalarOperator::Abs, std::slice::from_ref(&operand.ty));
        let choice = match selection {
            OperatorSelection::Selected(choice) => *choice,
            OperatorSelection::Ambiguous => {
                self.error(
                    "OSR-T0008",
                    "operator `abs` has multiple static implementations",
                    span,
                );
                return Expr::error(span);
            }
            OperatorSelection::None => {
                self.error(
                    "OSR-T0007",
                    format!("operator `abs` is not defined for `{}`", operand.ty),
                    span,
                );
                return Expr::error(span);
            }
        };
        if choice.binding.is_some() {
            return self.apply_operator_choice(Operator::Positive, vec![operand], span, choice);
        }

        // Core `abs` has no HIR operator variant because the backend's
        // exhaustive operator lowering intentionally remains unchanged.  A
        // synthetic binding makes the Python target a normal `builtins.abs`
        // call while retaining the same static signature and summaries.
        let binding = self.ensure_core_abs_binding(span);
        let callee_type = Type::Fn(
            FunctionType::new(vec![operand.ty.clone()], choice.result.clone())
                .with_summaries(choice.summaries.clone()),
        );
        let callee = Expr::pure(span, callee_type, ExprKind::Binding(binding));
        let summaries = operand.summaries.join(&choice.summaries);
        Expr {
            span,
            ty: choice.result,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![CallArgument::Positional(operand)],
            },
        }
    }

    pub(in crate::hir) fn ensure_core_abs_binding(&mut self, span: Span) -> BindingId {
        if let Some(binding) = &self.core_abs_binding {
            return binding.clone();
        }
        let id = BindingId::new(&self.module_name, "__osiris_abs", BindingKind::Function);
        let name = BindingName {
            id: id.clone(),
            canonical: "__osiris_abs".to_owned(),
            python: "__osiris_abs".to_owned(),
            kind: BindingKind::Function,
            span,
        };
        self.bindings.insert(
            id.clone(),
            Binding {
                name,
                source_spelling: "abs".to_owned(),
                ty: Type::Fn(
                    FunctionType::new(vec![Type::Any], Type::Any)
                        .with_summaries(CallSummaries::pure_scalar()),
                ),
                runtime: Some(RuntimeBinding {
                    module: "builtins".to_owned(),
                    name: "abs".to_owned(),
                    python_module: false,
                }),
                public: false,
                metadata: Vec::new(),
            },
        );
        self.core_abs_binding = Some(id.clone());
        id
    }
}
