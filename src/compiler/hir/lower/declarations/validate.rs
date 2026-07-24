use super::super::super::*;

impl<'a> Lowerer<'a> {
    pub(in crate::hir) fn resolve_exports(&mut self, module: &ast::Module) {
        let explicit_canonical = module
            .items
            .iter()
            .filter_map(|item| match &item.kind {
                AstItemKind::Export(export) => Some(&export.names),
                _ => None,
            })
            .flatten()
            .filter(|name| {
                !self
                    .aliases
                    .iter()
                    .any(|alias| alias.canonical == name.canonical)
            })
            .map(|name| name.canonical.clone())
            .collect::<BTreeSet<_>>();
        for item in &module.items {
            let AstItemKind::Export(export) = &item.kind else {
                continue;
            };
            for name in &export.names {
                let Some(id) = self.resolve_global_name(&name.canonical) else {
                    if self.phase_one_names.contains(&name.canonical) {
                        continue;
                    }
                    self.error(
                        "OSR-N0011",
                        format!("cannot export unknown name `{}`", name.spelling),
                        export.span,
                    );
                    continue;
                };
                if let Some(alias_index) = self
                    .aliases
                    .iter()
                    .position(|alias| alias.canonical == name.canonical)
                {
                    let alias_spelling = self.aliases[alias_index].spelling.clone();
                    let target_name = self
                        .bindings
                        .get(&id)
                        .map(|binding| binding.name.canonical.clone());
                    if target_name
                        .as_ref()
                        .is_none_or(|target| !explicit_canonical.contains(target))
                    {
                        self.error(
                            "OSR-N0015",
                            format!(
                                "public alias `{}` requires its canonical target to be exported",
                                alias_spelling
                            ),
                            export.span,
                        );
                        continue;
                    }
                    self.aliases[alias_index].public = true;
                } else if let Some(binding) = self.bindings.get_mut(&id) {
                    binding.public = true;
                }
                self.exports.insert(id);
            }
        }
    }

    pub(in crate::hir) fn validate_boundary_signatures(&mut self, module: &ast::Module) {
        for item in &module.items {
            match &item.kind {
                AstItemKind::Defn(function) => {
                    let Some(name) = function.name.as_ref() else {
                        continue;
                    };
                    let is_exported = self
                        .resolve_global_name(&name.canonical)
                        .is_some_and(|binding| self.exports.contains(&binding));
                    if is_exported && self.strict {
                        self.validate_explicit_function_signature(function, "exported");
                    }
                }
                AstItemKind::Def(definition) => {
                    let is_exported = self
                        .resolve_global_name(&definition.name.canonical)
                        .is_some_and(|binding| self.exports.contains(&binding));
                    if is_exported && self.strict && definition.type_annotation.is_none() {
                        self.error(
                            "OSR-T0017",
                            format!(
                                "exported value `{}` requires an explicit type",
                                definition.name.spelling
                            ),
                            definition.span,
                        );
                    }
                }
                AstItemKind::Extern(external) => {
                    for declaration in &external.items {
                        match &declaration.kind {
                            AstItemKind::Defn(function) => {
                                self.validate_explicit_function_signature(function, "extern");
                            }
                            AstItemKind::Def(definition)
                                if definition.type_annotation.is_none() =>
                            {
                                self.error(
                                    "OSR-T0017",
                                    format!(
                                        "extern value `{}` requires an explicit type",
                                        definition.name.spelling
                                    ),
                                    definition.span,
                                );
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }

    pub(in crate::hir) fn validate_published_contracts(&mut self, module: &ast::Module) {
        for item in &module.items {
            match &item.kind {
                AstItemKind::Def(definition) => {
                    let Some(binding) = self.resolve_global_name(&definition.name.canonical) else {
                        continue;
                    };
                    if !self.exports.contains(&binding) {
                        continue;
                    }
                    let ty = self.binding_type(&binding);
                    self.validate_published_type(
                        &definition.name.spelling,
                        "value",
                        &ty,
                        definition.type_annotation.is_some(),
                        definition.span,
                    );
                }
                AstItemKind::Defn(function) => {
                    let Some(name) = function.name.as_ref() else {
                        continue;
                    };
                    let Some(binding) = self.resolve_global_name(&name.canonical) else {
                        continue;
                    };
                    if !self.exports.contains(&binding) {
                        continue;
                    }
                    let Type::Fn(signature) = self.binding_type(&binding) else {
                        continue;
                    };
                    for (parameter, ty) in function.params.iter().zip(&signature.parameters) {
                        self.validate_published_type(
                            &name.spelling,
                            &format!("parameter `{}`", parameter.name.spelling),
                            ty,
                            parameter.type_annotation.is_some(),
                            parameter.span,
                        );
                    }
                    self.validate_published_type(
                        &name.spelling,
                        "return",
                        &signature.return_type,
                        function.return_type.is_some(),
                        function.span,
                    );
                }
                _ => {}
            }
        }
    }

    fn validate_published_type(
        &mut self,
        name: &str,
        position: &str,
        ty: &Type,
        explicit: bool,
        span: Span,
    ) {
        if contains_unresolved_boundary_type(ty) {
            self.error(
                "OSR-T0050",
                format!(
                    "exported `{name}` {position} type is unresolved; published interfaces cannot contain `Unknown`"
                ),
                span,
            );
        } else if !explicit && contains_dynamic_boundary_type(ty) {
            self.error(
                "OSR-T0051",
                format!(
                    "exported `{name}` {position} inferred a dynamic `Any` boundary; annotate it explicitly"
                ),
                span,
            );
        }
    }

    pub(in crate::hir) fn validate_explicit_function_signature(
        &mut self,
        function: &ast::Function,
        boundary: &str,
    ) {
        let name = function
            .name
            .as_ref()
            .map_or("<anonymous>", |name| name.spelling.as_str());
        for parameter in &function.params {
            if parameter.pattern.is_some() {
                self.error(
                    "OSR-T0019",
                    format!(
                        "{boundary} function `{name}` requires named parameters; destructure inside its body"
                    ),
                    parameter.span,
                );
            }
            if parameter.type_annotation.is_none() {
                self.error(
                    "OSR-T0017",
                    format!(
                        "{boundary} function `{name}` parameter `{}` requires an explicit type",
                        parameter.name.spelling
                    ),
                    parameter.span,
                );
            }
        }
        if function.return_type.is_none() {
            self.error(
                "OSR-T0018",
                format!("{boundary} function `{name}` requires an explicit return type"),
                function.span,
            );
        }
    }

    pub(in crate::hir) fn lower_items(&mut self, module: &ast::Module) {
        let mut scope = Scope::default();
        let mut decorators = self.lower_python_decorators(module, &mut scope);
        for item in &module.items {
            let kind = match &item.kind {
                AstItemKind::Import(import)
                    if crate::stdlib::is_standard_namespace(&import.module.canonical)
                        && self.interfaces.is_none_or(|interfaces| {
                            interfaces.interface(&import.module.canonical).is_none()
                        }) =>
                {
                    None
                }
                AstItemKind::Import(import) => self
                    .global_id(import.alias.as_ref().unwrap_or(&import.module))
                    .map(|binding| {
                        ItemKind::Import(Import {
                            binding,
                            module: import.module.canonical.clone(),
                            python: false,
                        })
                    }),
                AstItemKind::PyImport(import) => {
                    let default = import.module.rsplit('.').next().unwrap_or(&import.module);
                    let canonical = import
                        .alias
                        .as_ref()
                        .map_or(default, |alias| alias.canonical.as_str());
                    self.resolve_global_name(canonical).map(|binding| {
                        ItemKind::Import(Import {
                            binding,
                            module: import.module.clone(),
                            python: true,
                        })
                    })
                }
                AstItemKind::Def(definition) => {
                    let Some(binding) = self.global_id(&definition.name) else {
                        continue;
                    };
                    if self.binding_is_dynamic(&binding) && definition.value.is_none() {
                        self.error(
                            "OSR-T0042",
                            format!(
                                "dynamic Var `{}` requires an initial value",
                                definition.name.spelling
                            ),
                            definition.span,
                        );
                    }
                    let value = definition
                        .value
                        .as_ref()
                        .map(|value| self.lower_expr(value, &mut scope));
                    if let Some(value) = &value {
                        let declared = self.binding_type(&binding);
                        if definition.type_annotation.is_some() {
                            self.check_assignable(&value.ty, &declared, value.span);
                        } else {
                            self.set_binding_type(&binding, value.ty.clone());
                        }
                    }
                    Some(ItemKind::Value(Value { binding, value }))
                }
                AstItemKind::Defn(function) => self.lower_function(function).map(|mut function| {
                    function.decorators = decorators.remove(&function.binding).unwrap_or_default();
                    ItemKind::Function(function)
                }),
                AstItemKind::Defstruct(structure) => {
                    self.lower_struct(structure).map(|mut structure| {
                        structure.decorators =
                            decorators.remove(&structure.binding).unwrap_or_default();
                        ItemKind::Struct(structure)
                    })
                }
                AstItemKind::Expr(expression) => {
                    Some(ItemKind::Expr(self.lower_expr(expression, &mut scope)))
                }
                AstItemKind::DefstaticSchema(schema) => {
                    Some(ItemKind::StaticSchema(schema.clone()))
                }
                AstItemKind::StaticRecord(record) => Some(ItemKind::StaticRecord(record.clone())),
                AstItemKind::Extern(external) => {
                    self.lower_extern_functions(external);
                    None
                }
                AstItemKind::ImportForSyntax(_)
                | AstItemKind::Export(_)
                | AstItemKind::Alias(_)
                | AstItemKind::PyDecorate(_)
                | AstItemKind::Defmacro(_)
                | AstItemKind::DefnForSyntax(_)
                | AstItemKind::Error(_) => None,
            };
            if let Some(kind) = kind {
                self.items.push(Item {
                    span: item.span,
                    metadata: item.metadata.clone(),
                    kind,
                });
            }
        }
    }

    fn lower_python_decorators(
        &mut self,
        module: &ast::Module,
        scope: &mut Scope,
    ) -> BTreeMap<BindingId, Vec<Expr>> {
        let targets = module
            .items
            .iter()
            .filter_map(|item| match &item.kind {
                AstItemKind::Defn(function) => function
                    .name
                    .as_ref()
                    .and_then(|name| self.resolve_global_name(&name.canonical)),
                AstItemKind::Defstruct(structure) => {
                    self.resolve_global_name(&structure.name.canonical)
                }
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        let mut result = BTreeMap::new();
        for item in &module.items {
            let AstItemKind::PyDecorate(declaration) = &item.kind else {
                continue;
            };
            let Some(target) = self.resolve_alias_target(&declaration.target.canonical) else {
                self.error(
                    "OSR-H0030",
                    format!(
                        "unknown Python decorator target `{}`",
                        declaration.target.spelling
                    ),
                    declaration.span,
                );
                continue;
            };
            if !targets.contains(&target) {
                self.error(
                    "OSR-H0031",
                    format!(
                        "Python decorator target `{}` must be a function or struct generated by this module",
                        declaration.target.spelling
                    ),
                    declaration.span,
                );
                continue;
            }
            if result.contains_key(&target) {
                self.error(
                    "OSR-H0032",
                    format!(
                        "Python decorator target `{}` has more than one py/decorate declaration",
                        declaration.target.spelling
                    ),
                    declaration.span,
                );
                continue;
            }
            result.insert(
                target,
                declaration
                    .decorators
                    .iter()
                    .map(|decorator| self.lower_expr(decorator, scope))
                    .collect(),
            );
        }
        result
    }
}

fn contains_unresolved_boundary_type(ty: &Type) -> bool {
    type_contains(ty, |candidate| {
        matches!(candidate, Type::Unknown | Type::Error)
    })
}

fn contains_dynamic_boundary_type(ty: &Type) -> bool {
    type_contains(ty, |candidate| matches!(candidate, Type::Any))
}

fn type_contains(ty: &Type, predicate: impl Copy + Fn(&Type) -> bool) -> bool {
    predicate(ty)
        || match ty {
            Type::Option(inner) | Type::List(inner) | Type::Vector(inner) | Type::Set(inner) => {
                type_contains(inner, predicate)
            }
            Type::Union(items) | Type::Tuple(items) => {
                items.iter().any(|item| type_contains(item, predicate))
            }
            Type::Map(key, value) => {
                type_contains(key, predicate) || type_contains(value, predicate)
            }
            Type::Fn(function) => {
                function
                    .parameters
                    .iter()
                    .any(|parameter| type_contains(parameter, predicate))
                    || type_contains(&function.return_type, predicate)
            }
            Type::Nominal { args, .. } => args
                .iter()
                .any(|argument| type_contains(argument, predicate)),
            _ => false,
        }
}
