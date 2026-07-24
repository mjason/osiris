use super::super::super::*;

impl<'a> Lowerer<'a> {
    pub(in crate::hir) fn lower_close(
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
                "OSR-T0031",
                "osiris.kernel/close* expects exactly one positional argument",
                span,
            );
            return Expr::error(span);
        }
        let value = self.lower_expr(&call.positional[0], scope);
        let binding = self.ensure_core_collection_binding("close", span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(vec![value.ty.clone()], Type::None)
                    .with_summaries(CallSummaries::unknown()),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: Type::None,
            summaries: value.summaries.join(&CallSummaries::unknown()),
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![CallArgument::Positional(value)],
            },
        }
    }

    /// Lower the compiler-owned target of the `letfn` surface macro.  All
    /// names are installed before any lambda body is lowered, which is the
    /// essential difference from an ordinary sequential `let`: self- and
    /// mutually-recursive local functions resolve to the same lexical frame.
    /// The resulting expression deliberately reuses `ExprKind::Let`; the
    /// backend already emits nested helper definitions before their binding
    /// assignments, preserving Python closure cells without a new runtime ABI.
    pub(in crate::hir) fn lower_letfn(
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
                "OSR-T0032",
                "osiris.kernel/letfn* expects a binding vector and body",
                span,
            );
            return Expr::error(span);
        }

        let entries = match &call.positional[0].kind {
            AstExprKind::Vector(entries) => entries,
            _ => {
                self.error(
                    "OSR-T0032",
                    "osiris.kernel/letfn* expects a binding vector",
                    call.positional[0].span,
                );
                return Expr::error(span);
            }
        };
        if entries.len() % 2 != 0 {
            self.error(
                "OSR-T0032",
                "letfn bindings require name/function pairs",
                call.positional[0].span,
            );
        }

        // Keep the frame alive while lowering every function and the body.
        scope.push();
        let mut pending = Vec::new();
        for pair in entries.chunks(2) {
            let Some(name_expression) = pair.first() else {
                continue;
            };
            let Some(value_expression) = pair.get(1) else {
                continue;
            };
            let AstExprKind::Name(name) = &name_expression.kind else {
                self.error(
                    "OSR-T0032",
                    "letfn binding names must be symbols",
                    name_expression.span,
                );
                continue;
            };
            if !matches!(value_expression.kind, AstExprKind::Fn(_)) {
                self.error(
                    "OSR-T0032",
                    "letfn binding values must be fn expressions (multi-arity forms are not supported)",
                    value_expression.span,
                );
            }
            let binding = self.declare_local(
                name,
                BindingKind::Value,
                Type::Any,
                name_expression.metadata.clone(),
                name_expression.span,
                scope,
            );
            let expected_parameters = match &value_expression.kind {
                AstExprKind::Fn(function) => {
                    let mut parameters = Vec::with_capacity(function.params.len());
                    for parameter in &function.params {
                        let ty = match parameter.type_annotation.as_ref() {
                            Some(ty) => self.resolve_type_expr(ty),
                            None => self.types.fresh_var(),
                        };
                        parameters.push(ty);
                    }
                    let return_type = match function.return_type.as_ref() {
                        Some(ty) => self.resolve_type_expr(ty),
                        None => self.types.fresh_var(),
                    };
                    self.set_binding_type(
                        &binding,
                        Type::Fn(FunctionType::new(parameters.clone(), return_type)),
                    );
                    Some(parameters)
                }
                _ => None,
            };
            pending.push((binding, value_expression, expected_parameters));
        }

        let mut lowered_bindings = Vec::new();
        for (binding, value_expression, expected_parameters) in pending {
            let value = match (&value_expression.kind, expected_parameters.as_deref()) {
                (AstExprKind::Fn(function), Some(expected)) => self
                    .lower_lambda_with_expected_parameters(
                        function,
                        value_expression.span,
                        scope,
                        expected,
                    ),
                _ => self.lower_expr(value_expression, scope),
            };
            self.set_binding_type(&binding, value.ty.clone());
            lowered_bindings.push(LetBinding { binding, value });
        }
        let body = self.lower_expr(&call.positional[1], scope);
        scope.pop();

        let summaries = lowered_bindings
            .iter()
            .fold(body.summaries.clone(), |summary, binding| {
                summary.join(&binding.value.summaries)
            });
        Expr {
            span,
            ty: body.ty.clone(),
            summaries,
            kind: ExprKind::Let {
                bindings: lowered_bindings,
                body: Box::new(body),
            },
        }
    }
}
