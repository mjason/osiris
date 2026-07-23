use super::super::super::*;

impl<'a> Lowerer<'a> {
    pub(in crate::hir) fn lower_lazy_seq(
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
                "OSR-T0025",
                "osiris.prelude/lazy-seq* expects one zero-argument function",
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
        let Type::Fn(signature) = &function.ty else {
            self.error(
                "OSR-T0025",
                "lazy-seq thunk must be a function",
                function.span,
            );
            return Expr::error(span);
        };
        if !signature.parameters.is_empty() {
            self.error(
                "OSR-T0025",
                "lazy-seq thunk must accept no arguments",
                function.span,
            );
        }
        let binding = self.ensure_core_collection_binding("lazy_seq", span);
        let callee = Expr::pure(
            span,
            Type::Fn(FunctionType::new(vec![function.ty.clone()], Type::Any)),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: Type::Any,
            summaries: function.summaries.clone(),
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![CallArgument::Positional(function)],
            },
        }
    }

    /// Lower the Clojure-style `delay` macro target.  The thunk is lowered as
    /// an ordinary zero-argument lambda, while the result carries a nominal
    /// `Delay[T]` marker so `force` can recover the delayed value type without
    /// making every delayed expression an `Any` boundary.
    pub(in crate::hir) fn lower_delay(
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
                "OSR-T0028",
                "osiris.prelude/delay* expects one zero-argument function",
                span,
            );
            return Expr::error(span);
        }
        let thunk = match &call.positional[0].kind {
            AstExprKind::Fn(function) => self.lower_lambda_with_expected_parameters(
                function,
                call.positional[0].span,
                scope,
                &[],
            ),
            _ => self.lower_expr(&call.positional[0], scope),
        };
        let Type::Fn(signature) = &thunk.ty else {
            self.error("OSR-T0028", "delay thunk must be a function", thunk.span);
            return Expr::error(span);
        };
        if !signature.parameters.is_empty() {
            self.error(
                "OSR-T0028",
                "delay thunk must accept no arguments",
                thunk.span,
            );
        }
        let value_type = (*signature.return_type).clone();
        let result_type = Type::Nominal {
            binding: core_delay_type_binding().as_str().to_owned(),
            args: vec![value_type],
        };
        let binding = self.ensure_core_collection_binding("delay", span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(vec![thunk.ty.clone()], result_type.clone())
                    .with_summaries(thunk.summaries.clone()),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: result_type,
            summaries: thunk.summaries.clone(),
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![CallArgument::Positional(thunk)],
            },
        }
    }

    /// `force`/`deref` accepts a Delay[T] and returns T.  For ordinary values
    /// the runtime helper is intentionally an identity function, matching
    /// Clojure's useful idempotent `deref` boundary for extension values.
    pub(in crate::hir) fn lower_force(
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
                "OSR-T0029",
                "osiris.prelude/force* expects exactly one positional argument",
                span,
            );
            return Expr::error(span);
        }
        let value = self.lower_expr(&call.positional[0], scope);
        let result_type = match &value.ty {
            Type::Nominal { binding, args }
                if binding == core_delay_type_binding().as_str() && args.len() == 1 =>
            {
                args[0].clone()
            }
            Type::Unknown => Type::Any,
            other => other.clone(),
        };
        let binding = self.ensure_core_collection_binding("force", span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(vec![value.ty.clone()], result_type.clone())
                    .with_summaries(CallSummaries::pure_scalar()),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: result_type,
            summaries: value.summaries.clone(),
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![CallArgument::Positional(value)],
            },
        }
    }

    /// `deref` is the blocking boundary for delays, promises, and futures.
    /// Clojure's optional timeout/default pair is kept as ordinary arguments
    /// so the Python runtime can implement the wait without another AST node.
    pub(in crate::hir) fn lower_deref(
        &mut self,
        call: &ast::CallExpr,
        span: Span,
        scope: &mut Scope,
    ) -> Expr {
        if !call.keywords.is_empty() || !(call.positional.len() == 1 || call.positional.len() == 3)
        {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0034",
                "osiris.prelude/deref* expects one argument or value/timeout/default",
                span,
            );
            return Expr::error(span);
        }
        let values = call
            .positional
            .iter()
            .map(|value| self.lower_expr(value, scope))
            .collect::<Vec<_>>();
        if values.len() == 3
            && !matches!(
                values[1].ty,
                Type::Int | Type::Float | Type::Any | Type::Unknown
            )
        {
            self.error(
                "OSR-T0034",
                "deref timeout must be an Int or Float number of milliseconds",
                values[1].span,
            );
        }
        let mut result_type = async_value_type(&values[0].ty);
        if let Some(default) = values.get(2) {
            // A freshly-created Promise carries a type variable.  A concrete
            // timeout default is useful evidence for that variable (and keeps
            // the generated function annotation precise); otherwise preserve
            // the ordinary union behavior for known asynchronous values.
            if contains_type_variable(&result_type) {
                if self.types.unify(&result_type, &default.ty).is_ok() {
                    result_type = self.types.resolve(&result_type);
                } else {
                    result_type = self.types.join(&result_type, &default.ty);
                }
            } else {
                result_type = self.types.join(&result_type, &default.ty);
            }
        }
        let summaries = values
            .iter()
            .fold(CallSummaries::unknown(), |summary, value| {
                summary.join(&value.summaries)
            });
        let binding = self.ensure_core_collection_binding("deref", span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(
                    values.iter().map(|value| value.ty.clone()).collect(),
                    result_type.clone(),
                )
                .with_summaries(CallSummaries::unknown()),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: result_type,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: values.into_iter().map(CallArgument::Positional).collect(),
            },
        }
    }
}
