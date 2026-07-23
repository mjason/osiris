use super::*;

impl Lowerer {
    pub(super) fn lower_item(&mut self, form: &Form) -> Item {
        let Some(parts) = list_parts(form) else {
            return Item::new(form, ItemKind::Expr(self.lower_expr(form)));
        };
        let Some(head) = parts.first().and_then(symbol_name) else {
            return Item::new(form, ItemKind::Expr(self.lower_expr(form)));
        };
        match DeclarationForm::from_name(&head.canonical) {
            Some(DeclarationForm::Import) => {
                Item::new(form, ItemKind::Import(self.lower_import(form, false)))
            }
            Some(DeclarationForm::ImportForSyntax) => Item::new(
                form,
                ItemKind::ImportForSyntax(self.lower_import(form, true)),
            ),
            Some(DeclarationForm::PythonImport) => {
                Item::new(form, ItemKind::PyImport(self.lower_py_import(form)))
            }
            Some(DeclarationForm::PythonDecorate) => {
                Item::new(form, ItemKind::PyDecorate(self.lower_py_decorate(form)))
            }
            Some(DeclarationForm::Export) => {
                Item::new(form, ItemKind::Export(self.lower_export(form)))
            }
            Some(DeclarationForm::Alias) => {
                Item::new(form, ItemKind::Alias(self.lower_alias(form)))
            }
            Some(DeclarationForm::Def) => Item::new(form, ItemKind::Def(self.lower_def(form))),
            Some(DeclarationForm::Defn) => Item::new(
                form,
                ItemKind::Defn(self.lower_function(
                    form,
                    FunctionPhase::Runtime,
                    true,
                    true,
                    false,
                )),
            ),
            Some(DeclarationForm::Defstruct) => {
                Item::new(form, ItemKind::Defstruct(self.lower_defstruct(form)))
            }
            Some(DeclarationForm::DefstaticSchema) => Item::new(
                form,
                ItemKind::DefstaticSchema(self.lower_defstatic_schema(form)),
            ),
            Some(DeclarationForm::StaticRecord) => {
                Item::new(form, ItemKind::StaticRecord(self.lower_static_record(form)))
            }
            Some(DeclarationForm::Extern) => {
                Item::new(form, ItemKind::Extern(self.lower_extern(form)))
            }
            Some(DeclarationForm::Defmacro) => {
                Item::new(form, ItemKind::Defmacro(self.lower_macro(form)))
            }
            Some(DeclarationForm::DefnForSyntax) => Item::new(
                form,
                ItemKind::DefnForSyntax(self.lower_function(
                    form,
                    FunctionPhase::Syntax,
                    true,
                    true,
                    false,
                )),
            ),
            None => Item::new(form, ItemKind::Expr(self.lower_expr(form))),
        }
    }

    pub(super) fn lower_import(&mut self, form: &Form, syntax: bool) -> Import {
        let info = NodeInfo::from_form(form);
        let parts = list_parts(form).unwrap_or_default();
        let module = match parts.get(1) {
            Some(part) => self.require_name(part, "import module"),
            None => {
                self.error(AST_MISSING_NAME, "import requires a module name", form.span);
                None
            }
        }
        .unwrap_or_else(error_name);
        let mut alias = None;
        let mut members = Vec::new();
        let mut index = 2;
        while index < parts.len() {
            let Some(keyword) = keyword_name(&parts[index]) else {
                self.error(
                    AST_INVALID_KEYWORD_ARGS,
                    "import options must use keyword clauses",
                    parts[index].span,
                );
                index += 1;
                continue;
            };
            let key = keyword.canonical.as_str();
            let Some(value) = parts.get(index + 1) else {
                self.error(
                    AST_EXPECTED_PAIR,
                    format!("import option `{}` requires a value", keyword.spelling),
                    parts[index].span,
                );
                break;
            };
            match key {
                ":as" => {
                    if alias.is_some() {
                        self.error(
                            AST_INVALID_KEYWORD_ARGS,
                            "duplicate `:as` option",
                            keyword_span(&parts[index]),
                        );
                    }
                    alias = self.require_name(value, "import alias");
                }
                ":refer" | ":only" => {
                    members.extend(self.lower_name_collection(value, "import member"));
                }
                _ => self.error(
                    AST_UNKNOWN_CLAUSE,
                    format!("unknown import option `{}`", keyword.spelling),
                    keyword_span(&parts[index]),
                ),
            }
            index += 2;
        }
        Import {
            span: info.span,
            metadata: info.metadata,
            module,
            alias,
            members,
            phase: if syntax {
                ImportPhase::Syntax
            } else {
                ImportPhase::Runtime
            },
        }
    }

    pub(super) fn lower_py_import(&mut self, form: &Form) -> PyImport {
        let info = NodeInfo::from_form(form);
        let parts = list_parts(form).unwrap_or_default();
        let module = parts
            .get(1)
            .and_then(|part| match &part.kind {
                FormKind::String(value) => Some(value.clone()),
                FormKind::Symbol(name) => Some(name.spelling.clone()),
                _ => {
                    self.error(
                        AST_INVALID_NAME,
                        "Python import module must be a string or symbol",
                        part.span,
                    );
                    None
                }
            })
            .unwrap_or_else(|| {
                if parts.get(1).is_none() {
                    self.error(
                        AST_MISSING_NAME,
                        "Python import requires a module",
                        form.span,
                    );
                }
                String::new()
            });
        let mut alias = None;
        let mut index = 2;
        while index < parts.len() {
            let Some(keyword) = keyword_name(&parts[index]) else {
                self.error(
                    AST_INVALID_KEYWORD_ARGS,
                    "Python import options must use keyword clauses",
                    parts[index].span,
                );
                index += 1;
                continue;
            };
            let Some(value) = parts.get(index + 1) else {
                self.error(
                    AST_EXPECTED_PAIR,
                    format!(
                        "Python import option `{}` requires a value",
                        keyword.spelling
                    ),
                    parts[index].span,
                );
                break;
            };
            if keyword.canonical == ":as" {
                alias = self.require_name(value, "Python import alias");
            } else {
                self.error(
                    AST_UNKNOWN_CLAUSE,
                    format!("unknown Python import option `{}`", keyword.spelling),
                    parts[index].span,
                );
            }
            index += 2;
        }
        PyImport {
            span: info.span,
            metadata: info.metadata,
            module,
            alias,
        }
    }

    pub(super) fn lower_py_decorate(&mut self, form: &Form) -> PyDecorate {
        let info = NodeInfo::from_form(form);
        let parts = list_parts(form).unwrap_or_default();
        let target = match parts.get(1) {
            Some(part) => self.require_name(part, "Python decorator target"),
            None => {
                self.error(
                    AST_MISSING_NAME,
                    "py/decorate requires a target name",
                    form.span,
                );
                None
            }
        }
        .unwrap_or_else(error_name);
        let decorators = parts
            .iter()
            .skip(2)
            .map(|part| self.lower_expr(part))
            .collect::<Vec<_>>();
        if decorators.is_empty() {
            self.error(
                AST_WRONG_SHAPE,
                "py/decorate requires at least one decorator expression",
                form.span,
            );
        }
        PyDecorate {
            span: info.span,
            metadata: info.metadata,
            target,
            target_span: parts.get(1).map_or(form.span, |target| target.span),
            decorators,
        }
    }

    pub(super) fn lower_export(&mut self, form: &Form) -> Export {
        let info = NodeInfo::from_form(form);
        let parts = list_parts(form).unwrap_or_default();
        let names = parts
            .get(1)
            .map(|value| self.lower_name_collection(value, "exported name"))
            .unwrap_or_else(|| {
                self.error(
                    AST_EXPECTED_VECTOR,
                    "export expects a vector or list of names",
                    form.span,
                );
                Vec::new()
            });
        if parts.len() > 2 {
            self.error(
                AST_WRONG_SHAPE,
                "export accepts one name collection",
                form.span,
            );
        }
        Export {
            span: info.span,
            metadata: info.metadata,
            names,
        }
    }

    pub(super) fn lower_alias(&mut self, form: &Form) -> Alias {
        let info = NodeInfo::from_form(form);
        let parts = list_parts(form).unwrap_or_default();
        if parts.len() != 3 {
            self.error(
                AST_WRONG_SHAPE,
                "alias expects a local and target name",
                form.span,
            );
        }
        let local = parts
            .get(1)
            .and_then(|part| self.require_name(part, "alias local name"))
            .unwrap_or_else(error_name);
        let target = parts
            .get(2)
            .and_then(|part| self.require_name(part, "alias target name"))
            .unwrap_or_else(error_name);
        Alias {
            span: info.span,
            metadata: info.metadata,
            local,
            target,
        }
    }

    pub(super) fn lower_def(&mut self, form: &Form) -> Def {
        let info = NodeInfo::from_form(form);
        let parts = list_parts(form).unwrap_or_default();
        if parts.len() < 2 || parts.len() > 4 {
            self.error(
                AST_WRONG_SHAPE,
                "def expects a name, optional type, and value",
                form.span,
            );
        }
        let name_form = parts.get(1);
        let name = name_form
            .and_then(|part| self.require_name(part, "def name"))
            .unwrap_or_else(error_name);
        let metadata = merge_declaration_metadata(
            info.metadata,
            name_form.map_or(&[], |name| name.metadata.as_slice()),
        );
        let (type_annotation, value) = match parts.len() {
            0..=2 => (None, parts.get(2).map(|part| self.lower_expr(part))),
            3 => (Some(self.lower_type(&parts[2])), None),
            _ => (
                Some(self.lower_type(&parts[2])),
                Some(self.lower_expr(&parts[3])),
            ),
        };
        if parts.len() == 3 {
            // A three-form declaration is overwhelmingly the common `(def n
            // value)` shape.  Treat it as a value unless the middle datum is
            // recognisably type-like. A call is also a list, so collection
            // shape alone cannot distinguish `(Array Float)` from
            // `(Point :x 1)`.
            let candidate = &parts[2];
            if !looks_like_type(candidate) {
                return Def {
                    span: info.span,
                    metadata,
                    name,
                    type_annotation: None,
                    value: Some(self.lower_expr(candidate)),
                };
            }
        }
        Def {
            span: info.span,
            metadata,
            name,
            type_annotation,
            value,
        }
    }

    pub(super) fn lower_function(
        &mut self,
        form: &Form,
        phase: FunctionPhase,
        named: bool,
        body_required: bool,
        extern_declaration: bool,
    ) -> Function {
        let info = NodeInfo::from_form(form);
        let parts = list_parts(form).unwrap_or_default();
        let mut index = 1;
        let name_form = if named { parts.get(index) } else { None };
        let name = if named {
            let result = match parts.get(index) {
                Some(part) => self.require_name(part, "function name"),
                None => {
                    self.error(
                        AST_MISSING_NAME,
                        "function declaration requires a name",
                        form.span,
                    );
                    None
                }
            };
            index += 1;
            Some(result.unwrap_or_else(error_name))
        } else {
            None
        };
        let params_form = parts.get(index);
        if params_form.is_none() {
            self.error(
                AST_EXPECTED_VECTOR,
                "function expects a parameter vector",
                form.span,
            );
        }
        let params = params_form
            .map(|part| self.lower_params(part, phase))
            .unwrap_or_default();
        index += usize::from(params_form.is_some());

        let metadata_return_type = name_form
            .map(|name| self.lower_metadata_type(&name.metadata, "function return"))
            .unwrap_or_default();
        let explicit_return_type = if parts
            .get(index)
            .and_then(symbol_name)
            .is_some_and(|arrow| arrow.canonical == "->")
        {
            index += 1;
            match parts.get(index) {
                Some(type_form) => {
                    index += 1;
                    Some(self.lower_type(type_form))
                }
                None => {
                    self.error(AST_WRONG_SHAPE, "`->` requires a return type", form.span);
                    None
                }
            }
        } else {
            None
        };
        if explicit_return_type.is_some() && metadata_return_type.present {
            self.report_type_annotation_conflict(
                "function return",
                name_form.map_or(form.span, |name| name.span),
            );
        }
        let return_type = explicit_return_type.or(metadata_return_type.annotation);
        let contract = if extern_declaration
            && parts
                .get(index)
                .and_then(keyword_name)
                .is_some_and(|name| name.canonical.trim_start_matches(':') == "contract")
        {
            let clause = &parts[index];
            index += 1;
            match parts.get(index) {
                Some(contract) => {
                    index += 1;
                    self.lower_extern_contract(contract)
                }
                None => {
                    self.error(
                        AST_INVALID_CONTRACT,
                        "`:contract` requires a declaration map",
                        clause.span,
                    );
                    None
                }
            }
        } else {
            None
        };
        let body = if extern_declaration {
            if let Some(unexpected) = parts.get(index) {
                self.error(
                    AST_WRONG_SHAPE,
                    "extern function declaration cannot contain a body or extra clauses",
                    unexpected.span,
                );
            }
            Vec::new()
        } else {
            parts[index..]
                .iter()
                .map(|part| self.lower_expr(part))
                .collect::<Vec<_>>()
        };
        if body_required && body.is_empty() {
            self.error(AST_WRONG_SHAPE, "function body cannot be empty", form.span);
        }
        Function {
            span: info.span,
            metadata: info.metadata,
            name,
            params,
            return_type,
            contract,
            body,
            phase,
            phase_form: (phase != FunctionPhase::Runtime).then(|| form.clone()),
        }
    }
}
