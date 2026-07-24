use super::super::super::*;

impl<'a> Lowerer<'a> {
    pub(in crate::hir) fn lower_control_intrinsic(
        &mut self,
        call: &ast::CallExpr,
        span: Span,
        scope: &mut Scope,
        intrinsic: ControlIntrinsic,
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
                "OSR-T0026",
                format!(
                    "osiris.kernel/{}* expects exactly one positional argument",
                    match intrinsic {
                        ControlIntrinsic::Truthy => "truthy",
                        ControlIntrinsic::Nil => "nil",
                        ControlIntrinsic::Present => "present",
                        ControlIntrinsic::Nonempty => "nonempty",
                    }
                ),
                span,
            );
            return Expr::error(span);
        }

        let value = self.lower_expr(&call.positional[0], scope);
        let present_type = non_nil_type(&self.types.resolve(&value.ty));
        if intrinsic == ControlIntrinsic::Nonempty
            && !matches!(
                &present_type,
                Type::List(_)
                    | Type::Vector(_)
                    | Type::Tuple(_)
                    | Type::Str
                    | Type::Bytes
                    | Type::Never
                    | Type::Error
            )
        {
            self.error(
                "OSR-T0026",
                format!(
                    "when-first requires an indexable List, Vector, Tuple, Str, or Bytes value; found `{}`",
                    value.ty
                ),
                value.span,
            );
            return Expr::error(span);
        }

        let result_type = match intrinsic {
            ControlIntrinsic::Truthy | ControlIntrinsic::Nil | ControlIntrinsic::Nonempty => {
                Type::Bool
            }
            ControlIntrinsic::Present => present_type,
        };
        let binding = self.ensure_core_collection_binding(intrinsic.runtime_name(), span);
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

    /// Lower the failure-only runtime entry used by the Clojure-style
    /// `assert` macro.  The macro supplies a literal false condition in its
    /// else branch, but keeping the condition as an argument makes the ABI
    /// useful to packaged prelude implementations as well.
    pub(in crate::hir) fn lower_assert(
        &mut self,
        call: &ast::CallExpr,
        span: Span,
        scope: &mut Scope,
    ) -> Expr {
        if !call.keywords.is_empty() || !(1..=2).contains(&call.positional.len()) {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0026",
                "osiris.kernel/assert* expects a condition and optional message",
                span,
            );
            return Expr::error(span);
        }
        let condition = self.lower_expr(&call.positional[0], scope);
        let message = call
            .positional
            .get(1)
            .map(|value| self.lower_expr(value, scope));
        let mut summaries = condition.summaries.clone();
        if let Some(message) = &message {
            summaries = summaries.join(&message.summaries);
        }
        summaries.effects = summaries
            .effects
            .union(&EffectRow::singleton(Effect::Throw));
        let binding = self.ensure_core_collection_binding("assert_value", span);
        let mut parameter_types = vec![condition.ty.clone()];
        let mut arguments = vec![CallArgument::Positional(condition)];
        if let Some(message) = message {
            parameter_types.push(message.ty.clone());
            arguments.push(CallArgument::Positional(message));
        }
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(parameter_types, Type::Never).with_summaries(summaries.clone()),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: Type::Never,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments,
            },
        }
    }
}
