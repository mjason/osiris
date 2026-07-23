use super::super::super::*;

impl<'a> Lowerer<'a> {
    pub(in crate::hir) fn lower_body(
        &mut self,
        body: &[ast::Expr],
        scope: &mut Scope,
        span: Span,
    ) -> Expr {
        let expressions = body
            .iter()
            .map(|expression| self.lower_expr(expression, scope))
            .collect::<Vec<_>>();
        match expressions.len() {
            0 => Expr::error(span),
            1 => expressions.into_iter().next().expect("one expression"),
            _ => {
                let ty = expressions
                    .last()
                    .map_or(Type::None, |expr| expr.ty.clone());
                let summaries = join_summaries(expressions.iter().map(|expr| &expr.summaries));
                Expr {
                    span,
                    ty,
                    summaries,
                    kind: ExprKind::Do(expressions),
                }
            }
        }
    }

    pub(in crate::hir) fn wrap_let_bindings(
        &self,
        bindings: Vec<LetBinding>,
        body: Expr,
        span: Span,
    ) -> Expr {
        if bindings.is_empty() {
            return body;
        }
        let summaries = bindings
            .iter()
            .fold(body.summaries.clone(), |summary, binding| {
                summary.join(&binding.value.summaries)
            });
        Expr {
            span,
            ty: body.ty.clone(),
            summaries,
            kind: ExprKind::Let {
                bindings,
                body: Box::new(body),
            },
        }
    }
}
