//! Versioned semantic data for editors, agents, and interface tooling.
//!
//! This module is a projection of compiler::Analysis. It never parses or
//! infers types itself.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::{Value as JsonValue, json};

use crate::{
    ast,
    compiler::Analysis,
    diagnostic::Diagnostic,
    hir::{self, Expr, ExprKind, ItemKind},
    macro_expand::ExpansionTrace,
    name::BindingKind,
    source::Span,
    syntax::{Form, FormKind, MetadataEntry},
    types::{CallSummaries, DataProperties, EffectRow, TemporalSummary, Type},
};

/// Bumped when the JSON shape of SemanticDocument changes incompatibly.
pub const SEMANTIC_DOCUMENT_VERSION: u32 = 1;

/// A localized label. Locale only affects presentation and completion order.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct LocalizedLabel {
    #[serde(rename = "zh-CN")]
    pub zh_cn: String,
    pub en: String,
}

impl LocalizedLabel {
    #[must_use]
    pub fn new(canonical: impl Into<String>, chinese: Option<String>) -> Self {
        let en = canonical.into();
        Self {
            zh_cn: chinese.unwrap_or_else(|| en.clone()),
            en,
        }
    }

    #[must_use]
    pub fn for_locale(&self, locale: &str) -> &str {
        if locale.eq_ignore_ascii_case("zh-cn") || locale.eq_ignore_ascii_case("zh") {
            &self.zh_cn
        } else {
            &self.en
        }
    }
}

/// Author-written or phase-1 macro metadata, retaining the reader datum.
#[derive(Clone, Debug, Serialize)]
pub struct AuthoredMetadata {
    pub key: JsonValue,
    pub value: JsonValue,
    pub key_text: String,
    pub value_text: String,
    pub span: Span,
    pub raw: JsonValue,
}

/// A static record projected without extension-specific interpretation.
#[derive(Clone, Debug, Serialize)]
pub struct SemanticRecord {
    pub schema: String,
    pub owner: String,
    pub fields: JsonValue,
    pub span: Span,
    pub metadata: Vec<AuthoredMetadata>,
    pub raw: JsonValue,
}

/// A fact with explicit provenance and trust class.
#[derive(Clone, Debug, Serialize)]
pub struct SemanticFact {
    pub kind: String,
    pub value: JsonValue,
    pub provenance: Vec<FactOrigin>,
    pub trust: String,
    pub span: Span,
}

#[derive(Clone, Debug, Serialize)]
pub struct FactOrigin {
    pub kind: String,
    pub span: Span,
    pub detail: Option<String>,
}

/// The four intentionally separate metadata/fact layers.
#[derive(Clone, Debug, Default, Serialize)]
pub struct SemanticLayers {
    pub authored: Vec<AuthoredMetadata>,
    pub records: Vec<SemanticRecord>,
    pub declared: Vec<SemanticFact>,
    pub verified: Vec<SemanticFact>,
}

/// JSON-stable effect, temporal, and data summaries.
#[derive(Clone, Debug, Serialize)]
pub struct SemanticSummary {
    pub effects: EffectRow,
    /// Compatibility spelling for clients using the singular term.
    pub effect: EffectRow,
    pub temporal: TemporalSummary,
    pub data: DataProperties,
}

impl SemanticSummary {
    #[must_use]
    pub fn from_call(summary: &CallSummaries) -> Self {
        Self {
            effects: summary.effects.clone(),
            effect: summary.effects.clone(),
            temporal: summary.temporal.clone(),
            data: summary.data.clone(),
        }
    }

    #[must_use]
    pub fn unknown() -> Self {
        Self::from_call(&CallSummaries::unknown())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SemanticAlias {
    pub spelling: String,
    pub canonical: String,
    pub public: bool,
    pub preferred: bool,
    pub span: Span,
    pub labels: LocalizedLabel,
}

/// One resolved binding, including locals and parameters.
#[derive(Clone, Debug, Serialize)]
pub struct SemanticSymbol {
    pub binding_id: String,
    pub canonical: String,
    pub source: String,
    pub source_spelling: String,
    pub python: String,
    pub kind: BindingKind,
    pub aliases: Vec<SemanticAlias>,
    pub public: bool,
    #[serde(rename = "type")]
    pub ty: Type,
    pub metadata: SemanticLayers,
    pub summary: SemanticSummary,
    pub labels: LocalizedLabel,
    pub span: Span,
    pub definition: Span,
    pub references: Vec<Span>,
    pub occurrences: Vec<Span>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MacroTraceView {
    pub macro_name: String,
    pub macro_binding_id: String,
    pub call_span: Span,
    pub expansion_span: Span,
    pub depth: usize,
    pub origin: Vec<Span>,
}

impl From<&ExpansionTrace> for MacroTraceView {
    fn from(trace: &ExpansionTrace) -> Self {
        Self {
            macro_name: trace.macro_name.clone(),
            macro_binding_id: trace.macro_binding_id.clone(),
            call_span: trace.call_span,
            expansion_span: trace.expansion_span,
            depth: trace.depth,
            origin: trace.origin.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct OperationNode {
    pub id: String,
    pub kind: String,
    pub span: Span,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding_id: Option<String>,
    #[serde(rename = "type")]
    pub ty: Type,
    pub summary: SemanticSummary,
    pub labels: LocalizedLabel,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub macro_origins: Vec<Span>,
}

#[derive(Clone, Debug, Serialize)]
pub struct OperationEdge {
    pub from: String,
    pub to: String,
    pub kind: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct OperationGraph {
    pub nodes: Vec<OperationNode>,
    pub edges: Vec<OperationEdge>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SemanticDiagnostic {
    pub code: String,
    pub severity: String,
    pub message: String,
    pub span: Span,
}

impl From<&Diagnostic> for SemanticDiagnostic {
    fn from(diagnostic: &Diagnostic) -> Self {
        Self {
            code: diagnostic.code.to_owned(),
            severity: format!("{:?}", diagnostic.severity).to_lowercase(),
            message: diagnostic.message.clone(),
            span: diagnostic.span,
        }
    }
}

/// Versioned semantic document consumed by LSP, Agent tools, and inspect.
#[derive(Clone, Debug, Serialize)]
pub struct SemanticDocument {
    pub version: u32,
    pub document_version: i64,
    pub source: String,
    pub source_len: usize,
    pub module: String,
    pub symbols: Vec<SemanticSymbol>,
    pub authored: Vec<AuthoredMetadata>,
    pub records: Vec<SemanticRecord>,
    pub declared: Vec<SemanticFact>,
    pub verified: Vec<SemanticFact>,
    pub macro_traces: Vec<MacroTraceView>,
    pub operation_graph: OperationGraph,
    /// Flat aliases retained for early Agent clients.
    pub operations: Vec<OperationNode>,
    pub operation_edges: Vec<OperationEdge>,
    pub diagnostics: Vec<SemanticDiagnostic>,
}

/// Semantic view is the public name used by Agent integrations.
pub type SemanticView = SemanticDocument;

/// Projects an analysis into the versioned semantic model.
#[must_use]
pub fn project(analysis: &Analysis, source_name: impl Into<String>) -> SemanticDocument {
    SemanticDocument::from_analysis(analysis, source_name)
}

impl SemanticDocument {
    /// Projects one analysis without running another compiler pass.
    #[must_use]
    pub fn from_analysis(analysis: &Analysis, source_name: impl Into<String>) -> Self {
        Self::from_analysis_at_version(analysis, source_name, 0)
    }

    /// Projects one analysis and associates it with an editor version.
    #[must_use]
    pub fn from_analysis_at_version(
        analysis: &Analysis,
        source_name: impl Into<String>,
        document_version: i64,
    ) -> Self {
        let source = source_name.into();
        let aliases_by_target = aliases_by_target(&analysis.hir);
        let references = collect_references(&analysis.hir);
        let symbol_summaries = collect_symbol_summaries(&analysis.hir);
        let records = collect_records(&analysis.hir);
        let mut symbols = analysis
            .hir
            .bindings
            .iter()
            .map(|binding| {
                let id = binding.name.id.as_str().to_owned();
                let binding_aliases = aliases_by_target.get(&id).cloned().unwrap_or_default();
                let summary = symbol_summaries
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(SemanticSummary::unknown);
                let mut layers =
                    layers_for_metadata(&binding.metadata, binding.name.span, &summary);
                layers
                    .records
                    .extend(records_for_binding(&records, &binding.name.canonical));
                let occurrences = references.get(&id).cloned().unwrap_or_default();
                let definition = binding.name.span;
                let mut all_occurrences = occurrences.clone();
                all_occurrences.extend(binding_aliases.iter().map(|alias| alias.span));
                if !all_occurrences.contains(&definition) {
                    all_occurrences.push(definition);
                }
                all_occurrences.sort_by_key(|span| (span.start, span.end));
                all_occurrences.dedup();
                let preferred = preferred_alias(&binding_aliases, &binding.metadata);
                let labels = labels_for_name(&binding.name.canonical, preferred);
                SemanticSymbol {
                    binding_id: id,
                    canonical: binding.name.canonical.clone(),
                    source: binding.source_spelling.clone(),
                    source_spelling: binding.source_spelling.clone(),
                    python: binding.name.python.clone(),
                    kind: binding.name.kind,
                    aliases: binding_aliases,
                    public: binding.public,
                    ty: binding.ty.clone(),
                    metadata: layers,
                    summary,
                    labels,
                    span: binding.name.span,
                    definition,
                    references: occurrences,
                    occurrences: all_occurrences,
                }
            })
            .collect::<Vec<_>>();
        symbols.sort_by(|left, right| {
            (left.span.start, left.span.end, &left.binding_id).cmp(&(
                right.span.start,
                right.span.end,
                &right.binding_id,
            ))
        });

        let authored = collect_authored(analysis);
        let module_summary = module_summary(&analysis.hir);
        let mut declared = declared_facts(&analysis.hir.metadata, analysis.hir.span);
        let mut verified = vec![verified_module_fact(&analysis.hir, &module_summary)];
        for symbol in &symbols {
            declared.extend(symbol.metadata.declared.clone());
            verified.extend(symbol.metadata.verified.clone());
        }
        let macro_traces = analysis
            .expansion_traces
            .iter()
            .map(MacroTraceView::from)
            .collect::<Vec<_>>();
        let operation_graph = build_operation_graph(&analysis.hir, &analysis.expansion_traces);
        let operations = operation_graph.nodes.clone();
        let operation_edges = operation_graph.edges.clone();

        Self {
            version: SEMANTIC_DOCUMENT_VERSION,
            document_version,
            source,
            source_len: analysis.document.source_len,
            module: analysis.hir.name.clone(),
            symbols,
            authored,
            records,
            declared,
            verified,
            macro_traces,
            operation_graph,
            operations,
            operation_edges,
            diagnostics: analysis
                .diagnostics
                .iter()
                .map(SemanticDiagnostic::from)
                .collect(),
        }
    }

    #[must_use]
    pub fn new(analysis: &Analysis, source_name: impl Into<String>, document_version: i64) -> Self {
        Self::from_analysis_at_version(analysis, source_name, document_version)
    }

    #[must_use]
    pub fn symbol(&self, binding_id: &str) -> Option<&SemanticSymbol> {
        self.symbols
            .iter()
            .find(|symbol| symbol.binding_id == binding_id)
    }

    #[must_use]
    pub fn symbol_at(&self, offset: usize) -> Option<&SemanticSymbol> {
        self.symbols
            .iter()
            .filter(|symbol| {
                symbol
                    .occurrences
                    .iter()
                    .any(|span| contains(*span, offset))
            })
            .min_by_key(|symbol| {
                symbol
                    .occurrences
                    .iter()
                    .filter(|span| contains(**span, offset))
                    .map(|span| span.end.saturating_sub(span.start))
                    .min()
                    .unwrap_or(usize::MAX)
            })
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    pub fn to_pretty_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

fn contains(span: Span, offset: usize) -> bool {
    (span.start..=span.end).contains(&offset)
}

fn aliases_by_target(module: &hir::Module) -> BTreeMap<String, Vec<SemanticAlias>> {
    let canonical_by_id = module
        .bindings
        .iter()
        .map(|binding| {
            (
                binding.name.id.as_str().to_owned(),
                binding.name.canonical.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut aliases = BTreeMap::<String, Vec<SemanticAlias>>::new();
    for alias in &module.aliases {
        let Some(target_canonical) = canonical_by_id.get(alias.target.as_str()) else {
            continue;
        };
        aliases
            .entry(alias.target.as_str().to_owned())
            .or_default()
            .push(SemanticAlias {
                spelling: alias.spelling.clone(),
                canonical: alias.canonical.clone(),
                public: alias.public,
                preferred: false,
                span: alias.span,
                labels: labels_for_name(target_canonical, Some(alias.spelling.clone())),
            });
    }
    for values in aliases.values_mut() {
        values.sort_by(|left, right| {
            (!left.public, !left.preferred, &left.spelling).cmp(&(
                !right.public,
                !right.preferred,
                &right.spelling,
            ))
        });
        if let Some(first) = values.first_mut() {
            first.preferred = true;
        }
    }
    aliases
}

fn labels_for_name(canonical: &str, preferred: Option<String>) -> LocalizedLabel {
    let chinese = preferred.filter(|value| contains_cjk(value));
    LocalizedLabel::new(canonical.to_owned(), chinese)
}

fn contains_cjk(value: &str) -> bool {
    value.chars().any(|character| {
        matches!(
            character as u32,
            0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff
        )
    })
}

fn preferred_alias(aliases: &[SemanticAlias], metadata: &[MetadataEntry]) -> Option<String> {
    aliases
        .iter()
        .find(|alias| alias.public && contains_cjk(&alias.spelling))
        .or_else(|| aliases.iter().find(|alias| contains_cjk(&alias.spelling)))
        .map(|alias| alias.spelling.clone())
        .or_else(|| metadata_preferred_name(metadata))
}

fn metadata_preferred_name(metadata: &[MetadataEntry]) -> Option<String> {
    for entry in metadata {
        let key = form_name(&entry.key).unwrap_or_default();
        let key = key.trim_start_matches(':').to_ascii_lowercase();
        if matches!(key.as_str(), "preferred" | "name" | "zh-cn" | "zh_cn") {
            if let Some(value) = form_name(&entry.value) {
                return Some(value);
            }
        }
        if key == "osiris/names" || key == "names" {
            if let Some(value) = metadata_preferred_name_from_form(&entry.value) {
                return Some(value);
            }
        }
    }
    None
}

fn metadata_preferred_name_from_form(form: &Form) -> Option<String> {
    let FormKind::Map(entries) = &form.kind else {
        return form_name(form);
    };
    for pair in entries.chunks_exact(2) {
        let key = form_name(&pair[0])?
            .trim_start_matches(':')
            .to_ascii_lowercase();
        if matches!(key.as_str(), "preferred" | "zh-cn" | "zh_cn") {
            if let Some(name) = form_name(&pair[1]) {
                return Some(name);
            }
        }
        if key == "aliases" {
            if let FormKind::Vector(values) = &pair[1].kind {
                if let Some(name) = values
                    .iter()
                    .filter_map(form_name)
                    .find(|name| contains_cjk(name))
                {
                    return Some(name);
                }
            }
        }
    }
    None
}

fn form_name(form: &Form) -> Option<String> {
    match &form.kind {
        FormKind::Symbol(name) | FormKind::Keyword(name) => Some(name.canonical.clone()),
        FormKind::String(value) => Some(value.clone()),
        _ => None,
    }
}

fn metadata_entries(metadata: &[MetadataEntry]) -> Vec<AuthoredMetadata> {
    metadata
        .iter()
        .map(|entry| AuthoredMetadata {
            key: form_json(&entry.key),
            value: form_json(&entry.value),
            key_text: form_text(&entry.key),
            value_text: form_text(&entry.value),
            span: entry.key.span.cover(entry.value.span),
            raw: json!({ "key": form_json(&entry.key), "value": form_json(&entry.value) }),
        })
        .collect()
}

fn collect_authored(analysis: &Analysis) -> Vec<AuthoredMetadata> {
    let mut authored = Vec::new();
    for form in &analysis.document.forms {
        collect_form_authored(form, &mut authored);
    }

    let module = &analysis.hir;
    authored.extend(metadata_entries(&module.metadata));
    for item in &module.items {
        authored.extend(metadata_entries(&item.metadata));
    }
    for binding in &module.bindings {
        authored.extend(metadata_entries(&binding.metadata));
    }
    authored.sort_by(|left, right| {
        (
            left.span.start,
            left.span.end,
            &left.key_text,
            &left.value_text,
        )
            .cmp(&(
                right.span.start,
                right.span.end,
                &right.key_text,
                &right.value_text,
            ))
    });
    authored.dedup_by(|left, right| {
        left.span == right.span
            && left.key_text == right.key_text
            && left.value_text == right.value_text
    });
    authored
}

fn collect_form_authored(form: &Form, authored: &mut Vec<AuthoredMetadata>) {
    authored.extend(metadata_entries(&form.metadata));
    for entry in &form.metadata {
        collect_form_authored(&entry.key, authored);
        collect_form_authored(&entry.value, authored);
    }
    match &form.kind {
        FormKind::List(items)
        | FormKind::Vector(items)
        | FormKind::Map(items)
        | FormKind::Set(items) => {
            for item in items {
                collect_form_authored(item, authored);
            }
        }
        FormKind::ReaderMacro { form, .. } => collect_form_authored(form, authored),
        FormKind::None
        | FormKind::Bool(_)
        | FormKind::Integer(_)
        | FormKind::Float(_)
        | FormKind::String(_)
        | FormKind::Keyword(_)
        | FormKind::Symbol(_)
        | FormKind::Error(_) => {}
    }
}

fn layers_for_metadata(
    metadata: &[MetadataEntry],
    span: Span,
    summary: &SemanticSummary,
) -> SemanticLayers {
    let authored = metadata_entries(metadata);
    let declared = declared_facts(metadata, span);
    let verified = vec![SemanticFact {
        kind: "inferred-summary".to_owned(),
        value: json!({
            "effects": summary.effects,
            "temporal": summary.temporal,
            "data": summary.data,
        }),
        provenance: vec![FactOrigin {
            kind: "typed-hir".to_owned(),
            span,
            detail: Some("derived from local typed HIR".to_owned()),
        }],
        trust: "compiler-verified".to_owned(),
        span,
    }];
    SemanticLayers {
        authored,
        records: Vec::new(),
        declared,
        verified,
    }
}

fn declared_facts(metadata: &[MetadataEntry], span: Span) -> Vec<SemanticFact> {
    metadata
        .iter()
        .filter_map(|entry| {
            let key = form_name(&entry.key)?;
            let normalized = key.trim_start_matches(':').to_ascii_lowercase();
            let semantic = normalized.starts_with("osiris/")
                || matches!(
                    normalized.as_str(),
                    "pure"
                        | "effect"
                        | "effects"
                        | "future"
                        | "lookahead"
                        | "availability"
                        | "schema"
                        | "axis"
                        | "contract"
                        | "temporal"
                        | "data"
                );
            semantic.then(|| SemanticFact {
                kind: key,
                value: form_json(&entry.value),
                provenance: vec![FactOrigin {
                    kind: "authored-metadata".to_owned(),
                    span: entry.key.span.cover(entry.value.span),
                    detail: Some("declared by source metadata; not a proof".to_owned()),
                }],
                trust: "declared".to_owned(),
                span,
            })
        })
        .collect()
}

fn verified_module_fact(module: &hir::Module, summary: &SemanticSummary) -> SemanticFact {
    SemanticFact {
        kind: "module-summary".to_owned(),
        value: json!({
            "effects": summary.effects,
            "temporal": summary.temporal,
            "data": summary.data,
        }),
        provenance: vec![FactOrigin {
            kind: "typed-hir".to_owned(),
            span: module.span,
            detail: Some("joined summaries of module operations".to_owned()),
        }],
        trust: "compiler-verified".to_owned(),
        span: module.span,
    }
}

fn module_summary(module: &hir::Module) -> SemanticSummary {
    let mut summary = CallSummaries::pure_scalar();
    for item in &module.items {
        let item_summary = match &item.kind {
            ItemKind::Value(value) => value
                .value
                .as_ref()
                .map_or_else(CallSummaries::pure_scalar, |expr| expr.summaries.clone()),
            ItemKind::Function(function) => function.summaries.clone(),
            ItemKind::Struct(structure) => structure
                .checks
                .iter()
                .fold(CallSummaries::pure_scalar(), |joined, check| {
                    joined.join(&check.condition.summaries)
                }),
            ItemKind::Expr(expr) => expr.summaries.clone(),
            ItemKind::Import(_) | ItemKind::StaticSchema(_) | ItemKind::StaticRecord(_) => {
                CallSummaries::pure_scalar()
            }
        };
        summary = summary.join(&item_summary);
    }
    SemanticSummary::from_call(&summary)
}

fn collect_symbol_summaries(module: &hir::Module) -> BTreeMap<String, SemanticSummary> {
    let mut summaries = BTreeMap::new();
    for item in &module.items {
        match &item.kind {
            ItemKind::Function(function) => {
                summaries.insert(
                    function.binding.as_str().to_owned(),
                    SemanticSummary::from_call(&function.summaries),
                );
            }
            ItemKind::Value(value) => {
                let summary = value
                    .value
                    .as_ref()
                    .map_or_else(CallSummaries::pure_scalar, |expr| expr.summaries.clone());
                summaries.insert(
                    value.binding.as_str().to_owned(),
                    SemanticSummary::from_call(&summary),
                );
            }
            ItemKind::Struct(structure) => {
                let summary = structure
                    .checks
                    .iter()
                    .fold(CallSummaries::pure_scalar(), |joined, check| {
                        joined.join(&check.condition.summaries)
                    });
                summaries.insert(
                    structure.binding.as_str().to_owned(),
                    SemanticSummary::from_call(&summary),
                );
            }
            _ => {}
        }
    }
    summaries
}

fn collect_references(module: &hir::Module) -> BTreeMap<String, Vec<Span>> {
    let mut references = BTreeMap::<String, Vec<Span>>::new();
    for item in &module.items {
        match &item.kind {
            ItemKind::Value(value) => {
                if let Some(expression) = &value.value {
                    collect_expr_references(expression, &mut references);
                }
            }
            ItemKind::Function(function) => {
                collect_expr_references(&function.body, &mut references);
            }
            ItemKind::Struct(structure) => {
                for field in &structure.fields {
                    if let Some(default) = &field.default {
                        collect_expr_references(default, &mut references);
                    }
                }
                for check in &structure.checks {
                    collect_expr_references(&check.condition, &mut references);
                    if let Some(message) = &check.message {
                        collect_expr_references(message, &mut references);
                    }
                }
            }
            ItemKind::Expr(expression) => collect_expr_references(expression, &mut references),
            ItemKind::Import(_) | ItemKind::StaticSchema(_) | ItemKind::StaticRecord(_) => {}
        }
    }
    for spans in references.values_mut() {
        spans.sort_by_key(|span| (span.start, span.end));
        spans.dedup();
    }
    references
}

fn collect_expr_references(expression: &Expr, references: &mut BTreeMap<String, Vec<Span>>) {
    if let ExprKind::Binding(binding) = &expression.kind {
        references
            .entry(binding.as_str().to_owned())
            .or_default()
            .push(expression.span);
    }
    match &expression.kind {
        ExprKind::List(items)
        | ExprKind::Vector(items)
        | ExprKind::Set(items)
        | ExprKind::Do(items) => {
            for item in items {
                collect_expr_references(item, references);
            }
        }
        ExprKind::Map(entries) => {
            for (key, value) in entries {
                collect_expr_references(key, references);
                collect_expr_references(value, references);
            }
        }
        ExprKind::Call { callee, arguments } => {
            collect_expr_references(callee, references);
            for argument in arguments {
                match argument {
                    hir::CallArgument::Positional(value)
                    | hir::CallArgument::Keyword { value, .. } => {
                        collect_expr_references(value, references);
                    }
                }
            }
        }
        ExprKind::Operator { operands, .. } => {
            for operand in operands {
                collect_expr_references(operand, references);
            }
        }
        ExprKind::Attribute { value, .. } => collect_expr_references(value, references),
        ExprKind::Index { value, index } => {
            collect_expr_references(value, references);
            collect_expr_references(index, references);
        }
        ExprKind::Let { bindings, body } => {
            for binding in bindings {
                collect_expr_references(&binding.value, references);
            }
            collect_expr_references(body, references);
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_expr_references(condition, references);
            collect_expr_references(then_branch, references);
            collect_expr_references(else_branch, references);
        }
        ExprKind::Lambda { parameters, body } => {
            for parameter in parameters {
                if let Some(default) = &parameter.default {
                    collect_expr_references(default, references);
                }
            }
            collect_expr_references(body, references);
        }
        ExprKind::Try {
            body,
            catches,
            finally_body,
        } => {
            collect_expr_references(body, references);
            for catch in catches {
                collect_expr_references(&catch.body, references);
            }
            if let Some(finally_body) = finally_body {
                collect_expr_references(finally_body, references);
            }
        }
        ExprKind::Raise(value) => {
            if let Some(value) = value {
                collect_expr_references(value, references);
            }
        }
        ExprKind::None
        | ExprKind::Bool(_)
        | ExprKind::Integer(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Binding(_)
        | ExprKind::Error => {}
    }
}

fn collect_records(module: &hir::Module) -> Vec<SemanticRecord> {
    module
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::StaticRecord(record) => Some(SemanticRecord {
                schema: record.schema.canonical.clone(),
                owner: record.owner.canonical.clone(),
                fields: json!(
                    record
                        .fields
                        .iter()
                        .map(|(name, value)| (name.canonical.clone(), form_json_expr(value)))
                        .collect::<BTreeMap<_, _>>()
                ),
                span: record.span,
                metadata: metadata_entries(&record.metadata),
                raw: serde_json::to_value(record).unwrap_or(JsonValue::Null),
            }),
            _ => None,
        })
        .collect()
}

fn records_for_binding(records: &[SemanticRecord], canonical: &str) -> Vec<SemanticRecord> {
    records
        .iter()
        .filter(|record| record.owner == canonical)
        .cloned()
        .collect()
}

fn form_json(form: &Form) -> JsonValue {
    serde_json::to_value(form).unwrap_or(JsonValue::Null)
}

fn form_json_expr(expression: &ast::Expr) -> JsonValue {
    serde_json::to_value(expression).unwrap_or(JsonValue::Null)
}

fn form_text(form: &Form) -> String {
    match &form.kind {
        FormKind::None => "none".to_owned(),
        FormKind::Bool(value) => value.to_string(),
        FormKind::Integer(value) | FormKind::Float(value) => value.clone(),
        FormKind::String(value) => value.clone(),
        FormKind::Keyword(name) | FormKind::Symbol(name) => name.spelling.clone(),
        FormKind::Error(message) => format!("#<error:{message}>"),
        FormKind::List(items) => format!(
            "({})",
            items.iter().map(form_text).collect::<Vec<_>>().join(" ")
        ),
        FormKind::Vector(items) => format!(
            "[{}]",
            items.iter().map(form_text).collect::<Vec<_>>().join(" ")
        ),
        FormKind::Map(items) => format!(
            "{{{}}}",
            items.iter().map(form_text).collect::<Vec<_>>().join(" ")
        ),
        FormKind::Set(items) => format!(
            "#{{{}}}",
            items.iter().map(form_text).collect::<Vec<_>>().join(" ")
        ),
        FormKind::ReaderMacro { form, .. } => form_text(form),
    }
}

fn build_operation_graph(module: &hir::Module, traces: &[ExpansionTrace]) -> OperationGraph {
    let aliases = aliases_by_target(module);
    let mut builder = OperationBuilder {
        next: 0,
        nodes: Vec::new(),
        edges: Vec::new(),
        traces,
        bindings: module
            .bindings
            .iter()
            .map(|binding| {
                let id = binding.name.id.as_str().to_owned();
                let binding_aliases = aliases.get(&id).map(Vec::as_slice).unwrap_or_default();
                let label = labels_for_name(
                    &binding.name.canonical,
                    preferred_alias(binding_aliases, &binding.metadata),
                );
                (id, (binding.name.canonical.clone(), label))
            })
            .collect(),
    };
    for item in &module.items {
        builder.item(item);
    }
    let mut outputs = BTreeMap::<String, Vec<String>>::new();
    for edge in &builder.edges {
        outputs
            .entry(edge.from.clone())
            .or_default()
            .push(edge.to.clone());
    }
    for node in &mut builder.nodes {
        node.outputs = outputs.remove(&node.id).unwrap_or_default();
        node.outputs.sort();
        node.outputs.dedup();
    }
    OperationGraph {
        nodes: builder.nodes,
        edges: builder.edges,
    }
}

struct OperationBuilder<'a> {
    next: usize,
    nodes: Vec<OperationNode>,
    edges: Vec<OperationEdge>,
    traces: &'a [ExpansionTrace],
    bindings: BTreeMap<String, (String, LocalizedLabel)>,
}

impl OperationBuilder<'_> {
    fn id(&mut self) -> String {
        let id = format!("op-{}", self.next);
        self.next += 1;
        id
    }

    fn add(
        &mut self,
        kind: impl Into<String>,
        span: Span,
        binding_id: Option<String>,
        ty: Type,
        summaries: &CallSummaries,
        inputs: Vec<String>,
    ) -> String {
        let raw_kind = kind.into();
        let id = self.id();
        let binding_label = binding_id
            .as_ref()
            .and_then(|binding| self.bindings.get(binding))
            .cloned();
        let labels = binding_label
            .map(|(_, labels)| labels)
            .unwrap_or_else(|| operation_labels(&raw_kind));
        let macro_origins = self
            .traces
            .iter()
            .filter(|trace| {
                spans_overlap(trace.call_span, span) || spans_overlap(trace.expansion_span, span)
            })
            .flat_map(|trace| trace.origin.iter().map(|origin| (origin.start, origin.end)))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|(start, end)| Span::new(start, end))
            .collect::<Vec<_>>();
        for input in &inputs {
            self.edges.push(OperationEdge {
                from: input.clone(),
                to: id.clone(),
                kind: "data".to_owned(),
            });
        }
        self.nodes.push(OperationNode {
            id: id.clone(),
            kind: raw_kind,
            span,
            binding_id,
            ty,
            summary: SemanticSummary::from_call(summaries),
            labels,
            inputs,
            outputs: Vec::new(),
            macro_origins,
        });
        id
    }

    fn item(&mut self, item: &hir::Item) {
        match &item.kind {
            ItemKind::Import(import) => {
                self.add(
                    "import",
                    item.span,
                    Some(import.binding.as_str().to_owned()),
                    Type::Any,
                    &CallSummaries::pure_scalar(),
                    Vec::new(),
                );
            }
            ItemKind::Value(value) => {
                let input = value.value.as_ref().map(|expr| self.expr(expr));
                let ty = value
                    .value
                    .as_ref()
                    .map_or(Type::Any, |expr| expr.ty.clone());
                let summary = value
                    .value
                    .as_ref()
                    .map_or_else(CallSummaries::pure_scalar, |expr| expr.summaries.clone());
                self.add(
                    "value",
                    item.span,
                    Some(value.binding.as_str().to_owned()),
                    ty,
                    &summary,
                    input.into_iter().collect(),
                );
            }
            ItemKind::Function(function) => {
                let body = self.expr(&function.body);
                let ty = Type::Fn(
                    crate::types::FunctionType::new(
                        function
                            .parameters
                            .iter()
                            .map(|parameter| parameter.ty.clone())
                            .collect(),
                        function.return_type.clone(),
                    )
                    .with_summaries(function.summaries.clone()),
                );
                self.add(
                    "function",
                    item.span,
                    Some(function.binding.as_str().to_owned()),
                    ty,
                    &function.summaries,
                    vec![body],
                );
            }
            ItemKind::Struct(structure) => {
                let inputs = structure
                    .fields
                    .iter()
                    .filter_map(|field| field.default.as_ref().map(|default| self.expr(default)))
                    .collect();
                self.add(
                    "struct",
                    item.span,
                    Some(structure.binding.as_str().to_owned()),
                    Type::Any,
                    &CallSummaries::pure_scalar(),
                    inputs,
                );
            }
            ItemKind::Expr(expression) => {
                self.expr(expression);
            }
            ItemKind::StaticSchema(schema) => {
                self.add(
                    "static-schema",
                    schema.span,
                    None,
                    Type::Any,
                    &CallSummaries::pure_scalar(),
                    Vec::new(),
                );
            }
            ItemKind::StaticRecord(record) => {
                self.add(
                    "static-record",
                    record.span,
                    None,
                    Type::Any,
                    &CallSummaries::pure_scalar(),
                    Vec::new(),
                );
            }
        }
    }

    fn expr(&mut self, expression: &Expr) -> String {
        let mut inputs = Vec::new();
        let mut binding_id = None;
        let kind = match &expression.kind {
            ExprKind::Binding(binding) => {
                return self.add(
                    "binding",
                    expression.span,
                    Some(binding.as_str().to_owned()),
                    expression.ty.clone(),
                    &expression.summaries,
                    Vec::new(),
                );
            }
            ExprKind::Call { callee, arguments } => {
                if let ExprKind::Binding(binding) = &callee.kind {
                    binding_id = Some(binding.as_str().to_owned());
                }
                inputs.push(self.expr(callee));
                for argument in arguments {
                    inputs.push(match argument {
                        hir::CallArgument::Positional(value)
                        | hir::CallArgument::Keyword { value, .. } => self.expr(value),
                    });
                }
                "call"
            }
            ExprKind::Operator { operator, operands } => {
                inputs.extend(operands.iter().map(|operand| self.expr(operand)));
                return self.add(
                    format!("operator:{operator:?}").to_ascii_lowercase(),
                    expression.span,
                    None,
                    expression.ty.clone(),
                    &expression.summaries,
                    inputs,
                );
            }
            ExprKind::Attribute { value, attribute } => {
                inputs.push(self.expr(value));
                return self.add(
                    format!("attribute:{attribute}"),
                    expression.span,
                    None,
                    expression.ty.clone(),
                    &expression.summaries,
                    inputs,
                );
            }
            ExprKind::Index { value, index } => {
                inputs.push(self.expr(value));
                inputs.push(self.expr(index));
                "index"
            }
            ExprKind::List(items) => {
                inputs.extend(items.iter().map(|item| self.expr(item)));
                "list"
            }
            ExprKind::Vector(items) => {
                inputs.extend(items.iter().map(|item| self.expr(item)));
                "vector"
            }
            ExprKind::Set(items) => {
                inputs.extend(items.iter().map(|item| self.expr(item)));
                "set"
            }
            ExprKind::Map(entries) => {
                for (key, value) in entries {
                    inputs.push(self.expr(key));
                    inputs.push(self.expr(value));
                }
                "map"
            }
            ExprKind::Let { bindings, body } => {
                inputs.extend(bindings.iter().map(|binding| self.expr(&binding.value)));
                inputs.push(self.expr(body));
                "let"
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                inputs.push(self.expr(condition));
                inputs.push(self.expr(then_branch));
                inputs.push(self.expr(else_branch));
                "if"
            }
            ExprKind::Do(items) => {
                inputs.extend(items.iter().map(|item| self.expr(item)));
                "do"
            }
            ExprKind::Lambda { body, .. } => {
                inputs.push(self.expr(body));
                "lambda"
            }
            ExprKind::Try {
                body,
                catches,
                finally_body,
            } => {
                inputs.push(self.expr(body));
                inputs.extend(catches.iter().map(|catch| self.expr(&catch.body)));
                if let Some(finally_body) = finally_body {
                    inputs.push(self.expr(finally_body));
                }
                "try"
            }
            ExprKind::Raise(value) => {
                if let Some(value) = value {
                    inputs.push(self.expr(value));
                }
                "raise"
            }
            ExprKind::None => "none",
            ExprKind::Bool(_) => "bool",
            ExprKind::Integer(_) => "integer",
            ExprKind::Float(_) => "float",
            ExprKind::String(_) => "string",
            ExprKind::Error => "error",
        };
        self.add(
            kind,
            expression.span,
            binding_id,
            expression.ty.clone(),
            &expression.summaries,
            inputs,
        )
    }
}

fn spans_overlap(left: Span, right: Span) -> bool {
    left.start <= right.end && right.start <= left.end
}

fn operation_labels(kind: &str) -> LocalizedLabel {
    let zh_cn = match kind.split_once(':').map_or(kind, |(head, _)| head) {
        "import" => "导入",
        "value" => "值定义",
        "function" => "函数",
        "struct" => "结构",
        "static-schema" => "静态模式",
        "static-record" => "静态记录",
        "binding" => "绑定",
        "call" => "调用",
        "operator" => "运算",
        "attribute" => "字段访问",
        "index" => "索引",
        "list" => "列表",
        "vector" => "向量",
        "set" => "集合",
        "map" => "映射",
        "let" => "局部绑定",
        "if" => "条件",
        "do" => "顺序执行",
        "lambda" => "匿名函数",
        "try" => "异常处理",
        "raise" => "抛出异常",
        "none" => "空值",
        "bool" => "布尔值",
        "integer" => "整数",
        "float" => "浮点数",
        "string" => "字符串",
        "error" => "错误",
        _ => kind,
    };
    LocalizedLabel {
        zh_cn: zh_cn.to_owned(),
        en: kind.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value as JsonValue;

    use super::{SEMANTIC_DOCUMENT_VERSION, SemanticDocument};
    use crate::{
        compiler::{CompileOptions, analyze},
        project::PythonVersion,
    };

    #[test]
    fn semantic_projection_is_versioned_and_keeps_aliases_and_layers() {
        let source = r#"(module demo)
^{:doc "v"} (def value 1)
(alias 中文值 value)
(export [value 中文值])
"#;
        let analysis = analyze(source, &CompileOptions::new("demo", PythonVersion::MINIMUM));
        let document = SemanticDocument::from_analysis_at_version(&analysis, "demo.osr", 7);
        assert_eq!(document.version, SEMANTIC_DOCUMENT_VERSION);
        assert_eq!(document.document_version, 7);
        let value = document
            .symbols
            .iter()
            .find(|symbol| symbol.canonical == "value")
            .expect("value symbol");
        assert!(value.aliases.iter().any(|alias| alias.spelling == "中文值"));
        assert!(!value.metadata.authored.is_empty());
        let json: JsonValue =
            serde_json::from_str(&document.to_json().expect("json")).expect("valid json");
        assert_eq!(json["version"], SEMANTIC_DOCUMENT_VERSION);
        assert!(json["operation_graph"]["nodes"].is_array());
    }

    #[test]
    fn authored_layer_keeps_metadata_from_a_macro_call_site() {
        let source = r#"(module demo)
(defmacro define-one [name]
  `(def ~name 1))
^{:agent/intent :demo/create}
(define-one value)
"#;
        let analysis = analyze(source, &CompileOptions::new("demo", PythonVersion::MINIMUM));
        assert!(
            analysis.diagnostics.is_empty(),
            "{:#?}",
            analysis.diagnostics
        );
        let document = SemanticDocument::from_analysis(&analysis, "demo.osr");
        assert!(
            document.authored.iter().any(|entry| {
                entry.key_text.trim_start_matches(':') == "agent/intent"
                    && entry.value_text.trim_start_matches(':') == "demo/create"
            }),
            "{:#?}",
            document
                .authored
                .iter()
                .map(|entry| (&entry.key_text, &entry.value_text))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn operation_nodes_have_localized_labels_and_spans() {
        let analysis = analyze(
            r#"(module demo)
(def value (+ 1 2))"#,
            &CompileOptions::new("demo", PythonVersion::MINIMUM),
        );
        let document = SemanticDocument::from_analysis(&analysis, "demo.osr");
        assert!(
            document
                .operations
                .iter()
                .any(|node| node.span.end > node.span.start)
        );
        assert!(
            document
                .operations
                .iter()
                .all(|node| !node.labels.en.is_empty())
        );
    }
}
