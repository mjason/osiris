use super::super::super::*;

impl<'a> Lowerer<'a> {
    pub(in crate::hir) fn lower_trampoline(
        &mut self,
        call: &ast::CallExpr,
        span: Span,
        scope: &mut Scope,
    ) -> Expr {
        if !call.keywords.is_empty() || call.positional.is_empty() {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0024",
                "osiris.kernel/trampoline* expects a function and optional arguments",
                span,
            );
            return Expr::error(span);
        }
        let function = self.lower_expr(&call.positional[0], scope);
        let arguments = call.positional[1..]
            .iter()
            .map(|argument| self.lower_expr(argument, scope))
            .collect::<Vec<_>>();
        let result_type = match &function.ty {
            Type::Fn(signature) => {
                if signature.parameters.len() != arguments.len() {
                    self.error(
                        "OSR-T0024",
                        format!(
                            "trampoline function expects {} argument(s), found {}",
                            signature.parameters.len(),
                            arguments.len()
                        ),
                        function.span,
                    );
                }
                for (argument, parameter) in arguments.iter().zip(&signature.parameters) {
                    self.check_assignable(&argument.ty, parameter, argument.span);
                }
                if self.trampoline_has_invalid_bounce(&signature.return_type) {
                    self.error(
                        "OSR-T0024",
                        "trampoline bounce values must be zero-argument callables",
                        function.span,
                    );
                    Type::Error
                } else {
                    self.trampoline_result_type(&signature.return_type)
                }
            }
            Type::Any | Type::Unknown => Type::Any,
            _ => {
                self.error(
                    "OSR-T0024",
                    "trampoline function must be callable",
                    function.span,
                );
                Type::Error
            }
        };
        let summaries = arguments
            .iter()
            .fold(function.summaries.clone(), |summary, argument| {
                summary.join(&argument.summaries)
            });
        let binding = self.ensure_core_collection_binding("trampoline", span);
        let mut parameter_types = vec![function.ty.clone()];
        parameter_types.extend(arguments.iter().map(|argument| argument.ty.clone()));
        let callee = Expr::pure(
            span,
            Type::Fn(FunctionType::new(parameter_types, result_type.clone())),
            ExprKind::Binding(binding),
        );
        let mut lowered_arguments = vec![CallArgument::Positional(function)];
        lowered_arguments.extend(arguments.into_iter().map(CallArgument::Positional));
        Expr {
            span,
            ty: result_type,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: lowered_arguments,
            },
        }
    }

    pub(in crate::hir) fn trampoline_result_type(&self, ty: &Type) -> Type {
        match self.types.resolve(ty) {
            Type::Fn(signature) if signature.parameters.is_empty() => {
                self.trampoline_result_type(&signature.return_type)
            }
            Type::Option(inner) => Type::option(self.trampoline_result_type(&inner)),
            Type::Union(members) => Type::union(
                members
                    .iter()
                    .map(|member| self.trampoline_result_type(member)),
            ),
            other => other,
        }
    }

    /// A Python trampoline invokes every callable result with zero arguments.
    /// Reject a statically known callable that requires parameters instead of
    /// emitting code which would fail only after the first bounce. Dynamic
    /// `Any`/`Unknown` returns remain a deliberate runtime boundary.
    pub(in crate::hir) fn trampoline_has_invalid_bounce(&self, ty: &Type) -> bool {
        match self.types.resolve(ty) {
            Type::Fn(signature) => {
                !signature.parameters.is_empty()
                    || self.trampoline_has_invalid_bounce(&signature.return_type)
            }
            Type::Option(inner) => self.trampoline_has_invalid_bounce(&inner),
            Type::Union(members) => members
                .iter()
                .any(|member| self.trampoline_has_invalid_bounce(member)),
            _ => false,
        }
    }
}
