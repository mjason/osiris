use super::super::super::*;

impl<'a> Lowerer<'a> {
    /// Lower a callback submitted by `future`/`future-call` and preserve its
    /// result type inside the synthetic `Future[T]` nominal marker.
    pub(in crate::hir) fn lower_future_call(
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
                "OSR-T0035",
                "osiris.prelude/future-call* expects one zero-argument function",
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
        let result_type = match &function.ty {
            Type::Fn(signature) => {
                if !signature.parameters.is_empty() {
                    self.error(
                        "OSR-T0035",
                        "future-call function must accept no arguments",
                        function.span,
                    );
                }
                (*signature.return_type).clone()
            }
            Type::Any | Type::Unknown => Type::Any,
            _ => {
                self.error("OSR-T0035", "future-call expects a function", function.span);
                Type::Error
            }
        };
        let future_type = future_type(result_type);
        let binding = self.ensure_core_collection_binding("future_call", span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(vec![function.ty.clone()], future_type.clone())
                    .with_summaries(CallSummaries::unknown()),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: future_type,
            summaries: function.summaries.join(&CallSummaries::unknown()),
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![CallArgument::Positional(function)],
            },
        }
    }

    pub(in crate::hir) fn lower_future_predicate(
        &mut self,
        call: &ast::CallExpr,
        span: Span,
        scope: &mut Scope,
        runtime_name: &str,
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
                "OSR-T0036",
                format!("osiris.prelude/{runtime_name} expects one argument"),
                span,
            );
            return Expr::error(span);
        }
        let value = self.lower_expr(&call.positional[0], scope);
        let binding = self.ensure_core_collection_binding(runtime_name, span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(vec![value.ty.clone()], Type::Bool)
                    .with_summaries(CallSummaries::unknown()),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: Type::Bool,
            summaries: value.summaries.join(&CallSummaries::unknown()),
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![CallArgument::Positional(value)],
            },
        }
    }

    pub(in crate::hir) fn lower_promise(
        &mut self,
        call: &ast::CallExpr,
        span: Span,
        scope: &mut Scope,
    ) -> Expr {
        if !call.keywords.is_empty() || !call.positional.is_empty() {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0037",
                "osiris.prelude/promise* does not accept arguments",
                span,
            );
            return Expr::error(span);
        }
        let result_type = promise_type(self.types.fresh_var());
        let binding = self.ensure_core_collection_binding("promise", span);
        let callee = Expr::pure(
            span,
            Type::Fn(FunctionType::new(Vec::new(), result_type.clone())),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: result_type,
            summaries: CallSummaries::unknown(),
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: Vec::new(),
            },
        }
    }

    pub(in crate::hir) fn lower_deliver(
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
                "OSR-T0038",
                "osiris.prelude/deliver* expects a promise and a value",
                span,
            );
            return Expr::error(span);
        }
        let promise = self.lower_expr(&call.positional[0], scope);
        let value = self.lower_expr(&call.positional[1], scope);
        let result_type = match self.types.resolve(&promise.ty) {
            Type::Nominal { binding, args } if binding == core_promise_type_binding().as_str() => {
                let expected = args.first().cloned().unwrap_or(Type::Any);
                self.check_assignable(&value.ty, &expected, value.span);
                Type::Nominal {
                    binding: core_promise_type_binding().as_str().to_owned(),
                    args: vec![expected],
                }
            }
            Type::Any | Type::Unknown => promise_type(value.ty.clone()),
            _ => {
                self.error(
                    "OSR-T0038",
                    "deliver expects a Promise as its first argument",
                    promise.span,
                );
                promise_type(value.ty.clone())
            }
        };
        let binding = self.ensure_core_collection_binding("deliver", span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(
                    vec![promise.ty.clone(), value.ty.clone()],
                    result_type.clone(),
                )
                .with_summaries(CallSummaries::unknown()),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: result_type,
            summaries: promise
                .summaries
                .join(&value.summaries)
                .join(&CallSummaries::unknown()),
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![
                    CallArgument::Positional(promise),
                    CallArgument::Positional(value),
                ],
            },
        }
    }

    pub(in crate::hir) fn lower_lock(
        &mut self,
        call: &ast::CallExpr,
        span: Span,
        scope: &mut Scope,
    ) -> Expr {
        if !call.keywords.is_empty() || !call.positional.is_empty() {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0039",
                "osiris.prelude/lock* does not accept arguments",
                span,
            );
            return Expr::error(span);
        }
        let binding = self.ensure_core_collection_binding("lock", span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(Vec::new(), Type::Any).with_summaries(CallSummaries::unknown()),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: Type::Any,
            summaries: CallSummaries::unknown(),
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: Vec::new(),
            },
        }
    }

    pub(in crate::hir) fn lower_locking(
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
                "OSR-T0040",
                "osiris.prelude/locking* expects a lock and zero-argument function",
                span,
            );
            return Expr::error(span);
        }
        let lock = self.lower_expr(&call.positional[0], scope);
        let function = match &call.positional[1].kind {
            AstExprKind::Fn(function) => self.lower_lambda_with_expected_parameters(
                function,
                call.positional[1].span,
                scope,
                &[],
            ),
            _ => self.lower_expr(&call.positional[1], scope),
        };
        let result_type = match &function.ty {
            Type::Fn(signature) => {
                if !signature.parameters.is_empty() {
                    self.error(
                        "OSR-T0040",
                        "locking body function must accept no arguments",
                        function.span,
                    );
                }
                (*signature.return_type).clone()
            }
            Type::Any | Type::Unknown => Type::Any,
            _ => {
                self.error(
                    "OSR-T0040",
                    "locking expects a zero-argument function body",
                    function.span,
                );
                Type::Error
            }
        };
        let binding = self.ensure_core_collection_binding("locking", span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(
                    vec![lock.ty.clone(), function.ty.clone()],
                    result_type.clone(),
                )
                .with_summaries(CallSummaries::unknown()),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: result_type,
            summaries: lock
                .summaries
                .join(&function.summaries)
                .join(&CallSummaries::unknown()),
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![
                    CallArgument::Positional(lock),
                    CallArgument::Positional(function),
                ],
            },
        }
    }
}
