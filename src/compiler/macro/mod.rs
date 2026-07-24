//! Hygienic surface-form macro expansion.
//!
//! Macros transform reader forms before surface AST lowering. The standard
//! threading forms live here as prelude macros rather than HIR special cases,
//! keeping the runtime language and backend small.

use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

use serde::Serialize;

use crate::{
    core_forms::{is_authored_boundary, is_macro_declaration, is_phase_declaration},
    diagnostic::Diagnostic,
    name::{BindingId, BindingKind},
    source::Span,
    syntax::{
        Document, Form, FormKind, METADATA_TARGET_LIMITS, MetadataEntry, Name, ReaderMacroKind,
        check_metadata_resources, metadata_datum_is_serializable,
    },
};

const DEFAULT_MAX_EXPANSIONS: usize = 1_024;
const DEFAULT_MAX_EVAL_STEPS: usize = 100_000;
const DEFAULT_MAX_EVAL_DEPTH: usize = 128;
const DEFAULT_MAX_RESULT_NODES: usize = 65_536;

mod expander;

#[derive(Clone, Copy, Debug)]
pub struct ExpansionOptions {
    pub once: bool,
    pub max_expansions: usize,
}

impl Default for ExpansionOptions {
    fn default() -> Self {
        Self {
            once: false,
            max_expansions: DEFAULT_MAX_EXPANSIONS,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ExpansionTrace {
    pub macro_name: String,
    pub macro_binding_id: String,
    pub call_span: Span,
    pub expansion_span: Span,
    pub depth: usize,
    pub origin: Vec<Span>,
}

#[derive(Clone, Debug)]
pub struct ExpansionResult {
    pub document: Document,
    pub traces: Vec<ExpansionTrace>,
}

/// One data-only phase-1 interface import.
///
/// `namespace` is the stable definition namespace used to isolate private
/// helpers. `macro_names` maps each caller-visible spelling (for example
/// `q/pipeline` or a referred `pipeline`) to the canonical macro declaration
/// name contained in `forms`. No imported macro is callable unless it appears
/// in this map.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportedPhaseModule {
    pub namespace: String,
    pub forms: Vec<Form>,
    pub macro_names: BTreeMap<String, String>,
    /// Definition-site names that syntax quote may resolve into stable,
    /// module-qualified symbols. Values are canonical exported names.
    pub definition_names: BTreeMap<String, String>,
}

impl ImportedPhaseModule {
    #[must_use]
    pub fn new(
        namespace: impl Into<String>,
        forms: Vec<Form>,
        macro_names: BTreeMap<String, String>,
    ) -> Self {
        Self {
            namespace: namespace.into(),
            forms,
            macro_names,
            definition_names: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_definition_names(mut self, definition_names: BTreeMap<String, String>) -> Self {
        self.definition_names = definition_names;
        self
    }
}

#[derive(Clone, Debug)]
struct FunctionDef {
    name: String,
    source_name: String,
    macro_binding_id: Option<String>,
    namespace: Option<String>,
    imported: bool,
    params: Parameters,
    body: Vec<Form>,
    span: Span,
}

#[derive(Clone, Debug)]
struct Parameters {
    fixed: Vec<Pattern>,
    rest: Option<Box<Pattern>>,
}

#[derive(Clone, Debug)]
enum Pattern {
    Bind(String),
    Ignore,
    Vector(Parameters),
    Map(MapPattern),
}

#[derive(Clone, Debug)]
struct MapPattern {
    entries: Vec<MapPatternEntry>,
    defaults: BTreeMap<String, Form>,
    whole: Option<Box<Pattern>>,
}

#[derive(Clone, Debug)]
struct MapPatternEntry {
    binding: Pattern,
    lookup: Form,
}

#[derive(Clone, Debug)]
struct Lambda {
    params: Parameters,
    body: Vec<Form>,
    closure: Environment,
    namespace: Option<String>,
}

#[derive(Clone, Debug)]
enum Callable {
    Builtin(&'static str),
    User(String),
    Lambda(Rc<Lambda>),
}

#[derive(Clone, Debug)]
enum Value {
    Data(Form),
    Callable(Callable),
    Reduced(Box<Value>),
}

impl Value {
    fn into_data(self, span: Span) -> Result<Form, EvalError> {
        match self {
            Self::Data(form) => Ok(form),
            Self::Callable(_) => Err(EvalError::evaluation(
                "a phase-1 function cannot be used as syntax data",
                span,
            )),
            Self::Reduced(_) => Err(EvalError::evaluation(
                "a reduced phase-1 value must be consumed by `reduce` or `unreduced`",
                span,
            )),
        }
    }
}

type Environment = BTreeMap<String, Value>;

#[derive(Clone, Copy, Debug, Default)]
struct EvalBudget {
    steps: usize,
}

#[derive(Clone, Debug)]
struct EvalError {
    code: &'static str,
    message: String,
    span: Span,
}

impl EvalError {
    fn new(code: &'static str, message: impl Into<String>, span: Span) -> Self {
        Self {
            code,
            message: message.into(),
            span,
        }
    }

    fn evaluation(message: impl Into<String>, span: Span) -> Self {
        Self::new("OSR-M0004", message, span)
    }
}

/// Expands prelude and user macros in a recovered reader document.
#[must_use]
pub fn expand(document: &Document, options: ExpansionOptions) -> ExpansionResult {
    expand_with_imported_phase_forms(document, &[], options)
}

/// Expands a document after loading data-only phase-1 declarations from
/// dependency interfaces. Imported forms use the same parser, evaluator,
/// budgets, hygiene machinery, and diagnostics as local declarations.
#[must_use]
pub fn expand_with_imported_phase_forms(
    document: &Document,
    imported_phase_forms: &[Form],
    options: ExpansionOptions,
) -> ExpansionResult {
    let module_name = document_module_name(document).unwrap_or("osiris.anonymous");
    let mut expander = Expander::new(options, module_name);
    collect_standard_core(&mut expander, document, &[]);
    expander.collect_phase_one_declarations(imported_phase_forms);
    expand_document(document, expander)
}

/// Expands a document after loading isolated phase-1 modules from dependency
/// interfaces. Private helpers stay inside their definition namespace and
/// imported macros are visible only through `macro_names` entries.
#[must_use]
pub fn expand_with_imported_phase_modules(
    document: &Document,
    imported_phase_modules: &[ImportedPhaseModule],
    options: ExpansionOptions,
) -> ExpansionResult {
    expand_with_imported_phase_modules_for_module(
        document,
        imported_phase_modules,
        "osiris.anonymous",
        options,
    )
}

/// Expands isolated phase-1 modules while assigning local macros the same
/// stable module identity used by later compiler passes.
#[must_use]
pub fn expand_with_imported_phase_modules_for_module(
    document: &Document,
    imported_phase_modules: &[ImportedPhaseModule],
    fallback_module_name: &str,
    options: ExpansionOptions,
) -> ExpansionResult {
    let module_name = document_module_name(document).unwrap_or(fallback_module_name);
    let mut expander = Expander::new(options, module_name);
    collect_standard_core(&mut expander, document, imported_phase_modules);
    expander.collect_imported_phase_modules(imported_phase_modules);
    expand_document(document, expander)
}

fn collect_standard_core(
    expander: &mut Expander,
    document: &Document,
    imported_phase_modules: &[ImportedPhaseModule],
) {
    let surface = crate::ast::lower_document(document);
    let mut descriptors = Vec::new();
    if crate::stdlib::uses_implicit_core(&surface.module)
        && crate::stdlib::document_may_use_implicit_core_macro(document)
        && !imported_phase_modules
            .iter()
            .any(|module| module.namespace == crate::stdlib::CORE_NAMESPACE)
    {
        match crate::stdlib::interface_artifact(crate::stdlib::CORE_NAMESPACE) {
            Ok(interface) => {
                let visible = interface
                    .macros
                    .iter()
                    .flat_map(|macro_| {
                        [
                            (macro_.canonical.clone(), macro_.canonical.clone()),
                            (
                                format!("{}/{}", crate::stdlib::CORE_NAMESPACE, macro_.canonical),
                                macro_.canonical.clone(),
                            ),
                            (
                                format!("{}.{}", crate::stdlib::CORE_NAMESPACE, macro_.canonical),
                                macro_.canonical.clone(),
                            ),
                        ]
                    })
                    .collect();
                descriptors.push(
                    ImportedPhaseModule::new(
                        crate::stdlib::CORE_NAMESPACE,
                        interface.imported_phase_forms(),
                        visible,
                    )
                    .with_definition_names(
                        interface
                            .bindings
                            .iter()
                            .map(|binding| (binding.canonical.clone(), binding.canonical.clone()))
                            .chain(
                                interface.macros.iter().map(|macro_| {
                                    (macro_.canonical.clone(), macro_.canonical.clone())
                                }),
                            )
                            .collect(),
                    ),
                );
            }
            Err(error) => expander.diagnostics.push(Diagnostic::error(
                "OSR-M0010",
                format!("cannot load standard interface: {error}"),
                surface.module.span,
            )),
        }
    }
    for item in &surface.module.items {
        let crate::ast::ItemKind::Import(import) = &item.kind else {
            continue;
        };
        if !crate::stdlib::is_standard_namespace(&import.module.canonical) {
            continue;
        }
        if imported_phase_modules
            .iter()
            .any(|module| module.namespace == import.module.canonical)
        {
            continue;
        }
        let interface = match crate::stdlib::interface_artifact(&import.module.canonical) {
            Ok(interface) => interface,
            Err(error) => {
                expander.diagnostics.push(Diagnostic::error(
                    "OSR-M0010",
                    format!("cannot load standard interface: {error}"),
                    import.span,
                ));
                continue;
            }
        };
        let macro_names = interface
            .macros
            .iter()
            .map(|macro_| macro_.canonical.clone())
            .collect::<BTreeSet<_>>();
        if macro_names.is_empty() {
            continue;
        }
        let excluded = import
            .excluded
            .iter()
            .map(|name| name.canonical.as_str())
            .collect::<BTreeSet<_>>();
        let renamed = import
            .renamed
            .iter()
            .map(|rename| {
                (
                    rename.canonical.canonical.as_str(),
                    rename.local.canonical.as_str(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let qualifier = import
            .alias
            .as_ref()
            .map_or(import.module.canonical.as_str(), |alias| {
                alias.canonical.as_str()
            });
        let mut visible = BTreeMap::new();
        for canonical in &macro_names {
            if excluded.contains(canonical.as_str()) {
                continue;
            }
            visible.insert(format!("{qualifier}/{canonical}"), canonical.clone());
            visible.insert(format!("{qualifier}.{canonical}"), canonical.clone());
        }
        let referred = if import.refer_all {
            macro_names.iter().cloned().collect::<BTreeSet<_>>()
        } else {
            import
                .members
                .iter()
                .map(|name| name.canonical.clone())
                .collect()
        };
        for canonical in referred {
            if excluded.contains(canonical.as_str()) || !macro_names.contains(&canonical) {
                continue;
            }
            let local = renamed
                .get(canonical.as_str())
                .copied()
                .unwrap_or(canonical.as_str());
            visible.insert(local.to_owned(), canonical);
        }
        descriptors.push(
            ImportedPhaseModule::new(
                import.module.canonical.clone(),
                interface.imported_phase_forms(),
                visible,
            )
            .with_definition_names(
                interface
                    .bindings
                    .iter()
                    .map(|binding| (binding.canonical.clone(), binding.canonical.clone()))
                    .chain(
                        interface
                            .macros
                            .iter()
                            .map(|macro_| (macro_.canonical.clone(), macro_.canonical.clone())),
                    )
                    .collect(),
            ),
        );
    }
    expander.collect_imported_phase_modules(&descriptors);
}

fn document_module_name(document: &Document) -> Option<&str> {
    document.forms.iter().find_map(|form| {
        let FormKind::List(items) = &form.kind else {
            return None;
        };
        (items.first().and_then(symbol_canonical) == Some("module"))
            .then(|| items.get(1).and_then(symbol_canonical))
            .flatten()
    })
}

fn expand_document(document: &Document, mut expander: Expander) -> ExpansionResult {
    expander.collect_phase_one_declarations(&document.forms);
    let forms = document
        .forms
        .iter()
        .flat_map(|form| expander.expand_top_level_forms(form))
        .collect();
    let mut diagnostics = document.diagnostics.clone();
    diagnostics.append(&mut expander.diagnostics);
    diagnostics
        .sort_by_key(|diagnostic| (diagnostic.span.start, diagnostic.span.end, diagnostic.code));

    ExpansionResult {
        document: Document {
            format_version: document.format_version,
            source_len: document.source_len,
            tokens: document.tokens.clone(),
            forms,
            nodes: Vec::new(),
            diagnostics,
        },
        traces: expander.traces,
    }
}

/// Validate replayable declarations with the evaluator's own declaration
/// parser. Callers remain responsible for rejecting non-phase forms.
#[must_use]
pub fn validate_phase_forms(forms: &[Form]) -> Vec<Diagnostic> {
    let mut expander = Expander::new(ExpansionOptions::default(), "osiris.validation");
    expander.collect_phase_one_declarations(forms);
    expander.diagnostics
}

struct Expander {
    options: ExpansionOptions,
    local_module_name: String,
    expansions: usize,
    next_generated_name: u64,
    macros: BTreeMap<String, FunctionDef>,
    macro_exports: BTreeMap<String, String>,
    phase_functions: BTreeMap<String, FunctionDef>,
    definition_names: BTreeMap<String, BTreeMap<String, String>>,
    active_phase_namespace: Option<String>,
    active_origins: Vec<Span>,
    diagnostics: Vec<Diagnostic>,
    traces: Vec<ExpansionTrace>,
}

#[derive(Clone, Copy)]
enum PhaseDeclarationKind {
    Macro,
    Function,
}

mod collections;
mod declarations;
mod forms;
mod numbers;

use collections::*;
use declarations::*;
use forms::*;
use numbers::*;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
