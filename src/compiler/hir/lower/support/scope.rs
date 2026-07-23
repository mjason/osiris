use super::super::super::*;

impl<'a> Lowerer<'a> {
    pub(in crate::hir) fn lower_try(
        &mut self,
        expression: &ast::TryExpr,
        span: Span,
        scope: &mut Scope,
    ) -> Expr {
        let body = self.lower_body(&expression.body, scope, span);
        let mut types = vec![body.ty.clone()];
        let mut summaries = body.summaries.clone();
        let mut catches = Vec::new();
        for catch in &expression.catches {
            scope.push();
            let binding = catch.binding.as_ref().map(|parameter| {
                self.declare_local(
                    &parameter.name,
                    BindingKind::Value,
                    Type::Any,
                    parameter.metadata.clone(),
                    parameter.span,
                    scope,
                )
            });
            let catch_body = self.lower_body(&catch.body, scope, catch.span);
            scope.pop();
            types.push(catch_body.ty.clone());
            summaries = summaries.join(&catch_body.summaries);
            catches.push(Catch {
                exception_type: catch
                    .exception_type
                    .as_ref()
                    .map(|ty| self.resolve_type_expr(ty)),
                binding,
                body: catch_body,
            });
        }
        let finally_body = expression.finally_body.as_ref().map(|body| {
            let body = self.lower_body(body, scope, span);
            summaries = summaries.join(&body.summaries);
            Box::new(body)
        });
        Expr {
            span,
            ty: self.types.join_all(types.iter()),
            summaries,
            kind: ExprKind::Try {
                body: Box::new(body),
                catches,
                finally_body,
            },
        }
    }

    pub(in crate::hir) fn declare_local(
        &mut self,
        source_name: &Name,
        kind: BindingKind,
        ty: Type,
        metadata: Vec<MetadataEntry>,
        span: Span,
        scope: &mut Scope,
    ) -> BindingId {
        let scope_id = self.next_scope;
        self.next_scope += 1;
        if scope.current_contains(&source_name.canonical) {
            self.error(
                "OSR-N0013",
                format!("duplicate local name `{}`", source_name.spelling),
                span,
            );
        }
        let python_name = python_identifier(&source_name.canonical);
        let python_key = python_name.nfkc().collect::<String>();
        if let Some(existing) = scope.current_python_name(&python_key)
            && existing != source_name.canonical
        {
            self.error(
                "OSR-N0002",
                format!(
                    "name `{}` maps to Python identifier `{python_name}`, already used by `{existing}`",
                    source_name.spelling
                ),
                span,
            );
        }
        let scope_name = format!("{}::local-{scope_id}", self.module_name);
        let name = BindingName {
            id: BindingId::new(&scope_name, &source_name.canonical, kind),
            canonical: source_name.canonical.clone(),
            python: python_identifier(&source_name.canonical),
            kind,
            span,
        };
        let id = name.id.clone();
        scope.insert(source_name.canonical.clone(), id.clone(), python_key);
        self.bindings.insert(
            id.clone(),
            Binding {
                name,
                source_spelling: source_name.spelling.clone(),
                ty,
                runtime: None,
                public: false,
                metadata,
            },
        );
        id
    }

    pub(in crate::hir) fn resolve_global_name(&self, name: &str) -> Option<BindingId> {
        self.globals.get(name).cloned()
    }

    pub(in crate::hir) fn resolve_alias_target(&self, name: &str) -> Option<BindingId> {
        self.resolve_global_name(name)
            .or_else(|| self.qualified_imports.get(name).cloned())
    }

    pub(in crate::hir) fn global_id(&self, name: &Name) -> Option<BindingId> {
        self.resolve_global_name(&name.canonical)
    }

    pub(in crate::hir) fn binding_type(&self, id: &BindingId) -> Type {
        self.bindings
            .get(id)
            .map_or(Type::Error, |binding| self.types.resolve(&binding.ty))
    }

    pub(in crate::hir) fn binding_is_dynamic(&self, id: &BindingId) -> bool {
        self.bindings.get(id).is_some_and(|binding| {
            binding.name.kind == BindingKind::Value && metadata_flag(&binding.metadata, "dynamic")
        })
    }

    pub(in crate::hir) fn set_binding_type(&mut self, id: &BindingId, ty: Type) {
        if let Some(binding) = self.bindings.get_mut(id) {
            binding.ty = self.types.resolve(&ty);
        }
    }

    pub(in crate::hir) fn check_assignable(&mut self, actual: &Type, expected: &Type, span: Span) {
        let actual = self.types.resolve(actual);
        let expected = self.types.resolve(expected);
        if (contains_type_variable(&actual) || contains_type_variable(&expected))
            && self.types.unify(&actual, &expected).is_ok()
        {
            return;
        }
        if let Err(error) = self.types.check_assignable(&actual, &expected) {
            self.error("OSR-T0001", error.to_string(), span);
        }
    }

    pub(in crate::hir) fn error(
        &mut self,
        code: &'static str,
        message: impl Into<String>,
        span: Span,
    ) {
        self.diagnostics
            .push(Diagnostic::error(code, message, span));
    }
}
