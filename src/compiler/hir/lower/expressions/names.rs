use super::super::super::*;

impl<'a> Lowerer<'a> {
    pub(in crate::hir) fn lower_name(&mut self, name: &Name, span: Span, scope: &Scope) -> Expr {
        if let Some(id) = scope.resolve(&name.canonical) {
            let id = id.clone();
            self.record_migration_alias_use(name, &id, span);
            return Expr {
                span,
                ty: self.binding_type(&id),
                summaries: self
                    .local_value_summaries
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(CallSummaries::pure_scalar),
                kind: ExprKind::Binding(id),
            };
        }
        if let Some(id) = self.resolve_global_name(&name.canonical) {
            self.record_migration_alias_use(name, &id, span);
            return self.lower_global_binding_read(id, span);
        }
        if let Some(id) = self.qualified_imports.get(&name.canonical).cloned() {
            self.record_migration_alias_use(name, &id, span);
            return self.lower_global_binding_read(id, span);
        }
        if name.canonical == "osiris.kernel/mapv" {
            let binding = self.ensure_core_mapv_binding(span);
            return Expr::pure(
                span,
                self.binding_type(&binding),
                ExprKind::Binding(binding),
            );
        }

        if let Some((base, members)) = split_access_name(&name.canonical) {
            let value = if let Some(id) = scope
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
            return match self.fold_member_access(value, members, span) {
                Some(value) => value,
                None => Expr::error(span),
            };
        }

        self.error(
            "OSR-N0012",
            format!("unknown name `{}`", name.spelling),
            span,
        );
        Expr::error(span)
    }

    fn record_migration_alias_use(&mut self, name: &Name, binding: &BindingId, span: Span) {
        let canonical_spelling = name
            .canonical
            .rsplit(['/', '.'])
            .next()
            .unwrap_or(&name.canonical);
        let Some(target) = self.bindings.get(binding) else {
            return;
        };
        let Some(preferred_names) = self
            .migration_alias_profiles
            .get(&(binding.clone(), canonical_spelling.to_owned()))
        else {
            return;
        };
        let source_terminal = name
            .spelling
            .rmatch_indices(['/', '.'])
            .next()
            .map_or(name.spelling.as_str(), |(index, separator)| {
                &name.spelling[index + separator.len()..]
            });
        let terminal_offset = name.spelling.len().saturating_sub(source_terminal.len());
        self.migration_advisories.push(MigrationAdvisory {
            span: Span::new(span.start + terminal_offset, span.end),
            alias: source_terminal.to_owned(),
            canonical: target.name.canonical.clone(),
            preferred_names: preferred_names.clone(),
        });
    }

    /// Fold member accesses over an evaluated subject: typed struct fields
    /// keep their checks (unknown fields are OSR-T0016 and yield `None`);
    /// anything else is a dynamic Python attribute of type `Any` with
    /// unknown effect summaries. Shared by symbol member chains and the
    /// `.name` member forms (OEP-0001-R079).
    pub(in crate::hir) fn fold_member_access<'m>(
        &mut self,
        mut value: Expr,
        members: impl IntoIterator<Item = &'m str>,
        span: Span,
    ) -> Option<Expr> {
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
                    return None;
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
        Some(value)
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
