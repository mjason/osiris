//! Name-resolved, typed high-level intermediate representation.

use std::collections::{BTreeMap, BTreeSet};

use unicode_normalization::UnicodeNormalization;

use crate::{
    ast::{
        self, CallArg as AstCallArg, ExprKind as AstExprKind, ItemKind as AstItemKind, PatternKind,
        TypeExprKind,
    },
    diagnostic::Diagnostic,
    interface::{Interface, PublicBinding},
    name::{BindingId, BindingKind, BindingName, NameAllocator, python_identifier},
    source::Span,
    syntax::{Form, FormKind, MetadataEntry, Name},
    types::{
        Alignment, Availability, CallSummaries, DataProperties, Effect, EffectRow, FunctionType,
        OperatorInstance, OperatorSignature, ScalarOperator, TemporalBound, Type, TypeContext,
        TypeLiteral, TypeVarId, python_builtin_exception_binding,
        python_builtin_exception_from_binding, python_builtin_exception_names,
        scalar_operator_signatures,
    },
};

mod lower;
mod metadata;
mod model;
mod operators;
mod patterns;
mod sequence_operations;
mod type_resolution;

use metadata::*;
pub use model::*;
use operators::*;
use patterns::*;
pub(in crate::hir) use sequence_operations::*;
use type_resolution::*;
pub(crate) use type_resolution::{
    resolve_nominal_bindings, type_from_ast, type_from_ast_with_generics,
};

#[must_use]
pub fn lower_module(module: &ast::Module, fallback_name: &str) -> LowerResult {
    lower_module_with_trust_policy(module, fallback_name, &ContractTrustPolicy::default())
}

#[must_use]
pub fn lower_module_with_trust_policy(
    module: &ast::Module,
    fallback_name: &str,
    trust_policy: &ContractTrustPolicy,
) -> LowerResult {
    lower_module_internal(module, fallback_name, None, trust_policy)
}

/// Lower a module while resolving ordinary imports against an explicit,
/// already-validated interface provider.  The provider is intentionally
/// abstract so callers can pass either `ModuleGraph::interfaces()` or another
/// read-only interface catalog without coupling the HIR pass to filesystem or
/// Python package discovery.
#[must_use]
pub fn lower_module_with_interfaces<P: InterfaceProvider>(
    module: &ast::Module,
    fallback_name: &str,
    interfaces: &P,
) -> LowerResult {
    lower_module_with_interfaces_and_trust_policy(
        module,
        fallback_name,
        interfaces,
        &ContractTrustPolicy::default(),
    )
}

#[must_use]
pub fn lower_module_with_interfaces_and_trust_policy<P: InterfaceProvider>(
    module: &ast::Module,
    fallback_name: &str,
    interfaces: &P,
    trust_policy: &ContractTrustPolicy,
) -> LowerResult {
    lower_module_internal(module, fallback_name, Some(interfaces), trust_policy)
}

fn lower_module_internal(
    module: &ast::Module,
    fallback_name: &str,
    interfaces: Option<&dyn InterfaceProvider>,
    trust_policy: &ContractTrustPolicy,
) -> LowerResult {
    let name = module
        .name
        .as_ref()
        .map_or_else(|| fallback_name.to_owned(), |name| name.canonical.clone());
    let mut lowerer = Lowerer::new(name, module, interfaces, trust_policy);
    lowerer.predeclare(module);
    lowerer.resolve_aliases(module);
    lowerer.resolve_nominal_types(module.span);
    lowerer.resolve_exports(module);
    lowerer.validate_boundary_signatures(module);
    lowerer.lower_items(module);
    lowerer.finish(module)
}

struct Lowerer<'a> {
    module_name: String,
    allocator: NameAllocator,
    bindings: BTreeMap<BindingId, Binding>,
    local_value_summaries: BTreeMap<BindingId, CallSummaries>,
    globals: BTreeMap<String, BindingId>,
    callables: BTreeMap<BindingId, CallableInfo>,
    struct_type_parameters: BTreeMap<BindingId, BTreeMap<String, Type>>,
    phase_one_names: BTreeSet<String>,
    aliases: Vec<Alias>,
    exports: BTreeSet<BindingId>,
    extern_functions: Vec<ExternFunction>,
    items: Vec<Item>,
    diagnostics: Vec<Diagnostic>,
    types: TypeContext,
    next_scope: u32,
    interfaces: Option<&'a dyn InterfaceProvider>,
    qualified_imports: BTreeMap<String, BindingId>,
    operator_instances: BTreeMap<String, OperatorInstance>,
    operator_contract_evidence: BTreeMap<String, ContractEvidence>,
    core_abs_binding: Option<BindingId>,
    core_mapv_binding: Option<BindingId>,
    core_collection_bindings: BTreeMap<String, BindingId>,
    core_loop_binding: Option<BindingId>,
    core_recur_binding: Option<BindingId>,
    loop_arities: Vec<usize>,
    loop_state_types: Vec<Vec<Type>>,
    /// Lexical function depth used to keep `recur` inside its owning loop
    /// callback. A nested lambda must not capture an outer loop's recur.
    function_depth: usize,
    loop_callback_depths: Vec<usize>,
    /// Function-local `recur` frames.  Unlike an explicit `loop`, a function
    /// frame is installed for the body of every `defn`/`fn`; each frame is
    /// keyed by lexical function depth so a nested lambda cannot capture an
    /// outer function's recur target.
    function_recur_contexts: Vec<FunctionRecurContext>,
    struct_fields: BTreeMap<String, StructFieldTable>,
    trust_policy: &'a ContractTrustPolicy,
    contract_evidence_stack: Vec<ContractEvidence>,
    unknown_nominal_types: BTreeSet<String>,
}

#[derive(Clone)]
struct CallableInfo {
    signature: FunctionType,
    parameters: Vec<CallableParameter>,
    generic_variables: Vec<TypeVarId>,
    contract_evidence: ContractEvidence,
}

#[derive(Clone)]
struct CallableParameter {
    canonical: String,
    accepted_names: BTreeSet<String>,
    ty: Type,
    required: bool,
    variadic: bool,
    span: Span,
}

struct FunctionRecurContext {
    depth: usize,
    state_types: Vec<Type>,
    used: bool,
}

#[derive(Clone)]
struct StructFieldInfo {
    canonical: String,
    ty: Type,
}

#[derive(Clone, Default)]
struct StructFieldTable {
    generic_variables: Vec<TypeVarId>,
    fields: BTreeMap<String, StructFieldInfo>,
}

#[derive(Clone)]
struct OperatorChoice {
    result: Type,
    summaries: CallSummaries,
    binding: Option<BindingId>,
    contract_evidence: ContractEvidence,
}

pub(in crate::hir) enum OperatorSelection {
    Selected(Box<OperatorChoice>),
    None,
    Ambiguous,
}

impl Expr {
    fn pure(span: Span, ty: Type, kind: ExprKind) -> Self {
        Self {
            span,
            ty,
            summaries: CallSummaries::pure_scalar(),
            kind,
        }
    }

    fn error(span: Span) -> Self {
        Self::pure(span, Type::Error, ExprKind::Error)
    }
}

#[derive(Default)]
struct Scope {
    frames: Vec<BTreeMap<String, BindingId>>,
    python_frames: Vec<BTreeMap<String, String>>,
}

impl Scope {
    fn push(&mut self) {
        self.frames.push(BTreeMap::new());
        self.python_frames.push(BTreeMap::new());
    }

    fn pop(&mut self) {
        self.frames.pop();
        self.python_frames.pop();
    }

    fn insert(&mut self, name: String, binding: BindingId, python_name: String) {
        if self.frames.is_empty() {
            self.push();
        }
        let canonical_name = name.clone();
        self.frames
            .last_mut()
            .expect("scope frame exists")
            .insert(name, binding);
        self.python_frames
            .last_mut()
            .expect("scope frame exists")
            .insert(python_name, canonical_name);
    }

    fn current_contains(&self, name: &str) -> bool {
        self.frames
            .last()
            .is_some_and(|frame| frame.contains_key(name))
    }

    fn current_python_name(&self, name: &str) -> Option<&str> {
        self.python_frames
            .last()
            .and_then(|frame| frame.get(name))
            .map(String::as_str)
    }

    fn resolve(&self, name: &str) -> Option<&BindingId> {
        self.frames.iter().rev().find_map(|frame| frame.get(name))
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
