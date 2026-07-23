use super::*;

impl Lowerer {
    pub(super) fn lower_macro(&mut self, form: &Form) -> Macro {
        let function = self.lower_function(form, FunctionPhase::Macro, true, true, false);
        Macro {
            span: function.span,
            metadata: function.metadata,
            name: function.name.unwrap_or_else(error_name),
            params: function.params,
            return_type: function.return_type,
            body: function.body,
            phase_form: function
                .phase_form
                .expect("macro lowering retains its phase-1 form"),
        }
    }

    pub(super) fn lower_defstruct(&mut self, form: &Form) -> Defstruct {
        let info = NodeInfo::from_form(form);
        let parts = list_parts(form).unwrap_or_default();
        let (name, type_params, mut index) = match parts.get(1) {
            Some(Form {
                kind: FormKind::List(header),
                ..
            }) if !header.is_empty() => {
                let name = header
                    .first()
                    .and_then(|part| self.require_name(part, "struct name"))
                    .unwrap_or_else(error_name);
                let params = header[1..]
                    .iter()
                    .filter_map(|part| self.require_name(part, "struct type parameter"))
                    .collect::<Vec<_>>();
                (name, params, 2)
            }
            Some(part) => (
                self.require_name(part, "struct name")
                    .unwrap_or_else(error_name),
                Vec::new(),
                2,
            ),
            None => {
                self.error(AST_MISSING_NAME, "defstruct requires a name", form.span);
                (error_name(), Vec::new(), 1)
            }
        };
        let mut doc = None;
        let mut fields = Vec::new();
        let mut checks = Vec::new();
        while let Some(part) = parts.get(index) {
            match &part.kind {
                FormKind::String(value) if doc.is_none() => doc = Some(value.clone()),
                FormKind::String(_) => self.error(
                    AST_WRONG_SHAPE,
                    "defstruct accepts at most one documentation string",
                    part.span,
                ),
                FormKind::Vector(_) => fields.push(self.lower_field(part)),
                FormKind::List(clause)
                    if clause
                        .first()
                        .and_then(symbol_name)
                        .is_some_and(|name| name.canonical == "check") =>
                {
                    if clause.len() < 2 {
                        self.error(
                            AST_WRONG_SHAPE,
                            "struct check requires a condition",
                            part.span,
                        );
                    } else {
                        checks.push(StructCheck {
                            span: part.span,
                            metadata: part.metadata.clone(),
                            condition: self.lower_expr(&clause[1]),
                            message: clause.get(2).map(|message| self.lower_expr(message)),
                        });
                        if clause.len() > 3 {
                            self.error(
                                AST_WRONG_SHAPE,
                                "struct check accepts a condition and optional message",
                                part.span,
                            );
                        }
                    }
                }
                _ => self.error(
                    AST_UNKNOWN_CLAUSE,
                    "expected a field vector, documentation string, or check clause",
                    part.span,
                ),
            }
            index += 1;
        }
        Defstruct {
            span: info.span,
            metadata: info.metadata,
            name,
            type_params,
            doc,
            fields,
            checks,
        }
    }

    pub(super) fn lower_field(&mut self, form: &Form) -> Field {
        let info = NodeInfo::from_form(form);
        let parts = match &form.kind {
            FormKind::Vector(parts) => parts.as_slice(),
            _ => &[],
        };
        if parts.is_empty() || parts.len() > 4 {
            self.error(
                AST_WRONG_SHAPE,
                "field expects a name, type, and optional default",
                form.span,
            );
        }
        let name = parts
            .first()
            .and_then(|part| self.require_name(part, "field name"))
            .unwrap_or_else(error_name);
        let mut type_annotation = None;
        let mut default = None;
        let mut index = 1;
        if let Some(part) = parts.get(index) {
            if is_equal_symbol(part) {
                self.error(
                    AST_WRONG_SHAPE,
                    "field default requires a type before `=`",
                    part.span,
                );
            } else {
                type_annotation = Some(self.lower_type(part));
                index += 1;
            }
        }
        if parts.get(index).is_some_and(is_equal_symbol) {
            index += 1;
            if let Some(value) = parts.get(index) {
                default = Some(self.lower_expr(value));
                index += 1;
            } else {
                self.error(
                    AST_WRONG_SHAPE,
                    "field `=` requires a default expression",
                    form.span,
                );
            }
        }
        if index < parts.len() {
            self.error(
                AST_WRONG_SHAPE,
                "unexpected forms after field declaration",
                form.span,
            );
        }
        Field {
            span: info.span,
            metadata: info.metadata,
            name,
            type_annotation,
            default,
        }
    }

    pub(super) fn lower_defstatic_schema(&mut self, form: &Form) -> DefstaticSchema {
        let info = NodeInfo::from_form(form);
        let parts = list_parts(form).unwrap_or_default();
        let name = parts
            .get(1)
            .and_then(|part| self.require_name(part, "schema name"))
            .unwrap_or_else(error_name);
        if parts.len() < 2 {
            self.error(
                AST_MISSING_NAME,
                "defstatic-schema requires a name",
                form.span,
            );
        }
        let body = parts[2..]
            .iter()
            .map(|part| self.lower_expr(part))
            .collect();
        DefstaticSchema {
            span: info.span,
            metadata: info.metadata,
            name,
            body,
        }
    }

    pub(super) fn lower_static_record(&mut self, form: &Form) -> StaticRecord {
        let info = NodeInfo::from_form(form);
        let parts = list_parts(form).unwrap_or_default();
        let schema = parts
            .get(1)
            .and_then(|part| self.require_name(part, "record schema"))
            .unwrap_or_else(error_name);
        let owner = parts
            .get(2)
            .and_then(|part| self.require_name(part, "record owner"))
            .unwrap_or_else(error_name);
        let tail = parts.get(3..).unwrap_or_default();
        let fields = match tail {
            [
                Form {
                    kind: FormKind::Map(entries),
                    span,
                    ..
                },
            ] => self.lower_static_record_fields(entries, *span),
            [
                Form {
                    kind: FormKind::Map(entries),
                    span,
                    ..
                },
                ..,
            ] => {
                self.error(
                    AST_WRONG_SHAPE,
                    "static-record accepts exactly one field map",
                    form.span,
                );
                self.lower_static_record_fields(entries, *span)
            }
            [] => {
                self.error(
                    AST_WRONG_SHAPE,
                    "static-record expects a schema, owner, and one field map",
                    form.span,
                );
                Vec::new()
            }
            _ => {
                // Keep lowering the pre-map spelling so an editor can still
                // inspect its fields, while making the canonical shape clear.
                self.error(
                    AST_WRONG_SHAPE,
                    "static-record fields must be provided as a single map",
                    form.span,
                );
                self.lower_static_record_fields(tail, form.span)
            }
        };
        StaticRecord {
            span: info.span,
            metadata: info.metadata,
            schema,
            owner,
            fields,
        }
    }

    pub(super) fn lower_static_record_fields(
        &mut self,
        entries: &[Form],
        span: Span,
    ) -> Vec<(Name, Expr)> {
        if entries.len() % 2 != 0 {
            self.error(
                AST_EXPECTED_PAIR,
                "static-record fields require key/value pairs",
                span,
            );
        }
        entries
            .chunks(2)
            .filter_map(|pair| {
                let key = pair
                    .first()
                    .and_then(|part| self.require_record_field(part))?;
                let value = pair.get(1).map(|part| self.lower_expr(part))?;
                Some((key, value))
            })
            .collect()
    }

    pub(super) fn require_record_field(&mut self, form: &Form) -> Option<Name> {
        match &form.kind {
            FormKind::Keyword(name) | FormKind::Symbol(name) => Some(name.clone()),
            _ => {
                self.error(
                    AST_INVALID_NAME,
                    "record field must be a keyword or symbol",
                    form.span,
                );
                None
            }
        }
    }

    pub(super) fn lower_extern(&mut self, form: &Form) -> Extern {
        let info = NodeInfo::from_form(form);
        let parts = list_parts(form).unwrap_or_default();
        if parts.len() < 3 {
            self.error(
                AST_WRONG_SHAPE,
                "extern expects a backend, module, and declarations",
                form.span,
            );
        }
        let backend = match parts.get(1) {
            Some(part) => self.require_name(part, "extern backend"),
            None => {
                self.error(AST_MISSING_NAME, "extern requires a backend", form.span);
                None
            }
        }
        .unwrap_or_else(error_name);
        let module = parts
            .get(2)
            .and_then(|part| match &part.kind {
                FormKind::String(value) => Some(value.clone()),
                FormKind::Symbol(name) => Some(name.spelling.clone()),
                _ => {
                    self.error(
                        AST_INVALID_NAME,
                        "extern module must be a string or symbol",
                        part.span,
                    );
                    None
                }
            })
            .unwrap_or_else(|| {
                if parts.get(2).is_none() {
                    self.error(AST_MISSING_NAME, "extern requires a module", form.span);
                }
                String::new()
            });
        let mut items = Vec::new();
        for declaration in parts.get(3..).unwrap_or_default() {
            let item = if self.is_head(declaration, "defn") {
                Item::new(
                    declaration,
                    ItemKind::Defn(self.lower_function(
                        declaration,
                        FunctionPhase::Runtime,
                        true,
                        false,
                        true,
                    )),
                )
            } else {
                self.lower_item(declaration)
            };
            items.push(item);
        }
        Extern {
            span: info.span,
            metadata: info.metadata,
            backend,
            module,
            items,
        }
    }
}
