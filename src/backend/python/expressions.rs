use super::*;

impl<'hir> Backend<'hir> {
    pub(super) fn lower_value(&mut self, expression: &hir::Expr) -> Result<Lowered, BackendError> {
        let span = expression.span;
        let result = match &expression.kind {
            ExprKind::None => py::Expr::Literal(py::Literal::None),
            ExprKind::Bool(value) => py::Expr::Literal(py::Literal::Bool(*value)),
            ExprKind::Integer(value) => {
                return Ok(Lowered::value(py::Expr::Literal(py::Literal::IntegerText(
                    value.clone(),
                ))));
            }
            ExprKind::Float(value) => py::Expr::Literal(py::Literal::Float(
                value
                    .parse::<f64>()
                    .map_err(|_| self.error("invalid float literal", Some(span)))?,
            )),
            ExprKind::String(value) => py::Expr::Literal(py::Literal::String(value.clone())),
            ExprKind::Binding(binding) => return Ok(Lowered::value(self.binding_expr(binding)?)),
            ExprKind::List(items) => return self.lower_sequence(items, false),
            ExprKind::Vector(items) => return self.lower_sequence(items, true),
            ExprKind::Map(entries) => {
                let mut prefix = Vec::new();
                let mut pairs = Vec::new();
                for (key_expression, value_expression) in entries {
                    let key = self.lower_value(key_expression)?;
                    prefix.extend(key.prefix);
                    let value_key = key.value.ok_or_else(|| {
                        self.error(
                            "map key does not produce a value",
                            Some(key_expression.span),
                        )
                    })?;
                    let value = self.lower_value(value_expression)?;
                    prefix.extend(value.prefix);
                    let value_value = value.value.ok_or_else(|| {
                        self.error(
                            "map value does not produce a value",
                            Some(value_expression.span),
                        )
                    })?;
                    pairs.push(py::Expr::Tuple(vec![value_key, value_value]));
                }
                return Ok(Lowered {
                    prefix,
                    value: Some(py::Expr::call(
                        self.linked_runtime_helper("logical_map"),
                        vec![py::CallArgument::Positional(py::Expr::List(pairs))],
                    )),
                });
            }
            ExprKind::Set(items) => {
                let lowered = self.lower_sequence(items, false)?;
                let Some(items) = lowered.value else {
                    return Ok(lowered);
                };
                return Ok(Lowered {
                    prefix: lowered.prefix,
                    value: Some(py::Expr::call(
                        self.linked_runtime_helper("logical_set"),
                        vec![py::CallArgument::Positional(items)],
                    )),
                });
            }
            ExprKind::Call { callee, arguments } => {
                let callee_expression = callee;
                let standard_keywords = self.standard_positional_keywords(
                    callee_expression,
                    arguments
                        .iter()
                        .filter(|argument| matches!(argument, hir::CallArgument::Positional(_)))
                        .count(),
                );
                let callee = self.lower_value(callee_expression)?;
                let mut prefix = callee.prefix;
                let function = callee.value.ok_or_else(|| {
                    self.error(
                        "call target does not produce a value",
                        Some(callee_expression.span),
                    )
                })?;
                let mut args = Vec::new();
                let mut positional_index = 0_usize;
                for argument in arguments {
                    match argument {
                        hir::CallArgument::Positional(value_expression) => {
                            let value = self.lower_value(value_expression)?;
                            prefix.extend(value.prefix);
                            let value = value.value.ok_or_else(|| {
                                self.error(
                                    "call argument does not produce a value",
                                    Some(value_expression.span),
                                )
                            })?;
                            if let Some(name) = standard_keywords.get(&positional_index) {
                                args.push(py::CallArgument::Keyword(py::KeywordArgument::Named {
                                    name: python_identifier(name),
                                    value,
                                }));
                            } else {
                                args.push(py::CallArgument::Positional(value));
                            }
                            positional_index += 1;
                        }
                        hir::CallArgument::Keyword {
                            name,
                            value: value_expression,
                        } => {
                            let value = self.lower_value(value_expression)?;
                            prefix.extend(value.prefix);
                            args.push(py::CallArgument::Keyword(py::KeywordArgument::Named {
                                name: python_identifier(name),
                                value: value.value.ok_or_else(|| {
                                    self.error(
                                        "keyword argument does not produce a value",
                                        Some(value_expression.span),
                                    )
                                })?,
                            }));
                        }
                    }
                }
                return Ok(Lowered {
                    prefix,
                    value: Some(py::Expr::call(function, args)),
                });
            }
            ExprKind::Operator { operator, operands } => {
                return self.lower_operator(*operator, operands);
            }
            ExprKind::Attribute { value, attribute } => {
                let value_expression = value;
                let value = self.lower_value(value_expression)?;
                let prefix = value.prefix;
                let base = value.value.ok_or_else(|| {
                    self.error(
                        "attribute base does not produce a value",
                        Some(value_expression.span),
                    )
                })?;
                return Ok(Lowered {
                    prefix,
                    value: Some(py::Expr::Attribute {
                        value: Box::new(base),
                        attr: python_identifier(attribute),
                    }),
                });
            }
            ExprKind::Index { value, index } => {
                let value_expression = value;
                let index_expression = index;
                let value = self.lower_value(value_expression)?;
                let mut prefix = value.prefix;
                let base = value.value.ok_or_else(|| {
                    self.error(
                        "index base does not produce a value",
                        Some(value_expression.span),
                    )
                })?;
                let index = self.lower_value(index_expression)?;
                prefix.extend(index.prefix);
                let index = index.value.ok_or_else(|| {
                    self.error(
                        "index does not produce a value",
                        Some(index_expression.span),
                    )
                })?;
                return Ok(Lowered {
                    prefix,
                    value: Some(py::Expr::Subscript {
                        value: Box::new(base),
                        slice: Box::new(index),
                    }),
                });
            }
            ExprKind::Let { .. } | ExprKind::Do(_) | ExprKind::If { .. } | ExprKind::Try { .. } => {
                let temporary = self.fresh_temporary();
                let statements = self.lower_value_block(expression, &temporary)?;
                return Ok(Lowered {
                    prefix: statements,
                    value: Some(py::Expr::name(temporary)),
                });
            }
            ExprKind::Lambda { parameters, body } => return self.lower_lambda(parameters, body),
            ExprKind::Raise(_) => {
                return Ok(Lowered {
                    prefix: self.lower_tail(expression)?,
                    value: None,
                });
            }
            ExprKind::Error => {
                return Err(self.error("cannot generate Python for erroneous HIR", Some(span)));
            }
        };
        Ok(Lowered::value(result))
    }

    pub(super) fn lower_sequence(
        &mut self,
        items: &[hir::Expr],
        tuple: bool,
    ) -> Result<Lowered, BackendError> {
        let mut prefix = Vec::new();
        let mut values = Vec::new();
        for item in items {
            let lowered = self.lower_value(item)?;
            prefix.extend(lowered.prefix);
            values.push(lowered.value.ok_or_else(|| {
                self.error("collection item does not produce a value", Some(item.span))
            })?);
        }
        Ok(Lowered {
            prefix,
            value: Some(if tuple {
                py::Expr::Tuple(values)
            } else {
                py::Expr::List(values)
            }),
        })
    }

    pub(super) fn lower_operator(
        &mut self,
        operator: Operator,
        operands: &[hir::Expr],
    ) -> Result<Lowered, BackendError> {
        let mut prefix = Vec::new();
        let mut values = Vec::new();
        for operand in operands {
            let lowered = self.lower_value(operand)?;
            prefix.extend(lowered.prefix);
            values.push(lowered.value.ok_or_else(|| {
                self.error(
                    "operator operand does not produce a value",
                    Some(operand.span),
                )
            })?);
        }
        let value = match operator {
            Operator::And | Operator::Or => {
                if values.len() < 2 {
                    return Ok(Lowered {
                        prefix,
                        value: values.pop(),
                    });
                }
                py::Expr::BoolOp {
                    op: if operator == Operator::And {
                        py::BooleanOp::And
                    } else {
                        py::BooleanOp::Or
                    },
                    values,
                }
            }
            Operator::Not => unary(values, py::UnaryOp::Not, &mut prefix, "not")?,
            Operator::Negate => unary(values, py::UnaryOp::Negative, &mut prefix, "negate")?,
            Operator::Positive => unary(values, py::UnaryOp::Positive, &mut prefix, "positive")?,
            Operator::Equal
            | Operator::NotEqual
            | Operator::Less
            | Operator::LessEqual
            | Operator::Greater
            | Operator::GreaterEqual => {
                if values.len() < 2 {
                    return Err(self.error("comparison needs at least two operands", None));
                }
                let op = match operator {
                    Operator::Equal => py::CompareOp::Equal,
                    Operator::NotEqual => py::CompareOp::NotEqual,
                    Operator::Less => py::CompareOp::Less,
                    Operator::LessEqual => py::CompareOp::LessEqual,
                    Operator::Greater => py::CompareOp::Greater,
                    Operator::GreaterEqual => py::CompareOp::GreaterEqual,
                    _ => unreachable!(),
                };
                let left = values.remove(0);
                py::Expr::Compare {
                    left: Box::new(left),
                    comparisons: values.into_iter().map(|value| (op, value)).collect(),
                }
            }
            _ => {
                if values.is_empty() {
                    return Err(self.error("operator needs an operand", None));
                }
                let op = match operator {
                    Operator::Add => py::BinaryOp::Add,
                    Operator::Subtract => py::BinaryOp::Subtract,
                    Operator::Multiply => py::BinaryOp::Multiply,
                    Operator::Divide => py::BinaryOp::Divide,
                    Operator::FloorDivide => py::BinaryOp::FloorDivide,
                    Operator::Remainder => py::BinaryOp::Modulo,
                    _ => return Err(self.error("unsupported operator", None)),
                };
                let mut result = values.remove(0);
                for right in values {
                    result = py::Expr::BinOp {
                        left: Box::new(result),
                        op,
                        right: Box::new(right),
                    };
                }
                result
            }
        };
        Ok(Lowered {
            prefix,
            value: Some(value),
        })
    }

    pub(super) fn lower_lambda(
        &mut self,
        parameters: &[hir::Parameter],
        body: &hir::Expr,
    ) -> Result<Lowered, BackendError> {
        let mut py_parameters = py::Parameters::default();
        for parameter in parameters {
            let default = match parameter.default.as_ref() {
                Some(default_expression) => {
                    let lowered = self.lower_value(default_expression)?;
                    if !lowered.prefix.is_empty() {
                        return Err(self.error(
                            "lambda parameter defaults must be expression-only",
                            Some(default_expression.span),
                        ));
                    }
                    Some(lowered.value.ok_or_else(|| {
                        self.error(
                            "lambda parameter default does not produce a value",
                            Some(default_expression.span),
                        )
                    })?)
                }
                None => None,
            };
            let py_parameter = py::Parameter {
                name: self.python_name(&parameter.binding).to_owned(),
                annotation: None,
                default,
            };
            if parameter.variadic {
                py_parameters.vararg = Some(py_parameter);
            } else {
                py_parameters.positional.push(py_parameter);
            }
        }
        let lowered = self.lower_value(body)?;
        if lowered.prefix.is_empty() {
            return Ok(Lowered::value(py::Expr::Lambda {
                parameters: Box::new(py_parameters),
                body: Box::new(lowered.value.ok_or_else(|| {
                    self.error("lambda body does not produce a value", Some(body.span))
                })?),
            }));
        }
        let helper = self.fresh_helper("_osr_lambda");
        let mut helper_body = lowered.prefix;
        helper_body.push(py::Stmt::Return(lowered.value));
        // Keep the helper in the expression prefix instead of a module-level
        // queue.  A complex lambda may close over a function-local binding;
        // emitting its helper at module scope would make that binding
        // unresolved at runtime.  Prefix statements are emitted by each
        // enclosing lowering context, so this preserves the lambda's lexical
        // scope for both direct and nested closures.
        Ok(Lowered {
            prefix: vec![py::Stmt::FunctionDef(Box::new(py::FunctionDef {
                name: helper.clone(),
                parameters: py_parameters,
                returns: None,
                decorators: Vec::new(),
                body: helper_body,
                is_async: false,
            }))],
            value: Some(py::Expr::name(helper)),
        })
    }

    pub(super) fn lower_value_block(
        &mut self,
        expression: &hir::Expr,
        temporary: &str,
    ) -> Result<Vec<py::Stmt>, BackendError> {
        match &expression.kind {
            ExprKind::Let { bindings, body } => {
                let mut statements = Vec::new();
                for binding in bindings {
                    let lowered = self.lower_value(&binding.value)?;
                    statements.extend(lowered.prefix);
                    let value = lowered.value.ok_or_else(|| {
                        self.error(
                            "let binding does not produce a value",
                            Some(binding.value.span),
                        )
                    })?;
                    statements.push(py::Stmt::Assign(py::Assign {
                        targets: vec![self.binding_target(&binding.binding)?],
                        value,
                    }));
                }
                statements.extend(self.lower_value_block(body, temporary)?);
                Ok(statements)
            }
            ExprKind::Do(expressions) => {
                let mut statements = Vec::new();
                for expression in expressions.iter().take(expressions.len().saturating_sub(1)) {
                    statements.extend(self.lower_discard(expression)?);
                }
                if let Some(last) = expressions.last() {
                    statements.extend(self.lower_value_block(last, temporary)?);
                }
                Ok(statements)
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
                let mut statements = condition.prefix;
                statements.push(py::Stmt::If(py::IfStmt {
                    test: condition_value,
                    body: self.lower_value_block(then_branch, temporary)?,
                    orelse: self.lower_value_block(else_branch, temporary)?,
                }));
                Ok(statements)
            }
            ExprKind::Try {
                body,
                catches,
                finally_body,
            } => {
                if catches.is_empty() && finally_body.is_none() {
                    return self.lower_value_block(body, temporary);
                }
                let handlers = catches
                    .iter()
                    .map(|catch| {
                        Ok(py::ExceptHandler {
                            exception_type: catch
                                .exception_type
                                .as_ref()
                                .map(|ty| self.annotation(ty, Some(catch.body.span)))
                                .transpose()?,
                            name: catch
                                .binding
                                .as_ref()
                                .map(|binding| self.python_name(binding).to_owned()),
                            body: self.lower_value_block(&catch.body, temporary)?,
                        })
                    })
                    .collect::<Result<Vec<_>, BackendError>>()?;
                let finalbody = finally_body
                    .as_ref()
                    .map(|body| self.lower_discard(body))
                    .transpose()?
                    .unwrap_or_default();
                Ok(vec![py::Stmt::Try(py::Try {
                    body: self.lower_value_block(body, temporary)?,
                    handlers,
                    orelse: Vec::new(),
                    finalbody,
                })])
            }
            ExprKind::Raise(_) => self.lower_tail(expression),
            _ => {
                let lowered = self.lower_value(expression)?;
                let mut statements = lowered.prefix;
                if let Some(value) = lowered.value {
                    statements.push(py::Stmt::Assign(py::Assign {
                        targets: vec![py::Expr::name(temporary)],
                        value,
                    }));
                }
                Ok(statements)
            }
        }
    }
}
