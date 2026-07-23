use super::super::super::*;

impl<'a> Lowerer<'a> {
    pub(in crate::hir) fn lower_name(&mut self, name: &Name, span: Span, scope: &Scope) -> Expr {
        if let Some(id) = scope.resolve(&name.canonical) {
            return Expr {
                span,
                ty: self.binding_type(id),
                summaries: self
                    .local_value_summaries
                    .get(id)
                    .cloned()
                    .unwrap_or_else(CallSummaries::pure_scalar),
                kind: ExprKind::Binding(id.clone()),
            };
        }
        if let Some(id) = self.resolve_global_name(&name.canonical) {
            return self.lower_global_binding_read(id, span);
        }
        if let Some(id) = self.qualified_imports.get(&name.canonical).cloned() {
            return self.lower_global_binding_read(id, span);
        }
        if name.canonical == "osiris.prelude/mapv" {
            let binding = self.ensure_core_mapv_binding(span);
            return Expr::pure(
                span,
                self.binding_type(&binding),
                ExprKind::Binding(binding),
            );
        }

        if let Some((base, members)) = split_access_name(&name.canonical) {
            let mut value = if let Some(id) = scope
                .resolve(base)
                .cloned()
                .or_else(|| self.resolve_global_name(base))
            {
                Expr::pure(span, self.binding_type(&id), ExprKind::Binding(id))
            } else {
                self.error(
                    "OSR-N0012",
                    format!("unknown name `{}`", name.spelling),
                    span,
                );
                return Expr::error(span);
            };
            if self.interfaces.is_some()
                && matches!(&value.kind, ExprKind::Binding(id)
                    if self
                        .bindings
                        .get(id)
                        .is_some_and(|binding| binding.name.kind == BindingKind::Module))
            {
                self.error(
                    "OSR-H0013",
                    format!(
                        "unknown member `{}` on imported module `{base}`",
                        members.join(".")
                    ),
                    span,
                );
                return Expr::error(span);
            }
            for member in members {
                let member_name = member.to_owned();
                let (attribute, ty) = match self.struct_field_type(&value.ty, &member_name) {
                    Some((attribute, ty)) => (attribute, ty),
                    None if matches!(&value.ty, Type::Nominal { binding, .. } if self
                            .struct_fields
                            .contains_key(binding)) =>
                    {
                        self.error(
                            "OSR-T0016",
                            format!("unknown field `{member_name}` on type `{}`", value.ty),
                            span,
                        );
                        return Expr::error(span);
                    }
                    None => {
                        let summaries = value.summaries.join(&CallSummaries::unknown());
                        value.summaries = summaries;
                        (python_identifier(&member_name), Type::Any)
                    }
                };
                value = Expr {
                    span,
                    ty,
                    summaries: value.summaries.clone(),
                    kind: ExprKind::Attribute {
                        value: Box::new(value),
                        attribute,
                    },
                };
            }
            return value;
        }

        self.error(
            "OSR-N0012",
            format!("unknown name `{}`", name.spelling),
            span,
        );
        Expr::error(span)
    }

    pub(in crate::hir) fn lower_global_binding_read(
        &mut self,
        binding: BindingId,
        span: Span,
    ) -> Expr {
        let ty = self.binding_type(&binding);
        if !self.binding_is_dynamic(&binding) {
            return Expr::pure(span, ty, ExprKind::Binding(binding));
        }

        let summaries = dynamic_state_summaries();
        let runtime = self.ensure_core_collection_binding("dynamic_get", span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(vec![Type::Str, ty.clone()], ty.clone())
                    .with_summaries(summaries.clone()),
            ),
            ExprKind::Binding(runtime),
        );
        Expr {
            span,
            ty: ty.clone(),
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![
                    CallArgument::Positional(Expr::pure(
                        span,
                        Type::Str,
                        ExprKind::String(binding.as_str().to_owned()),
                    )),
                    CallArgument::Positional(Expr::pure(span, ty, ExprKind::Binding(binding))),
                ],
            },
        }
    }

    pub(in crate::hir) fn struct_field_type(
        &self,
        value_type: &Type,
        member: &str,
    ) -> Option<(String, Type)> {
        let Type::Nominal { binding, args } = value_type else {
            return None;
        };
        let table = self.struct_fields.get(binding)?;
        let field = table.fields.get(member)?;
        let substitutions = table
            .generic_variables
            .iter()
            .copied()
            .zip(args.iter().cloned())
            .collect::<BTreeMap<_, _>>();
        Some((
            python_identifier(&field.canonical),
            replace_type_variables(&field.ty, &substitutions),
        ))
    }
}
