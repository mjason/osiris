use super::super::super::*;

impl<'a> Lowerer<'a> {
    /// Lower the Clojure-style `time` macro target while preserving the
    /// measured expression's type and semantic summaries.
    pub(in crate::hir) fn lower_time(
        &mut self,
        call: &ast::CallExpr,
        span: Span,
        scope: &mut Scope,
    ) -> Expr {
        if !call.keywords.is_empty() || call.positional.len() != 1 {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0043",
                "osiris.prelude/time* expects one zero-argument function",
                span,
            );
            return Expr::error(span);
        }

        let function = match &call.positional[0].kind {
            AstExprKind::Fn(function) => self.lower_lambda_with_expected_parameters(
                function,
                call.positional[0].span,
                scope,
                &[],
            ),
            _ => self.lower_expr(&call.positional[0], scope),
        };
        let (result_type, body_summaries) = match &function.ty {
            Type::Fn(signature) => {
                if !signature.parameters.is_empty() {
                    self.error(
                        "OSR-T0043",
                        "time body function must accept no arguments",
                        function.span,
                    );
                }
                (
                    (*signature.return_type).clone(),
                    signature.summaries.clone(),
                )
            }
            Type::Any | Type::Unknown => (Type::Any, CallSummaries::unknown()),
            _ => {
                self.error(
                    "OSR-T0043",
                    "time expects a zero-argument function body",
                    function.span,
                );
                (Type::Error, CallSummaries::unknown())
            }
        };

        let mut summaries = function.summaries.join(&body_summaries);
        summaries.effects = summaries.effects.union(&EffectRow::singleton(Effect::Io));
        summaries.data = body_summaries.data.clone();
        let binding = self.ensure_core_collection_binding("time_value", span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(vec![function.ty.clone()], result_type.clone())
                    .with_summaries(summaries.clone()),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: result_type,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![CallArgument::Positional(function)],
            },
        }
    }

    pub(in crate::hir) fn lower_realized(
        &mut self,
        call: &ast::CallExpr,
        span: Span,
        scope: &mut Scope,
    ) -> Expr {
        if !call.keywords.is_empty() || call.positional.len() != 1 {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0030",
                "osiris.prelude/realized* expects exactly one positional argument",
                span,
            );
            return Expr::error(span);
        }
        let value = self.lower_expr(&call.positional[0], scope);
        let binding = self.ensure_core_collection_binding("realized", span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(vec![value.ty.clone()], Type::Bool)
                    .with_summaries(CallSummaries::pure_scalar()),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: Type::Bool,
            summaries: value.summaries.clone(),
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![CallArgument::Positional(value)],
            },
        }
    }

    pub(in crate::hir) fn lower_dynamic_binding(
        &mut self,
        call: &ast::CallExpr,
        span: Span,
        scope: &mut Scope,
    ) -> Expr {
        if !call.keywords.is_empty() || call.positional.len() != 2 {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0042",
                "osiris.prelude/binding* expects a binding vector and zero-argument body",
                span,
            );
            return Expr::error(span);
        }

        let AstExprKind::Vector(entries) = &call.positional[0].kind else {
            let _ = self.lower_expr(&call.positional[0], scope);
            let _ = self.lower_expr(&call.positional[1], scope);
            self.error(
                "OSR-T0042",
                "osiris.prelude/binding* expects a binding vector",
                call.positional[0].span,
            );
            return Expr::error(span);
        };
        if entries.len() % 2 != 0 {
            for entry in entries {
                let _ = self.lower_expr(entry, scope);
            }
            let _ = self.lower_expr(&call.positional[1], scope);
            self.error(
                "OSR-T0042",
                "binding requires dynamic Var/value pairs",
                call.positional[0].span,
            );
            return Expr::error(span);
        }

        // Binding values are simultaneous: evaluate every initializer once,
        // left-to-right, before installing any of the new dynamic values.
        scope.push();
        let mut initializers = Vec::new();
        let mut binding_ids = Vec::new();
        let mut values = Vec::new();
        let mut seen = BTreeSet::new();
        for pair in entries.chunks_exact(2) {
            let target_expression = &pair[0];
            let value = self.lower_expr(&pair[1], scope);
            let temporary_name = format!("\0dynamic-value{}", self.next_scope);
            let temporary_name = Name {
                spelling: temporary_name.clone(),
                canonical: temporary_name,
            };
            let temporary = self.declare_local(
                &temporary_name,
                BindingKind::Value,
                value.ty.clone(),
                Vec::new(),
                pair[1].span,
                scope,
            );
            let value_reference = Expr::pure(
                pair[1].span,
                value.ty.clone(),
                ExprKind::Binding(temporary.clone()),
            );
            initializers.push(LetBinding {
                binding: temporary,
                value,
            });

            let AstExprKind::Name(target_name) = &target_expression.kind else {
                self.error(
                    "OSR-T0042",
                    "binding targets must be dynamic top-level Value symbols",
                    target_expression.span,
                );
                continue;
            };
            if scope.resolve(&target_name.canonical).is_some() {
                self.error(
                    "OSR-T0042",
                    format!(
                        "binding target `{}` resolves to a local value, not a dynamic Var",
                        target_name.spelling
                    ),
                    target_expression.span,
                );
                continue;
            }
            let Some(target) = self.resolve_alias_target(&target_name.canonical) else {
                self.error(
                    "OSR-T0042",
                    format!("unknown binding target `{}`", target_name.spelling),
                    target_expression.span,
                );
                continue;
            };
            let Some(target_binding) = self.bindings.get(&target).cloned() else {
                continue;
            };
            if target_binding.name.kind != BindingKind::Value
                || !metadata_flag(&target_binding.metadata, "dynamic")
            {
                self.error(
                    "OSR-T0042",
                    format!(
                        "binding target `{}` is not a `^:dynamic` top-level Value",
                        target_name.spelling
                    ),
                    target_expression.span,
                );
                continue;
            }
            if !seen.insert(target.clone()) {
                self.error(
                    "OSR-T0042",
                    format!(
                        "dynamic Var `{}` is bound more than once",
                        target_name.spelling
                    ),
                    target_expression.span,
                );
                continue;
            }
            self.check_assignable(
                &value_reference.ty,
                &self.binding_type(&target),
                pair[1].span,
            );
            binding_ids.push(Expr::pure(
                target_expression.span,
                Type::Str,
                ExprKind::String(target.as_str().to_owned()),
            ));
            values.push(value_reference);
        }

        let function = match &call.positional[1].kind {
            AstExprKind::Fn(function) => self.lower_lambda_with_expected_parameters(
                function,
                call.positional[1].span,
                scope,
                &[],
            ),
            _ => self.lower_expr(&call.positional[1], scope),
        };
        scope.pop();

        let (result_type, body_summaries) = match &function.ty {
            Type::Fn(signature) => {
                if !signature.parameters.is_empty() {
                    self.error(
                        "OSR-T0042",
                        "binding body function must accept no arguments",
                        function.span,
                    );
                }
                (
                    (*signature.return_type).clone(),
                    signature.summaries.clone(),
                )
            }
            Type::Any | Type::Unknown => (Type::Any, CallSummaries::unknown()),
            _ => {
                self.error(
                    "OSR-T0042",
                    "binding expects a zero-argument function body",
                    function.span,
                );
                (Type::Error, CallSummaries::unknown())
            }
        };

        let id_vector = Expr::pure(
            call.positional[0].span,
            Type::Vector(Box::new(Type::Str)),
            ExprKind::Vector(binding_ids),
        );
        let value_item = self.types.join_all(values.iter().map(|value| &value.ty));
        let value_vector = Expr::pure(
            call.positional[0].span,
            Type::Vector(Box::new(if value_item == Type::Never {
                Type::Any
            } else {
                value_item
            })),
            ExprKind::Vector(values),
        );
        let mut summaries = function
            .summaries
            .join(&body_summaries)
            .join(&dynamic_state_summaries());
        summaries.data = body_summaries.data.clone();
        let runtime = self.ensure_core_collection_binding("binding_values", span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(
                    vec![
                        id_vector.ty.clone(),
                        value_vector.ty.clone(),
                        function.ty.clone(),
                    ],
                    result_type.clone(),
                )
                .with_summaries(summaries.clone()),
            ),
            ExprKind::Binding(runtime),
        );
        let call = Expr {
            span,
            ty: result_type,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![
                    CallArgument::Positional(id_vector),
                    CallArgument::Positional(value_vector),
                    CallArgument::Positional(function),
                ],
            },
        };
        self.wrap_let_bindings(initializers, call, span)
    }
}
