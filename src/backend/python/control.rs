use super::*;

impl<'hir> Backend<'hir> {
    pub(super) fn lower_tail(
        &mut self,
        expression: &hir::Expr,
    ) -> Result<Vec<py::Stmt>, BackendError> {
        match &expression.kind {
            ExprKind::Let { bindings, body } => {
                let mut result = Vec::new();
                for binding in bindings {
                    let lowered = self.lower_value(&binding.value)?;
                    result.extend(lowered.prefix);
                    let value = lowered.value.ok_or_else(|| {
                        self.error(
                            "let binding does not produce a value",
                            Some(binding.value.span),
                        )
                    })?;
                    result.push(py::Stmt::Assign(py::Assign {
                        targets: vec![self.binding_target(&binding.binding)?],
                        value,
                    }));
                }
                result.extend(self.lower_tail(body)?);
                Ok(result)
            }
            ExprKind::Do(expressions) => {
                let mut result = Vec::new();
                for expression in expressions.iter().take(expressions.len().saturating_sub(1)) {
                    let lowered = self.lower_value(expression)?;
                    result.extend(lowered.prefix);
                    if let Some(value) = lowered.value {
                        result.push(py::Stmt::Expr(value));
                    } else {
                        return Ok(result);
                    }
                }
                if let Some(last) = expressions.last() {
                    result.extend(self.lower_tail(last)?);
                }
                Ok(result)
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let condition = self.lower_value(condition)?;
                let condition_value = condition.value.ok_or_else(|| {
                    self.error(
                        "if condition does not produce a value",
                        Some(expression.span),
                    )
                })?;
                let then_body = self.lower_tail(then_branch)?;
                let else_body = self.lower_tail(else_branch)?;
                let mut result = condition.prefix;
                result.push(py::Stmt::If(py::IfStmt {
                    test: condition_value,
                    body: then_body,
                    orelse: else_body,
                }));
                Ok(result)
            }
            ExprKind::Try {
                body,
                catches,
                finally_body,
            } => {
                // Clojure permits a body-only `try`; Python requires at
                // least one handler or a finally suite.  In the body-only
                // case the construct has no observable exception boundary,
                // so preserve the expression directly instead of emitting
                // invalid Python syntax.
                if catches.is_empty() && finally_body.is_none() {
                    return self.lower_tail(body);
                }
                let mut handlers = Vec::new();
                for catch in catches {
                    let exception_type = catch
                        .exception_type
                        .as_ref()
                        .map(|ty| self.annotation(ty, Some(catch.body.span)))
                        .transpose()?;
                    let name = catch
                        .binding
                        .as_ref()
                        .map(|binding| self.python_name(binding).to_owned());
                    handlers.push(py::ExceptHandler {
                        exception_type,
                        name,
                        body: self.lower_tail(&catch.body)?,
                    });
                }
                let mut prefix = Vec::new();
                let body_stmts = self.lower_tail(body)?;
                let finally_stmts = finally_body
                    .as_ref()
                    .map(|body| self.lower_discard(body))
                    .transpose()?
                    .unwrap_or_default();
                prefix.push(py::Stmt::Try(py::Try {
                    body: body_stmts,
                    handlers,
                    orelse: Vec::new(),
                    finalbody: finally_stmts,
                }));
                Ok(prefix)
            }
            ExprKind::Raise(value) => {
                let mut result = Vec::new();
                let exception = value
                    .as_ref()
                    .map(|value| self.lower_value(value))
                    .transpose()?;
                if let Some(lowered) = exception {
                    result.extend(lowered.prefix);
                    result.push(py::Stmt::Raise(py::Raise {
                        exception: Some(lowered.value.ok_or_else(|| {
                            self.error(
                                "raise expression does not produce a value",
                                value.as_ref().map(|value| value.span),
                            )
                        })?),
                        cause: None,
                    }));
                } else {
                    result.push(py::Stmt::Raise(py::Raise {
                        exception: None,
                        cause: None,
                    }));
                }
                Ok(result)
            }
            _ => {
                let lowered = self.lower_value(expression)?;
                let mut result = lowered.prefix;
                if let Some(value) = lowered.value {
                    result.push(py::Stmt::Return(Some(value)));
                }
                Ok(result)
            }
        }
    }

    pub(super) fn lower_discard(
        &mut self,
        expression: &hir::Expr,
    ) -> Result<Vec<py::Stmt>, BackendError> {
        let lowered = self.lower_value(expression)?;
        let mut result = lowered.prefix;
        if let Some(value) = lowered.value {
            result.push(py::Stmt::Expr(value));
        }
        Ok(result)
    }
}
