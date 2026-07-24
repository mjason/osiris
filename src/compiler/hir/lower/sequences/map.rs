use super::super::super::*;

impl<'a> Lowerer<'a> {
    pub(in crate::hir) fn lower_mapv(
        &mut self,
        call: &ast::CallExpr,
        span: Span,
        scope: &mut Scope,
    ) -> Expr {
        if !call.keywords.is_empty() || call.positional.len() < 2 {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0020",
                "osiris.kernel/mapv expects a function and at least one collection",
                span,
            );
            return Expr::error(span);
        }

        let collections = call.positional[1..]
            .iter()
            .map(|collection| self.lower_expr(collection, scope))
            .collect::<Vec<_>>();
        let item_types = collections
            .iter()
            .map(|collection| indexed_type(&collection.ty))
            .collect::<Vec<_>>();
        let mut function = match &call.positional[0].kind {
            AstExprKind::Fn(function) => self.lower_lambda_with_expected_parameters(
                function,
                call.positional[0].span,
                scope,
                &item_types,
            ),
            _ => self.lower_expr(&call.positional[0], scope),
        };
        let Type::Fn(signature) = &function.ty else {
            self.error(
                "OSR-T0020",
                "first mapv argument must be a statically typed function",
                function.span,
            );
            return Expr::error(span);
        };
        if signature.parameters.len() != item_types.len() {
            self.error(
                "OSR-T0020",
                format!(
                    "mapv function must accept exactly {} argument(s)",
                    item_types.len()
                ),
                function.span,
            );
            return Expr::error(span);
        }
        let callback_summaries = signature.summaries.clone();
        let callback_return = (*signature.return_type).clone();
        for (item_type, parameter) in item_types.iter().zip(&signature.parameters) {
            self.check_assignable(item_type, parameter, function.span);
        }

        if let ExprKind::Lambda { parameters, .. } = &mut function.kind {
            for parameter in parameters {
                parameter.ty = self.types.resolve(&parameter.ty);
                self.set_binding_type(&parameter.binding, parameter.ty.clone());
            }
        }
        function.ty = self.types.resolve(&function.ty);
        let result_type = Type::Vector(Box::new(self.types.resolve(&callback_return)));
        let temporal = collections
            .iter()
            .fold(
                crate::types::TemporalSummary::pointwise(),
                |summary, collection| summary.compose(&collection.summaries.temporal),
            )
            .compose(&callback_summaries.temporal);
        let mut summaries = collections
            .iter()
            .fold(function.summaries.clone(), |summary, collection| {
                summary.join(&collection.summaries)
            })
            .join(&callback_summaries);
        summaries.temporal = temporal;
        summaries.data = DataProperties {
            alignment: Alignment::Positional,
            preserves_length: (collections.len() == 1).then_some(true),
            materializes: Some(true),
            reshapes: Some(false),
            ..DataProperties::unknown()
        };

        let binding = self.ensure_core_mapv_binding(span);
        let callee_type = Type::Fn(
            FunctionType::new(
                std::iter::once(function.ty.clone())
                    .chain(collections.iter().map(|collection| collection.ty.clone()))
                    .collect(),
                result_type.clone(),
            )
            .with_summaries(callback_summaries),
        );
        let callee = Expr::pure(span, callee_type, ExprKind::Binding(binding));
        Expr {
            span,
            ty: result_type,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: std::iter::once(CallArgument::Positional(function))
                    .chain(collections.into_iter().map(CallArgument::Positional))
                    .collect(),
            },
        }
    }

    pub(in crate::hir) fn lower_map_like(
        &mut self,
        call: &ast::CallExpr,
        span: Span,
        scope: &mut Scope,
        operation: CollectionOperation,
    ) -> Expr {
        if !call.keywords.is_empty() || call.positional.len() < 2 {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0020",
                format!(
                    "osiris.kernel/{} expects a function and at least one collection",
                    operation.runtime_name()
                ),
                span,
            );
            return Expr::error(span);
        }

        let collections = call.positional[1..]
            .iter()
            .map(|collection| self.lower_expr(collection, scope))
            .collect::<Vec<_>>();
        if matches!(
            operation,
            CollectionOperation::Filter | CollectionOperation::Filterv
        ) && collections.len() != 1
        {
            self.error(
                "OSR-T0020",
                format!(
                    "osiris.kernel/{} accepts exactly one collection",
                    operation.runtime_name()
                ),
                span,
            );
            return Expr::error(span);
        }
        let expected_parameters = collections
            .iter()
            .map(|collection| indexed_type(&collection.ty))
            .collect::<Vec<_>>();
        let function = match &call.positional[0].kind {
            AstExprKind::Fn(function) => self.lower_lambda_with_expected_parameters(
                function,
                call.positional[0].span,
                scope,
                &expected_parameters,
            ),
            _ => self.lower_expr(&call.positional[0], scope),
        };
        let Type::Fn(signature) = &function.ty else {
            self.error(
                "OSR-T0020",
                format!(
                    "first {} argument must be a statically typed function",
                    operation.runtime_name()
                ),
                function.span,
            );
            return Expr::error(span);
        };
        if signature.parameters.len() != expected_parameters.len() {
            self.error(
                "OSR-T0020",
                format!(
                    "{} callback expects {} arguments, found {}",
                    operation.runtime_name(),
                    expected_parameters.len(),
                    signature.parameters.len()
                ),
                function.span,
            );
            return Expr::error(span);
        }
        for (actual, expected) in expected_parameters.iter().zip(&signature.parameters) {
            self.check_assignable(actual, expected, function.span);
        }
        if matches!(
            operation,
            CollectionOperation::Filter | CollectionOperation::Filterv
        ) {
            self.check_assignable(&signature.return_type, &Type::Bool, function.span);
        }
        let callback_summaries = signature.summaries.clone();
        let callback_return = (*signature.return_type).clone();
        let result_item = if matches!(
            operation,
            CollectionOperation::Mapcat | CollectionOperation::Mapcatv
        ) {
            match callback_return {
                Type::List(item) | Type::Vector(item) | Type::Set(item) | Type::Map(_, item) => {
                    *item
                }
                Type::Any | Type::Unknown => Type::Any,
                other => {
                    self.error(
                        "OSR-T0020",
                        format!(
                            "{} callback must return a collection, found `{other}`",
                            operation.runtime_name()
                        ),
                        function.span,
                    );
                    return Expr::error(span);
                }
            }
        } else if matches!(
            operation,
            CollectionOperation::Filter | CollectionOperation::Filterv
        ) {
            expected_parameters.first().cloned().unwrap_or(Type::Any)
        } else {
            callback_return
        };
        let result_type = if operation.result_is_vector() {
            Type::Vector(Box::new(result_item))
        } else {
            Type::List(Box::new(result_item))
        };
        let mut summaries =
            join_summaries(collections.iter().map(|collection| &collection.summaries))
                .join(&function.summaries)
                .join(&callback_summaries);
        let temporal = collections
            .iter()
            .fold(
                crate::types::TemporalSummary::pointwise(),
                |summary, collection| summary.compose(&collection.summaries.temporal),
            )
            .compose(&callback_summaries.temporal);
        summaries.temporal = temporal;
        summaries.data = DataProperties {
            alignment: Alignment::Positional,
            preserves_length: (operation == CollectionOperation::Map && collections.len() == 1)
                .then_some(true),
            materializes: Some(operation.result_is_vector()),
            reshapes: Some(matches!(
                operation,
                CollectionOperation::Mapcat | CollectionOperation::Mapcatv
            )),
            ..DataProperties::unknown()
        };
        let binding = self.ensure_core_collection_binding(operation.runtime_name(), span);
        let mut parameter_types = vec![function.ty.clone()];
        parameter_types.extend(collections.iter().map(|collection| collection.ty.clone()));
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(parameter_types, result_type.clone())
                    .with_summaries(callback_summaries),
            ),
            ExprKind::Binding(binding),
        );
        let mut arguments = vec![CallArgument::Positional(function)];
        arguments.extend(collections.into_iter().map(CallArgument::Positional));
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
}
