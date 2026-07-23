use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::{
    ast,
    diagnostic::Diagnostic,
    interface::Interface,
    name::{BindingId, BindingName},
    source::Span,
    syntax::MetadataEntry,
    types::{CallSummaries, ScalarOperator, Type},
};

/// Read-only source of validated `.osri` interfaces used by the cross-module
/// lowering entry point. Implementations return data already checked by
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

    pub(super) fn unverified(&self) -> impl Iterator<Item = &ContractFact> {
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub decorators: Vec<Expr>,
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub decorators: Vec<Expr>,
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
    pub(super) fn scalar(self) -> Option<ScalarOperator> {
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
