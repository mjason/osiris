use super::super::super::*;

impl<'a> Lowerer<'a> {
    pub(in crate::hir) fn lower_doseq(
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
                "OSR-T0027",
                "osiris.kernel/doseq* expects a callback and one collection",
                span,
            );
            return Expr::error(span);
        }

        let collection = self.lower_expr(&call.positional[1], scope);
        let item_type = indexed_type(&collection.ty);
        let function = match &call.positional[0].kind {
            AstExprKind::Fn(function) => self.lower_lambda_with_expected_parameters(
                function,
                call.positional[0].span,
                scope,
                std::slice::from_ref(&item_type),
            ),
            _ => self.lower_expr(&call.positional[0], scope),
        };
        let Type::Fn(signature) = &function.ty else {
            self.error(
                "OSR-T0027",
                "doseq callback must be a statically typed function",
                function.span,
            );
            return Expr::error(span);
        };
        if signature.parameters.len() != 1 {
            self.error(
                "OSR-T0027",
                "doseq callback must accept exactly one item",
                function.span,
            );
            return Expr::error(span);
        }
        self.check_assignable(&item_type, &signature.parameters[0], function.span);
        self.check_assignable(&signature.return_type, &Type::None, function.span);

        let callback_summaries = signature.summaries.clone();
        let mut summaries = collection
            .summaries
            .join(&function.summaries)
            .join(&callback_summaries);
        summaries.temporal = collection
            .summaries
            .temporal
            .compose(&callback_summaries.temporal);
        summaries.data = DataProperties::scalar();
        let binding = self.ensure_core_collection_binding("doseq", span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(vec![function.ty.clone(), collection.ty.clone()], Type::None)
                    .with_summaries(callback_summaries),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: Type::None,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![
                    CallArgument::Positional(function),
                    CallArgument::Positional(collection),
                ],
            },
        }
    }

    pub(in crate::hir) fn lower_for_stop(
        &mut self,
        call: &ast::CallExpr,
        span: Span,
        scope: &mut Scope,
    ) -> Expr {
        if !call.args.is_empty() {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0027",
                "osiris.kernel/for-stop* does not accept arguments",
                span,
            );
            return Expr::error(span);
        }
        let binding = self.ensure_core_collection_binding("for_stop", span);
        Expr {
            span,
            ty: Type::Never,
            summaries: CallSummaries::pure_scalar(),
            kind: ExprKind::Call {
                callee: Box::new(Expr::pure(
                    span,
                    Type::Fn(
                        FunctionType::new(Vec::new(), Type::Never)
                            .with_summaries(CallSummaries::pure_scalar()),
                    ),
                    ExprKind::Binding(binding),
                )),
                arguments: Vec::new(),
            },
        }
    }
}
