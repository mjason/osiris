use super::super::super::*;

impl<'a> Lowerer<'a> {
    pub(in crate::hir) fn lower_reduce(
        &mut self,
        call: &ast::CallExpr,
        span: Span,
        scope: &mut Scope,
        named_fold: bool,
    ) -> Expr {
        let valid = if named_fold {
            call.positional.len() == 3
        } else {
            (2..=3).contains(&call.positional.len())
        };
        if !call.keywords.is_empty() || !valid {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0020",
                if named_fold {
                    "osiris.prelude/fold expects a function, initial value, and collection"
                } else {
                    "osiris.prelude/reduce expects a function, collection, and optional initial value"
                },
                span,
            );
            return Expr::error(span);
        }
        let collection = self.lower_expr(
            call.positional
                .last()
                .expect("validated collection argument"),
            scope,
        );
        let item_type = indexed_type(&collection.ty);
        let initial = if named_fold || call.positional.len() == 3 {
            Some(self.lower_expr(&call.positional[1], scope))
        } else {
            None
        };
        let accumulator_type = initial
            .as_ref()
            .map(|initial| initial.ty.clone())
            .unwrap_or_else(|| item_type.clone());
        let expected = [accumulator_type.clone(), item_type.clone()];
        let function = match &call.positional[0].kind {
            AstExprKind::Fn(function) => self.lower_lambda_with_expected_parameters(
                function,
                call.positional[0].span,
                scope,
                &expected,
            ),
            _ => self.lower_expr(&call.positional[0], scope),
        };
        let Type::Fn(signature) = &function.ty else {
            self.error(
                "OSR-T0020",
                "reduce callback must be a statically typed function",
                function.span,
            );
            return Expr::error(span);
        };
        if signature.parameters.len() != 2 {
            self.error(
                "OSR-T0020",
                "reduce callback must accept accumulator and item",
                function.span,
            );
            return Expr::error(span);
        }
        for (actual, expected) in expected.iter().zip(&signature.parameters) {
            self.check_assignable(actual, expected, function.span);
        }
        let callback_summaries = signature.summaries.clone();
        let callback_return = (*signature.return_type).clone();
        let callback_accumulator = unreduced_type(&callback_return);
        self.check_assignable(&callback_accumulator, &accumulator_type, function.span);
        let result_type = accumulator_type.clone();
        let mut summaries = collection
            .summaries
            .join(&function.summaries)
            .join(&callback_summaries);
        if let Some(initial) = &initial {
            summaries = summaries.join(&initial.summaries);
        }
        summaries.data = DataProperties::scalar();
        let runtime_name = if named_fold { "fold" } else { "reduce" };
        let binding = self.ensure_core_collection_binding(runtime_name, span);
        let mut parameter_types = vec![function.ty.clone()];
        if let Some(initial) = &initial {
            parameter_types.push(initial.ty.clone());
        }
        parameter_types.push(collection.ty.clone());
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(parameter_types, result_type.clone())
                    .with_summaries(callback_summaries),
            ),
            ExprKind::Binding(binding),
        );
        let mut arguments = vec![CallArgument::Positional(function)];
        if let Some(initial) = initial {
            arguments.push(CallArgument::Positional(initial));
        }
        arguments.push(CallArgument::Positional(collection));
        Expr {
            span,
            ty: result_type,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments,
            },
        }
    }

    pub(in crate::hir) fn lower_reduced_operation(
        &mut self,
        call: &ast::CallExpr,
        span: Span,
        scope: &mut Scope,
        operation: ReducedOperation,
    ) -> Expr {
        if !call.keywords.is_empty() || call.positional.len() != 1 {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            let source_name = match operation {
                ReducedOperation::Wrap => "reduced",
                ReducedOperation::Predicate => "reduced?",
                ReducedOperation::Unwrap => "unreduced",
            };
            self.error(
                "OSR-T0020",
                format!("osiris.prelude/{source_name} expects exactly one argument"),
                span,
            );
            return Expr::error(span);
        }

        let value = self.lower_expr(&call.positional[0], scope);
        let result_type = match operation {
            ReducedOperation::Wrap => reduced_type(value.ty.clone()),
            ReducedOperation::Predicate => Type::Bool,
            ReducedOperation::Unwrap => unreduced_type(&value.ty),
        };
        let binding = self.ensure_core_collection_binding(operation.runtime_name(), span);
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
}
