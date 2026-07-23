use super::*;

impl<'hir> Backend<'hir> {
    pub(super) fn binding(
        &self,
        id: &crate::name::BindingId,
    ) -> Result<&'hir hir::Binding, BackendError> {
        self.bindings
            .get(id)
            .copied()
            .ok_or_else(|| self.error("HIR references an unknown binding", None))
    }

    pub(super) fn binding_expr(
        &mut self,
        id: &crate::name::BindingId,
    ) -> Result<py::Expr, BackendError> {
        for overrides in self.binding_overrides.iter().rev() {
            if let Some(expression) = overrides.get(id) {
                return Ok(expression.clone());
            }
        }
        self.register_runtime_binding(id);
        Ok(py::Expr::name(self.python_name(id).to_owned()))
    }

    pub(super) fn binding_target(
        &self,
        id: &crate::name::BindingId,
    ) -> Result<py::Expr, BackendError> {
        Ok(py::Expr::name(self.python_name(id).to_owned()))
    }

    pub(super) fn python_name(&self, id: &crate::name::BindingId) -> &str {
        self.names
            .get(id)
            .map(String::as_str)
            .unwrap_or("_osr_unknown")
    }

    pub(super) fn annotation(
        &mut self,
        ty: &Type,
        span: Option<Span>,
    ) -> Result<py::Expr, BackendError> {
        let expression = match ty {
            Type::Bool => py::Expr::name("bool"),
            Type::Int => py::Expr::name("int"),
            Type::Float => py::Expr::name("float"),
            Type::Str => py::Expr::name("str"),
            Type::Bytes => py::Expr::name("bytes"),
            Type::None => py::Expr::name("None"),
            Type::Any => {
                self.typing.insert("Any".to_owned());
                py::Expr::name("Any")
            }
            Type::Never => {
                self.typing.insert(
                    if self.target.at_least(3, 11) {
                        "Never"
                    } else {
                        "NoReturn"
                    }
                    .to_owned(),
                );
                py::Expr::name(if self.target.at_least(3, 11) {
                    "Never"
                } else {
                    "NoReturn"
                })
            }
            Type::Unknown | Type::Error => {
                return Err(self.error(
                    "unresolved type cannot be emitted as a Python annotation",
                    span,
                ));
            }
            Type::Option(inner) => {
                self.typing.insert("Optional".to_owned());
                py::Expr::Subscript {
                    value: Box::new(py::Expr::name("Optional")),
                    slice: Box::new(self.annotation(inner, span)?),
                }
            }
            Type::Union(members) => {
                self.typing.insert("Union".to_owned());
                py::Expr::Subscript {
                    value: Box::new(py::Expr::name("Union")),
                    slice: Box::new(py::Expr::Tuple(
                        members
                            .iter()
                            .map(|member| self.annotation(member, span))
                            .collect::<Result<_, _>>()?,
                    )),
                }
            }
            Type::Tuple(members) => py::Expr::Subscript {
                value: Box::new(py::Expr::name("tuple")),
                slice: Box::new(py::Expr::Tuple(
                    members
                        .iter()
                        .map(|member| self.annotation(member, span))
                        .collect::<Result<_, _>>()?,
                )),
            },
            Type::List(item) => py::Expr::Subscript {
                value: Box::new(py::Expr::name("list")),
                slice: Box::new(self.annotation(item, span)?),
            },
            Type::Vector(item) => py::Expr::Subscript {
                value: Box::new(py::Expr::name("tuple")),
                slice: Box::new(py::Expr::Tuple(vec![
                    self.annotation(item, span)?,
                    py::Expr::Literal(py::Literal::Ellipsis),
                ])),
            },
            Type::Map(key, value) => py::Expr::Subscript {
                value: Box::new(py::Expr::name("dict")),
                slice: Box::new(py::Expr::Tuple(vec![
                    self.annotation(key, span)?,
                    self.annotation(value, span)?,
                ])),
            },
            Type::Set(item) => py::Expr::Subscript {
                value: Box::new(py::Expr::name("set")),
                slice: Box::new(self.annotation(item, span)?),
            },
            Type::Fn(function) => {
                self.typing.insert("Callable".to_owned());
                py::Expr::Subscript {
                    value: Box::new(py::Expr::name("Callable")),
                    slice: Box::new(py::Expr::Tuple(vec![
                        py::Expr::List(
                            function
                                .parameters
                                .iter()
                                .map(|parameter| self.annotation(parameter, span))
                                .collect::<Result<_, _>>()?,
                        ),
                        self.annotation(&function.return_type, span)?,
                    ])),
                }
            }
            Type::Nominal { binding, args } => {
                self.register_runtime_type(binding);
                let name = self.nominal_name(binding);
                if args.is_empty() {
                    name
                } else {
                    py::Expr::Subscript {
                        value: Box::new(name),
                        slice: Box::new(if args.len() == 1 {
                            self.annotation(&args[0], span)?
                        } else {
                            py::Expr::Tuple(
                                args.iter()
                                    .map(|arg| self.annotation(arg, span))
                                    .collect::<Result<_, _>>()?,
                            )
                        }),
                    }
                }
            }
            Type::Literal(value) => {
                self.typing.insert("Literal".to_owned());
                py::Expr::Subscript {
                    value: Box::new(py::Expr::name("Literal")),
                    slice: Box::new(py::Expr::string(value.canonical_text())),
                }
            }
            Type::TypeVar(variable) => {
                self.typing.insert("TypeVar".to_owned());
                let source = self
                    .typevar_names
                    .get(variable)
                    .cloned()
                    .unwrap_or_else(|| format!("_T{}", variable.0));
                let python = self
                    .typevars
                    .entry(source.clone())
                    .or_insert(source)
                    .clone();
                py::Expr::name(python)
            }
        };
        Ok(expression)
    }

    pub(super) fn nominal_name(&self, binding: &str) -> py::Expr {
        if let Some(name) = python_builtin_exception_from_binding(binding) {
            return py::Expr::name(name);
        }
        if let Some((id, _)) = self.bindings.iter().find(|(id, binding_name)| {
            id.as_str() == binding && binding_name.name.kind == crate::name::BindingKind::Type
        }) {
            return py::Expr::name(self.python_name(id).to_owned());
        }
        let name = nominal_short_name(binding);
        if let Some(mapped) = self.active_type_parameters.get(name) {
            return py::Expr::name(mapped.clone());
        }
        let mut parts = name
            .split('/')
            .flat_map(|part| part.split('.'))
            .map(python_identifier);
        let Some(first) = parts.next() else {
            return py::Expr::name("Any");
        };
        parts.fold(py::Expr::name(first), |value, attr| py::Expr::Attribute {
            value: Box::new(value),
            attr,
        })
    }

    pub(super) fn fresh_temporary(&mut self) -> String {
        loop {
            let name = format!("_osr_value_{}", self.temporary_counter);
            self.temporary_counter += 1;
            if self.reserved_names.insert(name.clone()) {
                return name;
            }
        }
    }
    pub(super) fn fresh_helper(&mut self, prefix: &str) -> String {
        loop {
            let name = format!("{}_{}", prefix, self.helper_counter);
            self.helper_counter += 1;
            if self.reserved_names.insert(name.clone()) {
                return name;
            }
        }
    }
    pub(super) fn error(&self, message: impl Into<String>, span: Option<Span>) -> BackendError {
        BackendError::new(message, span)
    }
}
