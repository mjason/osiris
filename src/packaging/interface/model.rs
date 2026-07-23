use std::{error::Error, fmt};

use crate::{
    interface_graph::InterfaceHashGroup,
    name::BindingKind,
    records::{StaticSchema, ValidatedRecord},
    syntax::{Form, MetadataEntry},
    types::{CallSummaries, OperatorInstance, Type},
};

pub const FORMAT_NAME: &str = "osiris-interface";
pub const FORMAT_VERSION: u32 = 2;
pub const COMPILER_ABI: &str = "osiris-compiler-v0";
pub const LANGUAGE_ABI: &str = "osiris-language-v1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InterfaceError {
    pub code: &'static str,
    pub message: String,
}

impl InterfaceError {
    pub(super) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for InterfaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl Error for InterfaceError {}

pub type InterfaceResult<T> = Result<T, InterfaceError>;

#[derive(Clone, Debug, PartialEq)]
pub struct Interface {
    pub format_version: u32,
    pub compiler_abi: String,
    pub language_abi: String,
    pub module: String,
    pub metadata: Vec<MetadataEntry>,
    pub bindings: Vec<PublicBinding>,
    pub aliases: Vec<PublicAlias>,
    pub functions: Vec<FunctionInterface>,
    pub structs: Vec<StructInterface>,
    pub operator_instances: Vec<OperatorInstance>,
    pub macros: Vec<MacroInterface>,
    pub phase_helpers: Vec<PhaseHelperInterface>,
    pub static_schemas: Vec<StaticSchema>,
    pub owned_records: Vec<ValidatedRecord>,
    pub graph: InterfaceHashGroup,
    pub hashes: InterfaceHashes,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InterfaceHashes {
    pub interface_body: String,
    pub semantic_body: String,
    pub tooling_body: String,
    pub content_integrity: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PublicBinding {
    pub id: String,
    pub canonical: String,
    pub python: String,
    pub kind: BindingKind,
    pub ty: Type,
    pub runtime: Option<RuntimeLocator>,
    pub metadata: Vec<MetadataEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeLocator {
    pub module: String,
    pub name: String,
    pub python_module: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PublicAlias {
    pub spelling: String,
    pub canonical: String,
    pub target: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionInterface {
    pub binding: String,
    pub parameters: Vec<ParameterInterface>,
    pub return_type: Type,
    pub contract_id: Option<String>,
    pub summaries: CallSummaries,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParameterInterface {
    pub id: String,
    pub canonical: String,
    pub ty: Type,
    pub has_default: bool,
    pub variadic: bool,
    pub aliases: Vec<String>,
    pub metadata: Vec<MetadataEntry>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StructInterface {
    pub binding: String,
    pub type_parameters: Vec<String>,
    pub fields: Vec<FieldInterface>,
    pub invariant_count: usize,
    pub doc: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldInterface {
    pub id: String,
    pub canonical: String,
    pub ty: Type,
    pub has_default: bool,
    pub aliases: Vec<String>,
    pub metadata: Vec<MetadataEntry>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MacroInterface {
    pub id: String,
    pub canonical: String,
    pub parameters: Form,
    pub minimum_arity: usize,
    pub variadic: bool,
    pub helper_bindings: Vec<String>,
    pub phase_ir: Form,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PhaseHelperInterface {
    pub id: String,
    pub canonical: String,
    pub phase_ir: Form,
}

impl Interface {
    /// Return replayable declarations in evaluator load order. Private
    /// functions precede public macros so callers can pass the result directly
    /// to `macro_expand::expand_with_imported_phase_forms`.
    #[must_use]
    pub fn imported_phase_forms(&self) -> Vec<Form> {
        self.phase_helpers
            .iter()
            .map(|helper| helper.phase_ir.clone())
            .chain(self.macros.iter().map(|macro_| macro_.phase_ir.clone()))
            .collect()
    }

    #[must_use]
    pub fn semantic_interface_hash(&self) -> &str {
        &self.graph.semantic_interface_hash
    }

    #[must_use]
    pub fn tooling_metadata_hash(&self) -> &str {
        &self.graph.tooling_metadata_hash
    }
}

pub(super) fn empty_hash_group(module: &str) -> InterfaceHashGroup {
    InterfaceHashGroup {
        id: module.to_owned(),
        members: Vec::new(),
        internal_edges: Vec::new(),
        external_dependencies: Vec::new(),
        semantic_interface_hash: String::new(),
        tooling_metadata_hash: String::new(),
    }
}
