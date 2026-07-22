//! Name-resolved, typed high-level intermediate representation.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
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

/// Read-only source of validated `.osri` interfaces used by the cross-module
/// lowering entry point.  Implementations must return data already checked by
/// [`crate::interface::read`]; this trait never loads or executes Python.
pub trait InterfaceProvider {
    fn interface(&self, module: &str) -> Option<&Interface>;
}

impl InterfaceProvider for BTreeMap<String, Interface> {
    fn interface(&self, module: &str) -> Option<&Interface> {
        self.get(module)
    }
}

impl InterfaceProvider for crate::module_graph::ModuleGraph {
    fn interface(&self, module: &str) -> Option<&Interface> {
        self.interface(module)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ContractTrustPolicy {
    pub hash: String,
    pub interfaces: BTreeMap<String, InterfaceTrustPolicy>,
}

impl ContractTrustPolicy {
    #[must_use]
    pub fn untrusted(hash: impl Into<String>) -> Self {
        Self {
            hash: hash.into(),
            interfaces: BTreeMap::new(),
        }
    }
}

impl Default for ContractTrustPolicy {
    fn default() -> Self {
        Self::untrusted(format!("sha256:{}", "0".repeat(64)))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InterfaceTrustPolicy {
    pub distribution: String,
    pub semantic_interface_hash: String,
    pub trusted_contract_ids: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ContractFact {
    pub distribution: Option<String>,
    pub provider_module: String,
    pub semantic_interface_hash: Option<String>,
    pub binding: String,
    pub contract_id: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ContractEvidence {
    pub declared: BTreeSet<ContractFact>,
    pub verified: BTreeSet<ContractFact>,
}

impl ContractEvidence {
    #[must_use]
    pub fn join(&self, other: &Self) -> Self {
        Self {
            declared: self.declared.union(&other.declared).cloned().collect(),
            verified: self.verified.union(&other.verified).cloned().collect(),
        }
    }

    fn unverified(&self) -> impl Iterator<Item = &ContractFact> {
        self.declared.difference(&self.verified)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct LowerResult {
    pub module: Module,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Module {
    pub name: String,
    pub trust_policy_hash: String,
    pub span: Span,
    pub metadata: Vec<MetadataEntry>,
    pub bindings: Vec<Binding>,
    pub aliases: Vec<Alias>,
    pub exports: Vec<BindingId>,
    pub extern_functions: Vec<ExternFunction>,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Binding {
    pub name: BindingName,
    pub source_spelling: String,
    pub ty: Type,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeBinding>,
    pub public: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Vec<MetadataEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RuntimeBinding {
    pub module: String,
    pub name: String,
    pub python_module: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Alias {
    pub spelling: String,
    pub canonical: String,
    pub target: BindingId,
    pub span: Span,
    pub public: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Item {
    pub span: Span,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata: Vec<MetadataEntry>,
    pub kind: ItemKind,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
#[allow(clippy::large_enum_variant)]
pub enum ItemKind {
    Import(Import),
    Value(Value),
    Function(Function),
    Struct(Struct),
    Expr(Expr),
    StaticSchema(ast::DefstaticSchema),
    StaticRecord(ast::StaticRecord),
}

#[derive(Clone, Debug, Serialize)]
pub struct Import {
    pub binding: BindingId,
    pub module: String,
    pub python: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Value {
    pub binding: BindingId,
    pub value: Option<Expr>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Function {
    pub binding: BindingId,
    pub parameters: Vec<Parameter>,
    pub return_type: Type,
    pub body: Expr,
    pub summaries: CallSummaries,
    pub contract_evidence: ContractEvidence,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub causal: Option<CausalRequirement>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CausalRequirement {
    pub decision_point: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ExternFunction {
    pub binding: BindingId,
    pub parameters: Vec<Parameter>,
    pub return_type: Type,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contract_id: Option<String>,
    pub summaries: CallSummaries,
    pub contract_evidence: ContractEvidence,
}

#[derive(Clone, Debug, Serialize)]
pub struct Parameter {
    pub binding: BindingId,
    pub ty: Type,
    pub default: Option<Expr>,
    pub variadic: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Struct {
    pub binding: BindingId,
    pub type_parameters: Vec<String>,
    pub fields: Vec<StructField>,
    pub checks: Vec<StructCheck>,
    pub doc: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct StructField {
    pub binding: BindingId,
    pub ty: Type,
    pub default: Option<Expr>,
}

#[derive(Clone, Debug, Serialize)]
pub struct StructCheck {
    pub span: Span,
    pub condition: Expr,
    pub message: Option<Expr>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Expr {
    pub span: Span,
    pub ty: Type,
    pub summaries: CallSummaries,
    pub kind: ExprKind,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum ExprKind {
    None,
    Bool(bool),
    Integer(String),
    Float(String),
    String(String),
    Binding(BindingId),
    List(Vec<Expr>),
    Vector(Vec<Expr>),
    Map(Vec<(Expr, Expr)>),
    Set(Vec<Expr>),
    Call {
        callee: Box<Expr>,
        arguments: Vec<CallArgument>,
    },
    Operator {
        operator: Operator,
        operands: Vec<Expr>,
    },
    Attribute {
        value: Box<Expr>,
        attribute: String,
    },
    Index {
        value: Box<Expr>,
        index: Box<Expr>,
    },
    Let {
        bindings: Vec<LetBinding>,
        body: Box<Expr>,
    },
    If {
        condition: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
    },
    Do(Vec<Expr>),
    Lambda {
        parameters: Vec<Parameter>,
        body: Box<Expr>,
    },
    Try {
        body: Box<Expr>,
        catches: Vec<Catch>,
        finally_body: Option<Box<Expr>>,
    },
    Raise(Option<Box<Expr>>),
    Error,
}

#[derive(Clone, Debug, Serialize)]
pub enum CallArgument {
    Positional(Expr),
    Keyword { name: String, value: Expr },
}

#[derive(Clone, Debug, Serialize)]
pub struct LetBinding {
    pub binding: BindingId,
    pub value: Expr,
}

#[derive(Clone, Debug, Serialize)]
pub struct Catch {
    pub exception_type: Option<Type>,
    pub binding: Option<BindingId>,
    pub body: Expr,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Operator {
    Add,
    Subtract,
    Multiply,
    Divide,
    FloorDivide,
    Remainder,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    And,
    Or,
    Not,
    Negate,
    Positive,
}

impl Operator {
    fn scalar(self) -> Option<ScalarOperator> {
        Some(match self {
            Self::Add => ScalarOperator::Add,
            Self::Subtract => ScalarOperator::Subtract,
            Self::Multiply => ScalarOperator::Multiply,
            Self::Divide => ScalarOperator::TrueDivide,
            Self::FloorDivide => ScalarOperator::FloorDivide,
            Self::Remainder => ScalarOperator::Remainder,
            Self::Equal => ScalarOperator::Equal,
            Self::NotEqual => ScalarOperator::NotEqual,
            Self::Less => ScalarOperator::Less,
            Self::LessEqual => ScalarOperator::LessEqual,
            Self::Greater => ScalarOperator::Greater,
            Self::GreaterEqual => ScalarOperator::GreaterEqual,
            Self::Negate => ScalarOperator::Negate,
            Self::Positive => ScalarOperator::Positive,
            Self::And | Self::Or | Self::Not => return None,
        })
    }
}

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

enum OperatorSelection {
    Selected(Box<OperatorChoice>),
    None,
    Ambiguous,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CollectionOperation {
    Map,
    Mapcat,
    Mapcatv,
    Filter,
    Filterv,
}

/// Sequence helpers share one small lowering path.  They remain ordinary
/// runtime functions in the Python prelude; the enum only gives typed HIR a
/// stable contract for their callback and result shapes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SequenceOperation {
    Cons,
    Concat,
    Count,
    EmptyQ,
    SeqQ,
    CollQ,
    SequentialQ,
    First,
    Rest,
    Next,
    Nth,
    Seq,
    Empty,
    Take,
    Drop,
    TakeWhile,
    DropWhile,
    Keep,
    KeepIndexed,
    Remove,
    Removev,
    Distinct,
    Dedupe,
    Partition,
    PartitionAll,
    PartitionBy,
    Interleave,
    Interpose,
    TakeLast,
    DropLast,
    MapIndexed,
    Iterate,
    Repeat,
    Repeatedly,
    Cycle,
    Sequence,
    Reductions,
    RunBang,
    Doall,
    Dorun,
    Some,
    Every,
    NotEvery,
    NotAny,
}

impl SequenceOperation {
    fn runtime_name(self) -> &'static str {
        match self {
            Self::Cons => "cons",
            Self::Concat => "concat",
            Self::Count => "count",
            Self::EmptyQ => "empty_q",
            Self::SeqQ => "seq_q",
            Self::CollQ => "coll_q",
            Self::SequentialQ => "sequential_q",
            Self::First => "first",
            Self::Rest => "rest",
            Self::Next => "next",
            Self::Nth => "nth",
            Self::Seq => "seq",
            Self::Empty => "empty",
            Self::Take => "take",
            Self::Drop => "drop",
            Self::TakeWhile => "take_while",
            Self::DropWhile => "drop_while",
            Self::Keep => "keep",
            Self::KeepIndexed => "keep_indexed",
            Self::Remove => "remove",
            Self::Removev => "removev",
            Self::Distinct => "distinct",
            Self::Dedupe => "dedupe",
            Self::Partition => "partition",
            Self::PartitionAll => "partition_all",
            Self::PartitionBy => "partition_by",
            Self::Interleave => "interleave",
            Self::Interpose => "interpose",
            Self::TakeLast => "take_last",
            Self::DropLast => "drop_last",
            Self::MapIndexed => "map_indexed",
            Self::Iterate => "iterate",
            Self::Repeat => "repeat",
            Self::Repeatedly => "repeatedly",
            Self::Cycle => "cycle",
            Self::Sequence => "sequence",
            Self::Reductions => "reductions",
            Self::RunBang => "run_bang",
            Self::Doall => "doall",
            Self::Dorun => "dorun",
            Self::Some => "some",
            Self::Every => "every_q",
            Self::NotEvery => "not_every_q",
            Self::NotAny => "not_any_q",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ControlIntrinsic {
    Truthy,
    Nil,
    Present,
    Nonempty,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReducedOperation {
    Wrap,
    Predicate,
    Unwrap,
}

impl ReducedOperation {
    fn runtime_name(self) -> &'static str {
        match self {
            Self::Wrap => "reduced",
            Self::Predicate => "reduced_p",
            Self::Unwrap => "unreduced",
        }
    }
}

impl ControlIntrinsic {
    fn runtime_name(self) -> &'static str {
        match self {
            Self::Truthy => "truthy",
            Self::Nil => "is_nil",
            Self::Present => "present",
            Self::Nonempty => "nonempty",
        }
    }
}

impl CollectionOperation {
    fn runtime_name(self) -> &'static str {
        match self {
            Self::Map => "map",
            Self::Mapcat => "mapcat",
            Self::Mapcatv => "mapcatv",
            Self::Filter => "filter",
            Self::Filterv => "filterv",
        }
    }

    fn result_is_vector(self) -> bool {
        matches!(self, Self::Mapcatv | Self::Filterv)
    }
}

impl<'a> Lowerer<'a> {
    fn new(
        module_name: String,
        module: &ast::Module,
        interfaces: Option<&'a dyn InterfaceProvider>,
        trust_policy: &'a ContractTrustPolicy,
    ) -> Self {
        let _ = module;
        let mut lowerer = Self {
            module_name,
            allocator: NameAllocator::default(),
            bindings: BTreeMap::new(),
            local_value_summaries: BTreeMap::new(),
            globals: BTreeMap::new(),
            callables: BTreeMap::new(),
            struct_type_parameters: BTreeMap::new(),
            phase_one_names: BTreeSet::new(),
            aliases: Vec::new(),
            exports: BTreeSet::new(),
            extern_functions: Vec::new(),
            items: Vec::new(),
            diagnostics: Vec::new(),
            types: TypeContext::new(),
            next_scope: 0,
            interfaces,
            qualified_imports: BTreeMap::new(),
            operator_instances: BTreeMap::new(),
            operator_contract_evidence: BTreeMap::new(),
            core_abs_binding: None,
            core_mapv_binding: None,
            core_collection_bindings: BTreeMap::new(),
            core_loop_binding: None,
            core_recur_binding: None,
            loop_arities: Vec::new(),
            loop_state_types: Vec::new(),
            function_depth: 0,
            loop_callback_depths: Vec::new(),
            function_recur_contexts: Vec::new(),
            struct_fields: BTreeMap::new(),
            trust_policy,
            contract_evidence_stack: Vec::new(),
            unknown_nominal_types: BTreeSet::new(),
        };
        lowerer.install_core_reduced_type(module.span);
        lowerer.install_core_delay_type(module.span);
        lowerer.install_core_future_type(module.span);
        lowerer.install_core_promise_type(module.span);
        lowerer
    }

    fn finish(self, source: &ast::Module) -> LowerResult {
        LowerResult {
            module: Module {
                name: self.module_name,
                trust_policy_hash: self.trust_policy.hash.clone(),
                span: source.span,
                metadata: source.metadata.clone(),
                bindings: self.bindings.into_values().collect(),
                aliases: self.aliases,
                exports: self.exports.into_iter().collect(),
                extern_functions: self.extern_functions,
                items: self.items,
            },
            diagnostics: self.diagnostics,
        }
    }

    fn predeclare(&mut self, module: &ast::Module) {
        for item in &module.items {
            match &item.kind {
                AstItemKind::Import(import) => {
                    let name = import.alias.as_ref().unwrap_or(&import.module);
                    self.declare(
                        name,
                        BindingKind::Module,
                        Type::Any,
                        item.metadata.clone(),
                        import.span,
                        Some(RuntimeBinding {
                            module: import.module.canonical.replace('/', "."),
                            name: name.canonical.clone(),
                            python_module: false,
                        }),
                    );
                    self.predeclare_interface_import(import, item.metadata.as_slice());
                }
                AstItemKind::PyImport(import) => {
                    let default_name = import.module.rsplit('.').next().unwrap_or(&import.module);
                    let name = import.alias.clone().unwrap_or_else(|| Name {
                        spelling: default_name.to_owned(),
                        canonical: default_name.to_owned(),
                    });
                    self.declare(
                        &name,
                        BindingKind::PythonModule,
                        Type::Any,
                        item.metadata.clone(),
                        import.span,
                        Some(RuntimeBinding {
                            module: import.module.clone(),
                            name: name.canonical.clone(),
                            python_module: true,
                        }),
                    );
                }
                AstItemKind::Def(definition) => {
                    let ty = definition
                        .type_annotation
                        .as_ref()
                        .map_or_else(|| self.types.fresh_var(), type_from_ast);
                    self.declare(
                        &definition.name,
                        BindingKind::Value,
                        ty,
                        definition.metadata.clone(),
                        definition.span,
                        None,
                    );
                }
                AstItemKind::Defn(function) => self.predeclare_function(function, false, None),
                AstItemKind::Defstruct(structure) => self.predeclare_struct(structure),
                AstItemKind::DefstaticSchema(schema) => {
                    let type_binding = BindingId::new(
                        &self.module_name,
                        &schema.name.canonical,
                        BindingKind::Type,
                    );
                    self.declare(
                        &schema.name,
                        BindingKind::Type,
                        Type::Nominal {
                            binding: type_binding.as_str().to_owned(),
                            args: Vec::new(),
                        },
                        schema.metadata.clone(),
                        schema.span,
                        None,
                    );
                }
                AstItemKind::Extern(extern_block) => {
                    for nested in &extern_block.items {
                        match &nested.kind {
                            AstItemKind::Defn(function) => self.predeclare_function(
                                function,
                                true,
                                Some(extern_block.module.as_str()),
                            ),
                            AstItemKind::Def(definition) => {
                                let ty = definition
                                    .type_annotation
                                    .as_ref()
                                    .map_or(Type::Any, type_from_ast);
                                self.declare(
                                    &definition.name,
                                    BindingKind::Value,
                                    ty,
                                    definition.metadata.clone(),
                                    definition.span,
                                    Some(RuntimeBinding {
                                        module: extern_block.module.clone(),
                                        name: python_identifier(&definition.name.canonical),
                                        python_module: true,
                                    }),
                                );
                            }
                            _ => self.error(
                                "OSR-H0001",
                                "extern currently accepts defn and def declarations",
                                nested.span,
                            ),
                        }
                    }
                }
                AstItemKind::Defmacro(macro_definition) => {
                    self.phase_one_names
                        .insert(macro_definition.name.canonical.clone());
                }
                AstItemKind::DefnForSyntax(function) => {
                    if let Some(name) = &function.name {
                        self.phase_one_names.insert(name.canonical.clone());
                    }
                }
                AstItemKind::ImportForSyntax(import) => {
                    let name = import.alias.as_ref().unwrap_or(&import.module);
                    self.phase_one_names.insert(name.canonical.clone());
                }
                AstItemKind::Export(_)
                | AstItemKind::Alias(_)
                | AstItemKind::StaticRecord(_)
                | AstItemKind::Expr(_)
                | AstItemKind::Error(_) => {}
            }
        }
    }

    /// Install the public surface of one validated Osiris interface.  We keep
    /// the module binding itself (for Python's `import module` emission), then
    /// synthesize target bindings for public members so both `:refer` and
    /// qualified calls use the dependency's stable binding id and signature.
    fn predeclare_interface_import(&mut self, import: &ast::Import, metadata: &[MetadataEntry]) {
        let Some(provider) = self.interfaces else {
            return;
        };
        let module_name = import.module.canonical.clone();
        let Some(interface) = provider.interface(&module_name).cloned() else {
            self.error(
                "OSR-H0010",
                format!("imported module `{module_name}` has no validated interface"),
                import.span,
            );
            return;
        };

        self.merge_imported_operator_instances(&interface, import.span);

        let mut bindings = BTreeMap::<String, BindingId>::new();
        for public in &interface.bindings {
            if let Some(id) =
                self.install_imported_binding(public, &interface, None, metadata, import.span)
            {
                bindings.insert(public.canonical.clone(), id.clone());
                let base = import
                    .alias
                    .as_ref()
                    .map_or(module_name.as_str(), |alias| alias.canonical.as_str());
                for qualifier in [base, module_name.as_str()] {
                    self.qualified_imports
                        .insert(format!("{qualifier}/{}", public.canonical), id.clone());
                    self.qualified_imports
                        .insert(format!("{qualifier}.{}", public.canonical), id.clone());
                }
            }
        }

        let base = import
            .alias
            .as_ref()
            .map_or(module_name.as_str(), |alias| alias.canonical.as_str())
            .to_owned();
        for alias in &interface.aliases {
            let Some(target) = bindings.get(&alias_target_canonical(&interface, alias)) else {
                continue;
            };
            for qualifier in [base.as_str(), module_name.as_str()] {
                self.qualified_imports
                    .insert(format!("{qualifier}/{}", alias.canonical), target.clone());
                self.qualified_imports
                    .insert(format!("{qualifier}.{}", alias.canonical), target.clone());
                self.qualified_imports
                    .insert(format!("{qualifier}/{}", alias.spelling), target.clone());
                self.qualified_imports
                    .insert(format!("{qualifier}.{}", alias.spelling), target.clone());
            }
        }

        let mut requested = BTreeSet::new();
        for member in &import.members {
            requested.insert(member.canonical.clone());
            let Some(public) = find_imported_binding(&interface, &member.canonical) else {
                self.error(
                    "OSR-H0011",
                    format!(
                        "module `{module_name}` does not export imported member `{}`",
                        member.spelling
                    ),
                    member_span(member, import.span),
                );
                continue;
            };
            let Some(id) = self.install_imported_binding(
                public,
                &interface,
                Some(member.canonical.as_str()),
                metadata,
                import.span,
            ) else {
                continue;
            };
            self.globals.insert(member.canonical.clone(), id);
        }

        // Keep a direct alias spelling available for `:refer` requests even
        // when the interface normalized its canonical alias separately.
        for alias in &interface.aliases {
            if requested.contains(&alias.canonical) || requested.contains(&alias.spelling) {
                if let Some(id) = bindings.get(&alias_target_canonical(&interface, alias)) {
                    self.globals
                        .insert(requested_alias_key(&requested, alias), id.clone());
                }
            }
        }
    }

    fn merge_imported_operator_instances(&mut self, interface: &Interface, span: Span) {
        for instance in &interface.operator_instances {
            if let Some(existing) = self.operator_instances.get(&instance.id) {
                if existing != instance {
                    self.error(
                        "OSR-H0020",
                        format!(
                            "imported operator instance id `{}` has conflicting declarations",
                            instance.id
                        ),
                        span,
                    );
                }
                continue;
            }
            if self.operator_instances.values().any(|existing| {
                existing.operator == instance.operator
                    && existing.operands == instance.operands
                    && existing.id != instance.id
            }) {
                self.error(
                    "OSR-H0021",
                    format!(
                        "operator `{}` has conflicting imported operand tuple",
                        instance.operator.stable_name()
                    ),
                    span,
                );
                continue;
            }
            if let Some(public) = interface
                .bindings
                .iter()
                .find(|binding| binding.id == instance.binding)
            {
                let evidence =
                    self.imported_contract_evidence(interface, public, Some(&instance.id));
                self.operator_contract_evidence
                    .insert(instance.id.clone(), evidence);
            }
            self.operator_instances
                .insert(instance.id.clone(), instance.clone());
        }
    }

    fn install_imported_binding(
        &mut self,
        public: &PublicBinding,
        interface: &Interface,
        local_name: Option<&str>,
        _metadata: &[MetadataEntry],
        span: Span,
    ) -> Option<BindingId> {
        let id = BindingId::from_interface(public.id.clone());
        if !self.bindings.contains_key(&id) {
            let runtime = public.runtime.as_ref().map_or_else(
                || RuntimeBinding {
                    module: interface.module.clone(),
                    name: public.python.clone(),
                    python_module: false,
                },
                |runtime| RuntimeBinding {
                    module: if runtime.module.is_empty() {
                        interface.module.clone()
                    } else {
                        runtime.module.clone()
                    },
                    name: runtime.name.clone(),
                    python_module: runtime.python_module,
                },
            );
            let name = BindingName {
                id: id.clone(),
                canonical: public.canonical.clone(),
                python: if public.python.is_empty() {
                    python_identifier(&public.canonical)
                } else {
                    public.python.clone()
                },
                kind: public.kind,
                span,
            };
            self.bindings.insert(
                id.clone(),
                Binding {
                    name,
                    source_spelling: local_name.unwrap_or(&public.canonical).to_owned(),
                    ty: public.ty.clone(),
                    runtime: Some(runtime),
                    public: false,
                    metadata: public.metadata.clone(),
                },
            );
            self.register_imported_callable(&id, public, interface);
        }
        if let Some(local_name) = local_name {
            if let Some(existing) = self.globals.get(local_name)
                && existing != &id
            {
                self.error(
                    "OSR-N0003",
                    format!("imported name `{local_name}` conflicts with another binding"),
                    span,
                );
                return None;
            }
        }
        Some(id)
    }

    fn register_imported_callable(
        &mut self,
        id: &BindingId,
        public: &PublicBinding,
        interface: &Interface,
    ) {
        match public.kind {
            BindingKind::Function => {
                let Some(function) = interface
                    .functions
                    .iter()
                    .find(|function| function.binding == id.as_str())
                else {
                    self.error(
                        "OSR-H0012",
                        format!(
                            "interface `{}` has no function signature for `{}`",
                            interface.module,
                            id.as_str()
                        ),
                        Span::default(),
                    );
                    return;
                };
                let mut variables = BTreeMap::new();
                let parameters = function
                    .parameters
                    .iter()
                    .map(|parameter| {
                        import_type_with_variables(&mut self.types, &parameter.ty, &mut variables)
                    })
                    .collect::<Vec<_>>();
                let return_type = import_type_with_variables(
                    &mut self.types,
                    &function.return_type,
                    &mut variables,
                );
                let signature = FunctionType::new(parameters.clone(), return_type)
                    .with_summaries(function.summaries.clone());
                self.set_binding_type(id, Type::Fn(signature.clone()));
                let callable_parameters = function
                    .parameters
                    .iter()
                    .zip(parameters)
                    .map(|(parameter, ty)| CallableParameter {
                        canonical: parameter.canonical.clone(),
                        accepted_names: interface_parameter_names(parameter),
                        ty,
                        required: !parameter.has_default && !parameter.variadic,
                        variadic: parameter.variadic,
                        span: Span::default(),
                    })
                    .collect();
                let generic_variables = variables
                    .values()
                    .filter_map(|ty| match ty {
                        Type::TypeVar(variable) => Some(*variable),
                        _ => None,
                    })
                    .collect();
                let contract_evidence = self.imported_contract_evidence(
                    interface,
                    public,
                    function.contract_id.as_deref(),
                );
                self.register_callable(
                    id.clone(),
                    signature,
                    callable_parameters,
                    generic_variables,
                    contract_evidence,
                );
            }
            BindingKind::Type => {
                let Some(structure) = interface
                    .structs
                    .iter()
                    .find(|structure| structure.binding == id.as_str())
                else {
                    return;
                };
                let mut variables = BTreeMap::new();
                let fields = structure
                    .fields
                    .iter()
                    .map(|field| {
                        import_type_with_variables(&mut self.types, &field.ty, &mut variables)
                    })
                    .collect::<Vec<_>>();
                let return_type =
                    import_type_with_variables(&mut self.types, &public.ty, &mut variables);
                let generic_variables = match &return_type {
                    Type::Nominal { args, .. } => args
                        .iter()
                        .filter_map(|argument| match argument {
                            Type::TypeVar(variable) => Some(*variable),
                            _ => None,
                        })
                        .collect::<Vec<_>>(),
                    _ => Vec::new(),
                };
                let mut field_table = StructFieldTable {
                    generic_variables: generic_variables.clone(),
                    fields: BTreeMap::new(),
                };
                for (field, ty) in structure.fields.iter().zip(&fields) {
                    let info = StructFieldInfo {
                        canonical: field.canonical.clone(),
                        ty: ty.clone(),
                    };
                    for name in interface_field_names(field) {
                        field_table.fields.insert(name, info.clone());
                    }
                }
                self.struct_fields.insert(public.id.clone(), field_table);
                let mut summaries = CallSummaries::pure_scalar();
                if structure.invariant_count > 0 {
                    summaries.effects = EffectRow::singleton(Effect::Throw);
                }
                let signature =
                    FunctionType::new(fields.clone(), return_type).with_summaries(summaries);
                let callable_parameters = structure
                    .fields
                    .iter()
                    .zip(fields)
                    .map(|(field, ty)| CallableParameter {
                        canonical: field.canonical.clone(),
                        accepted_names: interface_field_names(field),
                        ty,
                        required: !field.has_default,
                        variadic: false,
                        span: Span::default(),
                    })
                    .collect();
                self.register_callable(
                    id.clone(),
                    signature,
                    callable_parameters,
                    generic_variables,
                    ContractEvidence::default(),
                );
            }
            _ => {}
        }
    }

    fn predeclare_function(
        &mut self,
        function: &ast::Function,
        external: bool,
        runtime_module: Option<&str>,
    ) {
        let Some(name) = &function.name else {
            return;
        };
        let parameter_types = function
            .params
            .iter()
            .map(|parameter| {
                parameter
                    .type_annotation
                    .as_ref()
                    .map_or_else(|| self.types.fresh_var(), type_from_ast)
            })
            .collect::<Vec<_>>();
        let return_type = function
            .return_type
            .as_ref()
            .map_or_else(|| self.types.fresh_var(), type_from_ast);
        let mut signature = FunctionType::new(parameter_types, return_type);
        let contract_evidence = if external {
            signature.summaries = function
                .contract
                .as_ref()
                .map_or_else(CallSummaries::unknown, |contract| {
                    contract.summaries.clone()
                });
            self.local_extern_contract_evidence(function)
        } else {
            signature.summaries = CallSummaries::unknown();
            ContractEvidence::default()
        };
        let callable_parameters = function
            .params
            .iter()
            .zip(&signature.parameters)
            .map(|(parameter, ty)| CallableParameter {
                canonical: parameter.name.canonical.clone(),
                accepted_names: parameter_names(&parameter.name, &parameter.metadata),
                ty: ty.clone(),
                required: parameter.default.is_none() && !parameter.variadic,
                variadic: parameter.variadic,
                span: parameter.span,
            })
            .collect::<Vec<_>>();
        if let Some(id) = self.declare(
            name,
            BindingKind::Function,
            Type::Fn(signature.clone()),
            function.metadata.clone(),
            function.span,
            runtime_module.map(|module| RuntimeBinding {
                module: module.to_owned(),
                name: python_identifier(&name.canonical),
                python_module: true,
            }),
        ) {
            self.register_callable(
                id,
                signature,
                callable_parameters,
                Vec::new(),
                contract_evidence,
            );
        }
        if external && runtime_module.is_none() {
            self.error(
                "OSR-H0002",
                "external function has no Python runtime module",
                function.span,
            );
        }
    }

    fn imported_contract_evidence(
        &self,
        interface: &Interface,
        public: &PublicBinding,
        contract_id: Option<&str>,
    ) -> ContractEvidence {
        let policy = self
            .trust_policy
            .interfaces
            .get(&interface.module)
            .filter(|policy| policy.semantic_interface_hash == interface.semantic_interface_hash());
        let fact = ContractFact {
            distribution: policy.map(|policy| policy.distribution.clone()),
            provider_module: interface.module.clone(),
            semantic_interface_hash: Some(interface.semantic_interface_hash().to_owned()),
            binding: public.id.clone(),
            contract_id: contract_id.map(str::to_owned),
        };
        let verified = policy
            .zip(contract_id)
            .is_some_and(|(policy, id)| policy.trusted_contract_ids.contains(id));
        ContractEvidence {
            declared: BTreeSet::from([fact.clone()]),
            verified: if verified {
                BTreeSet::from([fact])
            } else {
                BTreeSet::new()
            },
        }
    }

    fn local_extern_contract_evidence(&self, function: &ast::Function) -> ContractEvidence {
        let Some(name) = &function.name else {
            return ContractEvidence::default();
        };
        let fact = ContractFact {
            distribution: None,
            provider_module: self.module_name.clone(),
            semantic_interface_hash: None,
            binding: BindingId::new(&self.module_name, &name.canonical, BindingKind::Function)
                .as_str()
                .to_owned(),
            contract_id: function
                .contract
                .as_ref()
                .map(|contract| contract.id.clone()),
        };
        ContractEvidence {
            declared: BTreeSet::from([fact]),
            verified: BTreeSet::new(),
        }
    }

    fn predeclare_struct(&mut self, structure: &ast::Defstruct) {
        let type_binding = BindingId::new(
            &self.module_name,
            &structure.name.canonical,
            BindingKind::Type,
        );
        let generic_variables = structure
            .type_params
            .iter()
            .map(|_| match self.types.fresh_var() {
                Type::TypeVar(variable) => variable,
                _ => unreachable!("fresh_var always returns a type variable"),
            })
            .collect::<Vec<_>>();
        let generic_parameters = structure
            .type_params
            .iter()
            .map(|name| name.canonical.clone())
            .zip(generic_variables.iter().copied().map(Type::TypeVar))
            .collect::<BTreeMap<_, _>>();
        let nominal = Type::Nominal {
            binding: type_binding.as_str().to_owned(),
            args: generic_variables
                .iter()
                .copied()
                .map(Type::TypeVar)
                .collect(),
        };
        let parameter_types = structure
            .fields
            .iter()
            .map(|field| {
                field
                    .type_annotation
                    .as_ref()
                    .map_or(Type::Unknown, |expression| {
                        type_from_ast_with_generics(expression, &generic_parameters)
                    })
            })
            .collect::<Vec<_>>();
        let mut field_table = StructFieldTable {
            generic_variables: generic_variables.clone(),
            fields: BTreeMap::new(),
        };
        for (field, ty) in structure.fields.iter().zip(&parameter_types) {
            let info = StructFieldInfo {
                canonical: field.name.canonical.clone(),
                ty: ty.clone(),
            };
            for name in parameter_names(&field.name, &field.metadata) {
                field_table.fields.insert(name, info.clone());
            }
        }
        self.struct_fields
            .insert(type_binding.as_str().to_owned(), field_table);
        let callable_parameters = structure
            .fields
            .iter()
            .zip(&parameter_types)
            .map(|(field, ty)| CallableParameter {
                canonical: field.name.canonical.clone(),
                accepted_names: parameter_names(&field.name, &field.metadata),
                ty: ty.clone(),
                required: field.default.is_none(),
                variadic: false,
                span: field.span,
            })
            .collect();
        let mut signature = FunctionType::new(parameter_types, nominal.clone());
        if !structure.checks.is_empty() {
            signature.summaries.effects = EffectRow::singleton(Effect::Throw);
        }
        if let Some(id) = self.declare(
            &structure.name,
            BindingKind::Type,
            nominal,
            structure.metadata.clone(),
            structure.span,
            None,
        ) {
            self.struct_type_parameters
                .insert(id.clone(), generic_parameters);
            self.register_callable(
                id,
                signature,
                callable_parameters,
                generic_variables,
                ContractEvidence::default(),
            );
        }
    }

    fn declare(
        &mut self,
        source_name: &Name,
        kind: BindingKind,
        ty: Type,
        metadata: Vec<MetadataEntry>,
        span: Span,
        runtime: Option<RuntimeBinding>,
    ) -> Option<BindingId> {
        if let Some(existing) = self.globals.get(&source_name.canonical) {
            self.error(
                "OSR-N0001",
                format!(
                    "name `{}` conflicts with existing binding `{}`",
                    source_name.spelling,
                    existing.as_str()
                ),
                span,
            );
            return None;
        }
        match self
            .allocator
            .declare(&self.module_name, &source_name.spelling, kind, span)
        {
            Ok(name) => {
                let id = name.id.clone();
                self.globals.insert(name.canonical.clone(), id.clone());
                self.bindings.insert(
                    id.clone(),
                    Binding {
                        name,
                        source_spelling: source_name.spelling.clone(),
                        ty,
                        runtime,
                        public: false,
                        metadata,
                    },
                );
                Some(id)
            }
            Err(diagnostic) => {
                self.diagnostics.push(diagnostic);
                None
            }
        }
    }

    fn register_callable(
        &mut self,
        id: BindingId,
        signature: FunctionType,
        parameters: Vec<CallableParameter>,
        generic_variables: Vec<TypeVarId>,
        contract_evidence: ContractEvidence,
    ) {
        let mut claimed_names = BTreeMap::<String, (usize, String)>::new();
        let mut conflicts = Vec::new();
        for (index, parameter) in parameters.iter().enumerate() {
            for name in &parameter.accepted_names {
                if let Some((existing_index, existing_name)) = claimed_names.get(name) {
                    if *existing_index != index {
                        conflicts.push((
                            name.clone(),
                            existing_name.clone(),
                            parameter.canonical.clone(),
                            parameter.span,
                        ));
                    }
                } else {
                    claimed_names.insert(name.clone(), (index, parameter.canonical.clone()));
                }
            }
        }
        for (name, first, second, span) in conflicts {
            self.error(
                "OSR-N0014",
                format!("parameter name or alias `{name}` refers to both `{first}` and `{second}`"),
                span,
            );
        }
        self.callables.insert(
            id,
            CallableInfo {
                signature,
                parameters,
                generic_variables,
                contract_evidence,
            },
        );
    }

    fn resolve_aliases(&mut self, module: &ast::Module) {
        for item in &module.items {
            let AstItemKind::Alias(alias) = &item.kind else {
                continue;
            };
            // An alias target may be a qualified member of an imported
            // interface (for example `series/rolling-mean`).  Such members
            // intentionally do not enter `globals`: they retain the
            // provider's stable imported BindingId in `qualified_imports`.
            // Resolve that table here so a local alias is only a spelling
            // change, never a wrapper or a second binding.
            let Some(target) = self.resolve_alias_target(&alias.target.canonical) else {
                if self.phase_one_names.contains(&alias.target.canonical) {
                    self.phase_one_names.insert(alias.local.canonical.clone());
                    continue;
                }
                self.error(
                    "OSR-N0010",
                    format!("unknown alias target `{}`", alias.target.spelling),
                    alias.span,
                );
                continue;
            };
            let Some(binding) = self.bindings.get(&target).cloned() else {
                continue;
            };
            match self
                .allocator
                .alias(&alias.local.spelling, &binding.name, alias.span)
            {
                Ok(()) => {
                    self.globals
                        .insert(alias.local.canonical.clone(), target.clone());
                    self.aliases.push(Alias {
                        spelling: alias.local.spelling.clone(),
                        canonical: alias.local.canonical.clone(),
                        target,
                        span: alias.span,
                        public: false,
                    });
                }
                Err(diagnostic) => self.diagnostics.push(diagnostic),
            }
        }
    }

    fn resolve_nominal_types(&mut self, span: Span) {
        let resolutions = self.nominal_type_resolutions();
        let mut unknown = BTreeSet::new();
        for binding in self.bindings.values() {
            collect_unresolved_nominal_bindings(&binding.ty, &resolutions, &mut unknown);
        }
        for callable in self.callables.values() {
            collect_unresolved_nominal_bindings(
                &Type::Fn(callable.signature.clone()),
                &resolutions,
                &mut unknown,
            );
            for parameter in &callable.parameters {
                collect_unresolved_nominal_bindings(&parameter.ty, &resolutions, &mut unknown);
            }
        }
        for name in unknown {
            self.report_unknown_nominal_type(&name, span);
        }
        let module = self.module_name.as_str();
        for binding in self.bindings.values_mut() {
            binding.ty = resolve_nominal_bindings(&binding.ty, &resolutions, module);
        }
        for callable in self.callables.values_mut() {
            callable.signature =
                resolve_function_nominal_bindings(&callable.signature, &resolutions, module);
            for parameter in &mut callable.parameters {
                parameter.ty = resolve_nominal_bindings(&parameter.ty, &resolutions, module);
            }
        }
        for table in self.struct_fields.values_mut() {
            for field in table.fields.values_mut() {
                field.ty = resolve_nominal_bindings(&field.ty, &resolutions, module);
            }
        }
        for instance in self.operator_instances.values_mut() {
            instance.operands = instance
                .operands
                .iter()
                .map(|operand| resolve_nominal_bindings(operand, &resolutions, module))
                .collect();
            instance.result = resolve_nominal_bindings(&instance.result, &resolutions, module);
        }
    }

    fn nominal_type_resolutions(&self) -> BTreeMap<String, String> {
        let mut resolutions = BTreeMap::new();
        for (spelling, id) in self.globals.iter().chain(&self.qualified_imports) {
            if self
                .bindings
                .get(id)
                .is_some_and(|binding| binding.name.kind == BindingKind::Type)
            {
                resolutions.insert(spelling.clone(), id.as_str().to_owned());
            }
        }

        // Preserve the existing convenient unqualified spelling when exactly
        // one imported or local type has that canonical name. Ambiguous short
        // names deliberately remain unresolved so the caller diagnoses them
        // as unknown instead of collapsing two provider types together.
        let mut candidates = BTreeMap::<String, BTreeSet<String>>::new();
        for (id, binding) in &self.bindings {
            if binding.name.kind == BindingKind::Type {
                candidates
                    .entry(binding.name.canonical.clone())
                    .or_default()
                    .insert(id.as_str().to_owned());
            }
        }
        for (name, bindings) in candidates {
            if bindings.len() == 1 {
                resolutions
                    .entry(name)
                    .or_insert_with(|| bindings.into_iter().next().expect("one type binding"));
            }
        }

        // A closed set of Python exception classes is available to `catch`
        // without declaring a nominal Osiris type or importing a runtime
        // module.  Local/imported declarations win on spelling conflicts;
        // unknown nominal names remain rejected below.
        for name in python_builtin_exception_names() {
            if let Some(binding) = python_builtin_exception_binding(name) {
                resolutions
                    .entry((*name).to_owned())
                    .or_insert_with(|| binding.clone());
                resolutions
                    .entry(format!("builtins/{name}"))
                    .or_insert_with(|| binding.clone());
                resolutions
                    .entry(format!("builtins.{name}"))
                    .or_insert(binding);
            }
        }
        resolutions
    }

    fn resolve_type_expr(&mut self, expression: &ast::TypeExpr) -> Type {
        self.resolve_type_expr_with_generics(expression, &BTreeMap::new())
    }

    fn resolve_type_expr_with_generics(
        &mut self,
        expression: &ast::TypeExpr,
        generic_parameters: &BTreeMap<String, Type>,
    ) -> Type {
        let ty = type_from_ast_with_generics(expression, generic_parameters);
        let resolutions = self.nominal_type_resolutions();
        let mut unknown = BTreeSet::new();
        collect_unresolved_nominal_bindings(&ty, &resolutions, &mut unknown);
        for name in unknown {
            self.report_unknown_nominal_type(&name, expression.span);
        }
        resolve_nominal_bindings(&ty, &resolutions, &self.module_name)
    }

    fn report_unknown_nominal_type(&mut self, name: &str, span: Span) {
        if self.unknown_nominal_types.insert(name.to_owned()) {
            self.error("OSR-T0021", format!("unknown nominal type `{name}`"), span);
        }
    }

    fn resolve_exports(&mut self, module: &ast::Module) {
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

    fn validate_boundary_signatures(&mut self, module: &ast::Module) {
        for item in &module.items {
            match &item.kind {
                AstItemKind::Defn(function) => {
                    let Some(name) = function.name.as_ref() else {
                        continue;
                    };
                    let is_exported = self
                        .resolve_global_name(&name.canonical)
                        .is_some_and(|binding| self.exports.contains(&binding));
                    if is_exported {
                        self.validate_explicit_function_signature(function, "exported");
                    }
                }
                AstItemKind::Extern(external) => {
                    for declaration in &external.items {
                        if let AstItemKind::Defn(function) = &declaration.kind {
                            self.validate_explicit_function_signature(function, "extern");
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn validate_explicit_function_signature(&mut self, function: &ast::Function, boundary: &str) {
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

    fn lower_items(&mut self, module: &ast::Module) {
        let mut scope = Scope::default();
        for item in &module.items {
            let kind = match &item.kind {
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
                AstItemKind::Defn(function) => {
                    self.lower_function(function).map(ItemKind::Function)
                }
                AstItemKind::Defstruct(structure) => {
                    self.lower_struct(structure).map(ItemKind::Struct)
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

    fn lower_extern_functions(&mut self, external: &ast::Extern) {
        for declaration in &external.items {
            let AstItemKind::Defn(function) = &declaration.kind else {
                continue;
            };
            let Some(name) = function.name.as_ref() else {
                continue;
            };
            let Some(binding) = self.global_id(name) else {
                continue;
            };
            let signature = match self.binding_type(&binding) {
                Type::Fn(signature) => signature,
                _ => continue,
            };
            let mut scope = Scope::default();
            scope.push();
            let mut parameters = Vec::new();
            for (index, parameter) in function.params.iter().enumerate() {
                let ty = signature
                    .parameters
                    .get(index)
                    .cloned()
                    .unwrap_or(Type::Error);
                let default = parameter
                    .default
                    .as_ref()
                    .map(|default| self.lower_expr(default, &mut scope));
                if let Some(default) = &default {
                    self.check_assignable(&default.ty, &ty, default.span);
                    self.require_pure(default, "extern parameter default");
                }
                let local = self.declare_local(
                    &parameter.name,
                    BindingKind::Parameter,
                    ty.clone(),
                    parameter.metadata.clone(),
                    parameter.span,
                    &mut scope,
                );
                parameters.push(Parameter {
                    binding: local,
                    ty,
                    default,
                    variadic: parameter.variadic,
                });
            }
            for parameter in &mut parameters {
                parameter.ty = self.types.resolve(&parameter.ty);
            }
            let return_type = self.types.resolve(&signature.return_type);
            let summaries = function
                .contract
                .as_ref()
                .map_or_else(CallSummaries::unknown, |contract| {
                    contract.summaries.clone()
                });
            let contract_evidence = self
                .callables
                .get(&binding)
                .map_or_else(ContractEvidence::default, |callable| {
                    callable.contract_evidence.clone()
                });
            let final_signature = FunctionType::new(
                parameters
                    .iter()
                    .map(|parameter| parameter.ty.clone())
                    .collect(),
                return_type.clone(),
            )
            .with_summaries(summaries.clone());
            self.set_binding_type(&binding, Type::Fn(final_signature.clone()));
            if let Some(callable) = self.callables.get_mut(&binding) {
                callable.signature = final_signature;
                for (shape, parameter) in callable.parameters.iter_mut().zip(&parameters) {
                    shape.ty = parameter.ty.clone();
                }
            }
            self.extern_functions.push(ExternFunction {
                binding,
                parameters,
                return_type,
                contract_id: function
                    .contract
                    .as_ref()
                    .map(|contract| contract.id.clone()),
                summaries,
                contract_evidence,
            });
        }
    }

    fn lower_function(&mut self, function: &ast::Function) -> Option<Function> {
        let binding = self.global_id(function.name.as_ref()?)?;
        let signature = match self.binding_type(&binding) {
            Type::Fn(signature) => signature,
            _ => return None,
        };
        let mut scope = Scope::default();
        scope.push();
        self.contract_evidence_stack
            .push(ContractEvidence::default());
        let mut parameters = Vec::new();
        let mut parameter_bindings = Vec::new();
        for (index, parameter) in function.params.iter().enumerate() {
            let ty = signature
                .parameters
                .get(index)
                .cloned()
                .unwrap_or(Type::Error);
            let default = parameter
                .default
                .as_ref()
                .map(|default| self.lower_expr(default, &mut scope));
            if let Some(default) = &default {
                self.check_assignable(&default.ty, &ty, default.span);
            }
            let local = self.declare_local(
                &parameter.name,
                BindingKind::Parameter,
                ty.clone(),
                parameter.metadata.clone(),
                parameter.span,
                &mut scope,
            );
            parameters.push(Parameter {
                binding: local.clone(),
                ty,
                default,
                variadic: parameter.variadic,
            });
            if let Some(pattern) = &parameter.pattern {
                let value = Expr::pure(
                    parameter.span,
                    self.binding_type(&local),
                    ExprKind::Binding(local),
                );
                self.lower_pattern_bindings(
                    pattern,
                    value,
                    &pattern.metadata,
                    &mut scope,
                    &mut parameter_bindings,
                );
            }
        }
        let state_types = parameters
            .iter()
            .map(|parameter| parameter.ty.clone())
            .collect::<Vec<_>>();
        self.function_recur_contexts.push(FunctionRecurContext {
            depth: self.function_depth,
            state_types,
            used: false,
        });
        let body = self.lower_body(&function.body, &mut scope, function.span);
        let function_recur = self
            .function_recur_contexts
            .pop()
            .expect("function recur context");
        let body = self.wrap_let_bindings(parameter_bindings, body, function.span);
        let body = if function_recur.used {
            self.validate_recur_tail(&body, true);
            self.wrap_function_recur(&parameters, body, function.span)
        } else {
            body
        };
        let contract_evidence = self
            .contract_evidence_stack
            .pop()
            .expect("function contract evidence scope");
        for parameter in &mut parameters {
            parameter.ty = self.types.resolve(&parameter.ty);
        }
        let declared_return = self.types.resolve(&signature.return_type);
        let return_type = if function.return_type.is_some() {
            self.check_assignable(&body.ty, &declared_return, body.span);
            declared_return
        } else {
            body.ty.clone()
        };
        let summaries = parameters
            .iter()
            .fold(body.summaries.clone(), |summary, parameter| {
                parameter
                    .default
                    .as_ref()
                    .map_or(summary.clone(), |default| summary.join(&default.summaries))
            });
        let final_signature = FunctionType::new(
            parameters
                .iter()
                .map(|parameter| parameter.ty.clone())
                .collect(),
            return_type.clone(),
        )
        .with_summaries(summaries.clone());
        self.set_binding_type(&binding, Type::Fn(final_signature.clone()));
        if let Some(callable) = self.callables.get_mut(&binding) {
            callable.signature = final_signature;
            callable.contract_evidence = contract_evidence.clone();
            for (shape, parameter) in callable.parameters.iter_mut().zip(&parameters) {
                shape.ty = parameter.ty.clone();
            }
        }
        let causal = match causal_requirement(&function.metadata) {
            Ok(requirement) => requirement,
            Err(message) => {
                self.error("OSR-C0004", message, function.span);
                None
            }
        };
        if let Some(requirement) = &causal {
            self.validate_causal_function(
                function
                    .name
                    .as_ref()
                    .map_or("<anonymous>", |name| name.spelling.as_str()),
                &summaries,
                &contract_evidence,
                requirement,
                function.span,
            );
        }
        Some(Function {
            binding,
            parameters,
            return_type,
            body,
            summaries,
            contract_evidence,
            causal,
        })
    }

    fn lower_struct(&mut self, structure: &ast::Defstruct) -> Option<Struct> {
        let binding = self.global_id(&structure.name)?;
        let generic_parameters = self
            .struct_type_parameters
            .get(&binding)
            .cloned()
            .unwrap_or_default();
        let mut scope = Scope::default();
        scope.push();
        let mut fields = Vec::new();
        for field in &structure.fields {
            let mut ty = field
                .type_annotation
                .as_ref()
                .map_or(Type::Unknown, |expression| {
                    self.resolve_type_expr_with_generics(expression, &generic_parameters)
                });
            if field.type_annotation.is_none() {
                self.error(
                    "OSR-T0002",
                    format!("struct field `{}` requires a type", field.name.spelling),
                    field.span,
                );
            }
            let default = field
                .default
                .as_ref()
                .map(|value| self.lower_expr(value, &mut Scope::default()));
            if let Some(default) = &default {
                if !contains_type_variable(&ty) {
                    self.check_assignable(&default.ty, &ty, default.span);
                }
                self.require_pure(default, "struct field default");
            }
            ty = self.types.resolve(&ty);
            let field_binding = self.declare_local(
                &field.name,
                BindingKind::Field,
                ty.clone(),
                field.metadata.clone(),
                field.span,
                &mut scope,
            );
            fields.push(StructField {
                binding: field_binding,
                ty,
                default,
            });
        }
        let checks = structure
            .checks
            .iter()
            .map(|check| {
                let condition = self.lower_expr(&check.condition, &mut scope);
                self.check_assignable(&condition.ty, &Type::Bool, condition.span);
                self.require_pure(&condition, "struct check");
                let message = check.message.as_ref().map(|message| {
                    let message = self.lower_expr(message, &mut scope);
                    self.check_assignable(&message.ty, &Type::Str, message.span);
                    self.require_pure(&message, "struct check message");
                    message
                });
                StructCheck {
                    span: check.span,
                    condition,
                    message,
                }
            })
            .collect::<Vec<_>>();
        let mut constructor_summaries =
            fields
                .iter()
                .fold(CallSummaries::pure_scalar(), |summary, field| {
                    field
                        .default
                        .as_ref()
                        .map_or(summary.clone(), |default| summary.join(&default.summaries))
                });
        constructor_summaries = checks.iter().fold(constructor_summaries, |summary, check| {
            let summary = summary.join(&check.condition.summaries);
            check
                .message
                .as_ref()
                .map_or(summary.clone(), |message| summary.join(&message.summaries))
        });
        if !checks.is_empty() {
            constructor_summaries.effects = constructor_summaries
                .effects
                .union(&EffectRow::singleton(Effect::Throw));
        }
        let constructor_signature = FunctionType::new(
            fields.iter().map(|field| field.ty.clone()).collect(),
            Type::Nominal {
                binding: binding.as_str().to_owned(),
                args: structure
                    .type_params
                    .iter()
                    .filter_map(|name| generic_parameters.get(&name.canonical).cloned())
                    .collect(),
            },
        )
        .with_summaries(constructor_summaries);
        if let Some(callable) = self.callables.get_mut(&binding) {
            callable.signature = constructor_signature;
            for (shape, field) in callable.parameters.iter_mut().zip(&fields) {
                shape.ty = field.ty.clone();
            }
        }
        Some(Struct {
            binding,
            type_parameters: structure
                .type_params
                .iter()
                .map(|name| name.canonical.clone())
                .collect(),
            fields,
            checks,
            doc: structure.doc.clone(),
        })
    }

    fn lower_body(&mut self, body: &[ast::Expr], scope: &mut Scope, span: Span) -> Expr {
        let expressions = body
            .iter()
            .map(|expression| self.lower_expr(expression, scope))
            .collect::<Vec<_>>();
        match expressions.len() {
            0 => Expr::error(span),
            1 => expressions.into_iter().next().expect("one expression"),
            _ => {
                let ty = expressions
                    .last()
                    .map_or(Type::None, |expr| expr.ty.clone());
                let summaries = join_summaries(expressions.iter().map(|expr| &expr.summaries));
                Expr {
                    span,
                    ty,
                    summaries,
                    kind: ExprKind::Do(expressions),
                }
            }
        }
    }

    fn wrap_let_bindings(&self, bindings: Vec<LetBinding>, body: Expr, span: Span) -> Expr {
        if bindings.is_empty() {
            return body;
        }
        let summaries = bindings
            .iter()
            .fold(body.summaries.clone(), |summary, binding| {
                summary.join(&binding.value.summaries)
            });
        Expr {
            span,
            ty: body.ty.clone(),
            summaries,
            kind: ExprKind::Let {
                bindings,
                body: Box::new(body),
            },
        }
    }

    fn lower_pattern_bindings(
        &mut self,
        pattern: &ast::Pattern,
        value: Expr,
        metadata: &[MetadataEntry],
        scope: &mut Scope,
        lowered: &mut Vec<LetBinding>,
    ) {
        match &pattern.kind {
            PatternKind::Name(name) => {
                let summaries = value.summaries.clone();
                let id = self.declare_local(
                    name,
                    BindingKind::Value,
                    value.ty.clone(),
                    metadata.to_vec(),
                    pattern.span,
                    scope,
                );
                self.local_value_summaries.insert(id.clone(), summaries);
                lowered.push(LetBinding { binding: id, value });
            }
            PatternKind::Ignore => {
                let _ = self.bind_pattern_temporary(value, pattern.span, scope, lowered);
            }
            PatternKind::Vector(patterns) => {
                let root = self.stabilize_pattern_root(value, pattern.span, scope, lowered);
                let mut index = 0_usize;
                while index < patterns.len() {
                    if pattern_name(&patterns[index]).is_some_and(|name| name == "&") {
                        self.error(
                            "OSR-H0003",
                            "vector rest destructuring is not supported in v0",
                            patterns[index].span,
                        );
                        break;
                    }
                    if pattern_keyword(&patterns[index]).is_some_and(|name| name == "as") {
                        let Some(alias) = patterns.get(index + 1) else {
                            self.error(
                                "OSR-H0003",
                                "`:as` in vector destructuring requires a pattern",
                                patterns[index].span,
                            );
                            break;
                        };
                        self.lower_pattern_bindings(
                            alias,
                            root.clone(),
                            &alias.metadata,
                            scope,
                            lowered,
                        );
                        index += 2;
                        continue;
                    }
                    let element = Expr {
                        span: patterns[index].span,
                        ty: indexed_type(&root.ty),
                        summaries: root.summaries.clone(),
                        kind: ExprKind::Index {
                            value: Box::new(root.clone()),
                            index: Box::new(Expr::pure(
                                patterns[index].span,
                                Type::Int,
                                ExprKind::Integer(index.to_string()),
                            )),
                        },
                    };
                    self.lower_pattern_bindings(
                        &patterns[index],
                        element,
                        &patterns[index].metadata,
                        scope,
                        lowered,
                    );
                    index += 1;
                }
            }
            PatternKind::Map(entries) => {
                let root = self.stabilize_pattern_root(value, pattern.span, scope, lowered);
                self.lower_map_pattern(entries, &root, scope, lowered);
            }
            PatternKind::Literal(_) | PatternKind::Error(_) => {
                self.error(
                    "OSR-H0003",
                    "binding pattern must contain names, vector patterns, or map patterns",
                    pattern.span,
                );
                let _ = self.bind_pattern_temporary(value, pattern.span, scope, lowered);
            }
        }
    }

    /// A local binding is already stable for repeated pattern access. Reusing
    /// it keeps generated Python readable while arbitrary expressions still
    /// receive one temporary evaluation before destructuring.
    fn stabilize_pattern_root(
        &mut self,
        value: Expr,
        span: Span,
        scope: &mut Scope,
        lowered: &mut Vec<LetBinding>,
    ) -> Expr {
        if matches!(&value.kind, ExprKind::Binding(_)) {
            value
        } else {
            self.bind_pattern_temporary(value, span, scope, lowered)
        }
    }

    fn bind_pattern_temporary(
        &mut self,
        value: Expr,
        span: Span,
        scope: &mut Scope,
        lowered: &mut Vec<LetBinding>,
    ) -> Expr {
        let spelling = format!("\0destructure{}", self.next_scope);
        let name = Name {
            spelling: spelling.clone(),
            canonical: spelling,
        };
        let ty = value.ty.clone();
        let summaries = value.summaries.clone();
        let binding = self.declare_local(
            &name,
            BindingKind::Value,
            ty.clone(),
            Vec::new(),
            span,
            scope,
        );
        self.local_value_summaries
            .insert(binding.clone(), summaries.clone());
        lowered.push(LetBinding {
            binding: binding.clone(),
            value,
        });
        Expr {
            span,
            ty,
            summaries,
            kind: ExprKind::Binding(binding),
        }
    }

    fn lower_map_pattern(
        &mut self,
        entries: &[(ast::Pattern, ast::Pattern)],
        root: &Expr,
        scope: &mut Scope,
        lowered: &mut Vec<LetBinding>,
    ) {
        let mut defaults = BTreeMap::<String, &ast::Pattern>::new();
        for (option, value) in entries {
            if pattern_keyword(option) != Some("or") {
                continue;
            }
            let PatternKind::Map(values) = &value.kind else {
                self.error(
                    "OSR-H0003",
                    "`:or` in map destructuring must be a map",
                    value.span,
                );
                continue;
            };
            for (name, default) in values {
                let Some(name) = pattern_name(name) else {
                    self.error(
                        "OSR-H0003",
                        "`:or` keys must be destructured names",
                        name.span,
                    );
                    continue;
                };
                defaults.insert(name.to_owned(), default);
            }
        }

        for (key_pattern, value_pattern) in entries {
            match pattern_keyword(key_pattern) {
                Some("keys" | "strs" | "syms") => {
                    let PatternKind::Vector(names) = &value_pattern.kind else {
                        self.error(
                            "OSR-H0003",
                            "`:keys`, `:strs`, and `:syms` require a vector",
                            value_pattern.span,
                        );
                        continue;
                    };
                    for source in names {
                        let PatternKind::Name(source_name) = &source.kind else {
                            self.error(
                                "OSR-H0003",
                                "map shorthand entries must be names",
                                source.span,
                            );
                            continue;
                        };
                        let local_name = destructured_local_name(source_name);
                        let default = defaults
                            .get(&local_name.canonical)
                            .or_else(|| defaults.get(&source_name.canonical))
                            .copied();
                        let value = self.map_pattern_access(
                            root,
                            &source_name.canonical,
                            default,
                            source.span,
                            scope,
                        );
                        let target = ast::Pattern {
                            span: source.span,
                            metadata: source.metadata.clone(),
                            kind: PatternKind::Name(local_name),
                        };
                        self.lower_pattern_bindings(
                            &target,
                            value,
                            &target.metadata,
                            scope,
                            lowered,
                        );
                    }
                }
                Some("as") => self.lower_pattern_bindings(
                    value_pattern,
                    root.clone(),
                    &value_pattern.metadata,
                    scope,
                    lowered,
                ),
                Some("or") => {}
                Some(other) => {
                    let value = self.map_pattern_access(
                        root,
                        other,
                        pattern_binding_name(value_pattern)
                            .and_then(|name| defaults.get(name).copied()),
                        value_pattern.span,
                        scope,
                    );
                    self.lower_pattern_bindings(
                        value_pattern,
                        value,
                        &value_pattern.metadata,
                        scope,
                        lowered,
                    );
                }
                None => {
                    let Some(key) = pattern_static_key(value_pattern) else {
                        self.error(
                            "OSR-H0003",
                            "explicit map destructuring requires a static key",
                            value_pattern.span,
                        );
                        continue;
                    };
                    let value = self.map_pattern_access(
                        root,
                        &key,
                        pattern_binding_name(key_pattern)
                            .and_then(|name| defaults.get(name).copied()),
                        key_pattern.span,
                        scope,
                    );
                    self.lower_pattern_bindings(
                        key_pattern,
                        value,
                        &key_pattern.metadata,
                        scope,
                        lowered,
                    );
                }
            }
        }
    }

    fn map_pattern_access(
        &mut self,
        root: &Expr,
        key: &str,
        default: Option<&ast::Pattern>,
        span: Span,
        scope: &mut Scope,
    ) -> Expr {
        if let Some((attribute, ty)) = self.struct_field_type(&root.ty, key) {
            return Expr {
                span,
                ty,
                summaries: root.summaries.clone(),
                kind: ExprKind::Attribute {
                    value: Box::new(root.clone()),
                    attribute,
                },
            };
        }

        let key_expression = Expr::pure(span, Type::Str, ExprKind::String(key.to_owned()));
        let value_type = indexed_type(&root.ty);
        let Some(default) = default else {
            return Expr {
                span,
                ty: value_type,
                summaries: root.summaries.clone(),
                kind: ExprKind::Index {
                    value: Box::new(root.clone()),
                    index: Box::new(key_expression),
                },
            };
        };
        let default = self.lower_pattern_default(default, scope);
        let result_type = self.types.join(&value_type, &default.ty);
        let summaries = root.summaries.join(&default.summaries);
        let callee = Expr::pure(
            span,
            Type::Fn(FunctionType::new(
                vec![Type::Str, default.ty.clone()],
                result_type.clone(),
            )),
            ExprKind::Attribute {
                value: Box::new(root.clone()),
                attribute: "get".to_owned(),
            },
        );
        Expr {
            span,
            ty: result_type,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![
                    CallArgument::Positional(key_expression),
                    CallArgument::Positional(default),
                ],
            },
        }
    }

    fn lower_pattern_default(&mut self, pattern: &ast::Pattern, scope: &mut Scope) -> Expr {
        match &pattern.kind {
            PatternKind::Name(name) => self.lower_name(name, pattern.span, scope),
            PatternKind::Vector(values) => {
                let values = values
                    .iter()
                    .map(|value| self.lower_pattern_default(value, scope))
                    .collect::<Vec<_>>();
                let ty = self.types.join_all(values.iter().map(|value| &value.ty));
                let summaries = join_summaries(values.iter().map(|value| &value.summaries));
                Expr {
                    span: pattern.span,
                    ty: Type::Vector(Box::new(ty)),
                    summaries,
                    kind: ExprKind::Vector(values),
                }
            }
            PatternKind::Literal(form) => match &form.kind {
                FormKind::None => Expr::pure(form.span, Type::None, ExprKind::None),
                FormKind::Bool(value) => Expr::pure(form.span, Type::Bool, ExprKind::Bool(*value)),
                FormKind::Integer(value) => {
                    Expr::pure(form.span, Type::Int, ExprKind::Integer(value.clone()))
                }
                FormKind::Float(value) => {
                    Expr::pure(form.span, Type::Float, ExprKind::Float(value.clone()))
                }
                FormKind::String(value) => {
                    Expr::pure(form.span, Type::Str, ExprKind::String(value.clone()))
                }
                FormKind::Keyword(name) => Expr::pure(
                    form.span,
                    Type::Str,
                    ExprKind::String(name.canonical.trim_start_matches(':').to_owned()),
                ),
                _ => {
                    self.error(
                        "OSR-H0003",
                        "runtime destructuring defaults must be names or literal data in v0",
                        form.span,
                    );
                    Expr::error(form.span)
                }
            },
            _ => {
                self.error(
                    "OSR-H0003",
                    "invalid runtime destructuring default",
                    pattern.span,
                );
                Expr::error(pattern.span)
            }
        }
    }

    fn lower_expr(&mut self, expression: &ast::Expr, scope: &mut Scope) -> Expr {
        match &expression.kind {
            AstExprKind::None => Expr::pure(expression.span, Type::None, ExprKind::None),
            AstExprKind::Bool(value) => {
                Expr::pure(expression.span, Type::Bool, ExprKind::Bool(*value))
            }
            AstExprKind::Integer(value) => {
                Expr::pure(expression.span, Type::Int, ExprKind::Integer(value.clone()))
            }
            AstExprKind::Float(value) => {
                Expr::pure(expression.span, Type::Float, ExprKind::Float(value.clone()))
            }
            AstExprKind::String(value) => {
                Expr::pure(expression.span, Type::Str, ExprKind::String(value.clone()))
            }
            AstExprKind::Keyword(name) => Expr::pure(
                expression.span,
                Type::Str,
                ExprKind::String(name.canonical.trim_start_matches(':').to_owned()),
            ),
            AstExprKind::Name(name) => self.lower_name(name, expression.span, scope),
            AstExprKind::List(items) => self.lower_sequence(items, expression.span, scope, true),
            AstExprKind::Vector(items) => self.lower_sequence(items, expression.span, scope, false),
            AstExprKind::Map(entries) => {
                let entries = entries
                    .iter()
                    .map(|(key, value)| {
                        (self.lower_expr(key, scope), self.lower_expr(value, scope))
                    })
                    .collect::<Vec<_>>();
                let key_type = self.types.join_all(entries.iter().map(|(key, _)| &key.ty));
                let value_type = self
                    .types
                    .join_all(entries.iter().map(|(_, value)| &value.ty));
                let summaries = join_summaries(
                    entries
                        .iter()
                        .flat_map(|(key, value)| [&key.summaries, &value.summaries]),
                );
                Expr {
                    span: expression.span,
                    ty: Type::Map(Box::new(key_type), Box::new(value_type)),
                    summaries,
                    kind: ExprKind::Map(entries),
                }
            }
            AstExprKind::Set(items) => {
                let items = items
                    .iter()
                    .map(|item| self.lower_expr(item, scope))
                    .collect::<Vec<_>>();
                let ty = self.types.join_all(items.iter().map(|item| &item.ty));
                let summaries = join_summaries(items.iter().map(|item| &item.summaries));
                Expr {
                    span: expression.span,
                    ty: Type::Set(Box::new(ty)),
                    summaries,
                    kind: ExprKind::Set(items),
                }
            }
            AstExprKind::Call(call) => self.lower_call(call, expression.span, scope),
            AstExprKind::Let { bindings, body } => {
                scope.push();
                let mut lowered = Vec::new();
                for binding in bindings {
                    let mut value = self.lower_expr(&binding.value, scope);
                    if let Some(annotation) = &binding.type_annotation {
                        let annotated = self.resolve_type_expr(annotation);
                        self.check_assignable(&value.ty, &annotated, binding.span);
                        value.ty = annotated;
                    }
                    self.lower_pattern_bindings(
                        &binding.pattern,
                        value,
                        &binding.metadata,
                        scope,
                        &mut lowered,
                    );
                }
                let body = self.lower_body(body, scope, expression.span);
                scope.pop();
                let summaries = lowered
                    .iter()
                    .fold(body.summaries.clone(), |summary, binding| {
                        summary.join(&binding.value.summaries)
                    });
                Expr {
                    span: expression.span,
                    ty: body.ty.clone(),
                    summaries,
                    kind: ExprKind::Let {
                        bindings: lowered,
                        body: Box::new(body),
                    },
                }
            }
            AstExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let condition = self.lower_expr(condition, scope);
                self.check_assignable(&condition.ty, &Type::Bool, condition.span);
                let then_branch = self.lower_expr(then_branch, scope);
                let else_branch = else_branch.as_ref().map_or_else(
                    || Expr::pure(expression.span, Type::None, ExprKind::None),
                    |branch| self.lower_expr(branch, scope),
                );
                let ty = self.types.join(&then_branch.ty, &else_branch.ty);
                let summaries = condition
                    .summaries
                    .join(&then_branch.summaries)
                    .join(&else_branch.summaries);
                Expr {
                    span: expression.span,
                    ty,
                    summaries,
                    kind: ExprKind::If {
                        condition: Box::new(condition),
                        then_branch: Box::new(then_branch),
                        else_branch: Box::new(else_branch),
                    },
                }
            }
            AstExprKind::Do(body) => self.lower_body(body, scope, expression.span),
            AstExprKind::Fn(function) => self.lower_lambda(function, expression.span, scope),
            AstExprKind::Raise(value) => {
                let value = value
                    .as_ref()
                    .map(|value| Box::new(self.lower_expr(value, scope)));
                if let Some(value) = &value
                    && !self.is_raiseable_type(&value.ty)
                {
                    self.error(
                        "OSR-T0033",
                        format!("raise expects an exception value, found `{}`", value.ty),
                        value.span,
                    );
                }
                let summaries = value
                    .as_ref()
                    .map_or_else(CallSummaries::pure_scalar, |value| value.summaries.clone());
                let summaries = CallSummaries {
                    effects: summaries
                        .effects
                        .union(&EffectRow::singleton(Effect::Throw)),
                    ..summaries
                };
                Expr {
                    span: expression.span,
                    ty: Type::Never,
                    summaries,
                    kind: ExprKind::Raise(value),
                }
            }
            AstExprKind::Try(try_expression) => {
                self.lower_try(try_expression, expression.span, scope)
            }
            AstExprKind::Quote(_)
            | AstExprKind::SyntaxQuote(_)
            | AstExprKind::Unquote(_)
            | AstExprKind::UnquoteSplicing(_) => {
                self.error(
                    "OSR-H0004",
                    "quoted syntax is only valid during macro expansion",
                    expression.span,
                );
                Expr::error(expression.span)
            }
            AstExprKind::Error(_) => Expr::error(expression.span),
        }
    }

    fn is_raiseable_type(&self, ty: &Type) -> bool {
        match self.types.resolve(ty) {
            Type::Any | Type::Unknown => true,
            Type::Nominal { binding, .. } => {
                python_builtin_exception_from_binding(&binding).is_some()
            }
            Type::Union(members) => members.iter().all(|member| self.is_raiseable_type(member)),
            Type::Never => true,
            _ => false,
        }
    }

    fn lower_sequence(
        &mut self,
        items: &[ast::Expr],
        span: Span,
        scope: &mut Scope,
        list: bool,
    ) -> Expr {
        let items = items
            .iter()
            .map(|item| self.lower_expr(item, scope))
            .collect::<Vec<_>>();
        let item_type = self.types.join_all(items.iter().map(|item| &item.ty));
        let summaries = join_summaries(items.iter().map(|item| &item.summaries));
        Expr {
            span,
            ty: if list {
                Type::List(Box::new(item_type))
            } else {
                Type::Vector(Box::new(item_type))
            },
            summaries,
            kind: if list {
                ExprKind::List(items)
            } else {
                ExprKind::Vector(items)
            },
        }
    }

    fn lower_name(&mut self, name: &Name, span: Span, scope: &Scope) -> Expr {
        if let Some(id) = scope.resolve(&name.canonical) {
            return Expr {
                span,
                ty: self.binding_type(id),
                summaries: self
                    .local_value_summaries
                    .get(id)
                    .cloned()
                    .unwrap_or_else(CallSummaries::pure_scalar),
                kind: ExprKind::Binding(id.clone()),
            };
        }
        if let Some(id) = self.resolve_global_name(&name.canonical) {
            return self.lower_global_binding_read(id, span);
        }
        if let Some(id) = self.qualified_imports.get(&name.canonical).cloned() {
            return self.lower_global_binding_read(id, span);
        }
        if name.canonical == "osiris.prelude/mapv" {
            let binding = self.ensure_core_mapv_binding(span);
            return Expr::pure(
                span,
                self.binding_type(&binding),
                ExprKind::Binding(binding),
            );
        }

        if let Some((base, members)) = split_access_name(&name.canonical) {
            let mut value = if let Some(id) = scope
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
                        return Expr::error(span);
                    }
                    // An attribute that is not covered by a declared struct
                    // layout is a dynamic Python boundary.  Keep the base
                    // expression's provenance, but do not let an `Any`
                    // result masquerade as pure or temporally known: Python
                    // descriptors/properties may execute arbitrary code and
                    // expose data whose contract the compiler cannot inspect.
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
            return value;
        }

        self.error(
            "OSR-N0012",
            format!("unknown name `{}`", name.spelling),
            span,
        );
        Expr::error(span)
    }

    fn lower_global_binding_read(&mut self, binding: BindingId, span: Span) -> Expr {
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

    fn struct_field_type(&self, value_type: &Type, member: &str) -> Option<(String, Type)> {
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

    fn lower_call(&mut self, call: &ast::CallExpr, span: Span, scope: &mut Scope) -> Expr {
        if let Some(name) = call.callee.name().map(|name| name.canonical.as_str()) {
            match name {
                "cons" | "osiris.prelude/cons" => {
                    return self.lower_sequence_call(call, span, scope, SequenceOperation::Cons);
                }
                "concat" | "osiris.prelude/concat" => {
                    return self.lower_sequence_call(call, span, scope, SequenceOperation::Concat);
                }
                "count" | "osiris.prelude/count" => {
                    return self.lower_sequence_call(call, span, scope, SequenceOperation::Count);
                }
                "empty?" | "osiris.prelude/empty?" => {
                    return self.lower_sequence_call(call, span, scope, SequenceOperation::EmptyQ);
                }
                "seq?" | "osiris.prelude/seq?" => {
                    return self.lower_sequence_call(call, span, scope, SequenceOperation::SeqQ);
                }
                "coll?" | "osiris.prelude/coll?" => {
                    return self.lower_sequence_call(call, span, scope, SequenceOperation::CollQ);
                }
                "sequential?" | "osiris.prelude/sequential?" => {
                    return self.lower_sequence_call(
                        call,
                        span,
                        scope,
                        SequenceOperation::SequentialQ,
                    );
                }
                "first" | "osiris.prelude/first" => {
                    return self.lower_sequence_call(call, span, scope, SequenceOperation::First);
                }
                "rest" | "osiris.prelude/rest" => {
                    return self.lower_sequence_call(call, span, scope, SequenceOperation::Rest);
                }
                "next" | "osiris.prelude/next" => {
                    return self.lower_sequence_call(call, span, scope, SequenceOperation::Next);
                }
                "nth" | "osiris.prelude/nth" => {
                    return self.lower_sequence_call(call, span, scope, SequenceOperation::Nth);
                }
                "seq" | "osiris.prelude/seq" => {
                    return self.lower_sequence_call(call, span, scope, SequenceOperation::Seq);
                }
                "empty" | "osiris.prelude/empty" => {
                    return self.lower_sequence_call(call, span, scope, SequenceOperation::Empty);
                }
                "take" | "osiris.prelude/take" => {
                    return self.lower_sequence_call(call, span, scope, SequenceOperation::Take);
                }
                "drop" | "osiris.prelude/drop" => {
                    return self.lower_sequence_call(call, span, scope, SequenceOperation::Drop);
                }
                "take-while" | "osiris.prelude/take-while" => {
                    return self.lower_sequence_call(
                        call,
                        span,
                        scope,
                        SequenceOperation::TakeWhile,
                    );
                }
                "drop-while" | "osiris.prelude/drop-while" => {
                    return self.lower_sequence_call(
                        call,
                        span,
                        scope,
                        SequenceOperation::DropWhile,
                    );
                }
                "keep" | "osiris.prelude/keep" => {
                    return self.lower_sequence_call(call, span, scope, SequenceOperation::Keep);
                }
                "keep-indexed" | "osiris.prelude/keep-indexed" => {
                    return self.lower_sequence_call(
                        call,
                        span,
                        scope,
                        SequenceOperation::KeepIndexed,
                    );
                }
                "remove" | "osiris.prelude/remove" => {
                    return self.lower_sequence_call(call, span, scope, SequenceOperation::Remove);
                }
                "removev" | "osiris.prelude/removev" => {
                    return self.lower_sequence_call(call, span, scope, SequenceOperation::Removev);
                }
                "distinct" | "osiris.prelude/distinct" => {
                    return self.lower_sequence_call(
                        call,
                        span,
                        scope,
                        SequenceOperation::Distinct,
                    );
                }
                "dedupe" | "osiris.prelude/dedupe" => {
                    return self.lower_sequence_call(call, span, scope, SequenceOperation::Dedupe);
                }
                "partition" | "osiris.prelude/partition" => {
                    return self.lower_sequence_call(
                        call,
                        span,
                        scope,
                        SequenceOperation::Partition,
                    );
                }
                "partition-all" | "osiris.prelude/partition-all" => {
                    return self.lower_sequence_call(
                        call,
                        span,
                        scope,
                        SequenceOperation::PartitionAll,
                    );
                }
                "partition-by" | "osiris.prelude/partition-by" => {
                    return self.lower_sequence_call(
                        call,
                        span,
                        scope,
                        SequenceOperation::PartitionBy,
                    );
                }
                "interleave" | "osiris.prelude/interleave" => {
                    return self.lower_sequence_call(
                        call,
                        span,
                        scope,
                        SequenceOperation::Interleave,
                    );
                }
                "interpose" | "osiris.prelude/interpose" => {
                    return self.lower_sequence_call(
                        call,
                        span,
                        scope,
                        SequenceOperation::Interpose,
                    );
                }
                "take-last" | "osiris.prelude/take-last" => {
                    return self.lower_sequence_call(
                        call,
                        span,
                        scope,
                        SequenceOperation::TakeLast,
                    );
                }
                "drop-last" | "osiris.prelude/drop-last" => {
                    return self.lower_sequence_call(
                        call,
                        span,
                        scope,
                        SequenceOperation::DropLast,
                    );
                }
                "map-indexed" | "osiris.prelude/map-indexed" => {
                    return self.lower_sequence_call(
                        call,
                        span,
                        scope,
                        SequenceOperation::MapIndexed,
                    );
                }
                "iterate" | "osiris.prelude/iterate" => {
                    return self.lower_sequence_call(call, span, scope, SequenceOperation::Iterate);
                }
                "repeat" | "osiris.prelude/repeat" => {
                    return self.lower_sequence_call(call, span, scope, SequenceOperation::Repeat);
                }
                "repeatedly" | "osiris.prelude/repeatedly" => {
                    return self.lower_sequence_call(
                        call,
                        span,
                        scope,
                        SequenceOperation::Repeatedly,
                    );
                }
                "cycle" | "osiris.prelude/cycle" => {
                    return self.lower_sequence_call(call, span, scope, SequenceOperation::Cycle);
                }
                "sequence" | "osiris.prelude/sequence" => {
                    return self.lower_sequence_call(
                        call,
                        span,
                        scope,
                        SequenceOperation::Sequence,
                    );
                }
                "reductions" | "osiris.prelude/reductions" => {
                    return self.lower_sequence_call(
                        call,
                        span,
                        scope,
                        SequenceOperation::Reductions,
                    );
                }
                "run!" | "osiris.prelude/run!" => {
                    return self.lower_sequence_call(call, span, scope, SequenceOperation::RunBang);
                }
                "doall" | "osiris.prelude/doall" => {
                    return self.lower_sequence_call(call, span, scope, SequenceOperation::Doall);
                }
                "dorun" | "osiris.prelude/dorun" => {
                    return self.lower_sequence_call(call, span, scope, SequenceOperation::Dorun);
                }
                "some" | "osiris.prelude/some" => {
                    return self.lower_sequence_call(call, span, scope, SequenceOperation::Some);
                }
                "every?" | "osiris.prelude/every?" => {
                    return self.lower_sequence_call(call, span, scope, SequenceOperation::Every);
                }
                "not-every?" | "osiris.prelude/not-every?" => {
                    return self.lower_sequence_call(
                        call,
                        span,
                        scope,
                        SequenceOperation::NotEvery,
                    );
                }
                "not-any?" | "osiris.prelude/not-any?" => {
                    return self.lower_sequence_call(call, span, scope, SequenceOperation::NotAny);
                }
                "mapv" | "osiris.prelude/mapv" => return self.lower_mapv(call, span, scope),
                "map" | "osiris.prelude/map" => {
                    return self.lower_map_like(call, span, scope, CollectionOperation::Map);
                }
                "mapcat" | "osiris.prelude/mapcat" => {
                    return self.lower_map_like(call, span, scope, CollectionOperation::Mapcat);
                }
                "mapcatv" | "osiris.prelude/mapcatv" => {
                    return self.lower_map_like(call, span, scope, CollectionOperation::Mapcatv);
                }
                "filter" | "osiris.prelude/filter" => {
                    return self.lower_map_like(call, span, scope, CollectionOperation::Filter);
                }
                "filterv" | "osiris.prelude/filterv" => {
                    return self.lower_map_like(call, span, scope, CollectionOperation::Filterv);
                }
                "reduce" | "osiris.prelude/reduce" => {
                    return self.lower_reduce(call, span, scope, false);
                }
                "fold" | "osiris.prelude/fold" => {
                    return self.lower_reduce(call, span, scope, true);
                }
                "reduced" | "osiris.prelude/reduced" => {
                    return self.lower_reduced_operation(call, span, scope, ReducedOperation::Wrap);
                }
                "reduced?" | "osiris.prelude/reduced?" => {
                    return self.lower_reduced_operation(
                        call,
                        span,
                        scope,
                        ReducedOperation::Predicate,
                    );
                }
                "unreduced" | "osiris.prelude/unreduced" => {
                    return self.lower_reduced_operation(
                        call,
                        span,
                        scope,
                        ReducedOperation::Unwrap,
                    );
                }
                "osiris.prelude/assert*" => {
                    return self.lower_assert(call, span, scope);
                }
                "osiris.prelude/truthy*" => {
                    return self.lower_control_intrinsic(
                        call,
                        span,
                        scope,
                        ControlIntrinsic::Truthy,
                    );
                }
                "osiris.prelude/nil*" => {
                    return self.lower_control_intrinsic(call, span, scope, ControlIntrinsic::Nil);
                }
                "osiris.prelude/present*" => {
                    return self.lower_control_intrinsic(
                        call,
                        span,
                        scope,
                        ControlIntrinsic::Present,
                    );
                }
                "osiris.prelude/nonempty*" => {
                    return self.lower_control_intrinsic(
                        call,
                        span,
                        scope,
                        ControlIntrinsic::Nonempty,
                    );
                }
                "osiris.prelude/doseq*" => {
                    return self.lower_doseq(call, span, scope);
                }
                "osiris.prelude/for-stop*" => {
                    return self.lower_for_stop(call, span, scope);
                }
                "osiris.prelude/loop*" => {
                    return self.lower_loop(call, span, scope);
                }
                "osiris.prelude/recur*" => {
                    return self.lower_recur(call, span, scope);
                }
                "osiris.prelude/trampoline*" => {
                    return self.lower_trampoline(call, span, scope);
                }
                "osiris.prelude/lazy-seq*" => {
                    return self.lower_lazy_seq(call, span, scope);
                }
                "delay" | "osiris.prelude/delay*" => {
                    return self.lower_delay(call, span, scope);
                }
                "force" | "osiris.prelude/force*" => {
                    return self.lower_force(call, span, scope);
                }
                "deref" | "osiris.prelude/deref*" => {
                    return self.lower_deref(call, span, scope);
                }
                "realized?" | "osiris.prelude/realized*" => {
                    return self.lower_realized(call, span, scope);
                }
                "osiris.prelude/future-call*" => {
                    return self.lower_future_call(call, span, scope);
                }
                "osiris.prelude/future-done*" => {
                    return self.lower_future_predicate(call, span, scope, "future_done");
                }
                "osiris.prelude/future-cancelled*" => {
                    return self.lower_future_predicate(call, span, scope, "future_cancelled");
                }
                "osiris.prelude/future-cancel*" => {
                    return self.lower_future_predicate(call, span, scope, "future_cancel");
                }
                "osiris.prelude/promise*" => {
                    return self.lower_promise(call, span, scope);
                }
                "osiris.prelude/deliver*" => {
                    return self.lower_deliver(call, span, scope);
                }
                "osiris.prelude/lock*" => {
                    return self.lower_lock(call, span, scope);
                }
                "osiris.prelude/locking*" => {
                    return self.lower_locking(call, span, scope);
                }
                "osiris.prelude/time*" => {
                    return self.lower_time(call, span, scope);
                }
                "osiris.prelude/binding*" => {
                    return self.lower_dynamic_binding(call, span, scope);
                }
                "osiris.prelude/close*" => {
                    return self.lower_close(call, span, scope);
                }
                "osiris.prelude/letfn*" => {
                    return self.lower_letfn(call, span, scope);
                }
                _ => {}
            }
            if name == "abs" {
                if !call.keywords.is_empty() || call.positional.len() != 1 {
                    for argument in &call.keywords {
                        let _ = self.lower_expr(&argument.value, scope);
                    }
                    self.error("OSR-T0006", "abs expects one positional operand", span);
                    return Expr::error(span);
                }
                return self.lower_abs(&call.positional[0], span, scope);
            }
            if let Some(mut operator) = operator_from_name(name) {
                if !call.keywords.is_empty() {
                    for argument in &call.keywords {
                        let _ = self.lower_expr(&argument.value, scope);
                    }
                    self.error(
                        "OSR-T0008",
                        "operators do not accept keyword arguments",
                        span,
                    );
                    return Expr::error(span);
                }
                operator = match (operator, call.positional.len()) {
                    (Operator::Subtract, 1) => Operator::Negate,
                    (Operator::Add, 1) => Operator::Positive,
                    (operator, _) => operator,
                };
                return self.lower_operator(operator, &call.positional, span, scope);
            }
            if name == "get" || name == "index" {
                if call.positional.len() != 2 || !call.keywords.is_empty() {
                    self.error("OSR-T0003", "index expects two positional arguments", span);
                    return Expr::error(span);
                }
                let value = self.lower_expr(&call.positional[0], scope);
                let index = self.lower_expr(&call.positional[1], scope);
                let mut summaries = value.summaries.join(&index.summaries);
                // Static collection/index facts are precise above.  Once the
                // value crosses an explicit `Any` boundary, indexing is a
                // dynamic Python operation (custom ``__getitem__`` may run
                // arbitrary code and return data with no declared contract).
                if matches!(value.ty, Type::Any | Type::Unknown) {
                    summaries = summaries.join(&CallSummaries::unknown());
                }
                return Expr {
                    span,
                    ty: indexed_type(&value.ty),
                    summaries,
                    kind: ExprKind::Index {
                        value: Box::new(value),
                        index: Box::new(index),
                    },
                };
            }
        }

        let callee = self.lower_expr(&call.callee, scope);
        let callable = self.callable_for_expr(&callee);
        let mut arguments = Vec::new();
        let mut summaries = callee.summaries.clone();
        for argument in &call.args {
            match argument {
                AstCallArg::Positional(argument) => {
                    let argument = self.lower_expr(argument, scope);
                    summaries = summaries.join(&argument.summaries);
                    arguments.push(CallArgument::Positional(argument));
                }
                AstCallArg::Keyword(argument) => {
                    let value = self.lower_expr(&argument.value, scope);
                    summaries = summaries.join(&value.summaries);
                    let source_name = argument.key.canonical.trim_start_matches(':');
                    let emitted_name = callable
                        .as_ref()
                        .and_then(|info| {
                            info.parameters
                                .iter()
                                .find(|parameter| parameter.accepted_names.contains(source_name))
                        })
                        .map_or_else(
                            || python_identifier(source_name),
                            |parameter| python_identifier(&parameter.canonical),
                        );
                    arguments.push(CallArgument::Keyword {
                        name: emitted_name,
                        value,
                    });
                }
            }
        }

        if let Some(info) = &callable {
            self.record_contract_evidence(&info.contract_evidence);
        }

        // Until an interface supplies a more precise higher-order transfer,
        // conservatively assume every function-valued argument may be
        // invoked. This keeps callback effects and future reads visible to
        // the enclosing causal analysis instead of treating function values
        // as inert data.
        let callback_summaries = arguments
            .iter()
            .filter_map(|argument| match argument {
                CallArgument::Positional(value) | CallArgument::Keyword { value, .. } => {
                    match &value.ty {
                        Type::Fn(function) => Some(function.summaries.clone()),
                        _ => None,
                    }
                }
            })
            .collect::<Vec<_>>();

        let (ty, latent) = match callable {
            Some(info) => {
                let info = self.instantiate_callable(&info);
                self.validate_call(&info, call, &arguments, span);
                let mut summaries = info.signature.summaries.clone();
                summaries.temporal =
                    self.specialize_temporal_summary(&summaries.temporal, &info, call, &arguments);
                ((*info.signature.return_type).clone(), summaries)
            }
            None => match &callee.ty {
                Type::Fn(function) => {
                    let positional_count = call.positional.len();
                    if function.parameters.len() != call.args.len() {
                        self.error(
                            "OSR-T0004",
                            format!(
                                "call expects {} arguments, found {}",
                                function.parameters.len(),
                                call.args.len()
                            ),
                            span,
                        );
                    }
                    for (actual, expected) in arguments
                        .iter()
                        .filter_map(|argument| match argument {
                            CallArgument::Positional(value) => Some(&value.ty),
                            CallArgument::Keyword { .. } => None,
                        })
                        .zip(
                            &function.parameters[..positional_count.min(function.parameters.len())],
                        )
                    {
                        self.check_assignable(actual, expected, span);
                    }
                    ((*function.return_type).clone(), function.summaries.clone())
                }
                Type::Any => (Type::Any, CallSummaries::unknown()),
                Type::Error | Type::Unknown | Type::TypeVar(_) => {
                    (Type::Error, CallSummaries::unknown())
                }
                other => {
                    self.error(
                        "OSR-T0005",
                        format!("value of type `{other}` is not callable"),
                        span,
                    );
                    (Type::Error, CallSummaries::unknown())
                }
            },
        };
        let ty = self.types.resolve(&ty);
        let temporal = summaries.temporal.compose(&latent.temporal);
        summaries = summaries.join(&latent);
        summaries.temporal = temporal;
        for callback in callback_summaries {
            summaries = summaries.join(&callback);
        }
        Expr {
            span,
            ty,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments,
            },
        }
    }

    fn specialize_temporal_summary(
        &self,
        summary: &crate::types::TemporalSummary,
        callable: &CallableInfo,
        source_call: &ast::CallExpr,
        arguments: &[CallArgument],
    ) -> crate::types::TemporalSummary {
        let mut substitutions = BTreeMap::new();
        let mut positional_index = 0_usize;
        for (source, lowered) in source_call.args.iter().zip(arguments) {
            let parameter = match (source, lowered) {
                (AstCallArg::Positional(_), CallArgument::Positional(_)) => {
                    let parameter = callable.parameters.get(positional_index).or_else(|| {
                        callable
                            .parameters
                            .last()
                            .filter(|parameter| parameter.variadic)
                    });
                    if parameter.is_some_and(|parameter| !parameter.variadic) {
                        positional_index += 1;
                    }
                    parameter
                }
                (AstCallArg::Keyword(keyword), CallArgument::Keyword { .. }) => {
                    let source_name = keyword.key.canonical.trim_start_matches(':');
                    callable
                        .parameters
                        .iter()
                        .find(|parameter| parameter.accepted_names.contains(source_name))
                }
                _ => None,
            };
            let value = match lowered {
                CallArgument::Positional(value) | CallArgument::Keyword { value, .. } => value,
            };
            if let (Some(parameter), Some(value)) =
                (parameter, self.symbolic_argument_expression(value))
            {
                substitutions.insert(parameter.canonical.clone(), value);
            }
        }
        summary.substitute(&substitutions)
    }

    fn lower_sequence_callback(
        &mut self,
        expression: &ast::Expr,
        _span: Span,
        scope: &mut Scope,
        expected: &[Type],
    ) -> Expr {
        let value = match &expression.kind {
            AstExprKind::Fn(function) => self.lower_lambda_with_expected_parameters(
                function,
                expression.span,
                scope,
                expected,
            ),
            _ => self.lower_expr(expression, scope),
        };
        if let Type::Fn(signature) = &value.ty {
            if signature.parameters.len() != expected.len() {
                self.error(
                    "OSR-T0041",
                    format!(
                        "sequence callback expects {} argument(s), found {}",
                        expected.len(),
                        signature.parameters.len()
                    ),
                    value.span,
                );
            }
            for (actual, parameter) in expected.iter().zip(&signature.parameters) {
                self.check_assignable(actual, parameter, value.span);
            }
        } else if !matches!(value.ty, Type::Any | Type::Unknown | Type::Error) {
            self.error(
                "OSR-T0041",
                "sequence callback must be a function",
                value.span,
            );
        }
        value
    }

    /// Lower the small eager/lazy sequence combinator ABI exposed by the
    /// prelude.  The runtime represents lazy results with the memoized
    /// `_LazySeq`, while typed HIR deliberately presents them as `List[T]` so
    /// callers can compose them without learning a second nominal protocol.
    fn lower_sequence_call(
        &mut self,
        call: &ast::CallExpr,
        span: Span,
        scope: &mut Scope,
        operation: SequenceOperation,
    ) -> Expr {
        let arity_ok = match operation {
            SequenceOperation::Concat => true,
            SequenceOperation::Interleave => call.positional.len() >= 2,
            SequenceOperation::Partition => (2..=4).contains(&call.positional.len()),
            SequenceOperation::PartitionAll => (2..=3).contains(&call.positional.len()),
            SequenceOperation::Repeat | SequenceOperation::Repeatedly => {
                (1..=2).contains(&call.positional.len())
            }
            SequenceOperation::Nth | SequenceOperation::Reductions => {
                (2..=3).contains(&call.positional.len())
            }
            SequenceOperation::Cons
            | SequenceOperation::Take
            | SequenceOperation::Drop
            | SequenceOperation::TakeWhile
            | SequenceOperation::DropWhile
            | SequenceOperation::Keep
            | SequenceOperation::KeepIndexed
            | SequenceOperation::Remove
            | SequenceOperation::Removev
            | SequenceOperation::PartitionBy
            | SequenceOperation::Interpose
            | SequenceOperation::TakeLast
            | SequenceOperation::MapIndexed
            | SequenceOperation::Iterate
            | SequenceOperation::RunBang
            | SequenceOperation::Some
            | SequenceOperation::Every
            | SequenceOperation::NotEvery
            | SequenceOperation::NotAny => call.positional.len() == 2,
            SequenceOperation::Count
            | SequenceOperation::EmptyQ
            | SequenceOperation::SeqQ
            | SequenceOperation::CollQ
            | SequenceOperation::SequentialQ
            | SequenceOperation::First
            | SequenceOperation::Rest
            | SequenceOperation::Next
            | SequenceOperation::Seq
            | SequenceOperation::Empty
            | SequenceOperation::Cycle
            | SequenceOperation::Distinct
            | SequenceOperation::Dedupe
            | SequenceOperation::Sequence => call.positional.len() == 1,
            SequenceOperation::DropLast | SequenceOperation::Doall | SequenceOperation::Dorun => {
                (1..=2).contains(&call.positional.len())
            }
        };
        if !call.keywords.is_empty() || !arity_ok {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0041",
                format!(
                    "osiris.prelude/{} received an invalid argument list",
                    operation.runtime_name()
                ),
                span,
            );
            return Expr::error(span);
        }

        let mut arguments = Vec::with_capacity(call.positional.len());
        let mut parameter_types = Vec::with_capacity(call.positional.len());
        let mut summaries = CallSummaries::unknown();
        let result_type;
        let list_of = |item: Type| Type::List(Box::new(item));
        let item_type = |value: &Type| indexed_type(value);

        match operation {
            SequenceOperation::Cons => {
                let value = self.lower_expr(&call.positional[0], scope);
                let collection = self.lower_expr(&call.positional[1], scope);
                let item = item_type(&self.types.resolve(&collection.ty));
                result_type = list_of(self.types.join(&value.ty, &item));
                summaries = value.summaries.join(&collection.summaries);
                parameter_types.extend([value.ty.clone(), collection.ty.clone()]);
                arguments.extend([
                    CallArgument::Positional(value),
                    CallArgument::Positional(collection),
                ]);
            }
            SequenceOperation::Concat => {
                let mut item = Type::Never;
                for expression in &call.positional {
                    let value = self.lower_expr(expression, scope);
                    let value_item = item_type(&self.types.resolve(&value.ty));
                    item = self.types.join(&item, &value_item);
                    summaries = summaries.join(&value.summaries);
                    parameter_types.push(value.ty.clone());
                    arguments.push(CallArgument::Positional(value));
                }
                result_type = list_of(if item == Type::Never { Type::Any } else { item });
            }
            SequenceOperation::Count => {
                let value = self.lower_expr(&call.positional[0], scope);
                summaries = summaries.join(&value.summaries);
                parameter_types.push(value.ty.clone());
                arguments.push(CallArgument::Positional(value));
                result_type = Type::Int;
            }
            SequenceOperation::EmptyQ => {
                let value = self.lower_expr(&call.positional[0], scope);
                summaries = summaries.join(&value.summaries);
                parameter_types.push(value.ty.clone());
                arguments.push(CallArgument::Positional(value));
                result_type = Type::Bool;
            }
            SequenceOperation::SeqQ | SequenceOperation::CollQ | SequenceOperation::SequentialQ => {
                let value = self.lower_expr(&call.positional[0], scope);
                summaries = summaries.join(&value.summaries);
                parameter_types.push(value.ty.clone());
                arguments.push(CallArgument::Positional(value));
                result_type = Type::Bool;
            }
            SequenceOperation::First | SequenceOperation::Rest | SequenceOperation::Next => {
                let value = self.lower_expr(&call.positional[0], scope);
                let item = item_type(&self.types.resolve(&value.ty));
                summaries = summaries.join(&value.summaries);
                parameter_types.push(value.ty.clone());
                arguments.push(CallArgument::Positional(value));
                result_type = match operation {
                    SequenceOperation::First => Type::option(item),
                    SequenceOperation::Rest => list_of(item),
                    SequenceOperation::Next => Type::option(list_of(item)),
                    _ => unreachable!(),
                };
            }
            SequenceOperation::Nth => {
                let collection = self.lower_expr(&call.positional[0], scope);
                let index = self.lower_expr(&call.positional[1], scope);
                let item = item_type(&self.types.resolve(&collection.ty));
                summaries = summaries.join(&collection.summaries).join(&index.summaries);
                parameter_types.extend([collection.ty.clone(), index.ty.clone()]);
                arguments.extend([
                    CallArgument::Positional(collection),
                    CallArgument::Positional(index),
                ]);
                let mut nth_result = item;
                if let Some(default) = call.positional.get(2) {
                    let default = self.lower_expr(default, scope);
                    nth_result = self.types.join(&nth_result, &default.ty);
                    summaries = summaries.join(&default.summaries);
                    parameter_types.push(default.ty.clone());
                    arguments.push(CallArgument::Positional(default));
                }
                result_type = nth_result;
            }
            SequenceOperation::Seq | SequenceOperation::Empty | SequenceOperation::Sequence => {
                let value = self.lower_expr(&call.positional[0], scope);
                let item = item_type(&self.types.resolve(&value.ty));
                let value_type = self.types.resolve(&value.ty);
                let empty_type = value_type.clone();
                summaries = summaries.join(&value.summaries);
                parameter_types.push(value.ty.clone());
                arguments.push(CallArgument::Positional(value));
                result_type = match operation {
                    SequenceOperation::Seq => Type::option(list_of(item)),
                    SequenceOperation::Empty => match &empty_type {
                        Type::List(_) | Type::Vector(_) | Type::Set(_) | Type::Map(_, _) => {
                            empty_type.clone()
                        }
                        Type::Str => Type::Str,
                        Type::Bytes => Type::Bytes,
                        _ => list_of(item),
                    },
                    SequenceOperation::Sequence => list_of(item),
                    _ => unreachable!(),
                };
            }
            SequenceOperation::Take | SequenceOperation::Drop | SequenceOperation::TakeLast => {
                let amount = self.lower_expr(&call.positional[0], scope);
                let collection = self.lower_expr(&call.positional[1], scope);
                self.check_assignable(&amount.ty, &Type::Int, amount.span);
                let item = item_type(&self.types.resolve(&collection.ty));
                summaries = summaries
                    .join(&amount.summaries)
                    .join(&collection.summaries);
                parameter_types.extend([amount.ty.clone(), collection.ty.clone()]);
                arguments.extend([
                    CallArgument::Positional(amount),
                    CallArgument::Positional(collection),
                ]);
                result_type = list_of(item);
            }
            SequenceOperation::TakeWhile
            | SequenceOperation::DropWhile
            | SequenceOperation::Keep
            | SequenceOperation::Remove
            | SequenceOperation::Removev
            | SequenceOperation::Some
            | SequenceOperation::Every
            | SequenceOperation::NotEvery
            | SequenceOperation::NotAny => {
                let collection = self.lower_expr(&call.positional[1], scope);
                let item = item_type(&self.types.resolve(&collection.ty));
                let callback = self.lower_sequence_callback(
                    &call.positional[0],
                    call.positional[0].span,
                    scope,
                    std::slice::from_ref(&item),
                );
                let callback_result = match &callback.ty {
                    Type::Fn(signature) => (*signature.return_type).clone(),
                    _ => Type::Any,
                };
                summaries = summaries
                    .join(&callback.summaries)
                    .join(&collection.summaries);
                parameter_types.extend([callback.ty.clone(), collection.ty.clone()]);
                arguments.extend([
                    CallArgument::Positional(callback),
                    CallArgument::Positional(collection),
                ]);
                result_type = match operation {
                    SequenceOperation::TakeWhile | SequenceOperation::DropWhile => list_of(item),
                    SequenceOperation::Keep | SequenceOperation::Remove => {
                        list_of(if operation == SequenceOperation::Keep {
                            non_nil_type(&callback_result)
                        } else {
                            item.clone()
                        })
                    }
                    SequenceOperation::Removev => Type::Vector(Box::new(item)),
                    SequenceOperation::Some => Type::option(callback_result),
                    SequenceOperation::Every
                    | SequenceOperation::NotEvery
                    | SequenceOperation::NotAny => Type::Bool,
                    _ => unreachable!(),
                };
            }
            SequenceOperation::Distinct | SequenceOperation::Dedupe => {
                let collection = self.lower_expr(&call.positional[0], scope);
                let item = item_type(&self.types.resolve(&collection.ty));
                summaries = summaries.join(&collection.summaries);
                parameter_types.push(collection.ty.clone());
                arguments.push(CallArgument::Positional(collection));
                result_type = list_of(item);
            }
            SequenceOperation::Partition | SequenceOperation::PartitionAll => {
                let values = call
                    .positional
                    .iter()
                    .map(|expression| self.lower_expr(expression, scope))
                    .collect::<Vec<_>>();
                self.check_assignable(&values[0].ty, &Type::Int, values[0].span);
                if values.len() >= 3 {
                    self.check_assignable(&values[1].ty, &Type::Int, values[1].span);
                }
                let collection = values.last().expect("partition arity validated");
                let mut item = item_type(&self.types.resolve(&collection.ty));
                if operation == SequenceOperation::Partition && values.len() == 4 {
                    let padding_item = item_type(&self.types.resolve(&values[2].ty));
                    item = self.types.join(&item, &padding_item);
                }
                for value in values {
                    summaries = summaries.join(&value.summaries);
                    parameter_types.push(value.ty.clone());
                    arguments.push(CallArgument::Positional(value));
                }
                result_type = list_of(list_of(item));
            }
            SequenceOperation::PartitionBy => {
                let collection = self.lower_expr(&call.positional[1], scope);
                let item = item_type(&self.types.resolve(&collection.ty));
                let callback = self.lower_sequence_callback(
                    &call.positional[0],
                    call.positional[0].span,
                    scope,
                    std::slice::from_ref(&item),
                );
                summaries = summaries
                    .join(&callback.summaries)
                    .join(&collection.summaries);
                parameter_types.extend([callback.ty.clone(), collection.ty.clone()]);
                arguments.extend([
                    CallArgument::Positional(callback),
                    CallArgument::Positional(collection),
                ]);
                result_type = list_of(list_of(item));
            }
            SequenceOperation::Interleave => {
                let mut item = Type::Never;
                for expression in &call.positional {
                    let collection = self.lower_expr(expression, scope);
                    let collection_item = item_type(&self.types.resolve(&collection.ty));
                    item = self.types.join(&item, &collection_item);
                    summaries = summaries.join(&collection.summaries);
                    parameter_types.push(collection.ty.clone());
                    arguments.push(CallArgument::Positional(collection));
                }
                result_type = list_of(if item == Type::Never { Type::Any } else { item });
            }
            SequenceOperation::Interpose => {
                let separator = self.lower_expr(&call.positional[0], scope);
                let collection = self.lower_expr(&call.positional[1], scope);
                let item = item_type(&self.types.resolve(&collection.ty));
                result_type = list_of(self.types.join(&separator.ty, &item));
                summaries = separator.summaries.join(&collection.summaries);
                parameter_types.extend([separator.ty.clone(), collection.ty.clone()]);
                arguments.extend([
                    CallArgument::Positional(separator),
                    CallArgument::Positional(collection),
                ]);
            }
            SequenceOperation::DropLast => {
                let values = call
                    .positional
                    .iter()
                    .map(|expression| self.lower_expr(expression, scope))
                    .collect::<Vec<_>>();
                if values.len() == 2 {
                    self.check_assignable(&values[0].ty, &Type::Int, values[0].span);
                }
                let collection = values.last().expect("drop-last arity validated");
                let item = item_type(&self.types.resolve(&collection.ty));
                for value in values {
                    summaries = summaries.join(&value.summaries);
                    parameter_types.push(value.ty.clone());
                    arguments.push(CallArgument::Positional(value));
                }
                result_type = list_of(item);
            }
            SequenceOperation::KeepIndexed | SequenceOperation::MapIndexed => {
                let collection = self.lower_expr(&call.positional[1], scope);
                let item = item_type(&self.types.resolve(&collection.ty));
                let callback = self.lower_sequence_callback(
                    &call.positional[0],
                    call.positional[0].span,
                    scope,
                    &[Type::Int, item.clone()],
                );
                let callback_result = match &callback.ty {
                    Type::Fn(signature) => (*signature.return_type).clone(),
                    _ => Type::Any,
                };
                summaries = summaries
                    .join(&callback.summaries)
                    .join(&collection.summaries);
                parameter_types.extend([callback.ty.clone(), collection.ty.clone()]);
                arguments.extend([
                    CallArgument::Positional(callback),
                    CallArgument::Positional(collection),
                ]);
                result_type = if operation == SequenceOperation::KeepIndexed {
                    list_of(non_nil_type(&callback_result))
                } else {
                    list_of(callback_result)
                };
            }
            SequenceOperation::Iterate => {
                let initial = self.lower_expr(&call.positional[1], scope);
                let callback = self.lower_sequence_callback(
                    &call.positional[0],
                    call.positional[0].span,
                    scope,
                    std::slice::from_ref(&initial.ty),
                );
                if let Type::Fn(signature) = &callback.ty {
                    self.check_assignable(&signature.return_type, &initial.ty, callback.span);
                }
                summaries = summaries.join(&callback.summaries).join(&initial.summaries);
                parameter_types.extend([callback.ty.clone(), initial.ty.clone()]);
                arguments.extend([
                    CallArgument::Positional(callback),
                    CallArgument::Positional(initial.clone()),
                ]);
                result_type = list_of(initial.ty);
            }
            SequenceOperation::Repeat => {
                let values = call
                    .positional
                    .iter()
                    .map(|expression| self.lower_expr(expression, scope))
                    .collect::<Vec<_>>();
                let value_type = values
                    .last()
                    .map(|value| value.ty.clone())
                    .expect("repeat arity validated");
                if values.len() == 2 {
                    self.check_assignable(&values[0].ty, &Type::Int, values[0].span);
                }
                for value in values {
                    summaries = summaries.join(&value.summaries);
                    parameter_types.push(value.ty.clone());
                    arguments.push(CallArgument::Positional(value));
                }
                result_type = list_of(value_type);
            }
            SequenceOperation::Repeatedly => {
                let callback_index = if call.positional.len() == 1 { 0 } else { 1 };
                let callback = self.lower_sequence_callback(
                    &call.positional[callback_index],
                    call.positional[callback_index].span,
                    scope,
                    &[],
                );
                if call.positional.len() == 2 {
                    let amount = self.lower_expr(&call.positional[0], scope);
                    self.check_assignable(&amount.ty, &Type::Int, amount.span);
                    summaries = summaries.join(&amount.summaries);
                    parameter_types.push(amount.ty.clone());
                    arguments.push(CallArgument::Positional(amount));
                }
                summaries = summaries.join(&callback.summaries);
                parameter_types.push(callback.ty.clone());
                arguments.push(CallArgument::Positional(callback.clone()));
                let callback_result = match callback.ty {
                    Type::Fn(signature) => (*signature.return_type).clone(),
                    _ => Type::Any,
                };
                result_type = list_of(callback_result);
            }
            SequenceOperation::Cycle => {
                let collection = self.lower_expr(&call.positional[0], scope);
                let item = item_type(&self.types.resolve(&collection.ty));
                summaries = summaries.join(&collection.summaries);
                parameter_types.push(collection.ty.clone());
                arguments.push(CallArgument::Positional(collection));
                result_type = list_of(item);
            }
            SequenceOperation::Reductions => {
                let collection_index = call.positional.len() - 1;
                let collection = self.lower_expr(&call.positional[collection_index], scope);
                let item = item_type(&self.types.resolve(&collection.ty));
                let (accumulator, initial_expr) = if call.positional.len() == 3 {
                    let initial = self.lower_expr(&call.positional[1], scope);
                    (initial.ty.clone(), Some(initial))
                } else {
                    (item.clone(), None)
                };
                let callback = self.lower_sequence_callback(
                    &call.positional[0],
                    call.positional[0].span,
                    scope,
                    &[accumulator.clone(), item],
                );
                if let Type::Fn(signature) = &callback.ty {
                    let callback_accumulator = unreduced_type(&signature.return_type);
                    self.check_assignable(&callback_accumulator, &accumulator, callback.span);
                }
                summaries = summaries
                    .join(&callback.summaries)
                    .join(&collection.summaries);
                parameter_types.push(callback.ty.clone());
                arguments.push(CallArgument::Positional(callback));
                if let Some(initial) = initial_expr {
                    summaries = summaries.join(&initial.summaries);
                    parameter_types.push(initial.ty.clone());
                    arguments.push(CallArgument::Positional(initial));
                }
                parameter_types.push(collection.ty.clone());
                arguments.push(CallArgument::Positional(collection));
                result_type = list_of(accumulator);
            }
            SequenceOperation::RunBang => {
                let collection = self.lower_expr(&call.positional[1], scope);
                let item = item_type(&self.types.resolve(&collection.ty));
                let callback = self.lower_sequence_callback(
                    &call.positional[0],
                    call.positional[0].span,
                    scope,
                    std::slice::from_ref(&item),
                );
                summaries = summaries
                    .join(&callback.summaries)
                    .join(&collection.summaries);
                parameter_types.extend([callback.ty.clone(), collection.ty.clone()]);
                arguments.extend([
                    CallArgument::Positional(callback),
                    CallArgument::Positional(collection),
                ]);
                result_type = Type::None;
            }
            SequenceOperation::Doall | SequenceOperation::Dorun => {
                let (limit, collection) = if call.positional.len() == 2 {
                    let limit = self.lower_expr(&call.positional[0], scope);
                    self.check_assignable(&limit.ty, &Type::Int, limit.span);
                    let collection = self.lower_expr(&call.positional[1], scope);
                    (Some(limit), collection)
                } else {
                    (None, self.lower_expr(&call.positional[0], scope))
                };
                summaries = summaries.join(&collection.summaries);
                if let Some(limit) = limit {
                    summaries = summaries.join(&limit.summaries);
                    parameter_types.push(limit.ty.clone());
                    arguments.push(CallArgument::Positional(limit));
                }
                parameter_types.push(collection.ty.clone());
                arguments.push(CallArgument::Positional(collection.clone()));
                result_type = if operation == SequenceOperation::Doall {
                    collection.ty
                } else {
                    Type::None
                };
            }
        }

        let binding = self.ensure_core_collection_binding(operation.runtime_name(), span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(parameter_types, result_type.clone())
                    .with_summaries(CallSummaries::unknown()),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: result_type,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments,
            },
        }
    }

    fn symbolic_argument_expression(&self, expression: &Expr) -> Option<String> {
        match &expression.kind {
            ExprKind::Integer(value) => Some(value.clone()),
            ExprKind::Binding(binding) => self
                .bindings
                .get(binding)
                .map(|binding| binding.name.canonical.clone()),
            _ => None,
        }
    }

    fn lower_mapv(&mut self, call: &ast::CallExpr, span: Span, scope: &mut Scope) -> Expr {
        if !call.keywords.is_empty() || call.positional.len() < 2 {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0020",
                "osiris.prelude/mapv expects a function and at least one collection",
                span,
            );
            return Expr::error(span);
        }

        let collections = call.positional[1..]
            .iter()
            .map(|collection| self.lower_expr(collection, scope))
            .collect::<Vec<_>>();
        let item_types = collections
            .iter()
            .map(|collection| indexed_type(&collection.ty))
            .collect::<Vec<_>>();
        let mut function = match &call.positional[0].kind {
            AstExprKind::Fn(function) => self.lower_lambda_with_expected_parameters(
                function,
                call.positional[0].span,
                scope,
                &item_types,
            ),
            _ => self.lower_expr(&call.positional[0], scope),
        };
        let Type::Fn(signature) = &function.ty else {
            self.error(
                "OSR-T0020",
                "first mapv argument must be a statically typed function",
                function.span,
            );
            return Expr::error(span);
        };
        if signature.parameters.len() != item_types.len() {
            self.error(
                "OSR-T0020",
                format!(
                    "mapv function must accept exactly {} argument(s)",
                    item_types.len()
                ),
                function.span,
            );
            return Expr::error(span);
        }
        let callback_summaries = signature.summaries.clone();
        let callback_return = (*signature.return_type).clone();
        for (item_type, parameter) in item_types.iter().zip(&signature.parameters) {
            self.check_assignable(item_type, parameter, function.span);
        }

        if let ExprKind::Lambda { parameters, .. } = &mut function.kind {
            for parameter in parameters {
                parameter.ty = self.types.resolve(&parameter.ty);
                self.set_binding_type(&parameter.binding, parameter.ty.clone());
            }
        }
        function.ty = self.types.resolve(&function.ty);
        let result_type = Type::Vector(Box::new(self.types.resolve(&callback_return)));
        let temporal = collections
            .iter()
            .fold(
                crate::types::TemporalSummary::pointwise(),
                |summary, collection| summary.compose(&collection.summaries.temporal),
            )
            .compose(&callback_summaries.temporal);
        let mut summaries = collections
            .iter()
            .fold(function.summaries.clone(), |summary, collection| {
                summary.join(&collection.summaries)
            })
            .join(&callback_summaries);
        summaries.temporal = temporal;
        summaries.data = DataProperties {
            alignment: Alignment::Positional,
            preserves_length: (collections.len() == 1).then_some(true),
            materializes: Some(true),
            reshapes: Some(false),
            ..DataProperties::unknown()
        };

        let binding = self.ensure_core_mapv_binding(span);
        let callee_type = Type::Fn(
            FunctionType::new(
                std::iter::once(function.ty.clone())
                    .chain(collections.iter().map(|collection| collection.ty.clone()))
                    .collect(),
                result_type.clone(),
            )
            .with_summaries(callback_summaries),
        );
        let callee = Expr::pure(span, callee_type, ExprKind::Binding(binding));
        Expr {
            span,
            ty: result_type,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: std::iter::once(CallArgument::Positional(function))
                    .chain(collections.into_iter().map(CallArgument::Positional))
                    .collect(),
            },
        }
    }

    fn lower_map_like(
        &mut self,
        call: &ast::CallExpr,
        span: Span,
        scope: &mut Scope,
        operation: CollectionOperation,
    ) -> Expr {
        if !call.keywords.is_empty() || call.positional.len() < 2 {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0020",
                format!(
                    "osiris.prelude/{} expects a function and at least one collection",
                    operation.runtime_name()
                ),
                span,
            );
            return Expr::error(span);
        }

        let collections = call.positional[1..]
            .iter()
            .map(|collection| self.lower_expr(collection, scope))
            .collect::<Vec<_>>();
        if matches!(
            operation,
            CollectionOperation::Filter | CollectionOperation::Filterv
        ) && collections.len() != 1
        {
            self.error(
                "OSR-T0020",
                format!(
                    "osiris.prelude/{} accepts exactly one collection",
                    operation.runtime_name()
                ),
                span,
            );
            return Expr::error(span);
        }
        let expected_parameters = collections
            .iter()
            .map(|collection| indexed_type(&collection.ty))
            .collect::<Vec<_>>();
        let function = match &call.positional[0].kind {
            AstExprKind::Fn(function) => self.lower_lambda_with_expected_parameters(
                function,
                call.positional[0].span,
                scope,
                &expected_parameters,
            ),
            _ => self.lower_expr(&call.positional[0], scope),
        };
        let Type::Fn(signature) = &function.ty else {
            self.error(
                "OSR-T0020",
                format!(
                    "first {} argument must be a statically typed function",
                    operation.runtime_name()
                ),
                function.span,
            );
            return Expr::error(span);
        };
        if signature.parameters.len() != expected_parameters.len() {
            self.error(
                "OSR-T0020",
                format!(
                    "{} callback expects {} arguments, found {}",
                    operation.runtime_name(),
                    expected_parameters.len(),
                    signature.parameters.len()
                ),
                function.span,
            );
            return Expr::error(span);
        }
        for (actual, expected) in expected_parameters.iter().zip(&signature.parameters) {
            self.check_assignable(actual, expected, function.span);
        }
        if matches!(
            operation,
            CollectionOperation::Filter | CollectionOperation::Filterv
        ) {
            self.check_assignable(&signature.return_type, &Type::Bool, function.span);
        }
        let callback_summaries = signature.summaries.clone();
        let callback_return = (*signature.return_type).clone();
        let result_item = if matches!(
            operation,
            CollectionOperation::Mapcat | CollectionOperation::Mapcatv
        ) {
            match callback_return {
                Type::List(item) | Type::Vector(item) | Type::Set(item) | Type::Map(_, item) => {
                    *item
                }
                Type::Any | Type::Unknown => Type::Any,
                other => {
                    self.error(
                        "OSR-T0020",
                        format!(
                            "{} callback must return a collection, found `{other}`",
                            operation.runtime_name()
                        ),
                        function.span,
                    );
                    return Expr::error(span);
                }
            }
        } else if matches!(
            operation,
            CollectionOperation::Filter | CollectionOperation::Filterv
        ) {
            expected_parameters.first().cloned().unwrap_or(Type::Any)
        } else {
            callback_return
        };
        let result_type = if operation.result_is_vector() {
            Type::Vector(Box::new(result_item))
        } else {
            Type::List(Box::new(result_item))
        };
        let mut summaries =
            join_summaries(collections.iter().map(|collection| &collection.summaries))
                .join(&function.summaries)
                .join(&callback_summaries);
        let temporal = collections
            .iter()
            .fold(
                crate::types::TemporalSummary::pointwise(),
                |summary, collection| summary.compose(&collection.summaries.temporal),
            )
            .compose(&callback_summaries.temporal);
        summaries.temporal = temporal;
        summaries.data = DataProperties {
            alignment: Alignment::Positional,
            preserves_length: (operation == CollectionOperation::Map && collections.len() == 1)
                .then_some(true),
            materializes: Some(true),
            reshapes: Some(matches!(
                operation,
                CollectionOperation::Mapcat | CollectionOperation::Mapcatv
            )),
            ..DataProperties::unknown()
        };
        let binding = self.ensure_core_collection_binding(operation.runtime_name(), span);
        let mut parameter_types = vec![function.ty.clone()];
        parameter_types.extend(collections.iter().map(|collection| collection.ty.clone()));
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(parameter_types, result_type.clone())
                    .with_summaries(callback_summaries),
            ),
            ExprKind::Binding(binding),
        );
        let mut arguments = vec![CallArgument::Positional(function)];
        arguments.extend(collections.into_iter().map(CallArgument::Positional));
        Expr {
            span,
            ty: result_type,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments,
            },
        }
    }

    fn lower_reduce(
        &mut self,
        call: &ast::CallExpr,
        span: Span,
        scope: &mut Scope,
        named_fold: bool,
    ) -> Expr {
        let valid = if named_fold {
            call.positional.len() == 3
        } else {
            (2..=3).contains(&call.positional.len())
        };
        if !call.keywords.is_empty() || !valid {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0020",
                if named_fold {
                    "osiris.prelude/fold expects a function, initial value, and collection"
                } else {
                    "osiris.prelude/reduce expects a function, collection, and optional initial value"
                },
                span,
            );
            return Expr::error(span);
        }
        let collection = self.lower_expr(
            call.positional
                .last()
                .expect("validated collection argument"),
            scope,
        );
        let item_type = indexed_type(&collection.ty);
        let initial = if named_fold || call.positional.len() == 3 {
            Some(self.lower_expr(&call.positional[1], scope))
        } else {
            None
        };
        let accumulator_type = initial
            .as_ref()
            .map(|initial| initial.ty.clone())
            .unwrap_or_else(|| item_type.clone());
        let expected = [accumulator_type.clone(), item_type.clone()];
        let function = match &call.positional[0].kind {
            AstExprKind::Fn(function) => self.lower_lambda_with_expected_parameters(
                function,
                call.positional[0].span,
                scope,
                &expected,
            ),
            _ => self.lower_expr(&call.positional[0], scope),
        };
        let Type::Fn(signature) = &function.ty else {
            self.error(
                "OSR-T0020",
                "reduce callback must be a statically typed function",
                function.span,
            );
            return Expr::error(span);
        };
        if signature.parameters.len() != 2 {
            self.error(
                "OSR-T0020",
                "reduce callback must accept accumulator and item",
                function.span,
            );
            return Expr::error(span);
        }
        for (actual, expected) in expected.iter().zip(&signature.parameters) {
            self.check_assignable(actual, expected, function.span);
        }
        let callback_summaries = signature.summaries.clone();
        let callback_return = (*signature.return_type).clone();
        // The callback feeds its result back as the next accumulator.  Keep
        // that invariant explicit while allowing `Reduced[T]` to carry the
        // same accumulator as an early result.
        let callback_accumulator = unreduced_type(&callback_return);
        self.check_assignable(&callback_accumulator, &accumulator_type, function.span);
        let result_type = accumulator_type.clone();
        let mut summaries = collection
            .summaries
            .join(&function.summaries)
            .join(&callback_summaries);
        if let Some(initial) = &initial {
            summaries = summaries.join(&initial.summaries);
        }
        summaries.data = DataProperties::scalar();
        let runtime_name = if named_fold { "fold" } else { "reduce" };
        let binding = self.ensure_core_collection_binding(runtime_name, span);
        let mut parameter_types = vec![function.ty.clone()];
        if let Some(initial) = &initial {
            parameter_types.push(initial.ty.clone());
        }
        parameter_types.push(collection.ty.clone());
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(parameter_types, result_type.clone())
                    .with_summaries(callback_summaries),
            ),
            ExprKind::Binding(binding),
        );
        let mut arguments = vec![CallArgument::Positional(function)];
        if let Some(initial) = initial {
            arguments.push(CallArgument::Positional(initial));
        }
        arguments.push(CallArgument::Positional(collection));
        Expr {
            span,
            ty: result_type,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments,
            },
        }
    }

    fn lower_reduced_operation(
        &mut self,
        call: &ast::CallExpr,
        span: Span,
        scope: &mut Scope,
        operation: ReducedOperation,
    ) -> Expr {
        if !call.keywords.is_empty() || call.positional.len() != 1 {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            let source_name = match operation {
                ReducedOperation::Wrap => "reduced",
                ReducedOperation::Predicate => "reduced?",
                ReducedOperation::Unwrap => "unreduced",
            };
            self.error(
                "OSR-T0020",
                format!("osiris.prelude/{source_name} expects exactly one argument"),
                span,
            );
            return Expr::error(span);
        }

        let value = self.lower_expr(&call.positional[0], scope);
        let result_type = match operation {
            ReducedOperation::Wrap => reduced_type(value.ty.clone()),
            ReducedOperation::Predicate => Type::Bool,
            ReducedOperation::Unwrap => unreduced_type(&value.ty),
        };
        let binding = self.ensure_core_collection_binding(operation.runtime_name(), span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(vec![value.ty.clone()], result_type.clone())
                    .with_summaries(CallSummaries::pure_scalar()),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: result_type,
            summaries: value.summaries.clone(),
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![CallArgument::Positional(value)],
            },
        }
    }

    fn lower_doseq(&mut self, call: &ast::CallExpr, span: Span, scope: &mut Scope) -> Expr {
        if !call.keywords.is_empty() || call.positional.len() != 2 {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0027",
                "osiris.prelude/doseq* expects a callback and one collection",
                span,
            );
            return Expr::error(span);
        }

        let collection = self.lower_expr(&call.positional[1], scope);
        let item_type = indexed_type(&collection.ty);
        let function = match &call.positional[0].kind {
            AstExprKind::Fn(function) => self.lower_lambda_with_expected_parameters(
                function,
                call.positional[0].span,
                scope,
                std::slice::from_ref(&item_type),
            ),
            _ => self.lower_expr(&call.positional[0], scope),
        };
        let Type::Fn(signature) = &function.ty else {
            self.error(
                "OSR-T0027",
                "doseq callback must be a statically typed function",
                function.span,
            );
            return Expr::error(span);
        };
        if signature.parameters.len() != 1 {
            self.error(
                "OSR-T0027",
                "doseq callback must accept exactly one item",
                function.span,
            );
            return Expr::error(span);
        }
        self.check_assignable(&item_type, &signature.parameters[0], function.span);
        self.check_assignable(&signature.return_type, &Type::None, function.span);

        let callback_summaries = signature.summaries.clone();
        let mut summaries = collection
            .summaries
            .join(&function.summaries)
            .join(&callback_summaries);
        summaries.temporal = collection
            .summaries
            .temporal
            .compose(&callback_summaries.temporal);
        summaries.data = DataProperties::scalar();
        let binding = self.ensure_core_collection_binding("doseq", span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(vec![function.ty.clone(), collection.ty.clone()], Type::None)
                    .with_summaries(callback_summaries),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: Type::None,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![
                    CallArgument::Positional(function),
                    CallArgument::Positional(collection),
                ],
            },
        }
    }

    fn lower_for_stop(&mut self, call: &ast::CallExpr, span: Span, scope: &mut Scope) -> Expr {
        if !call.args.is_empty() {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0027",
                "osiris.prelude/for-stop* does not accept arguments",
                span,
            );
            return Expr::error(span);
        }
        let binding = self.ensure_core_collection_binding("for_stop", span);
        Expr {
            span,
            ty: Type::Never,
            summaries: CallSummaries::pure_scalar(),
            kind: ExprKind::Call {
                callee: Box::new(Expr::pure(
                    span,
                    Type::Fn(
                        FunctionType::new(Vec::new(), Type::Never)
                            .with_summaries(CallSummaries::pure_scalar()),
                    ),
                    ExprKind::Binding(binding),
                )),
                arguments: Vec::new(),
            },
        }
    }

    fn lower_control_intrinsic(
        &mut self,
        call: &ast::CallExpr,
        span: Span,
        scope: &mut Scope,
        intrinsic: ControlIntrinsic,
    ) -> Expr {
        if !call.keywords.is_empty() || call.positional.len() != 1 {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0026",
                format!(
                    "osiris.prelude/{}* expects exactly one positional argument",
                    match intrinsic {
                        ControlIntrinsic::Truthy => "truthy",
                        ControlIntrinsic::Nil => "nil",
                        ControlIntrinsic::Present => "present",
                        ControlIntrinsic::Nonempty => "nonempty",
                    }
                ),
                span,
            );
            return Expr::error(span);
        }

        let value = self.lower_expr(&call.positional[0], scope);
        let present_type = non_nil_type(&self.types.resolve(&value.ty));
        if intrinsic == ControlIntrinsic::Nonempty
            && !matches!(
                &present_type,
                Type::List(_)
                    | Type::Vector(_)
                    | Type::Tuple(_)
                    | Type::Str
                    | Type::Bytes
                    | Type::Never
                    | Type::Error
            )
        {
            self.error(
                "OSR-T0026",
                format!(
                    "when-first requires an indexable List, Vector, Tuple, Str, or Bytes value; found `{}`",
                    value.ty
                ),
                value.span,
            );
            return Expr::error(span);
        }

        let result_type = match intrinsic {
            ControlIntrinsic::Truthy | ControlIntrinsic::Nil | ControlIntrinsic::Nonempty => {
                Type::Bool
            }
            ControlIntrinsic::Present => present_type,
        };
        let binding = self.ensure_core_collection_binding(intrinsic.runtime_name(), span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(vec![value.ty.clone()], result_type.clone())
                    .with_summaries(CallSummaries::pure_scalar()),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: result_type,
            summaries: value.summaries.clone(),
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![CallArgument::Positional(value)],
            },
        }
    }

    /// Lower the failure-only runtime entry used by the Clojure-style
    /// `assert` macro.  The macro supplies a literal false condition in its
    /// else branch, but keeping the condition as an argument makes the ABI
    /// useful to packaged prelude implementations as well.
    fn lower_assert(&mut self, call: &ast::CallExpr, span: Span, scope: &mut Scope) -> Expr {
        if !call.keywords.is_empty() || !(1..=2).contains(&call.positional.len()) {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0026",
                "osiris.prelude/assert* expects a condition and optional message",
                span,
            );
            return Expr::error(span);
        }
        let condition = self.lower_expr(&call.positional[0], scope);
        let message = call
            .positional
            .get(1)
            .map(|value| self.lower_expr(value, scope));
        let mut summaries = condition.summaries.clone();
        if let Some(message) = &message {
            summaries = summaries.join(&message.summaries);
        }
        summaries.effects = summaries
            .effects
            .union(&EffectRow::singleton(Effect::Throw));
        let binding = self.ensure_core_collection_binding("assert_value", span);
        let mut parameter_types = vec![condition.ty.clone()];
        let mut arguments = vec![CallArgument::Positional(condition)];
        if let Some(message) = message {
            parameter_types.push(message.ty.clone());
            arguments.push(CallArgument::Positional(message));
        }
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(parameter_types, Type::Never).with_summaries(summaries.clone()),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: Type::Never,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments,
            },
        }
    }

    /// Lower the macro-generated `loop*` primitive.  The callback is typed
    /// against the initial state values, so an unannotated loop still retains
    /// the ordinary local inference experience.  Runtime iteration itself is
    /// delegated to `osiris.prelude.loop`, which consumes `recur` tokens in a
    /// Python `while` loop and therefore does not grow the Python stack.
    fn lower_loop(&mut self, call: &ast::CallExpr, span: Span, scope: &mut Scope) -> Expr {
        if !call.keywords.is_empty() || call.positional.is_empty() {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0022",
                "osiris.prelude/loop* expects a callback and zero or more initial values",
                span,
            );
            return Expr::error(span);
        }

        let initials = call.positional[1..]
            .iter()
            .map(|initial| self.lower_expr(initial, scope))
            .collect::<Vec<_>>();
        let expected = initials
            .iter()
            .map(|initial| initial.ty.clone())
            .collect::<Vec<_>>();

        // `recur*` is only meaningful while lowering this callback.  Keep the
        // arity stack lexical so nested loops validate against their nearest
        // state vector.
        let callback_is_inline = matches!(&call.positional[0].kind, AstExprKind::Fn(_));
        self.loop_arities.push(expected.len());
        self.loop_state_types.push(expected.clone());
        self.loop_callback_depths
            .push(self.function_depth + usize::from(callback_is_inline));
        let function = match &call.positional[0].kind {
            AstExprKind::Fn(function) => self.lower_lambda_with_expected_parameters(
                function,
                call.positional[0].span,
                scope,
                &expected,
            ),
            _ => self.lower_expr(&call.positional[0], scope),
        };
        if let ExprKind::Lambda { body, .. } = &function.kind {
            self.validate_recur_tail(body, true);
        }
        self.loop_arities.pop();
        self.loop_state_types.pop();
        self.loop_callback_depths.pop();

        let Type::Fn(signature) = &function.ty else {
            self.error(
                "OSR-T0022",
                "osiris.prelude/loop* callback must be a function",
                function.span,
            );
            return Expr::error(span);
        };
        if signature.parameters.len() != expected.len() {
            self.error(
                "OSR-T0022",
                format!(
                    "loop callback expects {} state value(s), found {}",
                    expected.len(),
                    signature.parameters.len()
                ),
                function.span,
            );
            return Expr::error(span);
        }
        for (actual, expected) in expected.iter().zip(&signature.parameters) {
            self.check_assignable(actual, expected, function.span);
        }
        let callback_summaries = signature.summaries.clone();
        let result_type = self.types.resolve(&signature.return_type);
        let mut summaries = initials
            .iter()
            .fold(function.summaries.clone(), |summary, initial| {
                summary.join(&initial.summaries)
            })
            .join(&callback_summaries);
        summaries.data = DataProperties::scalar();

        let binding = self.ensure_core_loop_binding(span);
        let mut parameter_types = vec![function.ty.clone()];
        parameter_types.extend(initials.iter().map(|initial| initial.ty.clone()));
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(parameter_types, result_type.clone())
                    .with_summaries(callback_summaries),
            ),
            ExprKind::Binding(binding),
        );
        let mut arguments = vec![CallArgument::Positional(function)];
        arguments.extend(initials.into_iter().map(CallArgument::Positional));
        Expr {
            span,
            ty: result_type,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments,
            },
        }
    }

    /// Turn a function-local `recur` body into the same state-machine shape as
    /// an explicit `loop`.  The callback deliberately reuses the parameter
    /// binding ids from the surrounding function: the body was lowered against
    /// those ids already, and the backend will emit a readable local helper
    /// whose parameters shadow the outer function parameters for each step.
    fn wrap_function_recur(&mut self, parameters: &[Parameter], body: Expr, span: Span) -> Expr {
        let state_types = parameters
            .iter()
            .map(|parameter| self.types.resolve(&parameter.ty))
            .collect::<Vec<_>>();
        let callback_parameters = parameters
            .iter()
            .zip(&state_types)
            .map(|(parameter, ty)| Parameter {
                binding: parameter.binding.clone(),
                ty: ty.clone(),
                default: None,
                // A variadic outer parameter is represented by one tuple/list
                // state value in the loop.  The callback therefore receives a
                // fixed arity state vector and keeps the original binding's
                // value unchanged.
                variadic: false,
            })
            .collect::<Vec<_>>();
        let body_type = body.ty.clone();
        let body_summaries = body.summaries.clone();
        let callback_type = Type::Fn(
            FunctionType::new(state_types.clone(), body_type.clone())
                .with_summaries(body_summaries.clone()),
        );
        let callback = Expr {
            span,
            ty: callback_type.clone(),
            summaries: body_summaries.clone(),
            kind: ExprKind::Lambda {
                parameters: callback_parameters,
                body: Box::new(body),
            },
        };
        let initials = parameters
            .iter()
            .zip(&state_types)
            .map(|(parameter, ty)| {
                Expr::pure(
                    span,
                    ty.clone(),
                    ExprKind::Binding(parameter.binding.clone()),
                )
            })
            .collect::<Vec<_>>();
        let binding = self.ensure_core_loop_binding(span);
        let mut callee_parameters = vec![callback_type];
        callee_parameters.extend(state_types);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(callee_parameters, body_type.clone())
                    .with_summaries(body_summaries.clone()),
            ),
            ExprKind::Binding(binding),
        );
        let mut arguments = vec![CallArgument::Positional(callback)];
        arguments.extend(initials.into_iter().map(CallArgument::Positional));
        Expr {
            span,
            ty: body_type,
            summaries: body_summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments,
            },
        }
    }

    /// `recur` is a tail-only transfer to the owning loop.  The ordinary
    /// expression lowerer intentionally keeps the core AST small, so perform
    /// this structural check on the already typed callback body. Nested
    /// lambdas are skipped here; their function-depth check in `lower_recur`
    /// diagnoses attempts to capture an outer loop, while nested loops validate
    /// their own callbacks when they are lowered.
    fn validate_recur_tail(&mut self, expression: &Expr, tail: bool) {
        match &expression.kind {
            ExprKind::Call { callee, arguments } => {
                let is_recur = self
                    .core_recur_binding
                    .as_ref()
                    .is_some_and(|binding| {
                        matches!(&callee.kind, ExprKind::Binding(candidate) if candidate == binding)
                    });
                if is_recur && !tail {
                    self.error(
                        "OSR-T0023",
                        "recur must appear in tail position",
                        expression.span,
                    );
                }
                self.validate_recur_tail(callee, false);
                for argument in arguments {
                    let value = match argument {
                        CallArgument::Positional(value) | CallArgument::Keyword { value, .. } => {
                            value
                        }
                    };
                    self.validate_recur_tail(value, false);
                }
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.validate_recur_tail(condition, false);
                self.validate_recur_tail(then_branch, tail);
                self.validate_recur_tail(else_branch, tail);
            }
            ExprKind::Let { bindings, body } => {
                for binding in bindings {
                    self.validate_recur_tail(&binding.value, false);
                }
                self.validate_recur_tail(body, tail);
            }
            ExprKind::Do(expressions) => {
                for expression in expressions.iter().take(expressions.len().saturating_sub(1)) {
                    self.validate_recur_tail(expression, false);
                }
                if let Some(last) = expressions.last() {
                    self.validate_recur_tail(last, tail);
                }
            }
            ExprKind::Try {
                body,
                catches,
                finally_body,
            } => {
                let branch_tail = tail && finally_body.is_none();
                self.validate_recur_tail(body, branch_tail);
                for catch in catches {
                    self.validate_recur_tail(&catch.body, branch_tail);
                }
                if let Some(finally_body) = finally_body {
                    self.validate_recur_tail(finally_body, false);
                }
            }
            ExprKind::Lambda { .. } => {}
            ExprKind::Operator { operands, .. } => {
                for operand in operands {
                    self.validate_recur_tail(operand, false);
                }
            }
            ExprKind::Attribute { value, .. } | ExprKind::Raise(Some(value)) => {
                self.validate_recur_tail(value, false)
            }
            ExprKind::Index { value, index } => {
                self.validate_recur_tail(value, false);
                self.validate_recur_tail(index, false);
            }
            ExprKind::List(items) | ExprKind::Vector(items) | ExprKind::Set(items) => {
                for item in items {
                    self.validate_recur_tail(item, false);
                }
            }
            ExprKind::Map(entries) => {
                for (key, value) in entries {
                    self.validate_recur_tail(key, false);
                    self.validate_recur_tail(value, false);
                }
            }
            ExprKind::Raise(None)
            | ExprKind::None
            | ExprKind::Bool(_)
            | ExprKind::Integer(_)
            | ExprKind::Float(_)
            | ExprKind::String(_)
            | ExprKind::Binding(_)
            | ExprKind::Error => {}
        }
    }

    /// Lower a `recur*` token as a non-returning call.  Treating it as
    /// `Never` lets ordinary `if`/`do` inference keep the branch's value type,
    /// while the runtime implementation returns a private state token that
    /// `loop` consumes.
    fn lower_recur(&mut self, call: &ast::CallExpr, span: Span, scope: &mut Scope) -> Expr {
        let values = call
            .positional
            .iter()
            .map(|value| self.lower_expr(value, scope))
            .collect::<Vec<_>>();
        if !call.keywords.is_empty() {
            self.error("OSR-T0023", "recur does not accept keyword arguments", span);
        }
        let loop_arity = self.loop_arities.last().copied();
        let owns_loop = loop_arity.is_some()
            && self.loop_callback_depths.last().copied() == Some(self.function_depth);
        let function_context = (!owns_loop)
            .then(|| {
                self.function_recur_contexts
                    .iter()
                    .rposition(|context| context.depth == self.function_depth)
            })
            .flatten();
        let (expected_arity, expected_types, function_context) = if owns_loop {
            (
                loop_arity.expect("loop arity exists when callback is owned"),
                self.loop_state_types.last().cloned().unwrap_or_default(),
                None,
            )
        } else if let Some(index) = function_context {
            let context = &self.function_recur_contexts[index];
            (
                context.state_types.len(),
                context.state_types.clone(),
                Some(index),
            )
        } else {
            self.error(
                "OSR-T0023",
                if loop_arity.is_some() {
                    "recur may only appear in the owning loop callback"
                } else {
                    "recur may only appear inside a loop or function body"
                },
                span,
            );
            return Expr::error(span);
        };
        if let Some(index) = function_context {
            self.function_recur_contexts[index].used = true;
        }
        if values.len() != expected_arity {
            self.error(
                "OSR-T0023",
                format!(
                    "recur expects {} value(s), found {}",
                    expected_arity,
                    values.len()
                ),
                span,
            );
            return Expr::error(span);
        }
        for (value, expected) in values.iter().zip(&expected_types) {
            self.check_assignable(&value.ty, expected, value.span);
        }
        let summaries = join_summaries(values.iter().map(|value| &value.summaries));
        let binding = self.ensure_core_recur_binding(span, expected_arity);
        let callee_type = Type::Fn(
            FunctionType::new(
                values.iter().map(|value| value.ty.clone()).collect(),
                Type::Never,
            )
            .with_summaries(CallSummaries::pure_scalar()),
        );
        Expr {
            span,
            ty: Type::Never,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(Expr::pure(span, callee_type, ExprKind::Binding(binding))),
                arguments: values.into_iter().map(CallArgument::Positional).collect(),
            },
        }
    }

    fn lower_trampoline(&mut self, call: &ast::CallExpr, span: Span, scope: &mut Scope) -> Expr {
        if !call.keywords.is_empty() || call.positional.is_empty() {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0024",
                "osiris.prelude/trampoline* expects a function and optional arguments",
                span,
            );
            return Expr::error(span);
        }
        let function = self.lower_expr(&call.positional[0], scope);
        let arguments = call.positional[1..]
            .iter()
            .map(|argument| self.lower_expr(argument, scope))
            .collect::<Vec<_>>();
        let result_type = match &function.ty {
            Type::Fn(signature) => {
                if signature.parameters.len() != arguments.len() {
                    self.error(
                        "OSR-T0024",
                        format!(
                            "trampoline function expects {} argument(s), found {}",
                            signature.parameters.len(),
                            arguments.len()
                        ),
                        function.span,
                    );
                }
                for (argument, parameter) in arguments.iter().zip(&signature.parameters) {
                    self.check_assignable(&argument.ty, parameter, argument.span);
                }
                if self.trampoline_has_invalid_bounce(&signature.return_type) {
                    self.error(
                        "OSR-T0024",
                        "trampoline bounce values must be zero-argument callables",
                        function.span,
                    );
                    Type::Error
                } else {
                    self.trampoline_result_type(&signature.return_type)
                }
            }
            Type::Any | Type::Unknown => Type::Any,
            _ => {
                self.error(
                    "OSR-T0024",
                    "trampoline function must be callable",
                    function.span,
                );
                Type::Error
            }
        };
        let summaries = arguments
            .iter()
            .fold(function.summaries.clone(), |summary, argument| {
                summary.join(&argument.summaries)
            });
        let binding = self.ensure_core_collection_binding("trampoline", span);
        let mut parameter_types = vec![function.ty.clone()];
        parameter_types.extend(arguments.iter().map(|argument| argument.ty.clone()));
        let callee = Expr::pure(
            span,
            Type::Fn(FunctionType::new(parameter_types, result_type.clone())),
            ExprKind::Binding(binding),
        );
        let mut lowered_arguments = vec![CallArgument::Positional(function)];
        lowered_arguments.extend(arguments.into_iter().map(CallArgument::Positional));
        Expr {
            span,
            ty: result_type,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: lowered_arguments,
            },
        }
    }

    fn trampoline_result_type(&self, ty: &Type) -> Type {
        match self.types.resolve(ty) {
            Type::Fn(signature) if signature.parameters.is_empty() => {
                self.trampoline_result_type(&signature.return_type)
            }
            Type::Option(inner) => Type::option(self.trampoline_result_type(&inner)),
            Type::Union(members) => Type::union(
                members
                    .iter()
                    .map(|member| self.trampoline_result_type(member)),
            ),
            other => other,
        }
    }

    /// A Python trampoline invokes every callable result with zero arguments.
    /// Reject a statically known callable that requires parameters instead of
    /// emitting code which would fail only after the first bounce. Dynamic
    /// `Any`/`Unknown` returns remain a deliberate runtime boundary.
    fn trampoline_has_invalid_bounce(&self, ty: &Type) -> bool {
        match self.types.resolve(ty) {
            Type::Fn(signature) => {
                !signature.parameters.is_empty()
                    || self.trampoline_has_invalid_bounce(&signature.return_type)
            }
            Type::Option(inner) => self.trampoline_has_invalid_bounce(&inner),
            Type::Union(members) => members
                .iter()
                .any(|member| self.trampoline_has_invalid_bounce(member)),
            _ => false,
        }
    }

    fn lower_lazy_seq(&mut self, call: &ast::CallExpr, span: Span, scope: &mut Scope) -> Expr {
        if !call.keywords.is_empty() || call.positional.len() != 1 {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0025",
                "osiris.prelude/lazy-seq* expects one zero-argument function",
                span,
            );
            return Expr::error(span);
        }
        let function = match &call.positional[0].kind {
            AstExprKind::Fn(function) => self.lower_lambda_with_expected_parameters(
                function,
                call.positional[0].span,
                scope,
                &[],
            ),
            _ => self.lower_expr(&call.positional[0], scope),
        };
        let Type::Fn(signature) = &function.ty else {
            self.error(
                "OSR-T0025",
                "lazy-seq thunk must be a function",
                function.span,
            );
            return Expr::error(span);
        };
        if !signature.parameters.is_empty() {
            self.error(
                "OSR-T0025",
                "lazy-seq thunk must accept no arguments",
                function.span,
            );
        }
        let binding = self.ensure_core_collection_binding("lazy_seq", span);
        let callee = Expr::pure(
            span,
            Type::Fn(FunctionType::new(vec![function.ty.clone()], Type::Any)),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: Type::Any,
            summaries: function.summaries.clone(),
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![CallArgument::Positional(function)],
            },
        }
    }

    /// Lower the Clojure-style `delay` macro target.  The thunk is lowered as
    /// an ordinary zero-argument lambda, while the result carries a nominal
    /// `Delay[T]` marker so `force` can recover the delayed value type without
    /// making every delayed expression an `Any` boundary.
    fn lower_delay(&mut self, call: &ast::CallExpr, span: Span, scope: &mut Scope) -> Expr {
        if !call.keywords.is_empty() || call.positional.len() != 1 {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0028",
                "osiris.prelude/delay* expects one zero-argument function",
                span,
            );
            return Expr::error(span);
        }
        let thunk = match &call.positional[0].kind {
            AstExprKind::Fn(function) => self.lower_lambda_with_expected_parameters(
                function,
                call.positional[0].span,
                scope,
                &[],
            ),
            _ => self.lower_expr(&call.positional[0], scope),
        };
        let Type::Fn(signature) = &thunk.ty else {
            self.error("OSR-T0028", "delay thunk must be a function", thunk.span);
            return Expr::error(span);
        };
        if !signature.parameters.is_empty() {
            self.error(
                "OSR-T0028",
                "delay thunk must accept no arguments",
                thunk.span,
            );
        }
        let value_type = (*signature.return_type).clone();
        let result_type = Type::Nominal {
            binding: core_delay_type_binding().as_str().to_owned(),
            args: vec![value_type],
        };
        let binding = self.ensure_core_collection_binding("delay", span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(vec![thunk.ty.clone()], result_type.clone())
                    .with_summaries(thunk.summaries.clone()),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: result_type,
            summaries: thunk.summaries.clone(),
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![CallArgument::Positional(thunk)],
            },
        }
    }

    /// `force`/`deref` accepts a Delay[T] and returns T.  For ordinary values
    /// the runtime helper is intentionally an identity function, matching
    /// Clojure's useful idempotent `deref` boundary for extension values.
    fn lower_force(&mut self, call: &ast::CallExpr, span: Span, scope: &mut Scope) -> Expr {
        if !call.keywords.is_empty() || call.positional.len() != 1 {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0029",
                "osiris.prelude/force* expects exactly one positional argument",
                span,
            );
            return Expr::error(span);
        }
        let value = self.lower_expr(&call.positional[0], scope);
        let result_type = match &value.ty {
            Type::Nominal { binding, args }
                if binding == core_delay_type_binding().as_str() && args.len() == 1 =>
            {
                args[0].clone()
            }
            Type::Unknown => Type::Any,
            other => other.clone(),
        };
        let binding = self.ensure_core_collection_binding("force", span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(vec![value.ty.clone()], result_type.clone())
                    .with_summaries(CallSummaries::pure_scalar()),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: result_type,
            summaries: value.summaries.clone(),
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![CallArgument::Positional(value)],
            },
        }
    }

    /// `deref` is the blocking boundary for delays, promises, and futures.
    /// Clojure's optional timeout/default pair is kept as ordinary arguments
    /// so the Python runtime can implement the wait without another AST node.
    fn lower_deref(&mut self, call: &ast::CallExpr, span: Span, scope: &mut Scope) -> Expr {
        if !call.keywords.is_empty() || !(call.positional.len() == 1 || call.positional.len() == 3)
        {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0034",
                "osiris.prelude/deref* expects one argument or value/timeout/default",
                span,
            );
            return Expr::error(span);
        }
        let values = call
            .positional
            .iter()
            .map(|value| self.lower_expr(value, scope))
            .collect::<Vec<_>>();
        if values.len() == 3
            && !matches!(
                values[1].ty,
                Type::Int | Type::Float | Type::Any | Type::Unknown
            )
        {
            self.error(
                "OSR-T0034",
                "deref timeout must be an Int or Float number of milliseconds",
                values[1].span,
            );
        }
        let mut result_type = async_value_type(&values[0].ty);
        if let Some(default) = values.get(2) {
            // A freshly-created Promise carries a type variable.  A concrete
            // timeout default is useful evidence for that variable (and keeps
            // the generated function annotation precise); otherwise preserve
            // the ordinary union behavior for known asynchronous values.
            if contains_type_variable(&result_type) {
                if self.types.unify(&result_type, &default.ty).is_ok() {
                    result_type = self.types.resolve(&result_type);
                } else {
                    result_type = self.types.join(&result_type, &default.ty);
                }
            } else {
                result_type = self.types.join(&result_type, &default.ty);
            }
        }
        let summaries = values
            .iter()
            .fold(CallSummaries::unknown(), |summary, value| {
                summary.join(&value.summaries)
            });
        let binding = self.ensure_core_collection_binding("deref", span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(
                    values.iter().map(|value| value.ty.clone()).collect(),
                    result_type.clone(),
                )
                .with_summaries(CallSummaries::unknown()),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: result_type,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: values.into_iter().map(CallArgument::Positional).collect(),
            },
        }
    }

    /// Lower the callback submitted by `future`/`future-call` and preserve its
    /// result type inside the synthetic `Future[T]` nominal marker.
    fn lower_future_call(&mut self, call: &ast::CallExpr, span: Span, scope: &mut Scope) -> Expr {
        if !call.keywords.is_empty() || call.positional.len() != 1 {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0035",
                "osiris.prelude/future-call* expects one zero-argument function",
                span,
            );
            return Expr::error(span);
        }
        let function = match &call.positional[0].kind {
            AstExprKind::Fn(function) => self.lower_lambda_with_expected_parameters(
                function,
                call.positional[0].span,
                scope,
                &[],
            ),
            _ => self.lower_expr(&call.positional[0], scope),
        };
        let result_type = match &function.ty {
            Type::Fn(signature) => {
                if !signature.parameters.is_empty() {
                    self.error(
                        "OSR-T0035",
                        "future-call function must accept no arguments",
                        function.span,
                    );
                }
                (*signature.return_type).clone()
            }
            Type::Any | Type::Unknown => Type::Any,
            _ => {
                self.error("OSR-T0035", "future-call expects a function", function.span);
                Type::Error
            }
        };
        let future_type = future_type(result_type);
        let binding = self.ensure_core_collection_binding("future_call", span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(vec![function.ty.clone()], future_type.clone())
                    .with_summaries(CallSummaries::unknown()),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: future_type,
            summaries: function.summaries.join(&CallSummaries::unknown()),
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![CallArgument::Positional(function)],
            },
        }
    }

    fn lower_future_predicate(
        &mut self,
        call: &ast::CallExpr,
        span: Span,
        scope: &mut Scope,
        runtime_name: &str,
    ) -> Expr {
        if !call.keywords.is_empty() || call.positional.len() != 1 {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0036",
                format!("osiris.prelude/{runtime_name} expects one argument"),
                span,
            );
            return Expr::error(span);
        }
        let value = self.lower_expr(&call.positional[0], scope);
        let binding = self.ensure_core_collection_binding(runtime_name, span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(vec![value.ty.clone()], Type::Bool)
                    .with_summaries(CallSummaries::unknown()),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: Type::Bool,
            summaries: value.summaries.join(&CallSummaries::unknown()),
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![CallArgument::Positional(value)],
            },
        }
    }

    fn lower_promise(&mut self, call: &ast::CallExpr, span: Span, scope: &mut Scope) -> Expr {
        if !call.keywords.is_empty() || !call.positional.is_empty() {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0037",
                "osiris.prelude/promise* does not accept arguments",
                span,
            );
            return Expr::error(span);
        }
        let result_type = promise_type(self.types.fresh_var());
        let binding = self.ensure_core_collection_binding("promise", span);
        let callee = Expr::pure(
            span,
            Type::Fn(FunctionType::new(Vec::new(), result_type.clone())),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: result_type,
            summaries: CallSummaries::unknown(),
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: Vec::new(),
            },
        }
    }

    fn lower_deliver(&mut self, call: &ast::CallExpr, span: Span, scope: &mut Scope) -> Expr {
        if !call.keywords.is_empty() || call.positional.len() != 2 {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0038",
                "osiris.prelude/deliver* expects a promise and a value",
                span,
            );
            return Expr::error(span);
        }
        let promise = self.lower_expr(&call.positional[0], scope);
        let value = self.lower_expr(&call.positional[1], scope);
        let result_type = match self.types.resolve(&promise.ty) {
            Type::Nominal { binding, args } if binding == core_promise_type_binding().as_str() => {
                let expected = args.first().cloned().unwrap_or(Type::Any);
                self.check_assignable(&value.ty, &expected, value.span);
                Type::Nominal {
                    binding: core_promise_type_binding().as_str().to_owned(),
                    args: vec![expected],
                }
            }
            Type::Any | Type::Unknown => promise_type(value.ty.clone()),
            _ => {
                self.error(
                    "OSR-T0038",
                    "deliver expects a Promise as its first argument",
                    promise.span,
                );
                promise_type(value.ty.clone())
            }
        };
        let binding = self.ensure_core_collection_binding("deliver", span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(
                    vec![promise.ty.clone(), value.ty.clone()],
                    result_type.clone(),
                )
                .with_summaries(CallSummaries::unknown()),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: result_type,
            summaries: promise
                .summaries
                .join(&value.summaries)
                .join(&CallSummaries::unknown()),
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![
                    CallArgument::Positional(promise),
                    CallArgument::Positional(value),
                ],
            },
        }
    }

    fn lower_lock(&mut self, call: &ast::CallExpr, span: Span, scope: &mut Scope) -> Expr {
        if !call.keywords.is_empty() || !call.positional.is_empty() {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0039",
                "osiris.prelude/lock* does not accept arguments",
                span,
            );
            return Expr::error(span);
        }
        let binding = self.ensure_core_collection_binding("lock", span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(Vec::new(), Type::Any).with_summaries(CallSummaries::unknown()),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: Type::Any,
            summaries: CallSummaries::unknown(),
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: Vec::new(),
            },
        }
    }

    fn lower_locking(&mut self, call: &ast::CallExpr, span: Span, scope: &mut Scope) -> Expr {
        if !call.keywords.is_empty() || call.positional.len() != 2 {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0040",
                "osiris.prelude/locking* expects a lock and zero-argument function",
                span,
            );
            return Expr::error(span);
        }
        let lock = self.lower_expr(&call.positional[0], scope);
        let function = match &call.positional[1].kind {
            AstExprKind::Fn(function) => self.lower_lambda_with_expected_parameters(
                function,
                call.positional[1].span,
                scope,
                &[],
            ),
            _ => self.lower_expr(&call.positional[1], scope),
        };
        let result_type = match &function.ty {
            Type::Fn(signature) => {
                if !signature.parameters.is_empty() {
                    self.error(
                        "OSR-T0040",
                        "locking body function must accept no arguments",
                        function.span,
                    );
                }
                (*signature.return_type).clone()
            }
            Type::Any | Type::Unknown => Type::Any,
            _ => {
                self.error(
                    "OSR-T0040",
                    "locking expects a zero-argument function body",
                    function.span,
                );
                Type::Error
            }
        };
        let binding = self.ensure_core_collection_binding("locking", span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(
                    vec![lock.ty.clone(), function.ty.clone()],
                    result_type.clone(),
                )
                .with_summaries(CallSummaries::unknown()),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: result_type,
            summaries: lock
                .summaries
                .join(&function.summaries)
                .join(&CallSummaries::unknown()),
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![
                    CallArgument::Positional(lock),
                    CallArgument::Positional(function),
                ],
            },
        }
    }

    /// Lower the Clojure-style `time` macro target without obscuring the
    /// measured expression's type or semantic summaries.  Reading the clock
    /// and reporting the duration adds one explicit I/O effect; temporal and
    /// data properties continue to describe the thunk result.
    fn lower_time(&mut self, call: &ast::CallExpr, span: Span, scope: &mut Scope) -> Expr {
        if !call.keywords.is_empty() || call.positional.len() != 1 {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0043",
                "osiris.prelude/time* expects one zero-argument function",
                span,
            );
            return Expr::error(span);
        }

        let function = match &call.positional[0].kind {
            AstExprKind::Fn(function) => self.lower_lambda_with_expected_parameters(
                function,
                call.positional[0].span,
                scope,
                &[],
            ),
            _ => self.lower_expr(&call.positional[0], scope),
        };
        let (result_type, body_summaries) = match &function.ty {
            Type::Fn(signature) => {
                if !signature.parameters.is_empty() {
                    self.error(
                        "OSR-T0043",
                        "time body function must accept no arguments",
                        function.span,
                    );
                }
                (
                    (*signature.return_type).clone(),
                    signature.summaries.clone(),
                )
            }
            Type::Any | Type::Unknown => (Type::Any, CallSummaries::unknown()),
            _ => {
                self.error(
                    "OSR-T0043",
                    "time expects a zero-argument function body",
                    function.span,
                );
                (Type::Error, CallSummaries::unknown())
            }
        };

        let mut summaries = function.summaries.join(&body_summaries);
        summaries.effects = summaries.effects.union(&EffectRow::singleton(Effect::Io));
        summaries.data = body_summaries.data.clone();
        let binding = self.ensure_core_collection_binding("time_value", span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(vec![function.ty.clone()], result_type.clone())
                    .with_summaries(summaries.clone()),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: result_type,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![CallArgument::Positional(function)],
            },
        }
    }

    fn lower_realized(&mut self, call: &ast::CallExpr, span: Span, scope: &mut Scope) -> Expr {
        if !call.keywords.is_empty() || call.positional.len() != 1 {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0030",
                "osiris.prelude/realized* expects exactly one positional argument",
                span,
            );
            return Expr::error(span);
        }
        let value = self.lower_expr(&call.positional[0], scope);
        let binding = self.ensure_core_collection_binding("realized", span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(vec![value.ty.clone()], Type::Bool)
                    .with_summaries(CallSummaries::pure_scalar()),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: Type::Bool,
            summaries: value.summaries.clone(),
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![CallArgument::Positional(value)],
            },
        }
    }

    fn lower_dynamic_binding(
        &mut self,
        call: &ast::CallExpr,
        span: Span,
        scope: &mut Scope,
    ) -> Expr {
        if !call.keywords.is_empty() || call.positional.len() != 2 {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0042",
                "osiris.prelude/binding* expects a binding vector and zero-argument body",
                span,
            );
            return Expr::error(span);
        }

        let AstExprKind::Vector(entries) = &call.positional[0].kind else {
            let _ = self.lower_expr(&call.positional[0], scope);
            let _ = self.lower_expr(&call.positional[1], scope);
            self.error(
                "OSR-T0042",
                "osiris.prelude/binding* expects a binding vector",
                call.positional[0].span,
            );
            return Expr::error(span);
        };
        if entries.len() % 2 != 0 {
            for entry in entries {
                let _ = self.lower_expr(entry, scope);
            }
            let _ = self.lower_expr(&call.positional[1], scope);
            self.error(
                "OSR-T0042",
                "binding requires dynamic Var/value pairs",
                call.positional[0].span,
            );
            return Expr::error(span);
        }

        // Binding values are simultaneous: evaluate every initializer once,
        // left-to-right, before installing any of the new dynamic values.
        scope.push();
        let mut initializers = Vec::new();
        let mut binding_ids = Vec::new();
        let mut values = Vec::new();
        let mut seen = BTreeSet::new();
        for pair in entries.chunks_exact(2) {
            let target_expression = &pair[0];
            let value = self.lower_expr(&pair[1], scope);
            let temporary_name = format!("\0dynamic-value{}", self.next_scope);
            let temporary_name = Name {
                spelling: temporary_name.clone(),
                canonical: temporary_name,
            };
            let temporary = self.declare_local(
                &temporary_name,
                BindingKind::Value,
                value.ty.clone(),
                Vec::new(),
                pair[1].span,
                scope,
            );
            let value_reference = Expr::pure(
                pair[1].span,
                value.ty.clone(),
                ExprKind::Binding(temporary.clone()),
            );
            initializers.push(LetBinding {
                binding: temporary,
                value,
            });

            let AstExprKind::Name(target_name) = &target_expression.kind else {
                self.error(
                    "OSR-T0042",
                    "binding targets must be dynamic top-level Value symbols",
                    target_expression.span,
                );
                continue;
            };
            if scope.resolve(&target_name.canonical).is_some() {
                self.error(
                    "OSR-T0042",
                    format!(
                        "binding target `{}` resolves to a local value, not a dynamic Var",
                        target_name.spelling
                    ),
                    target_expression.span,
                );
                continue;
            }
            let Some(target) = self.resolve_alias_target(&target_name.canonical) else {
                self.error(
                    "OSR-T0042",
                    format!("unknown binding target `{}`", target_name.spelling),
                    target_expression.span,
                );
                continue;
            };
            let Some(target_binding) = self.bindings.get(&target).cloned() else {
                continue;
            };
            if target_binding.name.kind != BindingKind::Value
                || !metadata_flag(&target_binding.metadata, "dynamic")
            {
                self.error(
                    "OSR-T0042",
                    format!(
                        "binding target `{}` is not a `^:dynamic` top-level Value",
                        target_name.spelling
                    ),
                    target_expression.span,
                );
                continue;
            }
            if !seen.insert(target.clone()) {
                self.error(
                    "OSR-T0042",
                    format!(
                        "dynamic Var `{}` is bound more than once",
                        target_name.spelling
                    ),
                    target_expression.span,
                );
                continue;
            }
            self.check_assignable(
                &value_reference.ty,
                &self.binding_type(&target),
                pair[1].span,
            );
            binding_ids.push(Expr::pure(
                target_expression.span,
                Type::Str,
                ExprKind::String(target.as_str().to_owned()),
            ));
            values.push(value_reference);
        }

        let function = match &call.positional[1].kind {
            AstExprKind::Fn(function) => self.lower_lambda_with_expected_parameters(
                function,
                call.positional[1].span,
                scope,
                &[],
            ),
            _ => self.lower_expr(&call.positional[1], scope),
        };
        scope.pop();

        let (result_type, body_summaries) = match &function.ty {
            Type::Fn(signature) => {
                if !signature.parameters.is_empty() {
                    self.error(
                        "OSR-T0042",
                        "binding body function must accept no arguments",
                        function.span,
                    );
                }
                (
                    (*signature.return_type).clone(),
                    signature.summaries.clone(),
                )
            }
            Type::Any | Type::Unknown => (Type::Any, CallSummaries::unknown()),
            _ => {
                self.error(
                    "OSR-T0042",
                    "binding expects a zero-argument function body",
                    function.span,
                );
                (Type::Error, CallSummaries::unknown())
            }
        };

        let id_vector = Expr::pure(
            call.positional[0].span,
            Type::Vector(Box::new(Type::Str)),
            ExprKind::Vector(binding_ids),
        );
        let value_item = self.types.join_all(values.iter().map(|value| &value.ty));
        let value_vector = Expr::pure(
            call.positional[0].span,
            Type::Vector(Box::new(if value_item == Type::Never {
                Type::Any
            } else {
                value_item
            })),
            ExprKind::Vector(values),
        );
        let mut summaries = function
            .summaries
            .join(&body_summaries)
            .join(&dynamic_state_summaries());
        summaries.data = body_summaries.data.clone();
        let runtime = self.ensure_core_collection_binding("binding_values", span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(
                    vec![
                        id_vector.ty.clone(),
                        value_vector.ty.clone(),
                        function.ty.clone(),
                    ],
                    result_type.clone(),
                )
                .with_summaries(summaries.clone()),
            ),
            ExprKind::Binding(runtime),
        );
        let call = Expr {
            span,
            ty: result_type,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![
                    CallArgument::Positional(id_vector),
                    CallArgument::Positional(value_vector),
                    CallArgument::Positional(function),
                ],
            },
        };
        self.wrap_let_bindings(initializers, call, span)
    }

    fn lower_close(&mut self, call: &ast::CallExpr, span: Span, scope: &mut Scope) -> Expr {
        if !call.keywords.is_empty() || call.positional.len() != 1 {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0031",
                "osiris.prelude/close* expects exactly one positional argument",
                span,
            );
            return Expr::error(span);
        }
        let value = self.lower_expr(&call.positional[0], scope);
        let binding = self.ensure_core_collection_binding("close", span);
        let callee = Expr::pure(
            span,
            Type::Fn(
                FunctionType::new(vec![value.ty.clone()], Type::None)
                    .with_summaries(CallSummaries::unknown()),
            ),
            ExprKind::Binding(binding),
        );
        Expr {
            span,
            ty: Type::None,
            summaries: value.summaries.join(&CallSummaries::unknown()),
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![CallArgument::Positional(value)],
            },
        }
    }

    /// Lower the compiler-owned target of the `letfn` surface macro.  All
    /// names are installed before any lambda body is lowered, which is the
    /// essential difference from an ordinary sequential `let`: self- and
    /// mutually-recursive local functions resolve to the same lexical frame.
    /// The resulting expression deliberately reuses `ExprKind::Let`; the
    /// backend already emits nested helper definitions before their binding
    /// assignments, preserving Python closure cells without a new runtime ABI.
    fn lower_letfn(&mut self, call: &ast::CallExpr, span: Span, scope: &mut Scope) -> Expr {
        if !call.keywords.is_empty() || call.positional.len() != 2 {
            for argument in &call.args {
                let value = match argument {
                    AstCallArg::Positional(value) => value,
                    AstCallArg::Keyword(argument) => &argument.value,
                };
                let _ = self.lower_expr(value, scope);
            }
            self.error(
                "OSR-T0032",
                "osiris.prelude/letfn* expects a binding vector and body",
                span,
            );
            return Expr::error(span);
        }

        let entries = match &call.positional[0].kind {
            AstExprKind::Vector(entries) => entries,
            _ => {
                self.error(
                    "OSR-T0032",
                    "osiris.prelude/letfn* expects a binding vector",
                    call.positional[0].span,
                );
                return Expr::error(span);
            }
        };
        if entries.len() % 2 != 0 {
            self.error(
                "OSR-T0032",
                "letfn bindings require name/function pairs",
                call.positional[0].span,
            );
        }

        // Keep the frame alive while lowering every function and the body.
        scope.push();
        let mut pending = Vec::new();
        for pair in entries.chunks(2) {
            let Some(name_expression) = pair.first() else {
                continue;
            };
            let Some(value_expression) = pair.get(1) else {
                continue;
            };
            let AstExprKind::Name(name) = &name_expression.kind else {
                self.error(
                    "OSR-T0032",
                    "letfn binding names must be symbols",
                    name_expression.span,
                );
                continue;
            };
            if !matches!(value_expression.kind, AstExprKind::Fn(_)) {
                self.error(
                    "OSR-T0032",
                    "letfn binding values must be fn expressions (multi-arity forms are not supported)",
                    value_expression.span,
                );
            }
            let binding = self.declare_local(
                name,
                BindingKind::Value,
                Type::Any,
                name_expression.metadata.clone(),
                name_expression.span,
                scope,
            );
            let expected_parameters = match &value_expression.kind {
                AstExprKind::Fn(function) => {
                    let mut parameters = Vec::with_capacity(function.params.len());
                    for parameter in &function.params {
                        let ty = match parameter.type_annotation.as_ref() {
                            Some(ty) => self.resolve_type_expr(ty),
                            None => self.types.fresh_var(),
                        };
                        parameters.push(ty);
                    }
                    let return_type = match function.return_type.as_ref() {
                        Some(ty) => self.resolve_type_expr(ty),
                        None => self.types.fresh_var(),
                    };
                    self.set_binding_type(
                        &binding,
                        Type::Fn(FunctionType::new(parameters.clone(), return_type)),
                    );
                    Some(parameters)
                }
                _ => None,
            };
            pending.push((binding, value_expression, expected_parameters));
        }

        let mut lowered_bindings = Vec::new();
        for (binding, value_expression, expected_parameters) in pending {
            let value = match (&value_expression.kind, expected_parameters.as_deref()) {
                (AstExprKind::Fn(function), Some(expected)) => self
                    .lower_lambda_with_expected_parameters(
                        function,
                        value_expression.span,
                        scope,
                        expected,
                    ),
                _ => self.lower_expr(value_expression, scope),
            };
            self.set_binding_type(&binding, value.ty.clone());
            lowered_bindings.push(LetBinding { binding, value });
        }
        let body = self.lower_expr(&call.positional[1], scope);
        scope.pop();

        let summaries = lowered_bindings
            .iter()
            .fold(body.summaries.clone(), |summary, binding| {
                summary.join(&binding.value.summaries)
            });
        Expr {
            span,
            ty: body.ty.clone(),
            summaries,
            kind: ExprKind::Let {
                bindings: lowered_bindings,
                body: Box::new(body),
            },
        }
    }

    fn ensure_core_loop_binding(&mut self, span: Span) -> BindingId {
        if let Some(binding) = &self.core_loop_binding {
            return binding.clone();
        }
        let id = BindingId::new("osiris.prelude", "loop", BindingKind::Function);
        let signature =
            FunctionType::new(Vec::new(), Type::Any).with_summaries(CallSummaries::unknown());
        self.bindings.insert(
            id.clone(),
            Binding {
                name: BindingName {
                    id: id.clone(),
                    canonical: "osiris.prelude/loop*".to_owned(),
                    python: "_u0_osiris_loop".to_owned(),
                    kind: BindingKind::Function,
                    span,
                },
                source_spelling: "osiris.prelude/loop*".to_owned(),
                ty: Type::Fn(signature),
                runtime: Some(RuntimeBinding {
                    module: "osiris.prelude".to_owned(),
                    name: "loop".to_owned(),
                    python_module: false,
                }),
                public: false,
                metadata: Vec::new(),
            },
        );
        self.core_loop_binding = Some(id.clone());
        id
    }

    fn ensure_core_recur_binding(&mut self, span: Span, arity: usize) -> BindingId {
        if let Some(binding) = &self.core_recur_binding {
            return binding.clone();
        }
        let id = BindingId::new("osiris.prelude", "recur", BindingKind::Function);
        let signature = FunctionType::new(vec![Type::Any; arity], Type::Never)
            .with_summaries(CallSummaries::pure_scalar());
        self.bindings.insert(
            id.clone(),
            Binding {
                name: BindingName {
                    id: id.clone(),
                    canonical: "osiris.prelude/recur*".to_owned(),
                    python: "_u0_osiris_recur".to_owned(),
                    kind: BindingKind::Function,
                    span,
                },
                source_spelling: "osiris.prelude/recur*".to_owned(),
                ty: Type::Fn(signature),
                runtime: Some(RuntimeBinding {
                    module: "osiris.prelude".to_owned(),
                    name: "recur".to_owned(),
                    python_module: false,
                }),
                public: false,
                metadata: Vec::new(),
            },
        );
        self.core_recur_binding = Some(id.clone());
        id
    }

    fn ensure_core_collection_binding(&mut self, name: &str, span: Span) -> BindingId {
        if let Some(binding) = self.core_collection_bindings.get(name) {
            return binding.clone();
        }
        let id = BindingId::new("osiris.prelude", name, BindingKind::Function);
        let signature =
            FunctionType::new(Vec::new(), Type::Any).with_summaries(CallSummaries::unknown());
        self.bindings.insert(
            id.clone(),
            Binding {
                name: BindingName {
                    id: id.clone(),
                    canonical: format!("osiris.prelude/{name}"),
                    python: format!("_u0_osiris_{name}"),
                    kind: BindingKind::Function,
                    span,
                },
                source_spelling: format!("osiris.prelude/{name}"),
                ty: Type::Fn(signature),
                runtime: Some(RuntimeBinding {
                    module: "osiris.prelude".to_owned(),
                    name: name.to_owned(),
                    python_module: false,
                }),
                public: false,
                metadata: Vec::new(),
            },
        );
        self.core_collection_bindings
            .insert(name.to_owned(), id.clone());
        id
    }

    fn install_core_reduced_type(&mut self, span: Span) {
        let id = core_reduced_type_binding();
        self.bindings.insert(
            id.clone(),
            Binding {
                name: BindingName {
                    id: id.clone(),
                    canonical: "Reduced".to_owned(),
                    python: "_u0_osiris_Reduced".to_owned(),
                    kind: BindingKind::Type,
                    span,
                },
                source_spelling: "Reduced".to_owned(),
                ty: Type::Nominal {
                    binding: id.as_str().to_owned(),
                    args: Vec::new(),
                },
                runtime: Some(RuntimeBinding {
                    module: "osiris.prelude".to_owned(),
                    name: "Reduced".to_owned(),
                    python_module: false,
                }),
                public: false,
                metadata: Vec::new(),
            },
        );
    }

    fn install_core_delay_type(&mut self, span: Span) {
        let id = core_delay_type_binding();
        self.bindings.insert(
            id.clone(),
            Binding {
                name: BindingName {
                    id: id.clone(),
                    canonical: "Delay".to_owned(),
                    python: "_u0_osiris_Delay".to_owned(),
                    kind: BindingKind::Type,
                    span,
                },
                source_spelling: "Delay".to_owned(),
                ty: Type::Nominal {
                    binding: id.as_str().to_owned(),
                    args: Vec::new(),
                },
                runtime: Some(RuntimeBinding {
                    module: "osiris.prelude".to_owned(),
                    name: "Delay".to_owned(),
                    python_module: false,
                }),
                public: false,
                metadata: Vec::new(),
            },
        );
    }

    fn install_core_future_type(&mut self, span: Span) {
        let id = core_future_type_binding();
        self.bindings.insert(
            id.clone(),
            Binding {
                name: BindingName {
                    id: id.clone(),
                    canonical: "Future".to_owned(),
                    python: "_u0_osiris_Future".to_owned(),
                    kind: BindingKind::Type,
                    span,
                },
                source_spelling: "Future".to_owned(),
                ty: Type::Nominal {
                    binding: id.as_str().to_owned(),
                    args: Vec::new(),
                },
                runtime: Some(RuntimeBinding {
                    module: "osiris.prelude".to_owned(),
                    name: "Future".to_owned(),
                    python_module: false,
                }),
                public: false,
                metadata: Vec::new(),
            },
        );
    }

    fn install_core_promise_type(&mut self, span: Span) {
        let id = core_promise_type_binding();
        self.bindings.insert(
            id.clone(),
            Binding {
                name: BindingName {
                    id: id.clone(),
                    canonical: "Promise".to_owned(),
                    python: "_u0_osiris_Promise".to_owned(),
                    kind: BindingKind::Type,
                    span,
                },
                source_spelling: "Promise".to_owned(),
                ty: Type::Nominal {
                    binding: id.as_str().to_owned(),
                    args: Vec::new(),
                },
                runtime: Some(RuntimeBinding {
                    module: "osiris.prelude".to_owned(),
                    name: "Promise".to_owned(),
                    python_module: false,
                }),
                public: false,
                metadata: Vec::new(),
            },
        );
    }

    fn ensure_core_mapv_binding(&mut self, span: Span) -> BindingId {
        if let Some(binding) = &self.core_mapv_binding {
            return binding.clone();
        }
        let id = BindingId::new("osiris.prelude", "mapv", BindingKind::Function);
        let signature = FunctionType::new(vec![Type::Any, Type::Any], Type::Any)
            .with_summaries(CallSummaries::unknown());
        self.bindings.insert(
            id.clone(),
            Binding {
                name: BindingName {
                    id: id.clone(),
                    canonical: "osiris.prelude/mapv".to_owned(),
                    // This internal name cannot be produced by authored source.
                    python: "_u0_osiris_mapv".to_owned(),
                    kind: BindingKind::Function,
                    span,
                },
                source_spelling: "osiris.prelude/mapv".to_owned(),
                ty: Type::Fn(signature),
                runtime: Some(RuntimeBinding {
                    module: "osiris.prelude".to_owned(),
                    name: "mapv".to_owned(),
                    python_module: false,
                }),
                public: false,
                metadata: Vec::new(),
            },
        );
        self.core_mapv_binding = Some(id.clone());
        id
    }

    fn lower_abs(&mut self, operand: &ast::Expr, span: Span, scope: &mut Scope) -> Expr {
        let operand = self.lower_expr(operand, scope);
        let selection =
            self.select_operator(ScalarOperator::Abs, std::slice::from_ref(&operand.ty));
        let choice = match selection {
            OperatorSelection::Selected(choice) => *choice,
            OperatorSelection::Ambiguous => {
                self.error(
                    "OSR-T0008",
                    "operator `abs` has multiple static implementations",
                    span,
                );
                return Expr::error(span);
            }
            OperatorSelection::None => {
                self.error(
                    "OSR-T0007",
                    format!("operator `abs` is not defined for `{}`", operand.ty),
                    span,
                );
                return Expr::error(span);
            }
        };
        if choice.binding.is_some() {
            return self.apply_operator_choice(Operator::Positive, vec![operand], span, choice);
        }

        // Core `abs` has no HIR operator variant because the backend's
        // exhaustive operator lowering intentionally remains unchanged.  A
        // synthetic binding makes the Python target a normal `builtins.abs`
        // call while retaining the same static signature and summaries.
        let binding = self.ensure_core_abs_binding(span);
        let callee_type = Type::Fn(
            FunctionType::new(vec![operand.ty.clone()], choice.result.clone())
                .with_summaries(choice.summaries.clone()),
        );
        let callee = Expr::pure(span, callee_type, ExprKind::Binding(binding));
        let summaries = operand.summaries.join(&choice.summaries);
        Expr {
            span,
            ty: choice.result,
            summaries,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                arguments: vec![CallArgument::Positional(operand)],
            },
        }
    }

    fn ensure_core_abs_binding(&mut self, span: Span) -> BindingId {
        if let Some(binding) = &self.core_abs_binding {
            return binding.clone();
        }
        let id = BindingId::new(&self.module_name, "__osiris_abs", BindingKind::Function);
        let name = BindingName {
            id: id.clone(),
            canonical: "__osiris_abs".to_owned(),
            python: "__osiris_abs".to_owned(),
            kind: BindingKind::Function,
            span,
        };
        self.bindings.insert(
            id.clone(),
            Binding {
                name,
                source_spelling: "abs".to_owned(),
                ty: Type::Fn(
                    FunctionType::new(vec![Type::Any], Type::Any)
                        .with_summaries(CallSummaries::pure_scalar()),
                ),
                runtime: Some(RuntimeBinding {
                    module: "builtins".to_owned(),
                    name: "abs".to_owned(),
                    python_module: false,
                }),
                public: false,
                metadata: Vec::new(),
            },
        );
        self.core_abs_binding = Some(id.clone());
        id
    }

    fn callable_for_expr(&self, expression: &Expr) -> Option<CallableInfo> {
        if let ExprKind::Binding(binding) = &expression.kind {
            if let Some(callable) = self.callables.get(binding) {
                return Some(callable.clone());
            }
        }
        let ExprKind::Lambda { parameters, .. } = &expression.kind else {
            return None;
        };
        let Type::Fn(signature) = &expression.ty else {
            return None;
        };
        let parameters = parameters
            .iter()
            .map(|parameter| {
                let binding = self.bindings.get(&parameter.binding)?;
                Some(CallableParameter {
                    canonical: binding.name.canonical.clone(),
                    accepted_names: parameter_names(
                        &Name {
                            spelling: binding.source_spelling.clone(),
                            canonical: binding.name.canonical.clone(),
                        },
                        &binding.metadata,
                    ),
                    ty: parameter.ty.clone(),
                    required: parameter.default.is_none() && !parameter.variadic,
                    variadic: parameter.variadic,
                    span: binding.name.span,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(CallableInfo {
            signature: signature.clone(),
            parameters,
            generic_variables: Vec::new(),
            contract_evidence: ContractEvidence::default(),
        })
    }

    fn instantiate_callable(&mut self, callable: &CallableInfo) -> CallableInfo {
        if callable.generic_variables.is_empty() {
            return callable.clone();
        }
        let substitutions = callable
            .generic_variables
            .iter()
            .copied()
            .map(|variable| (variable, self.types.fresh_var()))
            .collect::<BTreeMap<_, _>>();
        let mut instantiated = callable.clone();
        instantiated.signature.parameters = instantiated
            .signature
            .parameters
            .iter()
            .map(|ty| replace_type_variables(ty, &substitutions))
            .collect();
        instantiated.signature.return_type = Box::new(replace_type_variables(
            &instantiated.signature.return_type,
            &substitutions,
        ));
        for parameter in &mut instantiated.parameters {
            parameter.ty = replace_type_variables(&parameter.ty, &substitutions);
        }
        instantiated.generic_variables.clear();
        instantiated
    }

    fn validate_call(
        &mut self,
        callable: &CallableInfo,
        source_call: &ast::CallExpr,
        arguments: &[CallArgument],
        span: Span,
    ) {
        let mut assigned = vec![false; callable.parameters.len()];
        let mut positional_index = 0_usize;
        let mut saw_keyword = false;
        for (source, lowered) in source_call.args.iter().zip(arguments) {
            match (source, lowered) {
                (AstCallArg::Positional(_), CallArgument::Positional(value)) => {
                    if saw_keyword {
                        self.error(
                            "OSR-T0012",
                            "a positional argument cannot follow a keyword argument",
                            value.span,
                        );
                    }
                    let parameter_index = positional_index;
                    positional_index += 1;
                    let Some(last_parameter) = callable.parameters.last() else {
                        self.error("OSR-T0004", "too many positional arguments", value.span);
                        continue;
                    };
                    if parameter_index >= callable.parameters.len() && !last_parameter.variadic {
                        self.error("OSR-T0004", "too many positional arguments", value.span);
                        continue;
                    }
                    let actual_parameter_index = if parameter_index >= callable.parameters.len()
                        || callable.parameters[parameter_index].variadic
                    {
                        callable.parameters.len() - 1
                    } else {
                        parameter_index
                    };
                    let parameter = &callable.parameters[actual_parameter_index];
                    if !parameter.variadic {
                        if assigned[actual_parameter_index] {
                            self.error(
                                "OSR-T0009",
                                format!(
                                    "argument for `{}` was supplied more than once",
                                    parameter.canonical
                                ),
                                value.span,
                            );
                        }
                        assigned[actual_parameter_index] = true;
                    }
                    self.check_assignable(&value.ty, &parameter.ty, value.span);
                }
                (AstCallArg::Keyword(source_keyword), CallArgument::Keyword { value, .. }) => {
                    saw_keyword = true;
                    let source_name = source_keyword.key.canonical.trim_start_matches(':');
                    let Some((parameter_index, parameter)) = callable
                        .parameters
                        .iter()
                        .enumerate()
                        .find(|(_, parameter)| parameter.accepted_names.contains(source_name))
                    else {
                        self.error(
                            "OSR-T0008",
                            format!("unknown keyword argument `:{source_name}`"),
                            source_keyword.span,
                        );
                        continue;
                    };
                    if parameter.variadic {
                        self.error(
                            "OSR-T0008",
                            format!(
                                "variadic parameter `{}` cannot be passed by keyword",
                                parameter.canonical
                            ),
                            source_keyword.span,
                        );
                        continue;
                    }
                    if assigned[parameter_index] {
                        self.error(
                            "OSR-T0009",
                            format!(
                                "argument for `{}` was supplied more than once",
                                parameter.canonical
                            ),
                            source_keyword.span,
                        );
                    }
                    assigned[parameter_index] = true;
                    self.check_assignable(&value.ty, &parameter.ty, value.span);
                }
                _ => {
                    self.error("OSR-H0005", "internal call lowering mismatch", span);
                }
            }
        }
        for (index, parameter) in callable.parameters.iter().enumerate() {
            if parameter.required && !assigned[index] {
                self.error(
                    "OSR-T0010",
                    format!("missing required argument `{}`", parameter.canonical),
                    parameter.span,
                );
            }
        }
    }

    fn require_pure(&mut self, expression: &Expr, context: &str) {
        if !expression.summaries.effects.effects.is_empty() || expression.summaries.effects.open {
            self.error(
                "OSR-T0013",
                format!("{context} must be pure"),
                expression.span,
            );
        }
    }

    fn record_contract_evidence(&mut self, evidence: &ContractEvidence) {
        if let Some(current) = self.contract_evidence_stack.last_mut() {
            *current = current.join(evidence);
        }
    }

    fn validate_causal_function(
        &mut self,
        name: &str,
        summaries: &CallSummaries,
        evidence: &ContractEvidence,
        requirement: &CausalRequirement,
        span: Span,
    ) {
        for fact in evidence.unverified() {
            let identity = fact.contract_id.as_deref().unwrap_or(&fact.binding);
            self.error(
                "OSR-C0001",
                format!(
                    "causal function `{name}` depends on untrusted declared contract `{identity}` from `{}`",
                    fact.provider_module
                ),
                span,
            );
        }
        match summaries.temporal.future {
            TemporalBound::Finite(0) => {}
            TemporalBound::Finite(value) => self.error(
                "OSR-C0002",
                format!("causal function `{name}` reads {value} step(s) into the future"),
                span,
            ),
            _ => self.error(
                "OSR-C0002",
                format!("causal function `{name}` has an unproved future bound"),
                span,
            ),
        }
        match &summaries.temporal.availability {
            Availability::Immediate => {}
            Availability::Named(actual)
                if requirement.decision_point.as_deref() == Some(actual.as_str()) => {}
            Availability::Named(actual) => self.error(
                "OSR-C0003",
                format!(
                    "causal function `{name}` cannot prove availability `{actual}` at its decision point"
                ),
                span,
            ),
            Availability::Unknown => self.error(
                "OSR-C0003",
                format!("causal function `{name}` has unknown data availability"),
                span,
            ),
        }
    }

    fn lower_operator(
        &mut self,
        operator: Operator,
        operands: &[ast::Expr],
        span: Span,
        scope: &mut Scope,
    ) -> Expr {
        let operands = operands
            .iter()
            .map(|operand| self.lower_expr(operand, scope))
            .collect::<Vec<_>>();
        let summaries = join_summaries(operands.iter().map(|operand| &operand.summaries));
        match operator {
            Operator::And | Operator::Or => {
                if operands.len() < 2 {
                    self.error(
                        "OSR-T0006",
                        "boolean operator expects at least two operands",
                        span,
                    );
                }
                for operand in &operands {
                    self.check_assignable(&operand.ty, &Type::Bool, operand.span);
                }
                Expr {
                    span,
                    ty: Type::Bool,
                    summaries,
                    kind: ExprKind::Operator { operator, operands },
                }
            }
            Operator::Not => {
                if operands.len() != 1 {
                    self.error("OSR-T0006", "not expects one operand", span);
                }
                if let Some(operand) = operands.first() {
                    self.check_assignable(&operand.ty, &Type::Bool, operand.span);
                }
                Expr {
                    span,
                    ty: Type::Bool,
                    summaries,
                    kind: ExprKind::Operator { operator, operands },
                }
            }
            _ => self.lower_scalar_operator(operator, operands, summaries, span),
        }
    }

    fn lower_scalar_operator(
        &mut self,
        operator: Operator,
        operands: Vec<Expr>,
        summaries: CallSummaries,
        span: Span,
    ) -> Expr {
        let Some(scalar) = operator.scalar() else {
            return Expr::error(span);
        };
        let unary = matches!(operator, Operator::Negate | Operator::Positive);
        let comparison = matches!(
            operator,
            Operator::Equal
                | Operator::NotEqual
                | Operator::Less
                | Operator::LessEqual
                | Operator::Greater
                | Operator::GreaterEqual
        );
        if (unary && operands.len() != 1) || (!unary && operands.len() < 2) {
            self.error("OSR-T0006", "operator has invalid arity", span);
            return Expr::error(span);
        }

        if unary {
            let selection = self.select_operator(scalar, std::slice::from_ref(&operands[0].ty));
            return match selection {
                OperatorSelection::Selected(choice) => {
                    self.apply_operator_choice(operator, operands, span, *choice)
                }
                OperatorSelection::Ambiguous => {
                    self.error(
                        "OSR-T0008",
                        format!(
                            "operator `{}` has multiple static implementations",
                            scalar.stable_name()
                        ),
                        span,
                    );
                    Expr::error(span)
                }
                OperatorSelection::None => {
                    self.error(
                        "OSR-T0007",
                        format!("operator is not defined for `{}`", operands[0].ty),
                        span,
                    );
                    Expr::error(span)
                }
            };
        }

        let mut current_type = operands[0].ty.clone();
        let mut choices = Vec::with_capacity(operands.len() - 1);
        for operand in &operands[1..] {
            let pair = [current_type.clone(), operand.ty.clone()];
            let selection = self.select_operator(scalar, &pair);
            let choice = match selection {
                OperatorSelection::Selected(choice) => *choice,
                OperatorSelection::Ambiguous => {
                    self.error(
                        "OSR-T0008",
                        format!(
                            "operator `{}` has multiple static implementations",
                            scalar.stable_name()
                        ),
                        span,
                    );
                    return Expr::error(span);
                }
                OperatorSelection::None => {
                    self.error(
                        "OSR-T0007",
                        format!(
                            "operator is not defined for `{}` and `{}`",
                            current_type, operand.ty
                        ),
                        span,
                    );
                    return Expr::error(span);
                }
            };
            current_type = if comparison {
                operand.ty.clone()
            } else {
                choice.result.clone()
            };
            choices.push(choice);
        }

        // Keep the compact n-ary core operator representation when no
        // extension capability was selected.  Imported instances are calls to
        // their declared binding, lowered left-to-right so every operand is
        // evaluated once and each selected summary is retained.
        if choices.iter().all(|choice| choice.binding.is_none()) {
            return Expr {
                span,
                ty: if comparison { Type::Bool } else { current_type },
                summaries,
                kind: ExprKind::Operator { operator, operands },
            };
        }

        let mut operand_iter = operands.into_iter();
        let mut current = operand_iter.next().unwrap_or_else(|| Expr::error(span));
        for (choice, operand) in choices.into_iter().zip(operand_iter) {
            current = self.apply_operator_choice(operator, vec![current, operand], span, choice);
        }
        current
    }

    fn select_operator(&self, operator: ScalarOperator, operands: &[Type]) -> OperatorSelection {
        let mut imported = self
            .operator_instances
            .values()
            .filter(|instance| instance.operator == operator)
            .filter_map(|instance| {
                if instance.operands.len() != operands.len()
                    || operands.iter().any(is_dynamic_operator_type)
                {
                    return None;
                }
                let mut variables = BTreeMap::new();
                if !operands
                    .iter()
                    .zip(&instance.operands)
                    .all(|(actual, expected)| {
                        operator_type_matches(&self.types, actual, expected, &mut variables)
                    })
                {
                    return None;
                }
                if contains_unresolved_operator_variable(&instance.result, &variables) {
                    return None;
                }
                let result = replace_type_variables(&instance.result, &variables);
                Some(OperatorChoice {
                    result,
                    summaries: instance.summaries.clone(),
                    binding: Some(BindingId::from_interface(instance.binding.clone())),
                    contract_evidence: self
                        .operator_contract_evidence
                        .get(&instance.id)
                        .cloned()
                        .unwrap_or_default(),
                })
            })
            .collect::<Vec<_>>();
        if imported.len() > 1 {
            return OperatorSelection::Ambiguous;
        }
        if let Some(choice) = imported.pop() {
            return OperatorSelection::Selected(Box::new(choice));
        }

        let signatures = scalar_operator_signatures(operator);
        select_operator_signature(&self.types, &signatures, operands).map_or(
            OperatorSelection::None,
            |signature| {
                OperatorSelection::Selected(Box::new(OperatorChoice {
                    result: signature.result.clone(),
                    summaries: signature.summaries.clone(),
                    binding: None,
                    contract_evidence: ContractEvidence::default(),
                }))
            },
        )
    }

    fn apply_operator_choice(
        &mut self,
        operator: Operator,
        operands: Vec<Expr>,
        span: Span,
        choice: OperatorChoice,
    ) -> Expr {
        self.record_contract_evidence(&choice.contract_evidence);
        let summaries = join_summaries(operands.iter().map(|operand| &operand.summaries))
            .join(&choice.summaries);
        if let Some(binding) = choice.binding {
            let callee_type = Type::Fn(
                FunctionType::new(
                    operands.iter().map(|operand| operand.ty.clone()).collect(),
                    choice.result.clone(),
                )
                .with_summaries(choice.summaries.clone()),
            );
            let callee = Expr::pure(span, callee_type, ExprKind::Binding(binding));
            return Expr {
                span,
                ty: choice.result,
                summaries,
                kind: ExprKind::Call {
                    callee: Box::new(callee),
                    arguments: operands.into_iter().map(CallArgument::Positional).collect(),
                },
            };
        }
        Expr {
            span,
            ty: choice.result,
            summaries,
            kind: ExprKind::Operator { operator, operands },
        }
    }

    fn lower_lambda(&mut self, function: &ast::FnExpr, span: Span, outer: &mut Scope) -> Expr {
        self.lower_lambda_with_expected_parameters(function, span, outer, &[])
    }

    fn lower_lambda_with_expected_parameters(
        &mut self,
        function: &ast::FnExpr,
        span: Span,
        outer: &mut Scope,
        expected: &[Type],
    ) -> Expr {
        self.function_depth += 1;
        outer.push();
        let mut parameters = Vec::new();
        let mut parameter_bindings = Vec::new();
        for (index, parameter) in function.params.iter().enumerate() {
            let ty = if let Some(annotation) = &parameter.type_annotation {
                self.resolve_type_expr(annotation)
            } else {
                expected
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| self.types.fresh_var())
            };
            if parameter.type_annotation.is_some()
                && let Some(expected) = expected.get(index)
            {
                self.check_assignable(expected, &ty, parameter.span);
            }
            let default = parameter
                .default
                .as_ref()
                .map(|value| self.lower_expr(value, outer));
            let binding = self.declare_local(
                &parameter.name,
                BindingKind::Parameter,
                ty.clone(),
                parameter.metadata.clone(),
                parameter.span,
                outer,
            );
            parameters.push(Parameter {
                binding: binding.clone(),
                ty,
                default,
                variadic: parameter.variadic,
            });
            if let Some(pattern) = &parameter.pattern {
                let value = Expr::pure(
                    parameter.span,
                    self.binding_type(&binding),
                    ExprKind::Binding(binding),
                );
                self.lower_pattern_bindings(
                    pattern,
                    value,
                    &pattern.metadata,
                    outer,
                    &mut parameter_bindings,
                );
            }
        }
        let state_types = parameters
            .iter()
            .map(|parameter| parameter.ty.clone())
            .collect::<Vec<_>>();
        self.function_recur_contexts.push(FunctionRecurContext {
            depth: self.function_depth,
            state_types,
            used: false,
        });
        let body = self.lower_body(&function.body, outer, span);
        let function_recur = self
            .function_recur_contexts
            .pop()
            .expect("lambda recur context");
        let body = self.wrap_let_bindings(parameter_bindings, body, span);
        let body = if function_recur.used {
            self.validate_recur_tail(&body, true);
            self.wrap_function_recur(&parameters, body, span)
        } else {
            body
        };
        outer.pop();
        self.function_depth = self.function_depth.saturating_sub(1);
        for parameter in &mut parameters {
            parameter.ty = self.types.resolve(&parameter.ty);
        }
        let return_type = function.return_type.as_ref().map_or_else(
            || body.ty.clone(),
            |annotation| {
                let annotation = self.resolve_type_expr(annotation);
                self.check_assignable(&body.ty, &annotation, body.span);
                annotation
            },
        );
        let function_type = Type::Fn(
            FunctionType::new(
                parameters
                    .iter()
                    .map(|parameter| parameter.ty.clone())
                    .collect(),
                return_type,
            )
            .with_summaries(body.summaries.clone()),
        );
        Expr::pure(
            span,
            function_type,
            ExprKind::Lambda {
                parameters,
                body: Box::new(body),
            },
        )
    }

    fn lower_try(&mut self, expression: &ast::TryExpr, span: Span, scope: &mut Scope) -> Expr {
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

    fn declare_local(
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

    fn resolve_global_name(&self, name: &str) -> Option<BindingId> {
        self.globals.get(name).cloned()
    }

    fn resolve_alias_target(&self, name: &str) -> Option<BindingId> {
        self.resolve_global_name(name)
            .or_else(|| self.qualified_imports.get(name).cloned())
    }

    fn global_id(&self, name: &Name) -> Option<BindingId> {
        self.resolve_global_name(&name.canonical)
    }

    fn binding_type(&self, id: &BindingId) -> Type {
        self.bindings
            .get(id)
            .map_or(Type::Error, |binding| self.types.resolve(&binding.ty))
    }

    fn binding_is_dynamic(&self, id: &BindingId) -> bool {
        self.bindings.get(id).is_some_and(|binding| {
            binding.name.kind == BindingKind::Value && metadata_flag(&binding.metadata, "dynamic")
        })
    }

    fn set_binding_type(&mut self, id: &BindingId, ty: Type) {
        if let Some(binding) = self.bindings.get_mut(id) {
            binding.ty = self.types.resolve(&ty);
        }
    }

    fn check_assignable(&mut self, actual: &Type, expected: &Type, span: Span) {
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

    fn error(&mut self, code: &'static str, message: impl Into<String>, span: Span) {
        self.diagnostics
            .push(Diagnostic::error(code, message, span));
    }
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

fn metadata_flag(metadata: &[MetadataEntry], expected: &str) -> bool {
    metadata.iter().any(|entry| {
        matches!(
            &entry.key.kind,
            FormKind::Keyword(key) | FormKind::Symbol(key)
                if key.canonical.trim_start_matches(':') == expected
        ) && matches!(entry.value.kind, FormKind::Bool(true))
    })
}

fn dynamic_state_summaries() -> CallSummaries {
    CallSummaries {
        effects: EffectRow::singleton(Effect::HiddenState),
        ..CallSummaries::pure_scalar()
    }
}

fn causal_requirement(metadata: &[MetadataEntry]) -> Result<Option<CausalRequirement>, String> {
    let mut value = None;
    for entry in metadata {
        let key = match &entry.key.kind {
            FormKind::Keyword(name) | FormKind::Symbol(name) => {
                name.canonical.trim_start_matches(':')
            }
            _ => continue,
        };
        if key != "osiris/causal" {
            continue;
        }
        if value.is_some() {
            return Err("`:osiris/causal` metadata is duplicated".to_owned());
        }
        value = Some(&entry.value);
    }
    let Some(value) = value else {
        return Ok(None);
    };
    match &value.kind {
        FormKind::Bool(false) => Ok(None),
        FormKind::Bool(true) => Ok(Some(CausalRequirement {
            decision_point: None,
        })),
        FormKind::Map(entries) if entries.len() == 2 => {
            let key = form_keyword_or_symbol(&entries[0]).map(|key| key.trim_start_matches(':'));
            if key != Some("decision-point") {
                return Err("`:osiris/causal` map requires exactly `:decision-point`".to_owned());
            }
            let decision_point = match &entries[1].kind {
                FormKind::Keyword(name) => name.canonical.trim_start_matches(':').to_owned(),
                FormKind::Symbol(name) => name.canonical.clone(),
                FormKind::String(value) => value.clone(),
                _ => {
                    return Err("causal `:decision-point` must be a static name".to_owned());
                }
            };
            if decision_point.is_empty() {
                return Err("causal `:decision-point` must not be empty".to_owned());
            }
            Ok(Some(CausalRequirement {
                decision_point: Some(decision_point),
            }))
        }
        _ => Err("`:osiris/causal` must be Bool or `{:decision-point <static-name>}`".to_owned()),
    }
}

fn parameter_names(name: &Name, metadata: &[MetadataEntry]) -> BTreeSet<String> {
    let mut names = BTreeSet::from([name.canonical.clone()]);
    for entry in metadata {
        let is_names_metadata = match &entry.key.kind {
            FormKind::Keyword(key) | FormKind::Symbol(key) => {
                key.canonical.trim_start_matches(':') == "osiris/names"
            }
            _ => false,
        };
        if is_names_metadata {
            collect_parameter_names(&entry.value, &mut names);
        }
    }
    names
}

fn find_imported_binding<'a>(interface: &'a Interface, name: &str) -> Option<&'a PublicBinding> {
    if let Some(binding) = interface
        .bindings
        .iter()
        .find(|binding| binding.canonical == name || binding.id == name)
    {
        return Some(binding);
    }
    let alias = interface
        .aliases
        .iter()
        .find(|alias| alias.canonical == name || alias.spelling == name)?;
    interface
        .bindings
        .iter()
        .find(|binding| binding.id == alias.target)
}

fn alias_target_canonical(interface: &Interface, alias: &crate::interface::PublicAlias) -> String {
    interface
        .bindings
        .iter()
        .find(|binding| binding.id == alias.target)
        .map_or_else(|| alias.target.clone(), |binding| binding.canonical.clone())
}

fn requested_alias_key(
    requested: &BTreeSet<String>,
    alias: &crate::interface::PublicAlias,
) -> String {
    if requested.contains(&alias.canonical) {
        alias.canonical.clone()
    } else {
        alias.spelling.clone()
    }
}

fn member_span(_member: &Name, fallback: Span) -> Span {
    // Surface `Name` intentionally carries no independent span; the import
    // declaration span is the closest stable diagnostic location.
    fallback
}

fn interface_parameter_names(parameter: &crate::interface::ParameterInterface) -> BTreeSet<String> {
    std::iter::once(parameter.canonical.clone())
        .chain(parameter.aliases.iter().cloned())
        .collect()
}

fn interface_field_names(field: &crate::interface::FieldInterface) -> BTreeSet<String> {
    std::iter::once(field.canonical.clone())
        .chain(field.aliases.iter().cloned())
        .collect()
}

fn import_type_with_variables(
    context: &mut TypeContext,
    ty: &Type,
    variables: &mut BTreeMap<TypeVarId, Type>,
) -> Type {
    match ty {
        Type::TypeVar(variable) => variables
            .entry(*variable)
            .or_insert_with(|| context.fresh_var())
            .clone(),
        Type::Option(inner) => Type::option(import_type_with_variables(context, inner, variables)),
        Type::Union(members) => Type::union(
            members
                .iter()
                .map(|member| import_type_with_variables(context, member, variables)),
        ),
        Type::Tuple(members) => Type::Tuple(
            members
                .iter()
                .map(|member| import_type_with_variables(context, member, variables))
                .collect(),
        ),
        Type::List(item) => Type::List(Box::new(import_type_with_variables(
            context, item, variables,
        ))),
        Type::Vector(item) => Type::Vector(Box::new(import_type_with_variables(
            context, item, variables,
        ))),
        Type::Map(key, value) => Type::Map(
            Box::new(import_type_with_variables(context, key, variables)),
            Box::new(import_type_with_variables(context, value, variables)),
        ),
        Type::Set(item) => Type::Set(Box::new(import_type_with_variables(
            context, item, variables,
        ))),
        Type::Fn(function) => Type::Fn(
            FunctionType::new(
                function
                    .parameters
                    .iter()
                    .map(|parameter| import_type_with_variables(context, parameter, variables))
                    .collect(),
                import_type_with_variables(context, &function.return_type, variables),
            )
            .with_summaries(function.summaries.clone()),
        ),
        Type::Nominal { binding, args } => Type::Nominal {
            binding: binding.clone(),
            args: args
                .iter()
                .map(|argument| import_type_with_variables(context, argument, variables))
                .collect(),
        },
        other => other.clone(),
    }
}

fn resolve_function_nominal_bindings(
    function: &FunctionType,
    resolutions: &BTreeMap<String, String>,
    fallback_module: &str,
) -> FunctionType {
    FunctionType::new(
        function
            .parameters
            .iter()
            .map(|parameter| resolve_nominal_bindings(parameter, resolutions, fallback_module))
            .collect(),
        resolve_nominal_bindings(&function.return_type, resolutions, fallback_module),
    )
    .with_summaries(function.summaries.clone())
}

fn collect_unresolved_nominal_bindings(
    ty: &Type,
    resolutions: &BTreeMap<String, String>,
    unknown: &mut BTreeSet<String>,
) {
    match ty {
        Type::Option(inner) | Type::List(inner) | Type::Vector(inner) | Type::Set(inner) => {
            collect_unresolved_nominal_bindings(inner, resolutions, unknown);
        }
        Type::Union(members) | Type::Tuple(members) => {
            for member in members {
                collect_unresolved_nominal_bindings(member, resolutions, unknown);
            }
        }
        Type::Map(key, value) => {
            collect_unresolved_nominal_bindings(key, resolutions, unknown);
            collect_unresolved_nominal_bindings(value, resolutions, unknown);
        }
        Type::Fn(function) => {
            for parameter in &function.parameters {
                collect_unresolved_nominal_bindings(parameter, resolutions, unknown);
            }
            collect_unresolved_nominal_bindings(&function.return_type, resolutions, unknown);
        }
        Type::Nominal { binding, args } => {
            if !binding.contains("::type::") && !resolutions.contains_key(binding) {
                unknown.insert(binding.clone());
            }
            for argument in args {
                collect_unresolved_nominal_bindings(argument, resolutions, unknown);
            }
        }
        Type::Bool
        | Type::Int
        | Type::Float
        | Type::Str
        | Type::Bytes
        | Type::None
        | Type::Any
        | Type::Never
        | Type::Unknown
        | Type::Error
        | Type::Literal(_)
        | Type::TypeVar(_) => {}
    }
}

pub(crate) fn resolve_nominal_bindings(
    ty: &Type,
    resolutions: &BTreeMap<String, String>,
    fallback_module: &str,
) -> Type {
    match ty {
        Type::Option(inner) => Type::option(resolve_nominal_bindings(
            inner,
            resolutions,
            fallback_module,
        )),
        Type::Union(members) => Type::union(
            members
                .iter()
                .map(|member| resolve_nominal_bindings(member, resolutions, fallback_module)),
        ),
        Type::Tuple(members) => Type::Tuple(
            members
                .iter()
                .map(|member| resolve_nominal_bindings(member, resolutions, fallback_module))
                .collect(),
        ),
        Type::List(item) => Type::List(Box::new(resolve_nominal_bindings(
            item,
            resolutions,
            fallback_module,
        ))),
        Type::Vector(item) => Type::Vector(Box::new(resolve_nominal_bindings(
            item,
            resolutions,
            fallback_module,
        ))),
        Type::Map(key, value) => Type::Map(
            Box::new(resolve_nominal_bindings(key, resolutions, fallback_module)),
            Box::new(resolve_nominal_bindings(
                value,
                resolutions,
                fallback_module,
            )),
        ),
        Type::Set(item) => Type::Set(Box::new(resolve_nominal_bindings(
            item,
            resolutions,
            fallback_module,
        ))),
        Type::Fn(function) => Type::Fn(resolve_function_nominal_bindings(
            function,
            resolutions,
            fallback_module,
        )),
        Type::Nominal { binding, args } => {
            let binding = if binding.contains("::type::") {
                binding.clone()
            } else {
                match resolutions.get(binding) {
                    Some(resolved) => resolved.clone(),
                    None if fallback_module.is_empty() => binding.clone(),
                    None => return Type::Error,
                }
            };
            Type::Nominal {
                binding,
                args: args
                    .iter()
                    .map(|argument| {
                        resolve_nominal_bindings(argument, resolutions, fallback_module)
                    })
                    .collect(),
            }
        }
        other => other.clone(),
    }
}

fn collect_parameter_names(form: &Form, names: &mut BTreeSet<String>) {
    let FormKind::Map(entries) = &form.kind else {
        return;
    };
    for pair in entries.chunks_exact(2) {
        let Some(key) = pair.first().and_then(form_keyword_or_symbol) else {
            if let Some(value) = pair.get(1) {
                collect_parameter_names(value, names);
            }
            continue;
        };
        match key.trim_start_matches(':') {
            "preferred" => {
                if let Some(name) = form_name_value(&pair[1]) {
                    names.insert(name);
                }
            }
            "aliases" => {
                if let FormKind::Vector(values) = &pair[1].kind {
                    for value in values {
                        if let Some(name) = form_name_value(value) {
                            names.insert(name);
                        }
                    }
                }
            }
            // Locale keys contain a nested name descriptor.  Recurse so the
            // same parser handles both a single locale and a future map of
            // locale descriptors without hard-coding locale names.
            _ => collect_parameter_names(&pair[1], names),
        }
    }
}

fn form_keyword_or_symbol(form: &Form) -> Option<&str> {
    match &form.kind {
        FormKind::Keyword(name) | FormKind::Symbol(name) => Some(name.canonical.as_str()),
        _ => None,
    }
}

fn form_name_value(form: &Form) -> Option<String> {
    match &form.kind {
        FormKind::Symbol(name) => Some(name.canonical.clone()),
        _ => None,
    }
}

pub(crate) fn type_from_ast(expression: &ast::TypeExpr) -> Type {
    type_from_ast_with_generics(expression, &BTreeMap::new())
}

fn replace_type_variables(ty: &Type, substitutions: &BTreeMap<TypeVarId, Type>) -> Type {
    match ty {
        Type::TypeVar(variable) => substitutions.get(variable).map_or_else(
            || ty.clone(),
            |replacement| replace_type_variables(replacement, substitutions),
        ),
        Type::Option(inner) => Type::option(replace_type_variables(inner, substitutions)),
        Type::Union(members) => Type::union(
            members
                .iter()
                .map(|member| replace_type_variables(member, substitutions)),
        ),
        Type::Tuple(members) => Type::Tuple(
            members
                .iter()
                .map(|member| replace_type_variables(member, substitutions))
                .collect(),
        ),
        Type::List(item) => Type::List(Box::new(replace_type_variables(item, substitutions))),
        Type::Vector(item) => Type::Vector(Box::new(replace_type_variables(item, substitutions))),
        Type::Map(key, value) => Type::Map(
            Box::new(replace_type_variables(key, substitutions)),
            Box::new(replace_type_variables(value, substitutions)),
        ),
        Type::Set(item) => Type::Set(Box::new(replace_type_variables(item, substitutions))),
        Type::Fn(function) => Type::Fn(
            FunctionType::new(
                function
                    .parameters
                    .iter()
                    .map(|parameter| replace_type_variables(parameter, substitutions))
                    .collect(),
                replace_type_variables(&function.return_type, substitutions),
            )
            .with_summaries(function.summaries.clone()),
        ),
        Type::Nominal { binding, args } => Type::Nominal {
            binding: binding.clone(),
            args: args
                .iter()
                .map(|argument| replace_type_variables(argument, substitutions))
                .collect(),
        },
        _ => ty.clone(),
    }
}

fn contains_type_variable(ty: &Type) -> bool {
    match ty {
        Type::TypeVar(_) => true,
        Type::Option(inner) | Type::List(inner) | Type::Vector(inner) | Type::Set(inner) => {
            contains_type_variable(inner)
        }
        Type::Union(members) | Type::Tuple(members) => members.iter().any(contains_type_variable),
        Type::Map(key, value) => contains_type_variable(key) || contains_type_variable(value),
        Type::Fn(function) => {
            function.parameters.iter().any(contains_type_variable)
                || contains_type_variable(&function.return_type)
        }
        Type::Nominal { args, .. } => args.iter().any(contains_type_variable),
        _ => false,
    }
}

pub(crate) fn type_from_ast_with_generics(
    expression: &ast::TypeExpr,
    generic_parameters: &BTreeMap<String, Type>,
) -> Type {
    match &expression.kind {
        TypeExprKind::Name(name) => generic_parameters
            .get(&name.canonical)
            .cloned()
            .unwrap_or_else(|| type_name(&name.canonical)),
        TypeExprKind::Apply { constructor, args } => {
            let name = match &constructor.kind {
                TypeExprKind::Name(name) => name.canonical.as_str(),
                _ => return Type::Error,
            };
            let args = args
                .iter()
                .map(|argument| type_from_ast_with_generics(argument, generic_parameters))
                .collect::<Vec<_>>();
            match (name, args.as_slice()) {
                ("Option", [item]) => Type::option(item.clone()),
                ("Union", items) => Type::union(items.iter().cloned()),
                ("Tuple", items) => Type::Tuple(items.to_vec()),
                ("List", [item]) => Type::List(Box::new(item.clone())),
                ("Vector", [item]) => Type::Vector(Box::new(item.clone())),
                ("Map", [key, value]) => Type::Map(Box::new(key.clone()), Box::new(value.clone())),
                ("Set", [item]) => Type::Set(Box::new(item.clone())),
                (name, args) => Type::Nominal {
                    binding: name.to_owned(),
                    args: args.to_vec(),
                },
            }
        }
        TypeExprKind::Function {
            parameters,
            return_type,
        } => Type::Fn(
            FunctionType::new(
                parameters
                    .iter()
                    .map(|parameter| type_from_ast_with_generics(parameter, generic_parameters))
                    .collect(),
                type_from_ast_with_generics(return_type, generic_parameters),
            )
            .with_summaries(CallSummaries::unknown()),
        ),
        TypeExprKind::Tuple(items) => Type::Tuple(
            items
                .iter()
                .map(|item| type_from_ast_with_generics(item, generic_parameters))
                .collect(),
        ),
        TypeExprKind::Union(items) => Type::union(
            items
                .iter()
                .map(|item| type_from_ast_with_generics(item, generic_parameters)),
        ),
        TypeExprKind::Literal(form) => TypeLiteral::from_form(form)
            .map(Type::Literal)
            .unwrap_or(Type::Error),
        TypeExprKind::Error(_) => Type::Error,
    }
}

fn type_name(name: &str) -> Type {
    match name {
        "Bool" => Type::Bool,
        "Int" => Type::Int,
        "Float" => Type::Float,
        "Str" => Type::Str,
        "Bytes" => Type::Bytes,
        "None" => Type::None,
        "Any" => Type::Any,
        "Never" => Type::Never,
        "Unknown" => Type::Unknown,
        "Error" => Type::Error,
        name => Type::Nominal {
            binding: name.to_owned(),
            args: Vec::new(),
        },
    }
}

fn operator_from_name(name: &str) -> Option<Operator> {
    Some(match name {
        "+" => Operator::Add,
        "-" => Operator::Subtract,
        "*" => Operator::Multiply,
        "/" => Operator::Divide,
        "//" => Operator::FloorDivide,
        "%" => Operator::Remainder,
        "=" | "==" => Operator::Equal,
        "!=" | "not=" => Operator::NotEqual,
        "<" => Operator::Less,
        "<=" => Operator::LessEqual,
        ">" => Operator::Greater,
        ">=" => Operator::GreaterEqual,
        "and" => Operator::And,
        "or" => Operator::Or,
        "not" => Operator::Not,
        _ => return None,
    })
}

fn select_operator_signature<'a>(
    context: &TypeContext,
    signatures: &'a [OperatorSignature],
    operands: &[Type],
) -> Option<&'a OperatorSignature> {
    signatures.iter().find(|signature| {
        signature.operands.len() == operands.len()
            && operands
                .iter()
                .zip(&signature.operands)
                .all(|(actual, expected)| context.is_assignable(actual, expected))
    })
}

fn is_dynamic_operator_type(ty: &Type) -> bool {
    match ty {
        Type::Any | Type::Unknown | Type::Error => true,
        Type::Option(inner) | Type::List(inner) | Type::Vector(inner) | Type::Set(inner) => {
            is_dynamic_operator_type(inner)
        }
        Type::Union(members) | Type::Tuple(members) => members.iter().any(is_dynamic_operator_type),
        Type::Map(key, value) => is_dynamic_operator_type(key) || is_dynamic_operator_type(value),
        Type::Fn(function) => {
            function.parameters.iter().any(is_dynamic_operator_type)
                || is_dynamic_operator_type(&function.return_type)
        }
        Type::Nominal { args, .. } => args.iter().any(is_dynamic_operator_type),
        _ => false,
    }
}

fn operator_type_matches(
    context: &TypeContext,
    actual: &Type,
    expected: &Type,
    variables: &mut BTreeMap<TypeVarId, Type>,
) -> bool {
    if is_dynamic_operator_type(actual) || is_dynamic_operator_type(expected) {
        return false;
    }
    match expected {
        Type::TypeVar(variable) => {
            if let Some(previous) = variables.get(variable) {
                return context.is_assignable(actual, previous)
                    || context.is_assignable(previous, actual);
            }
            if matches!(actual, Type::TypeVar(_)) {
                return false;
            }
            variables.insert(*variable, actual.clone());
            true
        }
        Type::Option(expected_inner) => match actual {
            Type::None => true,
            Type::Option(actual_inner) => {
                operator_type_matches(context, actual_inner, expected_inner, variables)
            }
            _ => operator_type_matches(context, actual, expected_inner, variables),
        },
        Type::Union(expected_members) => expected_members.iter().any(|member| {
            let mut trial = variables.clone();
            if operator_type_matches(context, actual, member, &mut trial) {
                *variables = trial;
                true
            } else {
                false
            }
        }),
        Type::Tuple(expected_members) => match actual {
            Type::Tuple(actual_members) if actual_members.len() == expected_members.len() => {
                actual_members
                    .iter()
                    .zip(expected_members)
                    .all(|(actual, expected)| {
                        operator_type_matches(context, actual, expected, variables)
                    })
            }
            _ => false,
        },
        Type::List(expected_inner) => match actual {
            Type::List(actual_inner) => {
                operator_type_matches(context, actual_inner, expected_inner, variables)
            }
            _ => false,
        },
        Type::Vector(expected_inner) => match actual {
            Type::Vector(actual_inner) => {
                operator_type_matches(context, actual_inner, expected_inner, variables)
            }
            _ => false,
        },
        Type::Set(expected_inner) => match actual {
            Type::Set(actual_inner) => {
                operator_type_matches(context, actual_inner, expected_inner, variables)
            }
            _ => false,
        },
        Type::Map(expected_key, expected_value) => match actual {
            Type::Map(actual_key, actual_value) => {
                operator_type_matches(context, actual_key, expected_key, variables)
                    && operator_type_matches(context, actual_value, expected_value, variables)
            }
            _ => false,
        },
        Type::Nominal {
            binding: expected_binding,
            args: expected_args,
        } => match actual {
            Type::Nominal {
                binding: actual_binding,
                args: actual_args,
            } if actual_binding == expected_binding && actual_args.len() == expected_args.len() => {
                actual_args
                    .iter()
                    .zip(expected_args)
                    .all(|(actual, expected)| {
                        operator_type_matches(context, actual, expected, variables)
                    })
            }
            _ => false,
        },
        _ => context.is_assignable(actual, expected),
    }
}

fn contains_unresolved_operator_variable(ty: &Type, variables: &BTreeMap<TypeVarId, Type>) -> bool {
    match ty {
        Type::TypeVar(variable) => !variables.contains_key(variable),
        Type::Option(inner) | Type::List(inner) | Type::Vector(inner) | Type::Set(inner) => {
            contains_unresolved_operator_variable(inner, variables)
        }
        Type::Union(members) | Type::Tuple(members) => members
            .iter()
            .any(|member| contains_unresolved_operator_variable(member, variables)),
        Type::Map(key, value) => {
            contains_unresolved_operator_variable(key, variables)
                || contains_unresolved_operator_variable(value, variables)
        }
        Type::Fn(function) => {
            function
                .parameters
                .iter()
                .any(|parameter| contains_unresolved_operator_variable(parameter, variables))
                || contains_unresolved_operator_variable(&function.return_type, variables)
        }
        Type::Nominal { args, .. } => args
            .iter()
            .any(|argument| contains_unresolved_operator_variable(argument, variables)),
        _ => false,
    }
}

fn indexed_type(value: &Type) -> Type {
    match value {
        Type::List(item) | Type::Vector(item) | Type::Set(item) => (**item).clone(),
        Type::Map(_, value) => (**value).clone(),
        Type::Tuple(items) => Type::union(items.iter().cloned()),
        Type::Str => Type::Str,
        Type::Bytes => Type::Int,
        Type::Any => Type::Any,
        _ => Type::Unknown,
    }
}

fn non_nil_type(value: &Type) -> Type {
    match value {
        Type::Option(inner) => (**inner).clone(),
        Type::None => Type::Never,
        other => other.clone(),
    }
}

fn pattern_name(pattern: &ast::Pattern) -> Option<&str> {
    match &pattern.kind {
        PatternKind::Name(name) => Some(name.canonical.as_str()),
        _ => None,
    }
}

fn pattern_binding_name(pattern: &ast::Pattern) -> Option<&str> {
    pattern_name(pattern).map(|name| name.rsplit('/').next().unwrap_or(name))
}

fn pattern_keyword(pattern: &ast::Pattern) -> Option<&str> {
    let PatternKind::Literal(Form {
        kind: FormKind::Keyword(name),
        ..
    }) = &pattern.kind
    else {
        return None;
    };
    Some(name.canonical.trim_start_matches(':'))
}

fn pattern_static_key(pattern: &ast::Pattern) -> Option<String> {
    let PatternKind::Literal(form) = &pattern.kind else {
        return None;
    };
    match &form.kind {
        FormKind::Keyword(name) => Some(name.canonical.trim_start_matches(':').to_owned()),
        FormKind::String(value) => Some(value.clone()),
        _ => None,
    }
}

fn destructured_local_name(source: &Name) -> Name {
    Name {
        spelling: source
            .spelling
            .rsplit('/')
            .next()
            .unwrap_or(&source.spelling)
            .to_owned(),
        canonical: source
            .canonical
            .rsplit('/')
            .next()
            .unwrap_or(&source.canonical)
            .to_owned(),
    }
}

fn join_summaries<'a>(summaries: impl IntoIterator<Item = &'a CallSummaries>) -> CallSummaries {
    summaries
        .into_iter()
        .fold(CallSummaries::pure_scalar(), |left, right| left.join(right))
}

fn core_reduced_type_binding() -> BindingId {
    BindingId::new("osiris.prelude", "Reduced", BindingKind::Type)
}

fn core_delay_type_binding() -> BindingId {
    BindingId::new("osiris.prelude", "Delay", BindingKind::Type)
}

fn core_future_type_binding() -> BindingId {
    BindingId::new("osiris.prelude", "Future", BindingKind::Type)
}

fn core_promise_type_binding() -> BindingId {
    BindingId::new("osiris.prelude", "Promise", BindingKind::Type)
}

fn future_type(value: Type) -> Type {
    Type::Nominal {
        binding: core_future_type_binding().as_str().to_owned(),
        args: vec![value],
    }
}

fn promise_type(value: Type) -> Type {
    Type::Nominal {
        binding: core_promise_type_binding().as_str().to_owned(),
        args: vec![value],
    }
}

fn async_value_type(ty: &Type) -> Type {
    match ty {
        Type::Nominal { binding, args }
            if (binding == core_delay_type_binding().as_str()
                || binding == core_future_type_binding().as_str()
                || binding == core_promise_type_binding().as_str())
                && args.len() == 1 =>
        {
            args[0].clone()
        }
        Type::Unknown => Type::Any,
        other => other.clone(),
    }
}

fn reduced_type(value: Type) -> Type {
    Type::Nominal {
        binding: core_reduced_type_binding().as_str().to_owned(),
        args: vec![value],
    }
}

fn unreduced_type(ty: &Type) -> Type {
    match ty {
        Type::Nominal { binding, args }
            if binding == core_reduced_type_binding().as_str() && args.len() == 1 =>
        {
            args[0].clone()
        }
        Type::Union(members) => Type::union(members.iter().map(unreduced_type)),
        Type::Option(inner) => Type::option(unreduced_type(inner)),
        other => other.clone(),
    }
}

fn split_access_name(name: &str) -> Option<(&str, Vec<&str>)> {
    if let Some((base, member)) = name.split_once('/') {
        return Some((base, vec![member]));
    }
    let mut parts = name.split('.');
    let base = parts.next()?;
    let members = parts.collect::<Vec<_>>();
    (!members.is_empty()).then_some((base, members))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::{
        ast::lower_document,
        interface,
        reader::read,
        types::{
            Alignment, Availability, Effect, TemporalBound, TemporalSummary, Type, TypeLiteral,
        },
    };

    use super::{
        CallArgument, ContractTrustPolicy, ExprKind, InterfaceTrustPolicy, ItemKind, lower_module,
        lower_module_with_interfaces, lower_module_with_interfaces_and_trust_policy,
    };

    fn lower(source: &str) -> super::LowerResult {
        let document = read(source);
        let ast = lower_document(&document);
        let mut result = lower_module(&ast.module, "example");
        result.diagnostics.splice(0..0, ast.diagnostics);
        result
    }

    fn dependency_interfaces() -> BTreeMap<String, interface::Interface> {
        let document = read(
            r#"(module dep.core)
               (defn add [[x Int] ^{:osiris/names {:preferred 值}} [value Int]]
                 -> Int (+ x value))
               (alias sum add)
               (export [add sum])"#,
        );
        let surface = lower_document(&document);
        assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
        let typed = lower_module(&surface.module, "dep.core");
        assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
        let interface = interface::build(&typed.module, &surface.module).expect("interface");
        BTreeMap::from([(interface.module.clone(), interface)])
    }

    fn operator_dependency_interfaces() -> BTreeMap<String, interface::Interface> {
        let document = read(
            r#"(module dep.series)
               (defstruct (Series T) [values (Vector T)])
               ^{:osiris/operator :multiply}
               (defn multiply-series
                 [[series (Series Float)] [multiplier Float]]
                 -> (Series Float) series)
               (export [Series multiply-series])"#,
        );
        let surface = lower_document(&document);
        assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
        let typed = lower_module(&surface.module, "dep.series");
        assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
        let interface = interface::build(&typed.module, &surface.module).expect("interface");
        BTreeMap::from([(interface.module.clone(), interface)])
    }

    fn same_named_operator_interfaces() -> BTreeMap<String, interface::Interface> {
        [("dep.alpha", "add-alpha-x"), ("dep.beta", "add-beta-x")]
            .into_iter()
            .map(|(module, function)| {
                let source = format!(
                    "(module {module})\n\
                 (defstruct X [value Int])\n\
                 ^{{:osiris/operator :add}}\n\
                 (defn {function} [[left X] [right X]] -> X left)\n\
                 (export [X {function}])"
                );
                let surface = lower_document(&read(&source));
                assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
                let typed = lower_module(&surface.module, module);
                assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
                let interface =
                    interface::build(&typed.module, &surface.module).expect("same-name interface");
                (module.to_owned(), interface)
            })
            .collect()
    }

    fn contract_dependency_interface(future: u64) -> interface::Interface {
        let source = format!(
            r#"(module dep.causal)
               (extern python "host.series"
                 (defn rolling [[value Int]] -> Int
                   :contract
                   {{:id "host.series/rolling-v1"
                    :effects :pure
                    :temporal {{:past window :future {future} :availability :published}}
                    :data {{:preserves-length true}}}}))
               (export [rolling])"#
        );
        let surface = lower_document(&read(&source));
        assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
        let typed = lower_module(&surface.module, "dep.causal");
        assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
        interface::build(&typed.module, &surface.module).expect("interface")
    }

    fn causal_caller(
        dependency: &interface::Interface,
        trust: &ContractTrustPolicy,
    ) -> super::LowerResult {
        let source = r#"(module app)
            (import dep.causal :as dep)
            ^{:osiris/causal {:decision-point :published}}
            (defn pipeline [[value Int]] -> Int (dep/rolling value))"#;
        let surface = lower_document(&read(source));
        assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
        let interfaces = BTreeMap::from([(dependency.module.clone(), dependency.clone())]);
        lower_module_with_interfaces_and_trust_policy(&surface.module, "app", &interfaces, trust)
    }

    fn lower_with_dependency(source: &str) -> super::LowerResult {
        let document = read(source);
        let surface = lower_document(&document);
        assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
        let interfaces = dependency_interfaces();
        let mut result = lower_module_with_interfaces(&surface.module, "app", &interfaces);
        result.diagnostics.splice(0..0, surface.diagnostics);
        result
    }

    fn lower_with_operator_dependency(source: &str) -> super::LowerResult {
        let document = read(source);
        let surface = lower_document(&document);
        assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
        let interfaces = operator_dependency_interfaces();
        let mut result = lower_module_with_interfaces(&surface.module, "app", &interfaces);
        result.diagnostics.splice(0..0, surface.diagnostics);
        result
    }

    #[test]
    fn exported_functions_require_explicit_parameter_and_return_types() {
        let result = lower(
            "(export [public])
             (defn public [value] value)",
        );
        assert_eq!(
            result
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code == "OSR-T0017")
                .count(),
            1
        );
        assert_eq!(
            result
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code == "OSR-T0018")
                .count(),
            1
        );
        assert!(result.diagnostics.iter().any(|diagnostic| {
            diagnostic.message
                == "exported function `public` parameter `value` requires an explicit type"
        }));
    }

    #[test]
    fn private_functions_may_keep_locally_inferred_signatures() {
        let result = lower("(defn private [value] value)");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn extern_functions_are_explicit_declared_type_boundaries() {
        let result = lower(
            r#"(extern python "host.ops"
                 (defn transform [value] value))"#,
        );
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "OSR-T0017")
        );
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "OSR-T0018")
        );
    }

    #[test]
    fn resolves_aliases_to_one_binding_identity() {
        let result = lower("(defn mean [[x Float]] -> Float x) (alias 均值 mean) (均值 1.0)");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.module.aliases.len(), 1);
        assert_eq!(
            result.module.aliases[0].target,
            result
                .module
                .bindings
                .iter()
                .find(|binding| binding.name.canonical == "mean")
                .unwrap()
                .name
                .id
        );
    }

    #[test]
    fn infers_scalar_operator_types() {
        let result = lower("(defn add [[x Int] [y Float]] -> Float (+ x y))");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let ItemKind::Function(function) = &result.module.items[0].kind else {
            panic!("expected function");
        };
        assert_eq!(function.body.ty, Type::Float);
    }

    #[test]
    fn rejects_non_boolean_conditions() {
        let result = lower("(defn bad [[x Int]] -> Int (if x 1 2))");
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "OSR-T0001")
        );
    }

    #[test]
    fn dynamic_python_calls_remain_any_and_unknown() {
        let result = lower("(py/import numpy :as np) (def values (np.asarray [1 2 3]))");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let ItemKind::Value(value) = &result.module.items[1].kind else {
            panic!("expected value");
        };
        let value = value.value.as_ref().expect("definition has value");
        assert_eq!(value.ty, Type::Any);
        assert!(value.summaries.effects.open);
    }

    #[test]
    fn dynamic_python_attribute_reads_remain_any_and_unknown() {
        let result = lower(
            "(py/import numpy :as np)
             (def values (np.asarray [1 2 3]))
             (def dtype (values.dtype))",
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let ItemKind::Value(value) = &result.module.items[2].kind else {
            panic!("expected value");
        };
        let expression = value.value.as_ref().expect("definition has value");
        assert_eq!(expression.ty, Type::Any);
        assert!(expression.summaries.effects.open);
        assert_eq!(expression.summaries.temporal, TemporalSummary::unknown());
        assert_eq!(
            expression.summaries.data,
            crate::types::DataProperties::unknown()
        );
    }

    #[test]
    fn dynamic_python_index_reads_remain_unknown_at_any_boundary() {
        let result = lower(
            "(def value Any)
             (def item (index value 0))",
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let ItemKind::Value(value) = &result.module.items[1].kind else {
            panic!("expected value");
        };
        let expression = value.value.as_ref().expect("definition has value");
        assert_eq!(expression.ty, Type::Any);
        assert!(expression.summaries.effects.open);
        assert_eq!(expression.summaries.temporal, TemporalSummary::unknown());
        assert_eq!(
            expression.summaries.data,
            crate::types::DataProperties::unknown()
        );
    }

    #[test]
    fn extern_calls_remain_unknown_without_a_contract() {
        let result = lower(
            r#"(extern python "host.ops"
                  (defn transform [[value Int]] -> Int))
               (defn call [[value Int]] -> Int (transform value))"#,
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let function = result
            .module
            .items
            .iter()
            .find_map(|item| match &item.kind {
                ItemKind::Function(function) => Some(function),
                _ => None,
            })
            .expect("runtime function should be lowered");

        assert!(function.body.summaries.effects.open);
        assert_eq!(
            function.body.summaries.temporal.future,
            TemporalBound::Unknown
        );
        assert_eq!(
            function.body.summaries.data,
            crate::types::DataProperties::unknown()
        );
        assert_eq!(result.module.extern_functions.len(), 1);
        assert!(result.module.extern_functions[0].contract_id.is_none());
    }

    #[test]
    fn extern_contract_summaries_are_applied_to_calls() {
        let result = lower(
            r#"(extern python "host.ops"
                  (defn rolling [[value Int]] -> Int
                    :contract
                    {:id "host.ops/rolling-v1"
                     :effects :pure
                     :temporal {:past window :future 0 :availability :published}
                     :data {:alignment :labelled :preserves-length true}}))
               (defn call [[value Int]] -> Int (rolling value))"#,
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let function = result
            .module
            .items
            .iter()
            .find_map(|item| match &item.kind {
                ItemKind::Function(function) => Some(function),
                _ => None,
            })
            .expect("runtime function should be lowered");
        assert!(!function.body.summaries.effects.open);
        assert!(function.body.summaries.effects.effects.is_empty());
        assert_eq!(
            function.body.summaries.temporal.past,
            TemporalBound::Symbolic("window".to_owned())
        );
        assert_eq!(
            function.body.summaries.temporal.availability,
            Availability::Named("published".to_owned())
        );
        assert_eq!(function.body.summaries.data.preserves_length, Some(true));
        assert_eq!(
            result.module.extern_functions[0].contract_id.as_deref(),
            Some("host.ops/rolling-v1")
        );
    }

    #[test]
    fn source_function_types_accept_and_propagate_rich_callback_summaries() {
        let result = lower(
            r#"(extern python "host.ops"
                  (defn invoke [[callback (Fn [] -> Int)]] -> Int
                    :contract
                    {:id "host.ops/invoke-v1"
                     :effects :pure
                     :temporal {:past 0 :future 0 :availability :published}})
                  (defn lead [] -> Int
                    :contract
                    {:id "host.ops/lead-v1"
                     :effects [:mutation]
                     :temporal {:past 2 :future 1 :availability :published}
                     :data {:axes [:time]
                            :alignment :labelled
                            :preserves-length true}}))
               ^{:osiris/causal {:decision-point :published}}
               (defn call [] -> Int
                 (invoke (fn [] (lead))))"#,
        );
        assert!(
            !result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "OSR-T0001"),
            "source-level Fn must not impose pure-scalar summaries: {:?}",
            result.diagnostics
        );
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "OSR-C0002"),
            "the actual callback future bound must reach the causal gate: {:?}",
            result.diagnostics
        );
        let function = result
            .module
            .items
            .iter()
            .find_map(|item| match &item.kind {
                ItemKind::Function(function) => Some(function),
                _ => None,
            })
            .expect("runtime function should be lowered");
        assert!(
            function
                .body
                .summaries
                .effects
                .effects
                .contains(&Effect::Mutation)
        );
        assert_eq!(
            function.body.summaries.temporal.past,
            TemporalBound::Finite(2)
        );
        assert_eq!(
            function.body.summaries.temporal.future,
            TemporalBound::Finite(1)
        );
        let ExprKind::Call { arguments, .. } = &function.body.kind else {
            panic!("call body should remain a higher-order invocation");
        };
        let Some(CallArgument::Positional(callback)) = arguments.first() else {
            panic!("invoke should receive its callback positionally");
        };
        let Type::Fn(callback) = &callback.ty else {
            panic!("callback argument should retain its inferred function type");
        };
        assert_eq!(callback.summaries.data.alignment, Alignment::Unknown);
    }

    #[test]
    fn symbolic_temporal_contracts_specialize_and_compose_across_calls() {
        let result = lower(
            r#"(extern python "host.ops"
                  (defn rolling [[value Int] [window Int]] -> Int
                    :contract
                    {:id "host.ops/rolling-v1"
                     :effects :pure
                     :temporal {:past "window-1"
                                :future 0
                                :availability :published}}))
               (defn twice [[value Int] [n Int]] -> Int
                 (let [mean (rolling value n)
                       deviation (- value mean)
                       second-mean (rolling deviation n)]
                   second-mean))"#,
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let function = result
            .module
            .items
            .iter()
            .find_map(|item| match &item.kind {
                ItemKind::Function(function) => Some(function),
                _ => None,
            })
            .expect("runtime function should be lowered");
        assert_eq!(
            function.summaries.temporal.past,
            TemporalBound::Symbolic("2*(n-1)".to_owned())
        );
        assert_eq!(function.summaries.temporal.future, TemporalBound::Finite(0));
        assert_eq!(
            function.summaries.temporal.availability,
            Availability::Named("published".to_owned())
        );
    }

    #[test]
    fn causal_regions_require_exact_local_contract_trust() {
        let dependency = contract_dependency_interface(0);
        let untrusted = causal_caller(
            &dependency,
            &ContractTrustPolicy::untrusted(format!("sha256:{}", "1".repeat(64))),
        );
        assert!(
            untrusted
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "OSR-C0001")
        );

        let policy_hash = format!("sha256:{}", "2".repeat(64));
        let trusted = ContractTrustPolicy {
            hash: policy_hash.clone(),
            interfaces: BTreeMap::from([(
                dependency.module.clone(),
                InterfaceTrustPolicy {
                    distribution: "host-series".to_owned(),
                    semantic_interface_hash: dependency.semantic_interface_hash().to_owned(),
                    trusted_contract_ids: std::collections::BTreeSet::from([
                        "host.series/rolling-v1".to_owned(),
                    ]),
                },
            )]),
        };
        let accepted = causal_caller(&dependency, &trusted);
        assert!(
            accepted.diagnostics.is_empty(),
            "{:?}",
            accepted.diagnostics
        );
        assert_eq!(accepted.module.trust_policy_hash, policy_hash);
        let function = accepted
            .module
            .items
            .iter()
            .find_map(|item| match &item.kind {
                ItemKind::Function(function) => Some(function),
                _ => None,
            })
            .expect("pipeline function");
        assert_eq!(function.contract_evidence.declared.len(), 1);
        assert_eq!(function.contract_evidence.verified.len(), 1);

        let wrong_hash = ContractTrustPolicy {
            interfaces: BTreeMap::from([(
                dependency.module.clone(),
                InterfaceTrustPolicy {
                    distribution: "host-series".to_owned(),
                    semantic_interface_hash: format!("sha256:{}", "f".repeat(64)),
                    trusted_contract_ids: std::collections::BTreeSet::from([
                        "host.series/rolling-v1".to_owned(),
                    ]),
                },
            )]),
            ..trusted
        };
        let rejected = causal_caller(&dependency, &wrong_hash);
        assert!(
            rejected
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "OSR-C0001")
        );
    }

    #[test]
    fn causal_regions_reject_future_reads_even_when_contract_is_trusted() {
        let dependency = contract_dependency_interface(1);
        let trust = ContractTrustPolicy {
            hash: format!("sha256:{}", "3".repeat(64)),
            interfaces: BTreeMap::from([(
                dependency.module.clone(),
                InterfaceTrustPolicy {
                    distribution: "host-series".to_owned(),
                    semantic_interface_hash: dependency.semantic_interface_hash().to_owned(),
                    trusted_contract_ids: std::collections::BTreeSet::from([
                        "host.series/rolling-v1".to_owned(),
                    ]),
                },
            )]),
        };
        let result = causal_caller(&dependency, &trust);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "OSR-C0002")
        );
        assert!(
            !result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "OSR-C0001")
        );
    }

    #[test]
    fn imported_function_signature_and_keyword_alias_are_typed() {
        let result = lower_with_dependency(
            "(module app)
             (import dep.core :as dep :refer [add])
             (defn call [] -> Int (dep/add 1 :值 2))",
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let function = result
            .module
            .items
            .iter()
            .find_map(|item| match &item.kind {
                ItemKind::Function(function) => Some(function),
                _ => None,
            })
            .expect("expected function");
        let ExprKind::Call { callee, .. } = &function.body.kind else {
            panic!("expected call");
        };
        assert_eq!(function.body.ty, Type::Int);
        let ExprKind::Binding(binding) = callee.kind.clone() else {
            panic!("qualified call should resolve to imported binding");
        };
        assert_eq!(binding.as_str(), "dep.core::function::add");
    }

    #[test]
    fn qualified_imported_targets_can_be_local_aliases_without_new_binding() {
        let result = lower_with_dependency(
            "(module app)
             (import dep.core :as dep)
             (alias 加法 dep/add)
             (alias 求和 dep/sum)
             (defn call [] -> Int (加法 1 2))",
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let canonical_alias = result
            .module
            .aliases
            .iter()
            .find(|alias| alias.canonical == "加法")
            .expect("qualified alias");
        let imported_alias = result
            .module
            .aliases
            .iter()
            .find(|alias| alias.canonical == "求和")
            .expect("qualified imported public alias");
        assert_eq!(canonical_alias.target.as_str(), "dep.core::function::add");
        assert_eq!(imported_alias.target, canonical_alias.target);
        let function = result
            .module
            .items
            .iter()
            .find_map(|item| match &item.kind {
                ItemKind::Function(function) => Some(function),
                _ => None,
            })
            .expect("expected function");
        let ExprKind::Call { callee, .. } = &function.body.kind else {
            panic!("expected call");
        };
        let ExprKind::Binding(binding) = &callee.kind else {
            panic!("alias call should resolve to a binding");
        };
        assert_eq!(binding.as_str(), canonical_alias.target.as_str());
    }

    #[test]
    fn imported_operator_instance_is_selected_across_an_osri_interface() {
        let result = lower_with_operator_dependency(
            r#"(module app)
               (import dep.series :as dep)
               (defn scale
                 [[series (Series Float)] [multiplier Float]]
                 -> (Series Float)
                 (* series multiplier))"#,
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let function = result
            .module
            .items
            .iter()
            .find_map(|item| match &item.kind {
                ItemKind::Function(function) => Some(function),
                _ => None,
            })
            .expect("expected scale function");
        let ExprKind::Call { callee, .. } = &function.body.kind else {
            panic!("operator should lower to a call of its static instance")
        };
        let ExprKind::Binding(binding) = &callee.kind else {
            panic!("operator callee should be a binding")
        };
        assert_eq!(binding.as_str(), "dep.series::function::multiply-series");
        assert_eq!(
            function.body.ty,
            Type::Nominal {
                binding: "dep.series::type::Series".to_owned(),
                args: vec![Type::Float]
            }
        );
    }

    #[test]
    fn same_named_nominals_keep_alias_identity_and_select_their_own_operator() {
        let source = read(
            r#"(module app)
               (import dep.alpha :as alpha)
               (import dep.beta :as beta)
               (alias AlphaX alpha/X)
               (defn alpha-id [[value AlphaX]] -> alpha/X value)
               (defn add-alpha [[left alpha/X] [right AlphaX]] -> alpha/X (+ left right))
               (defn add-beta [[left beta/X] [right beta/X]] -> beta/X (+ left right))"#,
        );
        let surface = lower_document(&source);
        assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
        let result =
            lower_module_with_interfaces(&surface.module, "app", &same_named_operator_interfaces());
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);

        let functions = result
            .module
            .items
            .iter()
            .filter_map(|item| match &item.kind {
                ItemKind::Function(function) => Some(function),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            functions[0].parameters[0].ty,
            Type::Nominal {
                binding: "dep.alpha::type::X".to_owned(),
                args: Vec::new(),
            }
        );
        assert_eq!(functions[0].parameters[0].ty, functions[0].return_type);

        let selected = functions[1..]
            .iter()
            .map(|function| {
                let ExprKind::Call { callee, .. } = &function.body.kind else {
                    panic!("static operator should lower to a call")
                };
                let ExprKind::Binding(binding) = &callee.kind else {
                    panic!("static operator call should target a binding")
                };
                binding.as_str().to_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            selected,
            vec![
                "dep.alpha::function::add-alpha-x".to_owned(),
                "dep.beta::function::add-beta-x".to_owned(),
            ]
        );

        let python =
            crate::backend::compile_module(&result.module, crate::types::PythonVersion::MINIMUM)
                .expect("same-name nominal module should emit Python")
                .source;
        assert!(python.contains("from dep.alpha import X"), "{python}");
        assert!(python.contains("from dep.beta import X as X_2"), "{python}");
        assert!(python.contains("value: X) -> X"), "{python}");
        assert!(python.contains("left: X_2, right: X_2) -> X_2"), "{python}");
    }

    #[test]
    fn imported_operator_summary_is_joined_into_expression_summary() {
        let document = read(
            r#"(module dep.summary)
               (defstruct (Series T) [values (Vector T)])
               (extern python "host.ops"
                 (defn runtime-multiply
                   [[series (Series Float)] [multiplier Float]]
                   -> (Series Float)))
               ^{:osiris/operator :multiply}
               (defn multiply-series
                 [[series (Series Float)] [multiplier Float]]
                 -> (Series Float)
                 (runtime-multiply series multiplier))
               (export [Series multiply-series])"#,
        );
        let surface = lower_document(&document);
        assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
        let typed = lower_module(&surface.module, "dep.summary");
        assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
        let dependency = interface::build(&typed.module, &surface.module).expect("interface");
        assert!(dependency.operator_instances[0].summaries.effects.open);
        let interfaces = BTreeMap::from([(dependency.module.clone(), dependency)]);
        let caller = read(
            r#"(module app)
               (import dep.summary)
               (defn scale
                 [[series (Series Float)] [multiplier Float]]
                 -> (Series Float)
                 (* series multiplier))"#,
        );
        let caller_surface = lower_document(&caller);
        let result = lower_module_with_interfaces(&caller_surface.module, "app", &interfaces);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let function = result
            .module
            .items
            .iter()
            .find_map(|item| match &item.kind {
                ItemKind::Function(function) => Some(function),
                _ => None,
            })
            .expect("expected scale function");
        assert!(function.body.summaries.effects.open);
    }

    #[test]
    fn abs_uses_a_static_core_call_without_an_operator_variant() {
        let result = lower("(defn magnitude [[value Int]] -> Int (abs value))");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let function = match &result.module.items[0].kind {
            ItemKind::Function(function) => function,
            other => panic!("expected function, got {other:?}"),
        };
        let ExprKind::Call { callee, .. } = &function.body.kind else {
            panic!("abs should lower to a normal call")
        };
        let ExprKind::Binding(binding) = &callee.kind else {
            panic!("abs callee should be a synthetic binding")
        };
        assert!(binding.as_str().ends_with("::function::__osiris_abs"));
        assert_eq!(function.body.ty, Type::Int);
    }

    #[test]
    fn unknown_qualified_import_member_is_diagnosed() {
        let result = lower_with_dependency(
            "(module app)
             (import dep.core :as dep)
             (def value (dep/missing 1))",
        );
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "OSR-H0013")
        );
    }

    #[test]
    fn unary_minus_is_lowered_as_negation() {
        let result = lower("(defn negate [[x Int]] -> Int (- x))");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let ItemKind::Function(function) = &result.module.items[0].kind else {
            panic!("expected function");
        };
        assert!(matches!(
            function.body.kind,
            super::ExprKind::Operator {
                operator: super::Operator::Negate,
                ..
            }
        ));
    }

    #[test]
    fn struct_constructor_checks_fields_and_defaults() {
        let result = lower(
            "(defstruct Point [x Int] [y Int = 0])
             (def point (Point :x 1))",
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let ItemKind::Value(value) = &result.module.items[1].kind else {
            panic!("expected value");
        };
        let expression = value.value.as_ref().expect("constructor expression");
        assert_eq!(
            expression.ty,
            Type::Nominal {
                binding: "example::type::Point".to_owned(),
                args: Vec::new()
            }
        );
        let super::ExprKind::Call { arguments, .. } = &expression.kind else {
            panic!("expected constructor call");
        };
        assert!(matches!(
            &arguments[0],
            super::CallArgument::Keyword { name, .. } if name == "x"
        ));
    }

    #[test]
    fn struct_field_access_keeps_declared_type_and_summary() {
        let result = lower(
            r#"(defstruct Point [x Int] [y Float])
               (defn distance [[point Point]] -> Float (+ point.x point.y))"#,
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let function = result
            .module
            .items
            .iter()
            .find_map(|item| match &item.kind {
                ItemKind::Function(function) => Some(function),
                _ => None,
            })
            .expect("expected distance function");
        assert_eq!(function.body.ty, Type::Float);
        let ExprKind::Operator { operands, .. } = &function.body.kind else {
            panic!("expected scalar operator")
        };
        assert_eq!(operands[0].ty, Type::Int);
        assert_eq!(operands[1].ty, Type::Float);
        assert!(matches!(
            &operands[0].kind,
            ExprKind::Attribute { attribute, .. } if attribute == "x"
        ));
    }

    #[test]
    fn imported_struct_field_access_uses_interface_field_type() {
        let document = read(
            r#"(module dep.fields)
               (defstruct (Series T)
                 ^{:osiris/names {"zh-CN" {:preferred 值}}}
                 [values (Vector T)])
               (export [Series])"#,
        );
        let surface = lower_document(&document);
        assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
        let typed = lower_module(&surface.module, "dep.fields");
        assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
        let dependency = interface::build(&typed.module, &surface.module).expect("interface");
        let interfaces = BTreeMap::from([(dependency.module.clone(), dependency)]);
        let caller = read(
            r#"(module app)
               (import dep.fields)
               (defn values [[series (Series Float)]] -> (Vector Float) series.值)"#,
        );
        let caller_surface = lower_document(&caller);
        assert!(
            caller_surface.diagnostics.is_empty(),
            "{:?}",
            caller_surface.diagnostics
        );
        let result = lower_module_with_interfaces(&caller_surface.module, "app", &interfaces);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let function = result
            .module
            .items
            .iter()
            .find_map(|item| match &item.kind {
                ItemKind::Function(function) => Some(function),
                _ => None,
            })
            .expect("expected values function");
        assert_eq!(function.body.ty, Type::Vector(Box::new(Type::Float)));
    }

    #[test]
    fn unknown_declared_struct_field_is_rejected() {
        let result =
            lower("(defstruct Point [x Int]) (defn bad [[point Point]] -> Int point.missing)");
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "OSR-T0016")
        );
    }

    #[test]
    fn generic_struct_constructor_instantiates_type_parameters() {
        let result = lower(
            "(defstruct (Range T) [min T] [max T = 1])
             (def range (Range 0 1))",
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let ItemKind::Value(value) = &result.module.items[1].kind else {
            panic!("expected value");
        };
        let expression = value.value.as_ref().expect("constructor expression");
        assert_eq!(
            expression.ty,
            Type::Nominal {
                binding: "example::type::Range".to_owned(),
                args: vec![Type::Int]
            }
        );
    }

    #[test]
    fn literal_type_arguments_reach_typed_hir_without_error_types() {
        let result = lower(
            r#"(defstruct (Array T Axes) [values Any])
               (defstruct (Frame Schema KeyMarker KeyValue OrderMarker OrderValue)
                 [values Any])
               (defn array-id
                  [[values (Array Float [:time :feature])]]
                  -> (Array Float [:time :feature])
                  values)
               (defn frame-id
                  [[frame (Frame {:value Float :time Datetime :category Str}
                                 :key [:time :category]
                                 :order [:time])]]
                  -> (Frame {:category Str :value Float :time Datetime}
                            :key [:time :category]
                            :order [:time])
                  frame)"#,
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let functions = result
            .module
            .items
            .iter()
            .filter_map(|item| match &item.kind {
                ItemKind::Function(function) => Some(function),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(functions.len(), 2);
        let Type::Nominal { args, .. } = &functions[0].parameters[0].ty else {
            panic!("array parameter is nominal")
        };
        assert_eq!(
            args[1],
            Type::Literal(TypeLiteral::Vector(vec![
                TypeLiteral::Keyword(":time".to_owned()),
                TypeLiteral::Keyword(":feature".to_owned()),
            ]))
        );
        let Type::Nominal { args, .. } = &functions[1].return_type else {
            panic!("frame return is nominal")
        };
        let Type::Literal(schema) = &args[0] else {
            panic!("frame schema is literal")
        };
        assert_eq!(
            schema.canonical_text(),
            "{:category Str :time Datetime :value Float}"
        );
    }

    #[test]
    fn unknown_nominal_type_is_not_fabricated_in_typed_hir() {
        let result = lower("(defn typo [[value Typo]] -> Typo value)");
        assert!(result.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "OSR-T0021" && diagnostic.message == "unknown nominal type `Typo`"
        }));
        let binding = result
            .module
            .bindings
            .iter()
            .find(|binding| binding.name.canonical == "typo")
            .expect("function binding remains recoverable");
        let Type::Fn(signature) = &binding.ty else {
            panic!("function remains typed")
        };
        assert_eq!(signature.parameters, [Type::Error]);
        assert_eq!(signature.return_type.as_ref(), &Type::Error);
    }

    #[test]
    fn struct_check_keeps_typed_message_and_throw_summary() {
        let result = lower(
            "(defstruct Checked [value Int]
               (check (> value 0) \"value must be positive\"))
             (def checked (Checked 1))",
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let ItemKind::Struct(structure) = &result.module.items[0].kind else {
            panic!("expected struct");
        };
        assert_eq!(structure.checks.len(), 1);
        assert_eq!(
            structure.checks[0]
                .message
                .as_ref()
                .map(|message| &message.ty),
            Some(&Type::Str)
        );
        assert!(structure.checks[0].condition.ty == Type::Bool);
    }

    #[test]
    fn parameter_aliases_are_canonicalized_and_type_checked() {
        let result = lower(
            "(defn f [^{:osiris/names {\"zh-CN\" {:preferred 周期 :aliases [时长]}}}
                       [window Int]] -> Int window)
             (f :时长 2)",
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let ItemKind::Expr(expression) = &result.module.items[1].kind else {
            panic!("expected call expression");
        };
        let super::ExprKind::Call { arguments, .. } = &expression.kind else {
            panic!("expected call");
        };
        assert!(matches!(
            &arguments[0],
            super::CallArgument::Keyword { name, .. } if name == "window"
        ));
    }

    #[test]
    fn phase_one_names_do_not_collide_with_runtime_names() {
        let result = lower(
            "(defn-for-syntax helper [] -> Int 1)
             (defn helper [] -> Int 2)
             (helper)",
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(
            result
                .module
                .bindings
                .iter()
                .filter(|binding| binding.name.canonical == "helper")
                .count(),
            1
        );
    }

    #[test]
    fn exporting_an_alias_requires_an_explicit_canonical_export() {
        let rejected = lower(
            "(defn canonical [] -> Int 1)
             (alias 本地名 canonical)
             (export [本地名])",
        );
        assert!(
            rejected
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "OSR-N0015")
        );
        assert!(rejected.module.exports.is_empty());
        assert!(!rejected.module.aliases[0].public);

        let accepted = lower(
            "(defn canonical [] -> Int 1)
             (alias 本地名 canonical)
             (export [canonical 本地名])",
        );
        assert!(
            accepted.diagnostics.is_empty(),
            "{:?}",
            accepted.diagnostics
        );
        assert_eq!(accepted.module.exports.len(), 1);
        assert!(accepted.module.aliases[0].public);
    }

    #[test]
    fn rejects_local_python_identifier_collisions() {
        let result = lower("(defn collision [[a-b Int] [a_b Int]] -> Int a-b)");
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "OSR-N0002")
        );
    }
}
