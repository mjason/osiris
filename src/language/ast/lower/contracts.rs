use super::*;

impl Lowerer {
    pub(super) fn lower_extern_contract(&mut self, form: &Form) -> Option<ExternContract> {
        let (entries, mut valid) = self.contract_entries(form, "extern contract");
        let mut id = None;
        let mut summaries = CallSummaries::unknown();
        for (key, value) in entries {
            match key.as_str() {
                "id" => match &value.kind {
                    FormKind::String(value)
                        if !value.is_empty()
                            && value.trim() == value
                            && !value.chars().any(char::is_control) =>
                    {
                        id = Some(value.clone());
                    }
                    _ => {
                        valid = false;
                        self.error(
                            AST_INVALID_CONTRACT,
                            "extern contract `:id` must be a non-empty stable string",
                            value.span,
                        );
                    }
                },
                "effects" => match self.lower_contract_effects(value) {
                    Some(effects) => summaries.effects = effects,
                    None => valid = false,
                },
                "temporal" => match self.lower_contract_temporal(value) {
                    Some(temporal) => summaries.temporal = temporal,
                    None => valid = false,
                },
                "data" => match self.lower_contract_data(value) {
                    Some(data) => summaries.data = data,
                    None => valid = false,
                },
                _ => {
                    valid = false;
                    self.error(
                        AST_UNKNOWN_CLAUSE,
                        format!("unknown extern contract field `:{key}`"),
                        value.span,
                    );
                }
            }
        }
        let Some(id) = id else {
            self.error(
                AST_INVALID_CONTRACT,
                "extern contract requires a stable `:id`",
                form.span,
            );
            return None;
        };
        valid.then_some(ExternContract {
            span: form.span,
            id,
            summaries,
        })
    }

    pub(super) fn lower_contract_effects(&mut self, form: &Form) -> Option<EffectRow> {
        if let FormKind::Keyword(name) = &form.kind {
            return match name.canonical.trim_start_matches(':') {
                "pure" => Some(EffectRow::pure()),
                "unknown" => Some(EffectRow::unknown()),
                _ => {
                    self.error(
                        AST_INVALID_CONTRACT,
                        "contract `:effects` must be `:pure`, `:unknown`, or a vector",
                        form.span,
                    );
                    None
                }
            };
        }
        let FormKind::Vector(values) = &form.kind else {
            self.error(
                AST_INVALID_CONTRACT,
                "contract `:effects` must be `:pure`, `:unknown`, or a vector",
                form.span,
            );
            return None;
        };
        let mut effects = BTreeSet::new();
        let mut valid = true;
        for value in values {
            let Some(name) = contract_name(value) else {
                valid = false;
                self.error(
                    AST_INVALID_CONTRACT,
                    "contract effects must be keyword or symbol names",
                    value.span,
                );
                continue;
            };
            let effect = match name.as_str() {
                "io" => Effect::Io,
                "throw" => Effect::Throw,
                "mutation" => Effect::Mutation,
                "hidden-state" => Effect::HiddenState,
                "python-dynamic" => Effect::PythonDynamic,
                custom if custom.contains('/') => Effect::Custom(custom.to_owned()),
                _ => {
                    valid = false;
                    self.error(
                        AST_INVALID_CONTRACT,
                        format!("unknown contract effect `{name}`"),
                        value.span,
                    );
                    continue;
                }
            };
            if !effects.insert(effect) {
                valid = false;
                self.error(
                    AST_INVALID_CONTRACT,
                    format!("duplicate contract effect `{name}`"),
                    value.span,
                );
            }
        }
        valid.then_some(EffectRow {
            effects,
            open: false,
        })
    }

    pub(super) fn lower_contract_temporal(&mut self, form: &Form) -> Option<TemporalSummary> {
        let (entries, mut valid) = self.contract_entries(form, "temporal contract");
        let mut summary = TemporalSummary::unknown();
        for (key, value) in entries {
            match key.as_str() {
                "past" => match self.lower_contract_bound(value) {
                    Some(bound) => summary.past = bound,
                    None => valid = false,
                },
                "future" => match self.lower_contract_bound(value) {
                    Some(bound) => summary.future = bound,
                    None => valid = false,
                },
                "availability" => match self.lower_contract_availability(value) {
                    Some(availability) => summary.availability = availability,
                    None => valid = false,
                },
                _ => {
                    valid = false;
                    self.error(
                        AST_UNKNOWN_CLAUSE,
                        format!("unknown temporal contract field `:{key}`"),
                        value.span,
                    );
                }
            }
        }
        valid.then_some(summary)
    }

    pub(super) fn lower_contract_bound(&mut self, form: &Form) -> Option<TemporalBound> {
        match &form.kind {
            FormKind::Integer(value) => match value.parse::<u64>() {
                Ok(value) => Some(TemporalBound::Finite(value)),
                Err(_) => {
                    self.error(
                        AST_INVALID_CONTRACT,
                        "temporal bounds must be non-negative integers",
                        form.span,
                    );
                    None
                }
            },
            FormKind::String(value) if !value.is_empty() => {
                Some(TemporalBound::Symbolic(value.clone()))
            }
            FormKind::Symbol(name) if !name.canonical.is_empty() => {
                Some(TemporalBound::Symbolic(name.canonical.clone()))
            }
            FormKind::Keyword(name) => match name.canonical.trim_start_matches(':') {
                "unbounded" => Some(TemporalBound::Unbounded),
                "unknown" => Some(TemporalBound::Unknown),
                _ => {
                    self.error(
                        AST_INVALID_CONTRACT,
                        "temporal bound keyword must be `:unbounded` or `:unknown`",
                        form.span,
                    );
                    None
                }
            },
            _ => {
                self.error(
                    AST_INVALID_CONTRACT,
                    "temporal bound must be a non-negative integer, symbol, string, `:unbounded`, or `:unknown`",
                    form.span,
                );
                None
            }
        }
    }

    pub(super) fn lower_contract_availability(&mut self, form: &Form) -> Option<Availability> {
        match &form.kind {
            FormKind::Keyword(name) => match name.canonical.trim_start_matches(':') {
                "immediate" => Some(Availability::Immediate),
                "unknown" => Some(Availability::Unknown),
                value if !value.is_empty() => Some(Availability::Named(value.to_owned())),
                _ => None,
            },
            FormKind::Symbol(name) if !name.canonical.is_empty() => {
                Some(Availability::Named(name.canonical.clone()))
            }
            FormKind::String(value) if !value.is_empty() => {
                Some(Availability::Named(value.clone()))
            }
            _ => {
                self.error(
                    AST_INVALID_CONTRACT,
                    "availability must be `:immediate`, `:unknown`, or a non-empty static name",
                    form.span,
                );
                None
            }
        }
    }

    pub(super) fn lower_contract_data(&mut self, form: &Form) -> Option<DataProperties> {
        let (entries, mut valid) = self.contract_entries(form, "data contract");
        let mut data = DataProperties::unknown();
        for (key, value) in entries {
            match key.as_str() {
                "schema" => match contract_optional_name(value) {
                    Ok(schema) => data.schema = schema,
                    Err(()) => {
                        valid = false;
                        self.error(
                            AST_INVALID_CONTRACT,
                            "data `:schema` must be none or a static name",
                            value.span,
                        );
                    }
                },
                "axes" => match contract_optional_names(value) {
                    Ok(axes) => data.axes = axes,
                    Err(()) => {
                        valid = false;
                        self.error(
                            AST_INVALID_CONTRACT,
                            "data `:axes` must be none or a vector of static names",
                            value.span,
                        );
                    }
                },
                "alignment" => match contract_name(value).as_deref() {
                    Some("positional") => data.alignment = Alignment::Positional,
                    Some("labelled") => data.alignment = Alignment::Labelled,
                    Some("as-of") => data.alignment = Alignment::AsOf,
                    Some("unknown") => data.alignment = Alignment::Unknown,
                    _ => {
                        valid = false;
                        self.error(
                            AST_INVALID_CONTRACT,
                            "data `:alignment` must be :positional, :labelled, :as-of, or :unknown",
                            value.span,
                        );
                    }
                },
                "ordered-by" => match contract_optional_names(value) {
                    Ok(keys) => data.ordered_by = keys,
                    Err(()) => {
                        valid = false;
                        self.error(
                            AST_INVALID_CONTRACT,
                            "data `:ordered-by` must be none or a vector of static names",
                            value.span,
                        );
                    }
                },
                "unique-by" => match contract_optional_names(value) {
                    Ok(keys) => data.unique_by = keys,
                    Err(()) => {
                        valid = false;
                        self.error(
                            AST_INVALID_CONTRACT,
                            "data `:unique-by` must be none or a vector of static names",
                            value.span,
                        );
                    }
                },
                "preserves-length" => match contract_optional_bool(value) {
                    Ok(flag) => data.preserves_length = flag,
                    Err(()) => {
                        valid = false;
                        self.error(
                            AST_INVALID_CONTRACT,
                            "data `:preserves-length` must be Bool or none",
                            value.span,
                        );
                    }
                },
                "materializes" => match contract_optional_bool(value) {
                    Ok(flag) => data.materializes = flag,
                    Err(()) => {
                        valid = false;
                        self.error(
                            AST_INVALID_CONTRACT,
                            "data `:materializes` must be Bool or none",
                            value.span,
                        );
                    }
                },
                "reshapes" => match contract_optional_bool(value) {
                    Ok(flag) => data.reshapes = flag,
                    Err(()) => {
                        valid = false;
                        self.error(
                            AST_INVALID_CONTRACT,
                            "data `:reshapes` must be Bool or none",
                            value.span,
                        );
                    }
                },
                "nulls-possible" => match contract_optional_bool(value) {
                    Ok(flag) => data.nulls_possible = flag,
                    Err(()) => {
                        valid = false;
                        self.error(
                            AST_INVALID_CONTRACT,
                            "data `:nulls-possible` must be Bool or none",
                            value.span,
                        );
                    }
                },
                "nan-possible" => match contract_optional_bool(value) {
                    Ok(flag) => data.nan_possible = flag,
                    Err(()) => {
                        valid = false;
                        self.error(
                            AST_INVALID_CONTRACT,
                            "data `:nan-possible` must be Bool or none",
                            value.span,
                        );
                    }
                },
                "nonfinite-possible" => match contract_optional_bool(value) {
                    Ok(flag) => data.nonfinite_possible = flag,
                    Err(()) => {
                        valid = false;
                        self.error(
                            AST_INVALID_CONTRACT,
                            "data `:nonfinite-possible` must be Bool or none",
                            value.span,
                        );
                    }
                },
                "nonfinite-policy" => match contract_optional_name(value) {
                    Ok(policy) => data.nonfinite_policy = policy,
                    Err(()) => {
                        valid = false;
                        self.error(
                            AST_INVALID_CONTRACT,
                            "data `:nonfinite-policy` must be none or a static name",
                            value.span,
                        );
                    }
                },
                _ => {
                    valid = false;
                    self.error(
                        AST_UNKNOWN_CLAUSE,
                        format!("unknown data contract field `:{key}`"),
                        value.span,
                    );
                }
            }
        }
        valid.then_some(data)
    }

    pub(super) fn contract_entries<'form>(
        &mut self,
        form: &'form Form,
        context: &str,
    ) -> (Vec<(String, &'form Form)>, bool) {
        let FormKind::Map(parts) = &form.kind else {
            self.error(
                AST_INVALID_CONTRACT,
                format!("{context} must be a map"),
                form.span,
            );
            return (Vec::new(), false);
        };
        let mut valid = true;
        if parts.len() % 2 != 0 {
            valid = false;
            self.error(
                AST_INVALID_CONTRACT,
                format!("{context} requires key/value pairs"),
                form.span,
            );
        }
        let mut seen = BTreeSet::new();
        let mut entries = Vec::new();
        for pair in parts.chunks_exact(2) {
            let FormKind::Keyword(name) = &pair[0].kind else {
                valid = false;
                self.error(
                    AST_INVALID_CONTRACT,
                    format!("{context} keys must be keywords"),
                    pair[0].span,
                );
                continue;
            };
            let key = name.canonical.trim_start_matches(':').to_owned();
            if !seen.insert(key.clone()) {
                valid = false;
                self.error(
                    AST_INVALID_CONTRACT,
                    format!("duplicate {context} field `:{key}`"),
                    pair[0].span,
                );
                continue;
            }
            entries.push((key, &pair[1]));
        }
        (entries, valid)
    }
}
