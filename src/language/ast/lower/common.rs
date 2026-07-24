use super::*;

impl Lowerer {
    pub(super) fn error(&mut self, code: &'static str, message: impl Into<String>, span: Span) {
        self.diagnostics
            .push(Diagnostic::error(code, message, span));
    }

    pub(super) fn lower_metadata_type(
        &mut self,
        metadata: &[MetadataEntry],
        context: &str,
    ) -> MetadataTypeAnnotation {
        let declared = metadata
            .iter()
            .find(|entry| metadata_key(&entry.key) == Some("type"));
        let tagged = metadata
            .iter()
            .find(|entry| metadata_key(&entry.key) == Some("tag"));
        let present = declared.is_some() || tagged.is_some();

        if let (Some(declared), Some(_)) = (declared, tagged) {
            self.error(
                AST_CONFLICTING_TYPE_ANNOTATION,
                format!(
                    "{context} has both `:type` and Clojure `:tag` metadata; `:type` takes precedence"
                ),
                declared.key.span,
            );
        }

        let Some(entry) = declared.or(tagged) else {
            return MetadataTypeAnnotation::default();
        };
        if matches!(entry.value.kind, FormKind::Bool(true)) {
            let message = if declared.is_some() {
                format!(
                    "`^:type` on a {context} is only a marker and does not name a type; use `^{{:type T}}` or `^T`"
                )
            } else {
                format!("Clojure `:tag` metadata on a {context} must name a type")
            };
            self.error(AST_INVALID_TYPE_METADATA, message, entry.value.span);
            return MetadataTypeAnnotation {
                present,
                annotation: None,
            };
        }
        if let Err(error) = parse_type(&entry.value, &BTreeMap::new()) {
            self.error(
                AST_INVALID_TYPE_METADATA,
                format!("invalid {context} type metadata: {error}"),
                error.span,
            );
            return MetadataTypeAnnotation {
                present,
                annotation: None,
            };
        }
        let mut annotation = self.lower_metadata_type_form(&entry.value);
        annotation.metadata = metadata.to_vec();
        MetadataTypeAnnotation {
            present,
            annotation: Some(annotation),
        }
    }

    pub(super) fn lower_type_parameters(
        &mut self,
        metadata: &[MetadataEntry],
        context: &str,
    ) -> Vec<Name> {
        let entries = metadata
            .iter()
            .filter(|entry| metadata_key(&entry.key) == Some("type-params"))
            .collect::<Vec<_>>();
        let Some(entry) = entries.first() else {
            return Vec::new();
        };
        if entries.len() > 1 {
            self.error(
                AST_INVALID_TYPE_METADATA,
                format!("{context} repeats `:type-params` metadata"),
                entry.key.span,
            );
        }
        let FormKind::Vector(values) = &entry.value.kind else {
            self.error(
                AST_INVALID_TYPE_METADATA,
                format!("{context} `:type-params` must be a vector of symbols"),
                entry.value.span,
            );
            return Vec::new();
        };
        let mut names = Vec::new();
        let mut seen = BTreeSet::new();
        for value in values {
            let Some(name) = symbol_name(value) else {
                self.error(
                    AST_INVALID_TYPE_METADATA,
                    format!("{context} type parameters must be symbols"),
                    value.span,
                );
                continue;
            };
            if !seen.insert(name.canonical.clone()) {
                self.error(
                    AST_INVALID_TYPE_METADATA,
                    format!("{context} repeats type parameter `{}`", name.spelling),
                    value.span,
                );
                continue;
            }
            names.push(name);
        }
        names
    }

    pub(super) fn lower_metadata_type_form(&mut self, form: &Form) -> TypeExpr {
        let FormKind::Symbol(name) = &form.kind else {
            return self.lower_type(form);
        };
        let arity = match name.canonical.as_str() {
            "Vector" | "List" | "Set" | "Option" => 1,
            "Map" => 2,
            _ => return self.lower_type(form),
        };
        let any = TypeExpr {
            span: form.span,
            metadata: Vec::new(),
            kind: TypeExprKind::Name(Name {
                spelling: "Any".to_owned(),
                canonical: "Any".to_owned(),
            }),
        };
        TypeExpr {
            span: form.span,
            metadata: Vec::new(),
            kind: TypeExprKind::Apply {
                constructor: Box::new(self.lower_type(form)),
                args: vec![any; arity],
            },
        }
    }

    pub(super) fn is_head(&self, form: &Form, expected: &str) -> bool {
        list_parts(form)
            .and_then(|parts| parts.first())
            .and_then(symbol_name)
            .is_some_and(|name| name.canonical == expected)
    }

    pub(super) fn lower_module_header(&mut self, form: &Form) -> (Option<Name>, Metadata) {
        let Some(parts) = list_parts(form) else {
            self.error(
                AST_EXPECTED_LIST,
                "module declaration must be a list",
                form.span,
            );
            return (None, form.metadata.clone());
        };
        if parts.len() != 2 {
            self.error(
                AST_WRONG_SHAPE,
                "module declaration expects exactly one module name",
                form.span,
            );
        }
        let name = match parts.get(1) {
            Some(part) => self.require_name(part, "module name"),
            None => {
                self.error(
                    AST_MISSING_NAME,
                    "module declaration requires a name",
                    form.span,
                );
                None
            }
        };
        (name, form.metadata.clone())
    }
}
