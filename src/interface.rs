//! Deterministic, data-only `.osri` compilation interfaces.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt::{self, Write},
};

use sha2::{Digest, Sha256};

use crate::{
    ast,
    hir::{self, ItemKind},
    interface_graph::{
        InterfaceBodyHashes, InterfaceHashEdge, InterfaceHashGroup, InterfaceHashMember,
        ResolvedHashDependency, calculate_interface_graph_hashes, verify_interface_hash_group,
    },
    macro_expand,
    name::{BindingId, BindingKind, python_identifier},
    printer::render_document_text,
    reader,
    records::{self, ProjectionKind, StaticDatum, StaticSchema, StaticType, ValidatedRecord},
    source::Span,
    syntax::{
        Document, Form, FormKind, METADATA_DECLARATION_LIMITS, METADATA_INTERFACE_LIMITS,
        METADATA_TARGET_LIMITS, MetadataEntry, MetadataLimitExceeded, MetadataResourceUsage, Name,
        ReaderMacroKind, check_metadata_resources, check_metadata_usage,
        metadata_datum_is_serializable,
    },
    types::{
        Alignment, Availability, CallSummaries, DataProperties, Effect, EffectRow, FunctionType,
        OperatorInstance, ScalarOperator, TemporalBound, TemporalSummary, Type, TypeLiteral,
        TypeVarId, nominal_short_name,
    },
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
    fn new(code: &'static str, message: impl Into<String>) -> Self {
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

fn empty_hash_group(module: &str) -> InterfaceHashGroup {
    InterfaceHashGroup {
        id: module.to_owned(),
        members: Vec::new(),
        internal_edges: Vec::new(),
        external_dependencies: Vec::new(),
        semantic_interface_hash: String::new(),
        tooling_metadata_hash: String::new(),
    }
}

/// Builds an interface from typed HIR plus the non-executable declarations
/// retained in the surface module.
pub fn build(typed: &hir::Module, surface: &ast::Module) -> InterfaceResult<Interface> {
    let static_data = records::analyze_module(surface);
    build_with_static_data(typed, surface, &static_data)
}

/// Build an interface using static declarations already validated in the
/// caller's complete interface environment.
pub fn build_with_static_data(
    typed: &hir::Module,
    surface: &ast::Module,
    static_data: &records::StaticModuleData,
) -> InterfaceResult<Interface> {
    let mut interface = from_hir(typed)?;
    if let Some(diagnostic) = static_data.diagnostics.first() {
        return Err(InterfaceError::new(
            "OSR-I0055",
            format!("invalid static declaration: {}", diagnostic.message),
        ));
    }

    let public_bindings = interface
        .bindings
        .iter()
        .map(|binding| binding.id.as_str())
        .collect::<BTreeSet<_>>();
    interface.static_schemas = static_data
        .schemas
        .iter()
        .filter(|schema| {
            let binding = BindingId::new(&typed.name, &schema.name, BindingKind::Type);
            public_bindings.contains(binding.as_str())
        })
        .cloned()
        .collect();
    interface.owned_records = static_data
        .records
        .iter()
        .filter(|record| record.public && record.module == typed.name)
        .cloned()
        .collect();
    let (macros, phase_helpers) = collect_phase_interface(surface, &typed.name)?;
    interface.macros = macros;
    interface.phase_helpers = phase_helpers;
    interface.operator_instances = collect_operator_instances(typed, surface, &interface)?;
    interface
        .static_schemas
        .sort_by(|left, right| left.name.cmp(&right.name));
    interface
        .owned_records
        .sort_by(|left, right| left.stable_record_id.cmp(&right.stable_record_id));
    validate_model(&interface)?;
    refresh_standalone_hashes(&mut interface)?;
    Ok(interface)
}

fn collect_operator_instances(
    typed: &hir::Module,
    surface: &ast::Module,
    interface: &Interface,
) -> InterfaceResult<Vec<OperatorInstance>> {
    let public_bindings = interface
        .bindings
        .iter()
        .map(|binding| (binding.canonical.as_str(), binding))
        .collect::<BTreeMap<_, _>>();
    let public_types = interface
        .bindings
        .iter()
        .filter(|binding| binding.kind == BindingKind::Type)
        .map(|binding| binding.id.as_str())
        .collect::<BTreeSet<_>>();
    let typed_bindings = typed
        .bindings
        .iter()
        .map(|binding| (binding.name.id.as_str(), binding))
        .collect::<BTreeMap<_, _>>();
    let function_interfaces = interface
        .functions
        .iter()
        .map(|function| (function.binding.as_str(), function))
        .collect::<BTreeMap<_, _>>();

    let mut declarations = Vec::new();
    for item in &surface.items {
        match &item.kind {
            ast::ItemKind::Defn(function) => declarations.push(function),
            ast::ItemKind::Extern(external) => {
                declarations.extend(external.items.iter().filter_map(|item| match &item.kind {
                    ast::ItemKind::Defn(function) => Some(function),
                    _ => None,
                }));
            }
            _ => {}
        }
    }

    let mut instances = Vec::new();
    let mut signatures = BTreeMap::<(ScalarOperator, Vec<Type>), String>::new();
    for function in declarations {
        let declared = ast::operator_declaration(&function.metadata).map_err(|error| {
            InterfaceError::new(
                "OSR-I0061",
                match error {
                    ast::OperatorMetadataError::Duplicate => {
                        "operator declaration metadata is duplicated"
                    }
                    ast::OperatorMetadataError::ExpectedName => {
                        "`:osiris/operator` must contain a keyword or symbol"
                    }
                },
            )
        })?;
        let Some(declared) = declared else {
            continue;
        };
        let operator = ScalarOperator::from_stable_name(&declared).ok_or_else(|| {
            InterfaceError::new("OSR-I0061", format!("unknown static operator `{declared}`"))
        })?;
        if !matches!(
            operator,
            ScalarOperator::Add
                | ScalarOperator::Subtract
                | ScalarOperator::Multiply
                | ScalarOperator::TrueDivide
                | ScalarOperator::Less
                | ScalarOperator::LessEqual
                | ScalarOperator::Greater
                | ScalarOperator::GreaterEqual
                | ScalarOperator::Equal
                | ScalarOperator::NotEqual
                | ScalarOperator::Negate
                | ScalarOperator::Positive
                | ScalarOperator::Abs
        ) {
            return Err(InterfaceError::new(
                "OSR-I0061",
                format!(
                    "operator `{}` is not publishable in the v0 capability set",
                    operator.stable_name()
                ),
            ));
        }
        let name = function.name.as_ref().ok_or_else(|| {
            InterfaceError::new("OSR-I0061", "operator implementation requires a name")
        })?;
        if function.return_type.is_none()
            || function
                .params
                .iter()
                .any(|parameter| parameter.type_annotation.is_none())
        {
            return Err(InterfaceError::new(
                "OSR-I0062",
                format!(
                    "operator implementation `{}` requires explicit parameter and return types",
                    name.canonical
                ),
            ));
        }
        let public = public_bindings
            .get(name.canonical.as_str())
            .ok_or_else(|| {
                InterfaceError::new(
                    "OSR-I0062",
                    format!(
                        "operator implementation `{}` must be a public exported function",
                        name.canonical
                    ),
                )
            })?;
        if public.kind != BindingKind::Function {
            return Err(InterfaceError::new(
                "OSR-I0062",
                format!(
                    "operator implementation `{}` is not a function",
                    name.canonical
                ),
            ));
        }
        let (operands, result, summaries) =
            if let Some(function) = function_interfaces.get(public.id.as_str()) {
                (
                    function
                        .parameters
                        .iter()
                        .map(|parameter| parameter.ty.clone())
                        .collect::<Vec<_>>(),
                    function.return_type.clone(),
                    function.summaries.clone(),
                )
            } else {
                let typed_binding = typed_bindings.get(public.id.as_str()).ok_or_else(|| {
                    InterfaceError::new(
                        "OSR-I0062",
                        format!("operator binding `{}` is absent from typed HIR", public.id),
                    )
                })?;
                let Type::Fn(signature) = &typed_binding.ty else {
                    return Err(InterfaceError::new(
                        "OSR-I0062",
                        format!("operator binding `{}` has no function signature", public.id),
                    ));
                };
                (
                    signature.parameters.clone(),
                    (*signature.return_type).clone(),
                    signature.summaries.clone(),
                )
            };
        let expected_arity = usize::from(matches!(
            operator,
            ScalarOperator::Add
                | ScalarOperator::Subtract
                | ScalarOperator::Multiply
                | ScalarOperator::TrueDivide
                | ScalarOperator::Less
                | ScalarOperator::LessEqual
                | ScalarOperator::Greater
                | ScalarOperator::GreaterEqual
                | ScalarOperator::Equal
                | ScalarOperator::NotEqual
        )) + 1;
        if operands.len() != expected_arity {
            return Err(InterfaceError::new(
                "OSR-I0063",
                format!(
                    "operator `{}` expects {expected_arity} operands, found {}",
                    operator.stable_name(),
                    operands.len()
                ),
            ));
        }
        if operands.iter().any(contains_dynamic_operator_type)
            || contains_dynamic_operator_type(&result)
        {
            return Err(InterfaceError::new(
                "OSR-I0063",
                "operator signatures cannot contain Any, Unknown, or Error",
            ));
        }
        let owner_binding = operands
            .iter()
            .find_map(|operand| match operand {
                Type::Nominal { binding, .. } if public_types.contains(binding.as_str()) => {
                    Some(binding.as_str())
                }
                _ => None,
            })
            .ok_or_else(|| {
                InterfaceError::new(
                    "OSR-I0064",
                    format!(
                        "operator `{}` violates the orphan rule: no operand is a public nominal type owned by `{}`",
                        operator.stable_name(),
                        interface.module
                    ),
                )
            })?;
        let instance = OperatorInstance::new(
            public.id.clone(),
            owner_binding,
            operator,
            operands.clone(),
            result,
            summaries,
        );
        if let Some(previous) = signatures.insert((operator, operands), instance.id.clone()) {
            return Err(InterfaceError::new(
                "OSR-I0065",
                format!(
                    "operator `{}` has conflicting instances `{previous}` and `{}`",
                    operator.stable_name(),
                    instance.id
                ),
            ));
        }
        instances.push(instance);
    }
    instances.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(instances)
}

fn contains_dynamic_operator_type(ty: &Type) -> bool {
    match ty {
        Type::Any | Type::Unknown | Type::Error => true,
        Type::Option(inner) | Type::List(inner) | Type::Vector(inner) | Type::Set(inner) => {
            contains_dynamic_operator_type(inner)
        }
        Type::Union(members) | Type::Tuple(members) => {
            members.iter().any(contains_dynamic_operator_type)
        }
        Type::Map(key, value) => {
            contains_dynamic_operator_type(key) || contains_dynamic_operator_type(value)
        }
        Type::Fn(function) => {
            function
                .parameters
                .iter()
                .any(contains_dynamic_operator_type)
                || contains_dynamic_operator_type(&function.return_type)
        }
        Type::Nominal { args, .. } => args.iter().any(contains_dynamic_operator_type),
        _ => false,
    }
}

fn is_publishable_operator(operator: ScalarOperator) -> bool {
    matches!(
        operator,
        ScalarOperator::Add
            | ScalarOperator::Subtract
            | ScalarOperator::Multiply
            | ScalarOperator::TrueDivide
            | ScalarOperator::Less
            | ScalarOperator::LessEqual
            | ScalarOperator::Greater
            | ScalarOperator::GreaterEqual
            | ScalarOperator::Equal
            | ScalarOperator::NotEqual
            | ScalarOperator::Negate
            | ScalarOperator::Positive
            | ScalarOperator::Abs
    )
}

fn operator_arity(operator: ScalarOperator) -> usize {
    usize::from(matches!(
        operator,
        ScalarOperator::Add
            | ScalarOperator::Subtract
            | ScalarOperator::Multiply
            | ScalarOperator::TrueDivide
            | ScalarOperator::FloorDivide
            | ScalarOperator::Remainder
            | ScalarOperator::Less
            | ScalarOperator::LessEqual
            | ScalarOperator::Greater
            | ScalarOperator::GreaterEqual
            | ScalarOperator::Equal
            | ScalarOperator::NotEqual
    )) + 1
}

fn collect_phase_interface(
    surface: &ast::Module,
    module: &str,
) -> InterfaceResult<(Vec<MacroInterface>, Vec<PhaseHelperInterface>)> {
    let exports = surface
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ast::ItemKind::Export(export) => Some(export.names.as_slice()),
            _ => None,
        })
        .flatten()
        .map(|name| name.canonical.clone())
        .collect::<BTreeSet<_>>();

    let mut macro_forms = BTreeMap::<String, Form>::new();
    let mut helper_forms = BTreeMap::<String, Form>::new();
    let mut all_phase_forms = Vec::new();
    for item in &surface.items {
        match &item.kind {
            ast::ItemKind::Defmacro(declaration) => {
                let form = normalize_form(&declaration.phase_form);
                all_phase_forms.push(form.clone());
                macro_forms.insert(declaration.name.canonical.clone(), form);
            }
            ast::ItemKind::DefnForSyntax(declaration) => {
                let Some(name) = declaration.name.as_ref() else {
                    continue;
                };
                let Some(phase_form) = declaration.phase_form.as_ref() else {
                    return Err(InterfaceError::new(
                        "OSR-I0060",
                        format!(
                            "phase-1 helper `{}` lost its declaration form",
                            name.canonical
                        ),
                    ));
                };
                let form = normalize_form(phase_form);
                all_phase_forms.push(form.clone());
                helper_forms.insert(name.canonical.clone(), form);
            }
            _ => {}
        }
    }
    if let Some(diagnostic) = macro_expand::validate_phase_forms(&all_phase_forms).first() {
        return Err(InterfaceError::new(
            "OSR-I0059",
            format!("invalid phase-1 declaration: {}", diagnostic.message),
        ));
    }

    let helper_names = helper_forms.keys().cloned().collect::<BTreeSet<_>>();
    let mut macros = Vec::new();
    let mut required_helpers = BTreeSet::new();
    for (name, phase_ir) in macro_forms
        .into_iter()
        .filter(|(name, _)| exports.contains(name))
    {
        let (declaration_name, parameters, _) = phase_declaration_parts(&phase_ir, "defmacro")?;
        if declaration_name != name {
            return Err(InterfaceError::new(
                "OSR-I0059",
                format!("macro declaration name differs from `{name}`"),
            ));
        }
        let (minimum_arity, variadic) = phase_parameter_arity(parameters)?;
        let closure = phase_helper_closure(&phase_ir, &helper_forms, &helper_names)?;
        required_helpers.extend(closure.iter().cloned());
        macros.push(MacroInterface {
            id: BindingId::new(module, &name, BindingKind::Macro)
                .as_str()
                .to_owned(),
            canonical: name,
            parameters: normalize_form(parameters),
            minimum_arity,
            variadic,
            helper_bindings: closure
                .into_iter()
                .map(|helper| {
                    BindingId::new(module, &helper, BindingKind::Macro)
                        .as_str()
                        .to_owned()
                })
                .collect(),
            phase_ir,
        });
    }
    macros.sort_by(|left, right| left.id.cmp(&right.id));

    let mut phase_helpers = required_helpers
        .into_iter()
        .map(|name| {
            let phase_ir = helper_forms.get(&name).cloned().ok_or_else(|| {
                InterfaceError::new(
                    "OSR-I0060",
                    format!("macro helper closure is missing `{name}`"),
                )
            })?;
            Ok(PhaseHelperInterface {
                id: BindingId::new(module, &name, BindingKind::Macro)
                    .as_str()
                    .to_owned(),
                canonical: name,
                phase_ir,
            })
        })
        .collect::<InterfaceResult<Vec<_>>>()?;
    phase_helpers.sort_by(|left, right| left.id.cmp(&right.id));
    Ok((macros, phase_helpers))
}

fn phase_helper_closure(
    declaration: &Form,
    helper_forms: &BTreeMap<String, Form>,
    helper_names: &BTreeSet<String>,
) -> InterfaceResult<Vec<String>> {
    let mut pending = phase_direct_helper_references(declaration, helper_names)?;
    let mut closure = BTreeSet::new();
    while let Some(name) = pending.pop_first() {
        if !closure.insert(name.clone()) {
            continue;
        }
        let helper = helper_forms.get(&name).ok_or_else(|| {
            InterfaceError::new("OSR-I0060", format!("missing phase-1 helper `{name}`"))
        })?;
        pending.extend(phase_direct_helper_references(helper, helper_names)?);
    }
    Ok(closure.into_iter().collect())
}

fn phase_direct_helper_references(
    declaration: &Form,
    helper_names: &BTreeSet<String>,
) -> InterfaceResult<BTreeSet<String>> {
    let (_, parameters, body) = match phase_declaration_head(declaration)? {
        "defmacro" => phase_declaration_parts(declaration, "defmacro")?,
        "defn-for-syntax" => phase_declaration_parts(declaration, "defn-for-syntax")?,
        _ => {
            return Err(InterfaceError::new(
                "OSR-I0059",
                "phase-1 IR must be a macro or syntax function declaration",
            ));
        }
    };
    let mut bound = BTreeSet::new();
    collect_pattern_bindings(parameters, &mut bound);
    let mut references = BTreeSet::new();
    for form in body {
        collect_phase_references(form, &bound, &mut references);
    }
    references.retain(|name| helper_names.contains(name));
    Ok(references)
}

fn phase_declaration_head(form: &Form) -> InterfaceResult<&str> {
    let FormKind::List(items) = &form.kind else {
        return Err(InterfaceError::new(
            "OSR-I0059",
            "phase-1 IR must be a list",
        ));
    };
    items
        .first()
        .and_then(form_symbol)
        .ok_or_else(|| InterfaceError::new("OSR-I0059", "phase-1 IR requires a symbol head"))
}

fn phase_declaration_parts<'a>(
    form: &'a Form,
    expected: &str,
) -> InterfaceResult<(&'a str, &'a Form, &'a [Form])> {
    let FormKind::List(items) = &form.kind else {
        return Err(InterfaceError::new(
            "OSR-I0059",
            "phase-1 declaration must be a list",
        ));
    };
    if items.first().and_then(form_symbol) != Some(expected) {
        return Err(InterfaceError::new(
            "OSR-I0059",
            format!("expected `{expected}` phase-1 declaration"),
        ));
    }
    let name = items
        .get(1)
        .and_then(form_symbol)
        .ok_or_else(|| InterfaceError::new("OSR-I0059", "phase-1 declaration has no name"))?;
    let mut index = 2;
    if matches!(
        items.get(index).map(|form| &form.kind),
        Some(FormKind::String(_))
    ) {
        index += 1;
    }
    let parameters = items.get(index).ok_or_else(|| {
        InterfaceError::new("OSR-I0059", "phase-1 declaration has no parameter vector")
    })?;
    index += 1;
    if items.get(index).and_then(form_symbol) == Some("->") {
        if items.get(index + 1).is_none() {
            return Err(InterfaceError::new(
                "OSR-I0059",
                "phase-1 return annotation has no type",
            ));
        }
        index += 2;
    }
    if index >= items.len() {
        return Err(InterfaceError::new(
            "OSR-I0059",
            "phase-1 declaration has no body",
        ));
    }
    Ok((name, parameters, &items[index..]))
}

fn phase_parameter_arity(parameters: &Form) -> InterfaceResult<(usize, bool)> {
    let FormKind::Vector(items) = &parameters.kind else {
        return Err(InterfaceError::new(
            "OSR-I0059",
            "macro parameters must be a vector",
        ));
    };
    let mut minimum = 0;
    let mut variadic = false;
    let mut index = 0;
    while index < items.len() {
        if form_symbol(&items[index]) == Some("&") {
            if variadic || index + 2 != items.len() {
                return Err(InterfaceError::new(
                    "OSR-I0059",
                    "`&` must precede the final macro parameter",
                ));
            }
            variadic = true;
            index += 2;
        } else {
            minimum += 1;
            index += 1;
        }
    }
    Ok((minimum, variadic))
}

fn form_symbol(form: &Form) -> Option<&str> {
    match &form.kind {
        FormKind::Symbol(name) => Some(&name.canonical),
        _ => None,
    }
}

fn collect_pattern_bindings(pattern: &Form, bindings: &mut BTreeSet<String>) {
    match &pattern.kind {
        FormKind::Symbol(name) if name.canonical != "_" && name.canonical != "&" => {
            bindings.insert(name.canonical.clone());
        }
        FormKind::Vector(items) => {
            for item in items {
                collect_pattern_bindings(item, bindings);
            }
        }
        _ => {}
    }
}

fn collect_phase_references(
    form: &Form,
    bound: &BTreeSet<String>,
    references: &mut BTreeSet<String>,
) {
    match &form.kind {
        FormKind::Symbol(name) => {
            if !bound.contains(&name.canonical) {
                references.insert(name.canonical.clone());
            }
        }
        FormKind::List(items)
        | FormKind::Vector(items)
        | FormKind::Map(items)
        | FormKind::Set(items) => {
            for item in items {
                collect_phase_references(item, bound, references);
            }
        }
        FormKind::ReaderMacro {
            macro_kind: ReaderMacroKind::Quote,
            ..
        } => {}
        FormKind::ReaderMacro {
            macro_kind: ReaderMacroKind::SyntaxQuote,
            form,
        } => {
            collect_syntax_quote_references(form, bound, references, 1);
        }
        FormKind::ReaderMacro { form, .. } => {
            collect_phase_references(form, bound, references);
        }
        FormKind::None
        | FormKind::Bool(_)
        | FormKind::Integer(_)
        | FormKind::Float(_)
        | FormKind::String(_)
        | FormKind::Keyword(_)
        | FormKind::Error(_) => {}
    }
}

fn collect_syntax_quote_references(
    form: &Form,
    bound: &BTreeSet<String>,
    references: &mut BTreeSet<String>,
    depth: usize,
) {
    match &form.kind {
        FormKind::ReaderMacro {
            macro_kind: ReaderMacroKind::Unquote | ReaderMacroKind::UnquoteSplicing,
            form,
        } if depth == 1 => collect_phase_references(form, bound, references),
        FormKind::ReaderMacro {
            macro_kind: ReaderMacroKind::SyntaxQuote,
            form,
        } => collect_syntax_quote_references(form, bound, references, depth + 1),
        FormKind::ReaderMacro {
            macro_kind: ReaderMacroKind::Unquote | ReaderMacroKind::UnquoteSplicing,
            form,
        } if depth > 1 => collect_syntax_quote_references(form, bound, references, depth - 1),
        FormKind::ReaderMacro {
            macro_kind: ReaderMacroKind::Quote,
            ..
        } => {}
        FormKind::List(items)
        | FormKind::Vector(items)
        | FormKind::Map(items)
        | FormKind::Set(items) => {
            for item in items {
                collect_syntax_quote_references(item, bound, references, depth);
            }
        }
        _ => {}
    }
}

pub fn from_hir(typed: &hir::Module) -> InterfaceResult<Interface> {
    let exports = typed
        .exports
        .iter()
        .map(|id| id.as_str().to_owned())
        .collect::<BTreeSet<_>>();
    let by_id = typed
        .bindings
        .iter()
        .map(|binding| (binding.name.id.as_str(), binding))
        .collect::<BTreeMap<_, _>>();

    let mut bindings = Vec::new();
    for id in &exports {
        let binding = by_id.get(id.as_str()).ok_or_else(|| {
            InterfaceError::new("OSR-I0001", format!("export `{id}` has no typed binding"))
        })?;
        if !binding.public {
            return Err(InterfaceError::new(
                "OSR-I0002",
                format!("export `{id}` is private in typed HIR"),
            ));
        }
        bindings.push(PublicBinding {
            id: id.clone(),
            canonical: binding.name.canonical.clone(),
            python: binding.name.python.clone(),
            kind: binding.name.kind,
            ty: binding.ty.clone(),
            runtime: binding.runtime.as_ref().map(|runtime| RuntimeLocator {
                module: runtime.module.clone(),
                name: runtime.name.clone(),
                python_module: runtime.python_module,
            }),
            metadata: normalize_metadata(&binding.metadata)?,
        });
    }
    bindings.sort_by(|left, right| left.id.cmp(&right.id));

    if let Some(binding) = typed
        .bindings
        .iter()
        .find(|binding| binding.public && !exports.contains(binding.name.id.as_str()))
    {
        return Err(InterfaceError::new(
            "OSR-I0003",
            format!(
                "public binding `{}` is absent from export table",
                binding.name.id.as_str()
            ),
        ));
    }

    let mut aliases = typed
        .aliases
        .iter()
        .filter(|alias| alias.public)
        .map(|alias| {
            if !exports.contains(alias.target.as_str()) {
                return Err(InterfaceError::new(
                    "OSR-I0004",
                    format!(
                        "public alias `{}` targets private binding `{}`",
                        alias.spelling,
                        alias.target.as_str()
                    ),
                ));
            }
            Ok(PublicAlias {
                spelling: alias.spelling.clone(),
                canonical: alias.canonical.clone(),
                target: alias.target.as_str().to_owned(),
            })
        })
        .collect::<InterfaceResult<Vec<_>>>()?;
    for binding in &bindings {
        for alias in metadata_aliases(&binding.canonical, &binding.metadata) {
            aliases.push(PublicAlias {
                spelling: alias.clone(),
                canonical: alias,
                target: binding.id.clone(),
            });
        }
    }
    aliases.sort_by(|left, right| {
        (&left.canonical, &left.target).cmp(&(&right.canonical, &right.target))
    });
    aliases
        .dedup_by(|left, right| left.canonical == right.canonical && left.target == right.target);

    let mut functions = Vec::new();
    let mut structs = Vec::new();
    for item in &typed.items {
        match &item.kind {
            ItemKind::Function(function) if exports.contains(function.binding.as_str()) => {
                let parameters = function
                    .parameters
                    .iter()
                    .map(|parameter| {
                        let binding = by_id.get(parameter.binding.as_str()).ok_or_else(|| {
                            InterfaceError::new(
                                "OSR-I0005",
                                format!(
                                    "parameter `{}` has no typed binding",
                                    parameter.binding.as_str()
                                ),
                            )
                        })?;
                        let metadata = normalize_metadata(&binding.metadata)?;
                        Ok(ParameterInterface {
                            id: parameter.binding.as_str().to_owned(),
                            canonical: binding.name.canonical.clone(),
                            ty: parameter.ty.clone(),
                            has_default: parameter.default.is_some(),
                            variadic: parameter.variadic,
                            aliases: metadata_aliases(&binding.name.canonical, &metadata),
                            metadata,
                        })
                    })
                    .collect::<InterfaceResult<Vec<_>>>()?;
                functions.push(FunctionInterface {
                    binding: function.binding.as_str().to_owned(),
                    parameters,
                    return_type: function.return_type.clone(),
                    contract_id: None,
                    summaries: function.summaries.clone(),
                });
            }
            ItemKind::Struct(structure) if exports.contains(structure.binding.as_str()) => {
                let fields = structure
                    .fields
                    .iter()
                    .map(|field| {
                        let binding = by_id.get(field.binding.as_str()).ok_or_else(|| {
                            InterfaceError::new(
                                "OSR-I0006",
                                format!("field `{}` has no typed binding", field.binding.as_str()),
                            )
                        })?;
                        let metadata = normalize_metadata(&binding.metadata)?;
                        Ok(FieldInterface {
                            id: field.binding.as_str().to_owned(),
                            canonical: binding.name.canonical.clone(),
                            ty: field.ty.clone(),
                            has_default: field.default.is_some(),
                            aliases: metadata_aliases(&binding.name.canonical, &metadata),
                            metadata,
                        })
                    })
                    .collect::<InterfaceResult<Vec<_>>>()?;
                structs.push(StructInterface {
                    binding: structure.binding.as_str().to_owned(),
                    type_parameters: structure.type_parameters.clone(),
                    fields,
                    invariant_count: structure.checks.len(),
                    doc: structure.doc.clone(),
                });
            }
            _ => {}
        }
    }
    for function in &typed.extern_functions {
        if !exports.contains(function.binding.as_str()) {
            continue;
        }
        let parameters = function
            .parameters
            .iter()
            .map(|parameter| {
                let binding = by_id.get(parameter.binding.as_str()).ok_or_else(|| {
                    InterfaceError::new(
                        "OSR-I0005",
                        format!(
                            "extern parameter `{}` has no typed binding",
                            parameter.binding.as_str()
                        ),
                    )
                })?;
                let metadata = normalize_metadata(&binding.metadata)?;
                Ok(ParameterInterface {
                    id: parameter.binding.as_str().to_owned(),
                    canonical: binding.name.canonical.clone(),
                    ty: parameter.ty.clone(),
                    has_default: parameter.default.is_some(),
                    variadic: parameter.variadic,
                    aliases: metadata_aliases(&binding.name.canonical, &metadata),
                    metadata,
                })
            })
            .collect::<InterfaceResult<Vec<_>>>()?;
        functions.push(FunctionInterface {
            binding: function.binding.as_str().to_owned(),
            parameters,
            return_type: function.return_type.clone(),
            contract_id: function.contract_id.clone(),
            summaries: function.summaries.clone(),
        });
    }
    functions.sort_by(|left, right| left.binding.cmp(&right.binding));
    structs.sort_by(|left, right| left.binding.cmp(&right.binding));

    let mut interface = Interface {
        format_version: FORMAT_VERSION,
        compiler_abi: COMPILER_ABI.to_owned(),
        language_abi: LANGUAGE_ABI.to_owned(),
        module: typed.name.clone(),
        metadata: normalize_metadata(&typed.metadata)?,
        bindings,
        aliases,
        functions,
        structs,
        operator_instances: Vec::new(),
        macros: Vec::new(),
        phase_helpers: Vec::new(),
        static_schemas: Vec::new(),
        owned_records: Vec::new(),
        graph: empty_hash_group(&typed.name),
        hashes: InterfaceHashes {
            interface_body: String::new(),
            semantic_body: String::new(),
            tooling_body: String::new(),
            content_integrity: String::new(),
        },
    };
    validate_model(&interface)?;
    refresh_standalone_hashes(&mut interface)?;
    Ok(interface)
}

/// Build the non-executable public shape of a module before its bodies have
/// been type checked.  This is used only while compiling a runtime SCC: every
/// member can see the other members' names and declared signatures, while no
/// body-derived effect/temporal/data fact is fabricated.  Callers must replace
/// the result with a normal [`build_with_static_data`] interface before
/// publishing an `.osri` artifact.
///
/// A provisional interface deliberately has empty graph/hash fields and is not
/// suitable for serialization or trust decisions.  Its function and operator
/// summaries are [`CallSummaries::unknown`].
pub(crate) fn build_provisional(surface: &ast::Module) -> InterfaceResult<Interface> {
    let module = surface
        .name
        .as_ref()
        .map(|name| name.canonical.clone())
        .ok_or_else(|| {
            InterfaceError::new("OSR-I0080", "provisional interface has no module name")
        })?;

    let exports = surface
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ast::ItemKind::Export(export) => Some(export.names.iter()),
            _ => None,
        })
        .flatten()
        .map(|name| name.canonical.clone())
        .collect::<BTreeSet<_>>();

    let mut declarations = BTreeMap::<String, ProvisionalDeclaration>::new();
    let mut type_variable = 0u32;
    for item in &surface.items {
        collect_provisional_item(&module, &item.kind, &mut declarations, &mut type_variable)?;
    }
    let type_resolutions = declarations
        .iter()
        .filter(|(_, declaration)| declaration.binding.kind == BindingKind::Type)
        .map(|(name, declaration)| (name.clone(), declaration.binding.id.clone()))
        .collect::<BTreeMap<_, _>>();
    for declaration in declarations.values_mut() {
        declaration.binding.ty =
            hir::resolve_nominal_bindings(&declaration.binding.ty, &type_resolutions, "");
        if let Some(function) = &mut declaration.function {
            for parameter in &mut function.parameters {
                parameter.ty = hir::resolve_nominal_bindings(&parameter.ty, &type_resolutions, "");
            }
            function.return_type =
                hir::resolve_nominal_bindings(&function.return_type, &type_resolutions, "");
        }
        if let Some(structure) = &mut declaration.structure {
            for field in &mut structure.fields {
                field.ty = hir::resolve_nominal_bindings(&field.ty, &type_resolutions, "");
            }
        }
    }
    // Operator ownership may refer to a struct declared later in the source;
    // resolve those declarations only after the complete local shape exists.
    for item in &surface.items {
        let mut functions = Vec::<&ast::Function>::new();
        match &item.kind {
            ast::ItemKind::Defn(function) => functions.push(function),
            ast::ItemKind::Extern(external) => {
                functions.extend(
                    external
                        .items
                        .iter()
                        .filter_map(|nested| match &nested.kind {
                            ast::ItemKind::Defn(function) => Some(function),
                            _ => None,
                        }),
                );
            }
            _ => {}
        }
        for function in functions {
            let Some(name) = &function.name else {
                continue;
            };
            let Some(declaration) = declarations.get(&name.canonical) else {
                continue;
            };
            let Some(signature) = declaration.function.clone() else {
                continue;
            };
            let binding = declaration.binding.clone();
            let operator = provisional_operator(function, &binding, &signature, &declarations);
            if let Some(declaration) = declarations.get_mut(&name.canonical) {
                declaration.operator = operator;
            }
        }
    }

    let mut bindings = declarations
        .iter()
        .filter(|(name, _)| exports.contains(*name))
        .map(|(_, declaration)| declaration.binding.clone())
        .collect::<Vec<_>>();
    bindings.sort_by(|left, right| left.id.cmp(&right.id));

    let exported_ids = bindings
        .iter()
        .map(|binding| binding.id.clone())
        .collect::<BTreeSet<_>>();
    let mut aliases = Vec::new();
    for item in &surface.items {
        let ast::ItemKind::Alias(alias) = &item.kind else {
            continue;
        };
        if !exports.contains(&alias.local.canonical) {
            continue;
        }
        let Some(target) = declarations.get(&alias.target.canonical) else {
            continue;
        };
        // Match HIR's boundary rule: a public alias cannot expose a private
        // canonical target.  The final lowering pass remains authoritative;
        // omission here merely makes an invalid provisional import fail closed.
        if !exports.contains(&alias.target.canonical) || !exported_ids.contains(&target.binding.id)
        {
            continue;
        }
        aliases.push(PublicAlias {
            spelling: alias.local.spelling.clone(),
            canonical: alias.local.canonical.clone(),
            target: target.binding.id.clone(),
        });
    }
    for binding in &bindings {
        for alias in metadata_aliases(&binding.canonical, &binding.metadata) {
            aliases.push(PublicAlias {
                spelling: alias.clone(),
                canonical: alias,
                target: binding.id.clone(),
            });
        }
    }
    aliases.sort_by(|left, right| {
        (&left.canonical, &left.target).cmp(&(&right.canonical, &right.target))
    });
    aliases
        .dedup_by(|left, right| left.canonical == right.canonical && left.target == right.target);

    let mut functions = declarations
        .iter()
        .filter(|(name, _)| exports.contains(*name))
        .filter_map(|(_, declaration)| declaration.function.clone())
        .collect::<Vec<_>>();
    functions.sort_by(|left, right| left.binding.cmp(&right.binding));
    let mut structs = declarations
        .iter()
        .filter(|(name, _)| exports.contains(*name))
        .filter_map(|(_, declaration)| declaration.structure.clone())
        .collect::<Vec<_>>();
    structs.sort_by(|left, right| left.binding.cmp(&right.binding));

    let mut operator_instances = declarations
        .iter()
        .filter(|(name, _)| exports.contains(*name))
        .filter_map(|(_, declaration)| declaration.operator.clone())
        .collect::<Vec<_>>();
    operator_instances.sort_by(|left, right| left.id.cmp(&right.id));

    // Static schemas are data-only and do not depend on function bodies.  A
    // best-effort projection here lets cyclic modules refer to a schema while
    // records are checked again against final interfaces later.
    let static_data = records::analyze_module(surface);
    let static_schemas = static_data
        .schemas
        .into_iter()
        .filter(|schema| exports.contains(&schema.name))
        .collect::<Vec<_>>();

    let (macros, phase_helpers) = collect_phase_interface(surface, &module)?;

    Ok(Interface {
        format_version: FORMAT_VERSION,
        compiler_abi: COMPILER_ABI.to_owned(),
        language_abi: LANGUAGE_ABI.to_owned(),
        module: module.clone(),
        metadata: surface.metadata.clone(),
        bindings,
        aliases,
        functions,
        structs,
        operator_instances,
        macros,
        phase_helpers,
        static_schemas,
        owned_records: Vec::new(),
        graph: empty_hash_group(&module),
        hashes: InterfaceHashes {
            interface_body: String::new(),
            semantic_body: String::new(),
            tooling_body: String::new(),
            content_integrity: String::new(),
        },
    })
}

#[derive(Clone)]
struct ProvisionalDeclaration {
    binding: PublicBinding,
    function: Option<FunctionInterface>,
    structure: Option<StructInterface>,
    operator: Option<OperatorInstance>,
}

fn collect_provisional_item(
    module: &str,
    item: &ast::ItemKind,
    declarations: &mut BTreeMap<String, ProvisionalDeclaration>,
    next_type_variable: &mut u32,
) -> InterfaceResult<()> {
    match item {
        ast::ItemKind::Def(definition) => {
            let binding = provisional_value_binding(
                module,
                &definition.name,
                definition
                    .type_annotation
                    .as_ref()
                    .map_or(Type::Unknown, hir::type_from_ast),
                definition.metadata.clone(),
                None,
            );
            declarations.insert(
                definition.name.canonical.clone(),
                ProvisionalDeclaration {
                    binding,
                    function: None,
                    structure: None,
                    operator: None,
                },
            );
        }
        ast::ItemKind::Defn(function) => {
            collect_provisional_function(module, function, None, declarations, next_type_variable)?;
        }
        ast::ItemKind::Defstruct(structure) => {
            collect_provisional_struct(module, structure, declarations, next_type_variable)?;
        }
        ast::ItemKind::DefstaticSchema(schema) => {
            let binding_id = BindingId::new(module, &schema.name.canonical, BindingKind::Type);
            let binding = PublicBinding {
                id: binding_id.as_str().to_owned(),
                canonical: schema.name.canonical.clone(),
                python: python_identifier(&schema.name.canonical),
                kind: BindingKind::Type,
                ty: Type::Nominal {
                    binding: binding_id.as_str().to_owned(),
                    args: Vec::new(),
                },
                runtime: None,
                metadata: normalize_metadata(&schema.metadata)?,
            };
            declarations.insert(
                schema.name.canonical.clone(),
                ProvisionalDeclaration {
                    binding,
                    function: None,
                    structure: None,
                    operator: None,
                },
            );
        }
        ast::ItemKind::Extern(external) => {
            for nested in &external.items {
                match &nested.kind {
                    ast::ItemKind::Def(definition) => {
                        let binding = provisional_value_binding(
                            module,
                            &definition.name,
                            definition
                                .type_annotation
                                .as_ref()
                                .map_or(Type::Any, hir::type_from_ast),
                            definition.metadata.clone(),
                            Some(RuntimeLocator {
                                module: external.module.clone(),
                                name: python_identifier(&definition.name.canonical),
                                python_module: true,
                            }),
                        );
                        declarations.insert(
                            definition.name.canonical.clone(),
                            ProvisionalDeclaration {
                                binding,
                                function: None,
                                structure: None,
                                operator: None,
                            },
                        );
                    }
                    ast::ItemKind::Defn(function) => {
                        collect_provisional_function(
                            module,
                            function,
                            Some(external.module.as_str()),
                            declarations,
                            next_type_variable,
                        )?;
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn provisional_value_binding(
    module: &str,
    name: &Name,
    ty: Type,
    metadata: Vec<MetadataEntry>,
    runtime: Option<RuntimeLocator>,
) -> PublicBinding {
    PublicBinding {
        id: BindingId::new(module, &name.canonical, BindingKind::Value)
            .as_str()
            .to_owned(),
        canonical: name.canonical.clone(),
        python: python_identifier(&name.canonical),
        kind: BindingKind::Value,
        ty,
        runtime,
        metadata,
    }
}

fn collect_provisional_function(
    module: &str,
    function: &ast::Function,
    runtime_module: Option<&str>,
    declarations: &mut BTreeMap<String, ProvisionalDeclaration>,
    _next_type_variable: &mut u32,
) -> InterfaceResult<()> {
    let Some(name) = &function.name else {
        return Ok(());
    };
    let parameters = function
        .params
        .iter()
        .enumerate()
        .map(|(index, parameter)| {
            let metadata = normalize_metadata(&parameter.metadata)?;
            Ok(ParameterInterface {
                id: format!(
                    "{}::provisional-parameter-{index}",
                    BindingId::new(module, &name.canonical, BindingKind::Function).as_str()
                ),
                canonical: parameter.name.canonical.clone(),
                ty: parameter
                    .type_annotation
                    .as_ref()
                    .map_or(Type::Unknown, hir::type_from_ast),
                has_default: parameter.default.is_some(),
                variadic: parameter.variadic,
                aliases: metadata_aliases(&parameter.name.canonical, &metadata),
                metadata,
            })
        })
        .collect::<InterfaceResult<Vec<_>>>()?;
    let return_type = function
        .return_type
        .as_ref()
        .map_or(Type::Unknown, hir::type_from_ast);
    let summaries = function
        .contract
        .as_ref()
        .map_or_else(CallSummaries::unknown, |contract| {
            contract.summaries.clone()
        });
    let binding_id = BindingId::new(module, &name.canonical, BindingKind::Function);
    let signature = FunctionType::new(
        parameters
            .iter()
            .map(|parameter| parameter.ty.clone())
            .collect(),
        return_type.clone(),
    )
    .with_summaries(summaries.clone());
    let binding = PublicBinding {
        id: binding_id.as_str().to_owned(),
        canonical: name.canonical.clone(),
        python: python_identifier(&name.canonical),
        kind: BindingKind::Function,
        ty: Type::Fn(signature),
        runtime: runtime_module.map(|runtime_module| RuntimeLocator {
            module: runtime_module.to_owned(),
            name: python_identifier(&name.canonical),
            python_module: true,
        }),
        metadata: normalize_metadata(&function.metadata)?,
    };
    let function_interface = FunctionInterface {
        binding: binding_id.as_str().to_owned(),
        parameters,
        return_type,
        contract_id: function
            .contract
            .as_ref()
            .map(|contract| contract.id.clone()),
        summaries,
    };
    declarations.insert(
        name.canonical.clone(),
        ProvisionalDeclaration {
            binding,
            function: Some(function_interface),
            structure: None,
            operator: None,
        },
    );
    Ok(())
}

fn collect_provisional_struct(
    module: &str,
    structure: &ast::Defstruct,
    declarations: &mut BTreeMap<String, ProvisionalDeclaration>,
    next_type_variable: &mut u32,
) -> InterfaceResult<()> {
    let mut generic_parameters = BTreeMap::new();
    let mut type_parameters = Vec::new();
    for parameter in &structure.type_params {
        let variable = Type::TypeVar(TypeVarId(*next_type_variable));
        *next_type_variable = (*next_type_variable).saturating_add(1);
        generic_parameters.insert(parameter.canonical.clone(), variable);
        type_parameters.push(parameter.canonical.clone());
    }
    let binding_id = BindingId::new(module, &structure.name.canonical, BindingKind::Type);
    let nominal = Type::Nominal {
        binding: binding_id.as_str().to_owned(),
        args: type_parameters
            .iter()
            .filter_map(|name| generic_parameters.get(name).cloned())
            .collect(),
    };
    let binding = PublicBinding {
        id: binding_id.as_str().to_owned(),
        canonical: structure.name.canonical.clone(),
        python: python_identifier(&structure.name.canonical),
        kind: BindingKind::Type,
        ty: nominal,
        runtime: None,
        metadata: normalize_metadata(&structure.metadata)?,
    };
    let fields = structure
        .fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            Ok(FieldInterface {
                id: format!("{}::provisional-field-{index}", binding_id.as_str()),
                canonical: field.name.canonical.clone(),
                ty: field.type_annotation.as_ref().map_or(Type::Unknown, |ty| {
                    hir::type_from_ast_with_generics(ty, &generic_parameters)
                }),
                has_default: field.default.is_some(),
                aliases: metadata_aliases(
                    &field.name.canonical,
                    &normalize_metadata(&field.metadata)?,
                ),
                metadata: normalize_metadata(&field.metadata)?,
            })
        })
        .collect::<InterfaceResult<Vec<_>>>()?;
    let structure_interface = StructInterface {
        binding: binding_id.as_str().to_owned(),
        type_parameters,
        fields,
        invariant_count: structure.checks.len(),
        doc: structure.doc.clone(),
    };
    declarations.insert(
        structure.name.canonical.clone(),
        ProvisionalDeclaration {
            binding,
            function: None,
            structure: Some(structure_interface),
            operator: None,
        },
    );
    Ok(())
}

fn provisional_operator(
    function: &ast::Function,
    binding: &PublicBinding,
    signature: &FunctionInterface,
    declarations: &BTreeMap<String, ProvisionalDeclaration>,
) -> Option<OperatorInstance> {
    let declared = ast::operator_declaration(&function.metadata)
        .ok()
        .flatten()?;
    let operator = ScalarOperator::from_stable_name(&declared)?;
    let operands = signature
        .parameters
        .iter()
        .map(|parameter| parameter.ty.clone())
        .collect::<Vec<_>>();
    let owner_binding = operands.iter().find_map(|operand| {
        let Type::Nominal {
            binding: nominal_binding,
            ..
        } = operand
        else {
            return None;
        };
        declarations.values().find_map(|declaration| {
            (declaration.binding.kind == BindingKind::Type
                && declaration.binding.id == *nominal_binding)
                .then(|| declaration.binding.id.clone())
        })
    })?;
    Some(OperatorInstance::new(
        binding.id.clone(),
        owner_binding,
        operator,
        operands,
        signature.return_type.clone(),
        CallSummaries::unknown(),
    ))
}

/// Check that a final interface retains the public shape advertised by a
/// provisional SCC interface.  Body-derived call summaries are intentionally
/// excluded from this comparison; `Unknown` in the provisional model acts as
/// a wildcard for value types and recursive type-variable ids are compared
/// structurally.
pub(crate) fn validate_provisional_shape(
    provisional: &Interface,
    final_interface: &Interface,
) -> InterfaceResult<()> {
    let provisional_bindings = provisional
        .bindings
        .iter()
        .map(|binding| (binding.canonical.as_str(), binding))
        .collect::<BTreeMap<_, _>>();
    let final_bindings = final_interface
        .bindings
        .iter()
        .map(|binding| (binding.canonical.as_str(), binding))
        .collect::<BTreeMap<_, _>>();
    if provisional_bindings.len() != final_bindings.len()
        || provisional_bindings
            .keys()
            .any(|name| !final_bindings.contains_key(name))
    {
        return Err(InterfaceError::new(
            "OSR-I0081",
            format!(
                "final interface `{}` changed its exported binding set",
                provisional.module
            ),
        ));
    }
    for (name, expected) in &provisional_bindings {
        let actual = final_bindings
            .get(name)
            .expect("binding set was checked above");
        if expected.kind != actual.kind || !provisional_type_matches(&expected.ty, &actual.ty) {
            return Err(InterfaceError::new(
                "OSR-I0081",
                format!(
                    "final interface `{}` changed binding `{name}`",
                    provisional.module
                ),
            ));
        }
    }

    let provisional_functions = provisional
        .functions
        .iter()
        .filter_map(|function| {
            provisional_bindings
                .values()
                .find(|binding| binding.id == function.binding)
                .map(|binding| (binding.canonical.as_str(), function))
        })
        .collect::<BTreeMap<_, _>>();
    let final_functions = final_interface
        .functions
        .iter()
        .filter_map(|function| {
            final_bindings
                .values()
                .find(|binding| binding.id == function.binding)
                .map(|binding| (binding.canonical.as_str(), function))
        })
        .collect::<BTreeMap<_, _>>();
    if provisional_functions.len() != final_functions.len() {
        return Err(InterfaceError::new(
            "OSR-I0081",
            format!(
                "final interface `{}` changed function declarations",
                provisional.module
            ),
        ));
    }
    for (name, expected) in provisional_functions {
        let Some(actual) = final_functions.get(name) else {
            return Err(InterfaceError::new(
                "OSR-I0081",
                format!(
                    "final interface `{}` removed function `{name}`",
                    provisional.module
                ),
            ));
        };
        if expected.parameters.len() != actual.parameters.len()
            || expected
                .parameters
                .iter()
                .zip(&actual.parameters)
                .any(|(left, right)| {
                    left.canonical != right.canonical
                        || left.has_default != right.has_default
                        || left.variadic != right.variadic
                        || !provisional_type_matches(&left.ty, &right.ty)
                })
            || !provisional_type_matches(&expected.return_type, &actual.return_type)
        {
            return Err(InterfaceError::new(
                "OSR-I0081",
                format!(
                    "final interface `{}` changed function `{name}`",
                    provisional.module
                ),
            ));
        }
    }

    let provisional_structs = provisional
        .structs
        .iter()
        .filter_map(|structure| {
            provisional_bindings
                .values()
                .find(|binding| binding.id == structure.binding)
                .map(|binding| (binding.canonical.as_str(), structure))
        })
        .collect::<BTreeMap<_, _>>();
    let final_structs = final_interface
        .structs
        .iter()
        .filter_map(|structure| {
            final_bindings
                .values()
                .find(|binding| binding.id == structure.binding)
                .map(|binding| (binding.canonical.as_str(), structure))
        })
        .collect::<BTreeMap<_, _>>();
    if provisional_structs.len() != final_structs.len() {
        return Err(InterfaceError::new(
            "OSR-I0081",
            format!(
                "final interface `{}` changed struct declarations",
                provisional.module
            ),
        ));
    }
    for (name, expected) in provisional_structs {
        let Some(actual) = final_structs.get(name) else {
            return Err(InterfaceError::new(
                "OSR-I0081",
                format!(
                    "final interface `{}` removed struct `{name}`",
                    provisional.module
                ),
            ));
        };
        if expected.type_parameters != actual.type_parameters
            || expected.fields.len() != actual.fields.len()
            || expected
                .fields
                .iter()
                .zip(&actual.fields)
                .any(|(left, right)| {
                    left.canonical != right.canonical
                        || left.has_default != right.has_default
                        || !provisional_type_matches(&left.ty, &right.ty)
                })
        {
            return Err(InterfaceError::new(
                "OSR-I0081",
                format!(
                    "final interface `{}` changed struct `{name}`",
                    provisional.module
                ),
            ));
        }
    }

    let aliases = |interface: &Interface| {
        interface
            .aliases
            .iter()
            .filter_map(|alias| {
                interface
                    .bindings
                    .iter()
                    .find(|binding| binding.id == alias.target)
                    .map(|binding| (alias.canonical.clone(), binding.canonical.clone()))
            })
            .collect::<BTreeSet<_>>()
    };
    if aliases(provisional) != aliases(final_interface) {
        return Err(InterfaceError::new(
            "OSR-I0081",
            format!("final interface `{}` changed aliases", provisional.module),
        ));
    }

    let operators = |interface: &Interface| {
        interface
            .operator_instances
            .iter()
            .map(|instance| {
                (
                    instance.operator,
                    instance
                        .binding
                        .split("::function::")
                        .nth(1)
                        .unwrap_or(instance.binding.as_str())
                        .to_owned(),
                    instance.owner_binding.clone(),
                    instance.operands.clone(),
                    instance.result.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    let provisional_operators = operators(provisional);
    let final_operators = operators(final_interface);
    if provisional_operators.len() != final_operators.len()
        || provisional_operators
            .iter()
            .zip(&final_operators)
            .any(|(left, right)| {
                left.0 != right.0
                    || left.1 != right.1
                    || left.2 != right.2
                    || left.3.len() != right.3.len()
                    || left
                        .3
                        .iter()
                        .zip(&right.3)
                        .any(|(a, b)| !provisional_type_matches(a, b))
                    || !provisional_type_matches(&left.4, &right.4)
            })
    {
        return Err(InterfaceError::new(
            "OSR-I0081",
            format!(
                "final interface `{}` changed operator declarations",
                provisional.module
            ),
        ));
    }
    Ok(())
}

fn provisional_type_matches(expected: &Type, actual: &Type) -> bool {
    match (expected, actual) {
        (Type::Unknown, _) | (_, Type::Unknown) => true,
        (Type::TypeVar(_), Type::TypeVar(_)) => true,
        (Type::Option(left), Type::Option(right))
        | (Type::List(left), Type::List(right))
        | (Type::Vector(left), Type::Vector(right))
        | (Type::Set(left), Type::Set(right)) => provisional_type_matches(left, right),
        (Type::Map(left_key, left_value), Type::Map(right_key, right_value)) => {
            provisional_type_matches(left_key, right_key)
                && provisional_type_matches(left_value, right_value)
        }
        (Type::Union(left), Type::Union(right)) | (Type::Tuple(left), Type::Tuple(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| provisional_type_matches(left, right))
        }
        (Type::Fn(left), Type::Fn(right)) => {
            left.parameters.len() == right.parameters.len()
                && left
                    .parameters
                    .iter()
                    .zip(&right.parameters)
                    .all(|(left, right)| provisional_type_matches(left, right))
                && provisional_type_matches(&left.return_type, &right.return_type)
        }
        (
            Type::Nominal {
                binding: left_binding,
                args: left_args,
            },
            Type::Nominal {
                binding: right_binding,
                args: right_args,
            },
        ) => {
            (left_binding == right_binding
                || (!left_binding.contains("::type::")
                    && nominal_short_name(right_binding) == left_binding))
                && left_args.len() == right_args.len()
                && left_args
                    .iter()
                    .zip(right_args)
                    .all(|(left, right)| provisional_type_matches(left, right))
        }
        (left, right) => left == right,
    }
}

pub fn emit(typed: &hir::Module, surface: &ast::Module) -> InterfaceResult<String> {
    render(&build(typed, surface)?)
}

pub fn render(interface: &Interface) -> InterfaceResult<String> {
    validate(interface)?;
    Ok(render_forms(&file_forms(interface, true)))
}

/// Parses and validates `.osri` without importing or executing Python.
pub fn read(source: &str) -> InterfaceResult<Interface> {
    let document = reader::read(source);
    if let Some(diagnostic) = document.diagnostics.first() {
        if diagnostic.code == "OSR-R0007" {
            return Err(InterfaceError::new(
                "OSR-I0043",
                "duplicate map key in interface",
            ));
        }
        return Err(InterfaceError::new(
            "OSR-I0010",
            format!("invalid S-expression: {}", diagnostic.message),
        ));
    }
    if document.forms.len() != 4 {
        return Err(InterfaceError::new(
            "OSR-I0011",
            "interface requires exactly header, body, graph, and hashes forms",
        ));
    }
    for form in &document.forms {
        reject_duplicate_maps(form)?;
    }
    let header = unwrap(&document.forms[0], "osiris-interface/header")?;
    let body = unwrap(&document.forms[1], "osiris-interface/body")?;
    let graph = unwrap(&document.forms[2], "osiris-interface/graph")?;
    let hashes = unwrap(&document.forms[3], "osiris-interface/hashes")?;
    let (format_version, compiler_abi, language_abi) = decode_header(header)?;
    let (
        module,
        metadata,
        bindings,
        aliases,
        functions,
        structs,
        operator_instances,
        macros,
        phase_helpers,
        static_schemas,
        owned_records,
    ) = decode_body(body)?;
    let mut interface = Interface {
        format_version,
        compiler_abi,
        language_abi,
        module,
        metadata,
        bindings,
        aliases,
        functions,
        structs,
        operator_instances,
        macros,
        phase_helpers,
        static_schemas,
        owned_records,
        graph: decode_graph(graph)?,
        hashes: decode_hashes(hashes)?,
    };
    normalize_model(&mut interface)?;
    validate(&interface)?;
    Ok(interface)
}

pub use read as parse;

fn validate(interface: &Interface) -> InterfaceResult<()> {
    if interface.format_version != FORMAT_VERSION {
        return Err(InterfaceError::new(
            "OSR-I0012",
            format!("unsupported format version `{}`", interface.format_version),
        ));
    }
    if interface.compiler_abi != COMPILER_ABI {
        return Err(InterfaceError::new(
            "OSR-I0013",
            format!("incompatible compiler ABI `{}`", interface.compiler_abi),
        ));
    }
    if interface.language_abi != LANGUAGE_ABI {
        return Err(InterfaceError::new(
            "OSR-I0014",
            format!("incompatible language ABI `{}`", interface.language_abi),
        ));
    }
    validate_model(interface)?;
    verify_interface_hash_group(&interface.graph)
        .map_err(|error| InterfaceError::new("OSR-I0073", error.to_string()))?;
    let expected = calculate_hashes(interface);
    let member = interface
        .graph
        .members
        .iter()
        .find(|member| member.module == interface.module)
        .ok_or_else(|| {
            InterfaceError::new(
                "OSR-I0073",
                format!(
                    "interface graph group does not contain module `{}`",
                    interface.module
                ),
            )
        })?;
    if member.semantic_body_hash != expected.semantic_body
        || member.tooling_body_hash != expected.tooling_body
    {
        return Err(InterfaceError::new(
            "OSR-I0073",
            "interface graph member body hashes do not match the interface body",
        ));
    }
    if interface.hashes != expected {
        return Err(InterfaceError::new(
            "OSR-I0015",
            "interface hash validation failed",
        ));
    }
    Ok(())
}

fn validate_model(interface: &Interface) -> InterfaceResult<()> {
    validate_interface_metadata_resources(interface)?;
    if interface.module.is_empty() {
        return Err(InterfaceError::new("OSR-I0016", "empty module name"));
    }
    unique(
        interface.bindings.iter().map(|binding| &binding.id),
        "binding id",
    )?;
    unique(
        interface.bindings.iter().map(|binding| &binding.canonical),
        "binding name",
    )?;
    unique(
        interface.aliases.iter().map(|alias| &alias.canonical),
        "alias",
    )?;
    unique(
        interface.functions.iter().map(|function| &function.binding),
        "function",
    )?;
    unique(
        interface.structs.iter().map(|structure| &structure.binding),
        "struct",
    )?;
    unique(
        interface
            .operator_instances
            .iter()
            .map(|instance| &instance.id),
        "operator instance id",
    )?;
    unique(
        interface.static_schemas.iter().map(|schema| &schema.name),
        "static schema name",
    )?;
    let mut schema_versions = BTreeSet::new();
    for schema in &interface.static_schemas {
        if !schema_versions.insert((&schema.schema_id, schema.version)) {
            return Err(InterfaceError::new(
                "OSR-I0024",
                format!(
                    "duplicate static schema id/version `{}@{}`",
                    schema.schema_id, schema.version
                ),
            ));
        }
    }
    unique(
        interface
            .owned_records
            .iter()
            .map(|record| &record.stable_record_id),
        "owned static record",
    )?;
    let bindings = interface
        .bindings
        .iter()
        .map(|binding| (binding.id.as_str(), binding))
        .collect::<BTreeMap<_, _>>();
    validate_nominal_type_identities(interface, &bindings)?;
    let mut names = interface
        .bindings
        .iter()
        .map(|binding| binding.canonical.as_str())
        .collect::<BTreeSet<_>>();
    for alias in &interface.aliases {
        if !bindings.contains_key(alias.target.as_str()) {
            return Err(InterfaceError::new(
                "OSR-I0017",
                format!(
                    "alias `{}` leaks missing/private target `{}`",
                    alias.spelling, alias.target
                ),
            ));
        }
        if !names.insert(&alias.canonical) {
            return Err(InterfaceError::new(
                "OSR-I0018",
                format!("public name `{}` is duplicated", alias.canonical),
            ));
        }
    }
    let mut contract_ids = BTreeSet::new();
    for function in &interface.functions {
        let binding = bindings.get(function.binding.as_str()).ok_or_else(|| {
            InterfaceError::new(
                "OSR-I0019",
                format!("function `{}` leaks a private binding", function.binding),
            )
        })?;
        if binding.kind != BindingKind::Function {
            return Err(InterfaceError::new(
                "OSR-I0020",
                format!("function `{}` references a non-function", function.binding),
            ));
        }
        if let Some(contract_id) = &function.contract_id {
            if contract_id.is_empty()
                || contract_id.trim() != contract_id
                || contract_id.chars().any(char::is_control)
            {
                return Err(InterfaceError::new(
                    "OSR-I0074",
                    format!("function `{}` has an invalid contract id", function.binding),
                ));
            }
            if !contract_ids.insert(contract_id) {
                return Err(InterfaceError::new(
                    "OSR-I0074",
                    format!("duplicate declared contract id `{contract_id}`"),
                ));
            }
        }
        unique(
            function.parameters.iter().map(|parameter| &parameter.id),
            "parameter id",
        )?;
        let mut parameter_names = BTreeSet::new();
        for parameter in &function.parameters {
            for name in std::iter::once(&parameter.canonical).chain(&parameter.aliases) {
                if !parameter_names.insert(name) {
                    return Err(InterfaceError::new(
                        "OSR-I0021",
                        format!("duplicate parameter name `{name}`"),
                    ));
                }
            }
        }
        let Type::Fn(signature) = &binding.ty else {
            return Err(InterfaceError::new(
                "OSR-I0074",
                format!(
                    "function `{}` binding has no function type",
                    function.binding
                ),
            ));
        };
        let parameters = function
            .parameters
            .iter()
            .map(|parameter| parameter.ty.clone())
            .collect::<Vec<_>>();
        if signature.parameters != parameters
            || signature.return_type.as_ref() != &function.return_type
            || signature.summaries != function.summaries
        {
            return Err(InterfaceError::new(
                "OSR-I0074",
                format!(
                    "function `{}` interface differs from its binding signature",
                    function.binding
                ),
            ));
        }
    }
    for structure in &interface.structs {
        let binding = bindings.get(structure.binding.as_str()).ok_or_else(|| {
            InterfaceError::new(
                "OSR-I0022",
                format!("struct `{}` leaks a private binding", structure.binding),
            )
        })?;
        if binding.kind != BindingKind::Type {
            return Err(InterfaceError::new(
                "OSR-I0023",
                format!("struct `{}` references a non-type", structure.binding),
            ));
        }
        unique(structure.fields.iter().map(|field| &field.id), "field id")?;
        unique(
            structure.fields.iter().map(|field| &field.canonical),
            "field name",
        )?;
        unique(structure.type_parameters.iter(), "type parameter")?;
        let Type::Nominal { args, .. } = &binding.ty else {
            return Err(InterfaceError::new(
                "OSR-I0084",
                format!("struct `{}` binding has no nominal type", structure.binding),
            ));
        };
        if args.len() != structure.type_parameters.len() {
            return Err(InterfaceError::new(
                "OSR-I0084",
                format!(
                    "struct `{}` declares {} type parameters but its nominal type has {} arguments",
                    structure.binding,
                    structure.type_parameters.len(),
                    args.len()
                ),
            ));
        }
    }

    // Operator capabilities are deliberately closed data.  Validate every
    // reference and signature against the public function/type surface so a
    // hand-edited `.osri` cannot smuggle in an implementation or overload.
    let function_interfaces = interface
        .functions
        .iter()
        .map(|function| (function.binding.as_str(), function))
        .collect::<BTreeMap<_, _>>();
    let mut operator_signatures = BTreeSet::new();
    for instance in &interface.operator_instances {
        let binding = bindings.get(instance.binding.as_str()).ok_or_else(|| {
            InterfaceError::new(
                "OSR-I0066",
                format!(
                    "operator instance `{}` references a missing/private function `{}`",
                    instance.id, instance.binding
                ),
            )
        })?;
        if binding.kind != BindingKind::Function {
            return Err(InterfaceError::new(
                "OSR-I0066",
                format!(
                    "operator instance `{}` binding is not a function",
                    instance.id
                ),
            ));
        }
        let owner = bindings
            .get(instance.owner_binding.as_str())
            .ok_or_else(|| {
                InterfaceError::new(
                    "OSR-I0067",
                    format!(
                        "operator instance `{}` references a missing/private owner type `{}`",
                        instance.id, instance.owner_binding
                    ),
                )
            })?;
        if owner.kind != BindingKind::Type {
            return Err(InterfaceError::new(
                "OSR-I0067",
                format!(
                    "operator instance `{}` owner binding is not a type",
                    instance.id
                ),
            ));
        }
        let expected_id = format!(
            "{}::operator::{}",
            instance.binding,
            instance.operator.stable_name()
        );
        if instance.id != expected_id {
            return Err(InterfaceError::new(
                "OSR-I0068",
                format!(
                    "operator instance id `{}` does not match its binding/operator",
                    instance.id
                ),
            ));
        }
        if !is_publishable_operator(instance.operator) {
            return Err(InterfaceError::new(
                "OSR-I0068",
                format!(
                    "operator `{}` is not publishable in the v0 capability set",
                    instance.operator.stable_name()
                ),
            ));
        }
        let expected_arity = operator_arity(instance.operator);
        if instance.operands.len() != expected_arity {
            return Err(InterfaceError::new(
                "OSR-I0069",
                format!(
                    "operator instance `{}` expects {expected_arity} operands, found {}",
                    instance.id,
                    instance.operands.len()
                ),
            ));
        }
        if instance.operands.iter().any(contains_dynamic_operator_type)
            || contains_dynamic_operator_type(&instance.result)
        {
            return Err(InterfaceError::new(
                "OSR-I0069",
                format!(
                    "operator instance `{}` contains Any, Unknown, or Error",
                    instance.id
                ),
            ));
        }
        if !instance.operands.iter().any(|operand| {
            matches!(
                operand,
                Type::Nominal { binding, .. } if binding == &instance.owner_binding
            )
        }) {
            return Err(InterfaceError::new(
                "OSR-I0070",
                format!(
                    "operator instance `{}` violates the orphan rule for `{}`",
                    instance.id, owner.canonical
                ),
            ));
        }

        let expected_type = Type::Fn(
            FunctionType::new(instance.operands.clone(), instance.result.clone())
                .with_summaries(instance.summaries.clone()),
        );
        if let Some(function) = function_interfaces.get(instance.binding.as_str()) {
            let function_type = Type::Fn(
                FunctionType::new(
                    function
                        .parameters
                        .iter()
                        .map(|parameter| parameter.ty.clone())
                        .collect(),
                    function.return_type.clone(),
                )
                .with_summaries(function.summaries.clone()),
            );
            if function_type != expected_type {
                return Err(InterfaceError::new(
                    "OSR-I0071",
                    format!(
                        "operator instance `{}` signature differs from its function interface",
                        instance.id
                    ),
                ));
            }
        }
        let binding_type = match &binding.ty {
            Type::Fn(function) => Type::Fn(function.clone()),
            _ => {
                return Err(InterfaceError::new(
                    "OSR-I0071",
                    format!(
                        "operator instance `{}` binding has no function type",
                        instance.id
                    ),
                ));
            }
        };
        if binding_type != expected_type {
            return Err(InterfaceError::new(
                "OSR-I0071",
                format!(
                    "operator instance `{}` signature differs from its function binding",
                    instance.id
                ),
            ));
        }
        if !operator_signatures.insert((instance.operator, instance.operands.clone())) {
            return Err(InterfaceError::new(
                "OSR-I0072",
                format!(
                    "duplicate operator instance signature for `{}`",
                    instance.operator.stable_name()
                ),
            ));
        }
    }

    unique(interface.macros.iter().map(|macro_| &macro_.id), "macro id")?;
    unique(
        interface.macros.iter().map(|macro_| &macro_.canonical),
        "macro name",
    )?;
    unique(
        interface.phase_helpers.iter().map(|helper| &helper.id),
        "phase helper id",
    )?;
    unique(
        interface
            .phase_helpers
            .iter()
            .map(|helper| &helper.canonical),
        "phase helper name",
    )?;
    let phase_forms = interface
        .phase_helpers
        .iter()
        .map(|helper| helper.phase_ir.clone())
        .chain(
            interface
                .macros
                .iter()
                .map(|macro_| macro_.phase_ir.clone()),
        )
        .collect::<Vec<_>>();
    if let Some(diagnostic) = macro_expand::validate_phase_forms(&phase_forms).first() {
        return Err(InterfaceError::new(
            "OSR-I0059",
            format!(
                "invalid replayable phase-1 declaration: {}",
                diagnostic.message
            ),
        ));
    }
    let helper_forms = interface
        .phase_helpers
        .iter()
        .map(|helper| (helper.canonical.clone(), helper.phase_ir.clone()))
        .collect::<BTreeMap<_, _>>();
    let helper_names = helper_forms.keys().cloned().collect::<BTreeSet<_>>();
    let mut required_helpers = BTreeSet::new();
    for macro_ in &interface.macros {
        let expected_id = BindingId::new(&interface.module, &macro_.canonical, BindingKind::Macro);
        if macro_.id != expected_id.as_str() {
            return Err(InterfaceError::new(
                "OSR-I0059",
                format!("macro `{}` has an invalid binding id", macro_.canonical),
            ));
        }
        let (name, parameters, _) = phase_declaration_parts(&macro_.phase_ir, "defmacro")?;
        if name != macro_.canonical || normalize_form(parameters) != macro_.parameters {
            return Err(InterfaceError::new(
                "OSR-I0059",
                format!(
                    "macro `{}` signature does not match its phase IR",
                    macro_.canonical
                ),
            ));
        }
        let arity = phase_parameter_arity(parameters)?;
        if arity != (macro_.minimum_arity, macro_.variadic) {
            return Err(InterfaceError::new(
                "OSR-I0059",
                format!(
                    "macro `{}` has an inconsistent arity signature",
                    macro_.canonical
                ),
            ));
        }
        let closure = phase_helper_closure(&macro_.phase_ir, &helper_forms, &helper_names)?;
        required_helpers.extend(closure.iter().cloned());
        let expected_bindings = closure
            .iter()
            .map(|name| {
                BindingId::new(&interface.module, name, BindingKind::Macro)
                    .as_str()
                    .to_owned()
            })
            .collect::<Vec<_>>();
        if macro_.helper_bindings != expected_bindings {
            return Err(InterfaceError::new(
                "OSR-I0060",
                format!(
                    "macro `{}` helper closure is inconsistent",
                    macro_.canonical
                ),
            ));
        }
    }
    if required_helpers.len() != interface.phase_helpers.len()
        || interface
            .phase_helpers
            .iter()
            .any(|helper| !required_helpers.contains(&helper.canonical))
    {
        return Err(InterfaceError::new(
            "OSR-I0060",
            "interface contains a phase helper outside exported macro closures",
        ));
    }
    for helper in &interface.phase_helpers {
        let expected_id = BindingId::new(&interface.module, &helper.canonical, BindingKind::Macro);
        if helper.id != expected_id.as_str() {
            return Err(InterfaceError::new(
                "OSR-I0060",
                format!(
                    "phase helper `{}` has an invalid binding id",
                    helper.canonical
                ),
            ));
        }
        let (name, _, _) = phase_declaration_parts(&helper.phase_ir, "defn-for-syntax")?;
        if name != helper.canonical {
            return Err(InterfaceError::new(
                "OSR-I0060",
                format!(
                    "phase helper `{}` name differs from its phase IR",
                    helper.canonical
                ),
            ));
        }
    }

    let schemas = interface
        .static_schemas
        .iter()
        .map(|schema| {
            let binding = BindingId::new(&interface.module, &schema.name, BindingKind::Type)
                .as_str()
                .to_owned();
            (binding, schema)
        })
        .collect::<BTreeMap<_, _>>();
    for (binding, schema) in &schemas {
        let public_binding = bindings.get(binding.as_str()).ok_or_else(|| {
            InterfaceError::new(
                "OSR-I0056",
                format!("static schema `{}` has no public type binding", schema.name),
            )
        })?;
        if public_binding.kind != BindingKind::Type || public_binding.canonical != schema.name {
            return Err(InterfaceError::new(
                "OSR-I0056",
                format!(
                    "static schema `{}` has an inconsistent public binding",
                    schema.name
                ),
            ));
        }
        schema.verify_integrity().map_err(|error| {
            InterfaceError::new(
                "OSR-I0056",
                format!("invalid static schema `{}`: {}", schema.name, error.message),
            )
        })?;
    }

    for record in &interface.owned_records {
        if !record.public || record.module != interface.module {
            return Err(InterfaceError::new(
                "OSR-I0057",
                "private or non-owned static record leaked into interface",
            ));
        }
        let owner_name = bindings
            .get(record.owner_binding_id.as_str())
            .map(|binding| binding.canonical.as_str())
            .or_else(|| {
                schemas
                    .get(record.owner_binding_id.as_str())
                    .map(|schema| schema.name.as_str())
            });
        if owner_name.is_none() {
            return Err(InterfaceError::new(
                "OSR-I0057",
                format!(
                    "static record `{}` has a missing/private owner `{}`",
                    record.stable_record_id, record.owner_binding_id
                ),
            ));
        }
        if owner_name != Some(record.owner_name.as_str()) {
            return Err(InterfaceError::new(
                "OSR-I0057",
                format!(
                    "static record `{}` owner name does not match `{}`",
                    record.stable_record_id, record.owner_binding_id
                ),
            ));
        }
        if let Some(schema) = schemas.get(record.schema.binding_id.as_str()) {
            records::verify_record_against_schema(record, schema, &record.schema.binding_id)
                .map_err(|error| {
                    InterfaceError::new(
                        "OSR-I0057",
                        format!(
                            "invalid static record `{}`: {}",
                            record.stable_record_id, error.message
                        ),
                    )
                })?;
        } else if record
            .schema
            .binding_id
            .split_once("::")
            .is_some_and(|(module, suffix)| {
                module != interface.module && suffix.starts_with("type::")
            })
        {
            // Imported schemas are validated against the dependency interface
            // during graph compilation. The owning interface retains the
            // exact schema binding/body hash and can still validate the
            // record's canonical identity without pretending to re-export it.
            record.verify_integrity().map_err(|error| {
                InterfaceError::new(
                    "OSR-I0057",
                    format!(
                        "invalid static record `{}`: {}",
                        record.stable_record_id, error.message
                    ),
                )
            })?;
        } else {
            return Err(InterfaceError::new(
                "OSR-I0057",
                format!(
                    "static record `{}` references a missing/private schema `{}`",
                    record.stable_record_id, record.schema.binding_id
                ),
            ));
        }
    }
    Ok(())
}

fn validate_nominal_type_identities(
    interface: &Interface,
    public_bindings: &BTreeMap<&str, &PublicBinding>,
) -> InterfaceResult<()> {
    for binding in &interface.bindings {
        let expected = BindingId::new(&interface.module, &binding.canonical, binding.kind);
        if binding.id != expected.as_str() {
            return Err(InterfaceError::new(
                "OSR-I0084",
                format!(
                    "public binding `{}` has non-canonical identity `{}`",
                    binding.canonical, binding.id
                ),
            ));
        }
        validate_type_nominal_identities(&binding.ty, interface, public_bindings)?;
        if binding.kind == BindingKind::Type
            && !matches!(
                &binding.ty,
                Type::Nominal { binding: nominal, .. } if nominal == &binding.id
            )
        {
            return Err(InterfaceError::new(
                "OSR-I0084",
                format!(
                    "public type `{}` does not carry its own binding identity",
                    binding.canonical
                ),
            ));
        }
    }
    for function in &interface.functions {
        for parameter in &function.parameters {
            validate_type_nominal_identities(&parameter.ty, interface, public_bindings)?;
        }
        validate_type_nominal_identities(&function.return_type, interface, public_bindings)?;
    }
    for structure in &interface.structs {
        for field in &structure.fields {
            validate_type_nominal_identities(&field.ty, interface, public_bindings)?;
        }
    }
    for instance in &interface.operator_instances {
        for operand in &instance.operands {
            validate_type_nominal_identities(operand, interface, public_bindings)?;
        }
        validate_type_nominal_identities(&instance.result, interface, public_bindings)?;
    }
    Ok(())
}

fn validate_type_nominal_identities(
    ty: &Type,
    interface: &Interface,
    public_bindings: &BTreeMap<&str, &PublicBinding>,
) -> InterfaceResult<()> {
    match ty {
        Type::Option(inner) | Type::List(inner) | Type::Vector(inner) | Type::Set(inner) => {
            validate_type_nominal_identities(inner, interface, public_bindings)?;
        }
        Type::Union(members) | Type::Tuple(members) => {
            for member in members {
                validate_type_nominal_identities(member, interface, public_bindings)?;
            }
        }
        Type::Map(key, value) => {
            validate_type_nominal_identities(key, interface, public_bindings)?;
            validate_type_nominal_identities(value, interface, public_bindings)?;
        }
        Type::Fn(function) => {
            for parameter in &function.parameters {
                validate_type_nominal_identities(parameter, interface, public_bindings)?;
            }
            validate_type_nominal_identities(&function.return_type, interface, public_bindings)?;
        }
        Type::Nominal { binding, args } => {
            let Some((owner_module, name)) = binding.rsplit_once("::type::") else {
                return Err(InterfaceError::new(
                    "OSR-I0084",
                    format!("nominal type has unresolved binding identity `{binding}`"),
                ));
            };
            if owner_module.is_empty()
                || name.is_empty()
                || BindingId::new(owner_module, name, BindingKind::Type).as_str() != binding
            {
                return Err(InterfaceError::new(
                    "OSR-I0084",
                    format!("nominal type has non-canonical binding identity `{binding}`"),
                ));
            }
            if owner_module == interface.module {
                let owner = public_bindings.get(binding.as_str()).ok_or_else(|| {
                    InterfaceError::new(
                        "OSR-I0084",
                        format!("nominal type leaks private or missing local type `{binding}`"),
                    )
                })?;
                if owner.kind != BindingKind::Type || owner.canonical != name {
                    return Err(InterfaceError::new(
                        "OSR-I0084",
                        format!("nominal type identity `{binding}` is not a public type"),
                    ));
                }
            }
            for argument in args {
                validate_type_nominal_identities(argument, interface, public_bindings)?;
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
    Ok(())
}

fn validate_interface_metadata_resources(interface: &Interface) -> InterfaceResult<()> {
    let bindings = interface
        .bindings
        .iter()
        .map(|binding| (binding.id.as_str(), binding))
        .collect::<BTreeMap<_, _>>();
    let mut counted_bindings = BTreeSet::new();
    let mut interface_usage = MetadataResourceUsage::default();

    let mut declaration = MetadataResourceUsage::default();
    include_metadata_target(
        &interface.metadata,
        "module declaration",
        &mut declaration,
        &mut interface_usage,
    )?;

    for function in &interface.functions {
        let mut declaration = MetadataResourceUsage::default();
        if counted_bindings.insert(function.binding.as_str()) {
            if let Some(binding) = bindings.get(function.binding.as_str()) {
                include_metadata_target(
                    &binding.metadata,
                    &format!("declaration `{}`", binding.canonical),
                    &mut declaration,
                    &mut interface_usage,
                )?;
            }
        }
        for parameter in &function.parameters {
            include_metadata_target(
                &parameter.metadata,
                &format!("parameter `{}`", parameter.canonical),
                &mut declaration,
                &mut interface_usage,
            )?;
        }
    }

    for structure in &interface.structs {
        let mut declaration = MetadataResourceUsage::default();
        if counted_bindings.insert(structure.binding.as_str()) {
            if let Some(binding) = bindings.get(structure.binding.as_str()) {
                include_metadata_target(
                    &binding.metadata,
                    &format!("declaration `{}`", binding.canonical),
                    &mut declaration,
                    &mut interface_usage,
                )?;
            }
        }
        for field in &structure.fields {
            include_metadata_target(
                &field.metadata,
                &format!("field `{}`", field.canonical),
                &mut declaration,
                &mut interface_usage,
            )?;
        }
    }

    for binding in &interface.bindings {
        if counted_bindings.insert(binding.id.as_str()) {
            let mut declaration = MetadataResourceUsage::default();
            include_metadata_target(
                &binding.metadata,
                &format!("declaration `{}`", binding.canonical),
                &mut declaration,
                &mut interface_usage,
            )?;
        }
    }

    for macro_ in &interface.macros {
        let context = format!("macro declaration `{}`", macro_.canonical);
        let mut declaration = MetadataResourceUsage::default();
        include_form_metadata(
            &macro_.parameters,
            &context,
            &mut declaration,
            &mut interface_usage,
        )?;
        include_form_metadata(
            &macro_.phase_ir,
            &context,
            &mut declaration,
            &mut interface_usage,
        )?;
    }

    for helper in &interface.phase_helpers {
        let context = format!("phase-1 declaration `{}`", helper.canonical);
        let mut declaration = MetadataResourceUsage::default();
        include_form_metadata(
            &helper.phase_ir,
            &context,
            &mut declaration,
            &mut interface_usage,
        )?;
    }
    Ok(())
}

fn include_form_metadata(
    root: &Form,
    context: &str,
    declaration: &mut MetadataResourceUsage,
    interface: &mut MetadataResourceUsage,
) -> InterfaceResult<()> {
    let mut pending = vec![root];
    while let Some(form) = pending.pop() {
        include_metadata_target(&form.metadata, context, declaration, interface)?;
        match &form.kind {
            FormKind::List(items)
            | FormKind::Vector(items)
            | FormKind::Map(items)
            | FormKind::Set(items) => pending.extend(items),
            FormKind::ReaderMacro { form, .. } => pending.push(form),
            _ => {}
        }
    }
    Ok(())
}

fn include_metadata_target(
    metadata: &[MetadataEntry],
    context: &str,
    declaration: &mut MetadataResourceUsage,
    interface: &mut MetadataResourceUsage,
) -> InterfaceResult<()> {
    if metadata.is_empty() {
        return Ok(());
    }
    let usage = validate_metadata_target(metadata, context)?;
    *declaration = declaration.saturating_add(usage);
    check_metadata_usage(*declaration, METADATA_DECLARATION_LIMITS)
        .map_err(|exceeded| metadata_resource_error(context, "declaration", exceeded))?;
    *interface = interface.saturating_add(usage);
    check_metadata_usage(*interface, METADATA_INTERFACE_LIMITS)
        .map_err(|exceeded| metadata_resource_error(context, "interface", exceeded))?;
    Ok(())
}

fn validate_metadata_target(
    metadata: &[MetadataEntry],
    context: &str,
) -> InterfaceResult<MetadataResourceUsage> {
    if metadata.iter().any(|entry| {
        !metadata_datum_is_serializable(&entry.key) || !metadata_datum_is_serializable(&entry.value)
    }) {
        return Err(InterfaceError::new(
            "OSR-I0083",
            format!("{context} contains non-serializable metadata data"),
        ));
    }
    check_metadata_resources(metadata, METADATA_TARGET_LIMITS)
        .map_err(|exceeded| metadata_resource_error(context, "syntax target", exceeded))
}

fn metadata_resource_error(
    context: &str,
    scope: &str,
    exceeded: MetadataLimitExceeded,
) -> InterfaceError {
    InterfaceError::new(
        "OSR-I0082",
        format!(
            "{context} exceeds the metadata {scope} {} limit of {} (found {})",
            exceeded.resource, exceeded.limit, exceeded.actual
        ),
    )
}

fn unique<'a>(values: impl IntoIterator<Item = &'a String>, kind: &str) -> InterfaceResult<()> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(InterfaceError::new(
                "OSR-I0024",
                format!("duplicate {kind} `{value}`"),
            ));
        }
    }
    Ok(())
}

fn calculate_hashes(interface: &Interface) -> InterfaceHashes {
    let full = body_form(interface, MetadataProjection::Full);
    let semantic = body_form(interface, MetadataProjection::Semantic);
    let tooling = tooling_body_form(interface);
    let interface_body = hash_form(&full);
    let semantic_body = hash_form(&semantic);
    let tooling_body = hash_form(&tooling);
    let header = header_form(interface);
    let hash_section = hashes_form(&interface_body, &semantic_body, &tooling_body, None);
    let content_integrity = hash_text(&render_forms(&[
        header,
        wrap("osiris-interface/body", full),
        wrap("osiris-interface/graph", graph_form(&interface.graph)),
        hash_section,
    ]));
    InterfaceHashes {
        interface_body,
        semantic_body,
        tooling_body,
        content_integrity,
    }
}

fn refresh_standalone_hashes(interface: &mut Interface) -> InterfaceResult<()> {
    interface.hashes = calculate_hashes(interface);
    let local = BTreeMap::from([(
        interface.module.clone(),
        InterfaceBodyHashes {
            semantic_body: interface.hashes.semantic_body.clone(),
            tooling_body: interface.hashes.tooling_body.clone(),
        },
    )]);
    let graph = calculate_interface_graph_hashes(&local, [], &BTreeMap::new())
        .map_err(|error| InterfaceError::new("OSR-I0073", error.to_string()))?;
    interface.graph = graph
        .groups
        .into_iter()
        .next()
        .expect("one local interface produces one hash group");
    interface.hashes = calculate_hashes(interface);
    Ok(())
}

pub fn install_hash_group(
    interface: &mut Interface,
    group: InterfaceHashGroup,
) -> InterfaceResult<()> {
    interface.graph = group;
    interface.hashes = calculate_hashes(interface);
    validate(interface)
}

fn file_forms(interface: &Interface, integrity: bool) -> Vec<Form> {
    vec![
        header_form(interface),
        wrap(
            "osiris-interface/body",
            body_form(interface, MetadataProjection::Full),
        ),
        wrap("osiris-interface/graph", graph_form(&interface.graph)),
        hashes_form(
            &interface.hashes.interface_body,
            &interface.hashes.semantic_body,
            &interface.hashes.tooling_body,
            integrity.then_some(interface.hashes.content_integrity.as_str()),
        ),
    ]
}

fn header_form(interface: &Interface) -> Form {
    wrap(
        "osiris-interface/header",
        map(vec![
            ("format", string(FORMAT_NAME)),
            ("format-version", integer(interface.format_version)),
            ("compiler-abi", string(&interface.compiler_abi)),
            ("language-abi", string(&interface.language_abi)),
        ]),
    )
}

#[derive(Clone, Copy)]
enum MetadataProjection {
    Full,
    Semantic,
}

fn body_form(interface: &Interface, projection: MetadataProjection) -> Form {
    map(vec![
        ("module", string(&interface.module)),
        (
            "metadata",
            metadata_form(&project_metadata(&interface.metadata, projection)),
        ),
        (
            "bindings",
            vector(
                interface
                    .bindings
                    .iter()
                    .map(|binding| binding_form(binding, projection))
                    .collect(),
            ),
        ),
        (
            "aliases",
            vector(interface.aliases.iter().map(alias_form).collect()),
        ),
        (
            "functions",
            vector(
                interface
                    .functions
                    .iter()
                    .map(|function| function_form(function, projection))
                    .collect(),
            ),
        ),
        (
            "structs",
            vector(
                interface
                    .structs
                    .iter()
                    .map(|structure| struct_form(structure, projection))
                    .collect(),
            ),
        ),
        (
            "operator-instances",
            vector(
                interface
                    .operator_instances
                    .iter()
                    .map(operator_instance_form)
                    .collect(),
            ),
        ),
        (
            "macros",
            vector(interface.macros.iter().map(macro_interface_form).collect()),
        ),
        (
            "phase-helpers",
            vector(
                interface
                    .phase_helpers
                    .iter()
                    .map(phase_helper_form)
                    .collect(),
            ),
        ),
        (
            "static-schemas",
            vector(
                interface
                    .static_schemas
                    .iter()
                    .map(|schema| static_schema_form(&interface.module, schema))
                    .collect(),
            ),
        ),
        (
            "owned-records",
            vector(
                interface
                    .owned_records
                    .iter()
                    .map(|record| static_record_form(record, projection))
                    .collect(),
            ),
        ),
    ])
}

fn tooling_body_form(interface: &Interface) -> Form {
    map(vec![
        ("module", string(&interface.module)),
        ("metadata", metadata_form(&interface.metadata)),
        (
            "bindings",
            vector(
                interface
                    .bindings
                    .iter()
                    .map(|binding| {
                        map(vec![
                            ("id", string(&binding.id)),
                            ("canonical", string(&binding.canonical)),
                            ("metadata", metadata_form(&binding.metadata)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "aliases",
            vector(interface.aliases.iter().map(alias_form).collect()),
        ),
        (
            "functions",
            vector(
                interface
                    .functions
                    .iter()
                    .map(|function| {
                        map(vec![
                            ("binding", string(&function.binding)),
                            (
                                "parameters",
                                vector(
                                    function
                                        .parameters
                                        .iter()
                                        .map(|parameter| {
                                            map(vec![
                                                ("id", string(&parameter.id)),
                                                ("canonical", string(&parameter.canonical)),
                                                ("aliases", strings_form(&parameter.aliases)),
                                                ("metadata", metadata_form(&parameter.metadata)),
                                            ])
                                        })
                                        .collect(),
                                ),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "structs",
            vector(
                interface
                    .structs
                    .iter()
                    .map(|structure| {
                        map(vec![
                            ("binding", string(&structure.binding)),
                            ("doc", optional_string(structure.doc.as_deref())),
                            (
                                "fields",
                                vector(
                                    structure
                                        .fields
                                        .iter()
                                        .map(|field| {
                                            map(vec![
                                                ("id", string(&field.id)),
                                                ("canonical", string(&field.canonical)),
                                                ("aliases", strings_form(&field.aliases)),
                                                ("metadata", metadata_form(&field.metadata)),
                                            ])
                                        })
                                        .collect(),
                                ),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "operator-instances",
            vector(
                interface
                    .operator_instances
                    .iter()
                    .map(|instance| {
                        map(vec![
                            ("id", string(&instance.id)),
                            ("binding", string(&instance.binding)),
                            ("owner-binding", string(&instance.owner_binding)),
                            ("operator", keyword(instance.operator.stable_name())),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "macros",
            vector(
                interface
                    .macros
                    .iter()
                    .map(|macro_| {
                        map(vec![
                            ("id", string(&macro_.id)),
                            ("canonical", string(&macro_.canonical)),
                            ("parameters", macro_.parameters.clone()),
                            ("minimum-arity", integer_usize(macro_.minimum_arity)),
                            ("variadic", boolean(macro_.variadic)),
                            ("metadata", metadata_form(&macro_.phase_ir.metadata)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "phase-helpers",
            vector(
                interface
                    .phase_helpers
                    .iter()
                    .map(|helper| {
                        map(vec![
                            ("id", string(&helper.id)),
                            ("canonical", string(&helper.canonical)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "static-schemas",
            vector(
                interface
                    .static_schemas
                    .iter()
                    .map(|schema| {
                        map(vec![
                            (
                                "binding",
                                string(
                                    BindingId::new(
                                        &interface.module,
                                        &schema.name,
                                        BindingKind::Type,
                                    )
                                    .as_str(),
                                ),
                            ),
                            ("name", string(&schema.name)),
                            ("schema-id", string(&schema.schema_id)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "owned-records",
            vector(
                interface
                    .owned_records
                    .iter()
                    .map(|record| {
                        map(vec![
                            ("stable-record-id", string(&record.stable_record_id)),
                            ("owner-binding-id", string(&record.owner_binding_id)),
                            ("owner-name", string(&record.owner_name)),
                            ("origin", record_origin_form(&record.origin)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

fn binding_form(binding: &PublicBinding, projection: MetadataProjection) -> Form {
    map(vec![
        ("id", string(&binding.id)),
        ("canonical", string(&binding.canonical)),
        ("python", string(&binding.python)),
        ("kind", keyword(binding_kind_name(binding.kind))),
        ("visibility", keyword("public")),
        ("type", type_form(&binding.ty)),
        (
            "runtime",
            binding.runtime.as_ref().map_or_else(none, |runtime| {
                map(vec![
                    ("module", string(&runtime.module)),
                    ("name", string(&runtime.name)),
                    ("python-module", boolean(runtime.python_module)),
                ])
            }),
        ),
        (
            "metadata",
            metadata_form(&project_metadata(&binding.metadata, projection)),
        ),
    ])
}

fn alias_form(alias: &PublicAlias) -> Form {
    map(vec![
        ("spelling", string(&alias.spelling)),
        ("canonical", string(&alias.canonical)),
        ("target", string(&alias.target)),
        ("visibility", keyword("public")),
    ])
}

fn function_form(function: &FunctionInterface, projection: MetadataProjection) -> Form {
    map(vec![
        ("binding", string(&function.binding)),
        (
            "parameters",
            vector(
                function
                    .parameters
                    .iter()
                    .map(|parameter| parameter_form(parameter, projection))
                    .collect(),
            ),
        ),
        ("return", type_form(&function.return_type)),
        (
            "contract-id",
            optional_string(function.contract_id.as_deref()),
        ),
        ("summaries", summaries_form(&function.summaries)),
    ])
}

fn parameter_form(parameter: &ParameterInterface, projection: MetadataProjection) -> Form {
    map(vec![
        ("id", string(&parameter.id)),
        ("canonical", string(&parameter.canonical)),
        ("type", type_form(&parameter.ty)),
        ("has-default", boolean(parameter.has_default)),
        ("variadic", boolean(parameter.variadic)),
        ("aliases", strings_form(&parameter.aliases)),
        (
            "metadata",
            metadata_form(&project_metadata(&parameter.metadata, projection)),
        ),
    ])
}

fn struct_form(structure: &StructInterface, projection: MetadataProjection) -> Form {
    map(vec![
        ("binding", string(&structure.binding)),
        ("type-parameters", strings_form(&structure.type_parameters)),
        (
            "fields",
            vector(
                structure
                    .fields
                    .iter()
                    .map(|field| field_form(field, projection))
                    .collect(),
            ),
        ),
        ("invariant-count", integer_usize(structure.invariant_count)),
        ("doc", optional_string(structure.doc.as_deref())),
    ])
}

fn operator_instance_form(instance: &OperatorInstance) -> Form {
    map(vec![
        ("id", string(&instance.id)),
        ("binding", string(&instance.binding)),
        ("owner-binding", string(&instance.owner_binding)),
        ("operator", keyword(instance.operator.stable_name())),
        (
            "operands",
            vector(instance.operands.iter().map(type_form).collect()),
        ),
        ("result", type_form(&instance.result)),
        ("summaries", summaries_form(&instance.summaries)),
    ])
}

fn field_form(field: &FieldInterface, projection: MetadataProjection) -> Form {
    map(vec![
        ("id", string(&field.id)),
        ("canonical", string(&field.canonical)),
        ("type", type_form(&field.ty)),
        ("has-default", boolean(field.has_default)),
        ("aliases", strings_form(&field.aliases)),
        (
            "metadata",
            metadata_form(&project_metadata(&field.metadata, projection)),
        ),
    ])
}

fn macro_interface_form(macro_: &MacroInterface) -> Form {
    map(vec![
        ("id", string(&macro_.id)),
        ("canonical", string(&macro_.canonical)),
        ("phase", keyword("macro")),
        ("visibility", keyword("public")),
        ("parameters", macro_.parameters.clone()),
        ("minimum-arity", integer_usize(macro_.minimum_arity)),
        ("variadic", boolean(macro_.variadic)),
        ("helper-bindings", strings_form(&macro_.helper_bindings)),
        ("phase-1-ir", macro_.phase_ir.clone()),
    ])
}

fn phase_helper_form(helper: &PhaseHelperInterface) -> Form {
    map(vec![
        ("id", string(&helper.id)),
        ("canonical", string(&helper.canonical)),
        ("phase", keyword("syntax")),
        ("visibility", keyword("private")),
        ("phase-1-ir", helper.phase_ir.clone()),
    ])
}

fn static_schema_form(module: &str, schema: &StaticSchema) -> Form {
    map(vec![
        (
            "binding",
            string(BindingId::new(module, &schema.name, BindingKind::Type).as_str()),
        ),
        ("name", string(&schema.name)),
        ("schema-id", string(&schema.schema_id)),
        ("version", integer_u64(schema.version)),
        (
            "fields",
            vector(schema.fields.iter().map(static_schema_field_form).collect()),
        ),
        (
            "indexes",
            vector(
                schema
                    .indexes
                    .iter()
                    .map(static_schema_index_form)
                    .collect(),
            ),
        ),
        ("body-hash", string(&schema.body_hash)),
        ("visibility", keyword("public")),
    ])
}

fn static_schema_field_form(field: &records::SchemaField) -> Form {
    map(vec![
        ("name", string(&field.name)),
        ("type", static_type_form(&field.datum_type)),
        ("required", boolean(field.required)),
        (
            "default",
            vector(field.default.iter().map(static_datum_form).collect()),
        ),
    ])
}

fn static_schema_index_form(index: &records::SchemaIndex) -> Form {
    map(vec![
        ("id", string(&index.id)),
        ("scope", string(&index.scope)),
        (
            "projections",
            vector(
                index
                    .projections
                    .iter()
                    .map(|projection| {
                        map(vec![
                            (
                                "kind",
                                keyword(match projection.kind {
                                    ProjectionKind::Field => "field",
                                    ProjectionKind::Each => "each",
                                }),
                            ),
                            ("field", string(&projection.field)),
                            ("role", string(&projection.role)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

fn static_type_form(datum_type: &StaticType) -> Form {
    match datum_type {
        StaticType::Any => keyword("any"),
        StaticType::None => keyword("none"),
        StaticType::Bool => keyword("bool"),
        StaticType::Int => keyword("int"),
        StaticType::Float => keyword("float"),
        StaticType::Str => keyword("str"),
        StaticType::Keyword => keyword("keyword"),
        StaticType::Symbol => keyword("symbol"),
        StaticType::List(inner) => vector(vec![keyword("list"), static_type_form(inner)]),
        StaticType::Vector(inner) => vector(vec![keyword("vector"), static_type_form(inner)]),
        StaticType::Map(key, value) => vector(vec![
            keyword("map"),
            static_type_form(key),
            static_type_form(value),
        ]),
        StaticType::Set(inner) => vector(vec![keyword("set"), static_type_form(inner)]),
        StaticType::Optional(inner) => vector(vec![keyword("optional"), static_type_form(inner)]),
        StaticType::OneOf(values) => vector(vec![
            keyword("one-of"),
            vector(values.iter().map(static_datum_form).collect()),
        ]),
    }
}

fn static_record_form(record: &ValidatedRecord, projection: MetadataProjection) -> Form {
    map(vec![
        ("schema", schema_identity_form(&record.schema)),
        ("owner-binding-id", string(&record.owner_binding_id)),
        ("owner-name", string(&record.owner_name)),
        ("module", string(&record.module)),
        ("visibility", keyword("public")),
        ("stable-record-id", string(&record.stable_record_id)),
        ("record-body-hash", string(&record.record_body_hash)),
        (
            "fields",
            vector(
                record
                    .fields
                    .iter()
                    .map(|(name, value)| vector(vec![string(name), static_datum_form(value)]))
                    .collect(),
            ),
        ),
        (
            "index-claims",
            vector(record.index_claims.iter().map(index_claim_form).collect()),
        ),
        (
            "origin",
            match projection {
                MetadataProjection::Full => record_origin_form(&record.origin),
                MetadataProjection::Semantic => none(),
            },
        ),
    ])
}

fn schema_identity_form(identity: &records::SchemaIdentity) -> Form {
    map(vec![
        ("binding-id", string(&identity.binding_id)),
        ("schema-id", string(&identity.schema_id)),
        ("version", integer_u64(identity.version)),
        ("body-hash", string(&identity.body_hash)),
    ])
}

fn index_claim_form(claim: &records::IndexClaim) -> Form {
    map(vec![
        ("index-id", string(&claim.index_id)),
        ("projection-field", string(&claim.projection_field)),
        ("projection-role", string(&claim.projection_role)),
        ("key", static_datum_form(&claim.key)),
        ("normalized-key", string(&claim.normalized_key)),
        (
            "raw-spelling",
            optional_string(claim.raw_spelling.as_deref()),
        ),
    ])
}

fn record_origin_form(origin: &records::RecordOrigin) -> Form {
    map(vec![
        ("module", string(&origin.module)),
        (
            "span",
            vector(vec![
                integer_usize(origin.span.start),
                integer_usize(origin.span.end),
            ]),
        ),
        (
            "macro-origin",
            optional_string(origin.macro_origin.as_deref()),
        ),
    ])
}

fn static_datum_form(value: &StaticDatum) -> Form {
    match value {
        StaticDatum::None => vector(vec![keyword("none")]),
        StaticDatum::Bool(value) => vector(vec![keyword("bool"), boolean(*value)]),
        StaticDatum::Int(value) => vector(vec![keyword("int"), string(value)]),
        StaticDatum::Float(bits) => vector(vec![keyword("float"), string(&format!("{bits:016x}"))]),
        StaticDatum::Str(value) => vector(vec![keyword("string"), string(value)]),
        StaticDatum::Keyword(value) => vector(vec![keyword("keyword"), string(value)]),
        StaticDatum::Symbol {
            spelling,
            binding_id,
        } => vector(vec![
            keyword("symbol"),
            string(spelling),
            optional_string(binding_id.as_deref()),
        ]),
        StaticDatum::List(values) => vector(vec![
            keyword("list"),
            vector(values.iter().map(static_datum_form).collect()),
        ]),
        StaticDatum::Vector(values) => vector(vec![
            keyword("vector"),
            vector(values.iter().map(static_datum_form).collect()),
        ]),
        StaticDatum::Map(entries) => vector(vec![
            keyword("map"),
            vector(
                entries
                    .iter()
                    .map(|(key, value)| {
                        vector(vec![static_datum_form(key), static_datum_form(value)])
                    })
                    .collect(),
            ),
        ]),
        StaticDatum::Set(values) => vector(vec![
            keyword("set"),
            vector(values.iter().map(static_datum_form).collect()),
        ]),
    }
}

fn graph_form(group: &InterfaceHashGroup) -> Form {
    map(vec![
        ("group-id", string(&group.id)),
        (
            "members",
            vector(
                group
                    .members
                    .iter()
                    .map(|member| {
                        map(vec![
                            ("module", string(&member.module)),
                            ("semantic-body", string(&member.semantic_body_hash)),
                            ("tooling-body", string(&member.tooling_body_hash)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "internal-edges",
            vector(group.internal_edges.iter().map(graph_edge_form).collect()),
        ),
        (
            "external-dependencies",
            vector(
                group
                    .external_dependencies
                    .iter()
                    .map(|dependency| {
                        map(vec![
                            ("from", string(&dependency.from)),
                            ("to", string(&dependency.to)),
                            ("kind", edge_kind_form(dependency.kind)),
                            (
                                "semantic-interface-hash",
                                string(&dependency.semantic_interface_hash),
                            ),
                            (
                                "tooling-metadata-hash",
                                string(&dependency.tooling_metadata_hash),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "semantic-interface-hash",
            string(&group.semantic_interface_hash),
        ),
        (
            "tooling-metadata-hash",
            string(&group.tooling_metadata_hash),
        ),
    ])
}

fn graph_edge_form(edge: &InterfaceHashEdge) -> Form {
    map(vec![
        ("from", string(&edge.from)),
        ("to", string(&edge.to)),
        ("kind", edge_kind_form(edge.kind)),
    ])
}

fn edge_kind_form(kind: crate::module_graph::EdgeKind) -> Form {
    keyword(match kind {
        crate::module_graph::EdgeKind::Runtime => "runtime",
        crate::module_graph::EdgeKind::Phase1 => "phase-1",
    })
}

fn hashes_form(
    interface_body: &str,
    semantic_body: &str,
    tooling_body: &str,
    integrity: Option<&str>,
) -> Form {
    let mut entries = vec![
        ("interface-body", string(interface_body)),
        ("semantic-body", string(semantic_body)),
        ("tooling-body", string(tooling_body)),
    ];
    if let Some(integrity) = integrity {
        entries.push(("content-integrity", string(integrity)));
    }
    wrap("osiris-interface/hashes", map(entries))
}

fn type_form(ty: &Type) -> Form {
    match ty {
        Type::Bool => keyword("bool"),
        Type::Int => keyword("int"),
        Type::Float => keyword("float"),
        Type::Str => keyword("str"),
        Type::Bytes => keyword("bytes"),
        Type::None => keyword("none"),
        Type::Any => keyword("any"),
        Type::Never => keyword("never"),
        Type::Unknown => keyword("unknown"),
        Type::Error => keyword("error"),
        Type::Option(value) => vector(vec![keyword("option"), type_form(value)]),
        Type::Union(values) => type_sequence("union", values),
        Type::Tuple(values) => type_sequence("tuple", values),
        Type::List(value) => vector(vec![keyword("list"), type_form(value)]),
        Type::Vector(value) => vector(vec![keyword("vector"), type_form(value)]),
        Type::Map(key, value) => vector(vec![keyword("map"), type_form(key), type_form(value)]),
        Type::Set(value) => vector(vec![keyword("set"), type_form(value)]),
        Type::Fn(function) => vector(vec![
            keyword("fn"),
            vector(function.parameters.iter().map(type_form).collect()),
            type_form(&function.return_type),
            summaries_form(&function.summaries),
        ]),
        Type::Nominal { binding, args } => {
            let mut values = vec![keyword("nominal"), string(binding)];
            values.extend(args.iter().map(type_form));
            vector(values)
        }
        Type::Literal(value) => vector(vec![keyword("literal"), type_literal_form(value)]),
        Type::TypeVar(variable) => vector(vec![keyword("type-var"), integer(variable.0)]),
    }
}

fn type_literal_form(value: &TypeLiteral) -> Form {
    match value {
        TypeLiteral::None => vector(vec![keyword("none")]),
        TypeLiteral::Bool(value) => vector(vec![keyword("bool"), boolean(*value)]),
        TypeLiteral::Integer(value) => vector(vec![keyword("integer"), string(value)]),
        TypeLiteral::Float(bits) => vector(vec![keyword("float"), string(&format!("{bits:016x}"))]),
        TypeLiteral::String(value) => vector(vec![keyword("string"), string(value)]),
        TypeLiteral::Keyword(value) => vector(vec![keyword("keyword"), string(value)]),
        TypeLiteral::Symbol(value) => vector(vec![keyword("symbol"), string(value)]),
        TypeLiteral::List(values) => vector(vec![
            keyword("list"),
            vector(values.iter().map(type_literal_form).collect()),
        ]),
        TypeLiteral::Vector(values) => vector(vec![
            keyword("vector"),
            vector(values.iter().map(type_literal_form).collect()),
        ]),
        TypeLiteral::Map(entries) => vector(vec![
            keyword("map"),
            vector(
                entries
                    .iter()
                    .map(|(key, value)| {
                        vector(vec![type_literal_form(key), type_literal_form(value)])
                    })
                    .collect(),
            ),
        ]),
        TypeLiteral::Set(values) => vector(vec![
            keyword("set"),
            vector(values.iter().map(type_literal_form).collect()),
        ]),
    }
}

fn type_sequence(tag: &str, types: &[Type]) -> Form {
    let mut values = vec![keyword(tag)];
    values.extend(types.iter().map(type_form));
    vector(values)
}

fn summaries_form(summaries: &CallSummaries) -> Form {
    map(vec![
        ("effects", effects_form(&summaries.effects)),
        ("temporal", temporal_form(&summaries.temporal)),
        ("data", data_form(&summaries.data)),
    ])
}

fn effects_form(row: &EffectRow) -> Form {
    map(vec![
        ("open", boolean(row.open)),
        (
            "items",
            vector(
                row.effects
                    .iter()
                    .map(|effect| match effect {
                        Effect::Io => keyword("io"),
                        Effect::Throw => keyword("throw"),
                        Effect::Mutation => keyword("mutation"),
                        Effect::HiddenState => keyword("hidden-state"),
                        Effect::PythonDynamic => keyword("python-dynamic"),
                        Effect::Custom(name) => vector(vec![keyword("custom"), string(name)]),
                    })
                    .collect(),
            ),
        ),
    ])
}

fn temporal_form(summary: &TemporalSummary) -> Form {
    map(vec![
        ("past", bound_form(&summary.past)),
        ("future", bound_form(&summary.future)),
        (
            "availability",
            match &summary.availability {
                Availability::Immediate => keyword("immediate"),
                Availability::Named(name) => vector(vec![keyword("named"), string(name)]),
                Availability::Unknown => keyword("unknown"),
            },
        ),
    ])
}

fn bound_form(bound: &TemporalBound) -> Form {
    match bound {
        TemporalBound::Finite(value) => vector(vec![keyword("finite"), integer_u64(*value)]),
        TemporalBound::Symbolic(value) => vector(vec![keyword("symbolic"), string(value)]),
        TemporalBound::Unbounded => keyword("unbounded"),
        TemporalBound::Unknown => keyword("unknown"),
    }
}

fn data_form(data: &DataProperties) -> Form {
    map(vec![
        ("schema", optional_string(data.schema.as_deref())),
        (
            "axes",
            data.axes
                .as_ref()
                .map_or_else(none, |axes| strings_form(axes)),
        ),
        (
            "alignment",
            keyword(match data.alignment {
                Alignment::Positional => "positional",
                Alignment::Labelled => "labelled",
                Alignment::AsOf => "as-of",
                Alignment::Unknown => "unknown",
            }),
        ),
        (
            "ordered-by",
            data.ordered_by
                .as_ref()
                .map_or_else(none, |keys| strings_form(keys)),
        ),
        (
            "unique-by",
            data.unique_by
                .as_ref()
                .map_or_else(none, |keys| strings_form(keys)),
        ),
        ("preserves-length", optional_bool(data.preserves_length)),
        ("materializes", optional_bool(data.materializes)),
        ("reshapes", optional_bool(data.reshapes)),
        ("nulls-possible", optional_bool(data.nulls_possible)),
        ("nan-possible", optional_bool(data.nan_possible)),
        ("nonfinite-possible", optional_bool(data.nonfinite_possible)),
        (
            "nonfinite-policy",
            optional_string(data.nonfinite_policy.as_deref()),
        ),
    ])
}

fn decode_header(form: &Form) -> InterfaceResult<(u32, String, String)> {
    let values = strict_map(
        form,
        &["format", "format-version", "compiler-abi", "language-abi"],
    )?;
    if expect_string(get(&values, "format")?, "format")? != FORMAT_NAME {
        return Err(InterfaceError::new("OSR-I0030", "unknown interface format"));
    }
    Ok((
        expect_u32(get(&values, "format-version")?, "format-version")?,
        expect_string(get(&values, "compiler-abi")?, "compiler-abi")?,
        expect_string(get(&values, "language-abi")?, "language-abi")?,
    ))
}

#[allow(clippy::type_complexity)]
fn decode_body(
    form: &Form,
) -> InterfaceResult<(
    String,
    Vec<MetadataEntry>,
    Vec<PublicBinding>,
    Vec<PublicAlias>,
    Vec<FunctionInterface>,
    Vec<StructInterface>,
    Vec<OperatorInstance>,
    Vec<MacroInterface>,
    Vec<PhaseHelperInterface>,
    Vec<StaticSchema>,
    Vec<ValidatedRecord>,
)> {
    let values = strict_map(
        form,
        &[
            "module",
            "metadata",
            "bindings",
            "aliases",
            "functions",
            "structs",
            "operator-instances",
            "macros",
            "phase-helpers",
            "static-schemas",
            "owned-records",
        ],
    )?;
    let module = expect_string(get(&values, "module")?, "module")?;
    let static_schemas = decode_vector(get(&values, "static-schemas")?, |form| {
        decode_static_schema(form, &module)
    })?;
    Ok((
        module,
        decode_metadata(get(&values, "metadata")?)?,
        decode_vector(get(&values, "bindings")?, decode_binding)?,
        decode_vector(get(&values, "aliases")?, decode_alias)?,
        decode_vector(get(&values, "functions")?, decode_function)?,
        decode_vector(get(&values, "structs")?, decode_struct)?,
        decode_vector(
            get(&values, "operator-instances")?,
            decode_operator_instance,
        )?,
        decode_vector(get(&values, "macros")?, decode_macro_interface)?,
        decode_vector(get(&values, "phase-helpers")?, decode_phase_helper)?,
        static_schemas,
        decode_vector(get(&values, "owned-records")?, decode_static_record)?,
    ))
}

fn decode_graph(form: &Form) -> InterfaceResult<InterfaceHashGroup> {
    let values = strict_map(
        form,
        &[
            "group-id",
            "members",
            "internal-edges",
            "external-dependencies",
            "semantic-interface-hash",
            "tooling-metadata-hash",
        ],
    )?;
    Ok(InterfaceHashGroup {
        id: expect_string(get(&values, "group-id")?, "interface hash group id")?,
        members: decode_vector(get(&values, "members")?, decode_graph_member)?,
        internal_edges: decode_vector(get(&values, "internal-edges")?, decode_graph_edge)?,
        external_dependencies: decode_vector(
            get(&values, "external-dependencies")?,
            decode_graph_dependency,
        )?,
        semantic_interface_hash: expect_hash(get(&values, "semantic-interface-hash")?)?,
        tooling_metadata_hash: expect_hash(get(&values, "tooling-metadata-hash")?)?,
    })
}

fn decode_graph_member(form: &Form) -> InterfaceResult<InterfaceHashMember> {
    let values = strict_map(form, &["module", "semantic-body", "tooling-body"])?;
    Ok(InterfaceHashMember {
        module: expect_string(get(&values, "module")?, "interface hash group member")?,
        semantic_body_hash: expect_hash(get(&values, "semantic-body")?)?,
        tooling_body_hash: expect_hash(get(&values, "tooling-body")?)?,
    })
}

fn decode_graph_edge(form: &Form) -> InterfaceResult<InterfaceHashEdge> {
    let values = strict_map(form, &["from", "to", "kind"])?;
    Ok(InterfaceHashEdge {
        from: expect_string(get(&values, "from")?, "interface edge source")?,
        to: expect_string(get(&values, "to")?, "interface edge target")?,
        kind: decode_edge_kind(get(&values, "kind")?)?,
    })
}

fn decode_graph_dependency(form: &Form) -> InterfaceResult<ResolvedHashDependency> {
    let values = strict_map(
        form,
        &[
            "from",
            "to",
            "kind",
            "semantic-interface-hash",
            "tooling-metadata-hash",
        ],
    )?;
    Ok(ResolvedHashDependency {
        from: expect_string(get(&values, "from")?, "interface dependency source")?,
        to: expect_string(get(&values, "to")?, "interface dependency target")?,
        kind: decode_edge_kind(get(&values, "kind")?)?,
        semantic_interface_hash: expect_hash(get(&values, "semantic-interface-hash")?)?,
        tooling_metadata_hash: expect_hash(get(&values, "tooling-metadata-hash")?)?,
    })
}

fn decode_edge_kind(form: &Form) -> InterfaceResult<crate::module_graph::EdgeKind> {
    match expect_keyword(form, "interface edge kind")? {
        "runtime" => Ok(crate::module_graph::EdgeKind::Runtime),
        "phase-1" => Ok(crate::module_graph::EdgeKind::Phase1),
        value => Err(InterfaceError::new(
            "OSR-I0073",
            format!("unknown interface edge kind `:{value}`"),
        )),
    }
}

fn decode_hashes(form: &Form) -> InterfaceResult<InterfaceHashes> {
    let values = strict_map(
        form,
        &[
            "interface-body",
            "semantic-body",
            "tooling-body",
            "content-integrity",
        ],
    )?;
    Ok(InterfaceHashes {
        interface_body: expect_hash(get(&values, "interface-body")?)?,
        semantic_body: expect_hash(get(&values, "semantic-body")?)?,
        tooling_body: expect_hash(get(&values, "tooling-body")?)?,
        content_integrity: expect_hash(get(&values, "content-integrity")?)?,
    })
}

fn decode_binding(form: &Form) -> InterfaceResult<PublicBinding> {
    let values = strict_map(
        form,
        &[
            "id",
            "canonical",
            "python",
            "kind",
            "visibility",
            "type",
            "runtime",
            "metadata",
        ],
    )?;
    require_public(get(&values, "visibility")?)?;
    Ok(PublicBinding {
        id: expect_string(get(&values, "id")?, "binding id")?,
        canonical: expect_string(get(&values, "canonical")?, "canonical")?,
        python: expect_string(get(&values, "python")?, "python")?,
        kind: decode_binding_kind(get(&values, "kind")?)?,
        ty: decode_type(get(&values, "type")?)?,
        runtime: if is_none(get(&values, "runtime")?) {
            None
        } else {
            Some(decode_runtime(get(&values, "runtime")?)?)
        },
        metadata: decode_metadata(get(&values, "metadata")?)?,
    })
}

fn decode_runtime(form: &Form) -> InterfaceResult<RuntimeLocator> {
    let values = strict_map(form, &["module", "name", "python-module"])?;
    Ok(RuntimeLocator {
        module: expect_string(get(&values, "module")?, "runtime module")?,
        name: expect_string(get(&values, "name")?, "runtime name")?,
        python_module: expect_bool(get(&values, "python-module")?, "python-module")?,
    })
}

fn decode_alias(form: &Form) -> InterfaceResult<PublicAlias> {
    let values = strict_map(form, &["spelling", "canonical", "target", "visibility"])?;
    require_public(get(&values, "visibility")?)?;
    Ok(PublicAlias {
        spelling: expect_string(get(&values, "spelling")?, "alias spelling")?,
        canonical: expect_string(get(&values, "canonical")?, "alias canonical")?,
        target: expect_string(get(&values, "target")?, "alias target")?,
    })
}

fn decode_function(form: &Form) -> InterfaceResult<FunctionInterface> {
    let values = strict_map(
        form,
        &[
            "binding",
            "parameters",
            "return",
            "contract-id",
            "summaries",
        ],
    )?;
    Ok(FunctionInterface {
        binding: expect_string(get(&values, "binding")?, "function binding")?,
        parameters: decode_vector(get(&values, "parameters")?, decode_parameter)?,
        return_type: decode_type(get(&values, "return")?)?,
        contract_id: decode_optional_string(get(&values, "contract-id")?, "contract id")?,
        summaries: decode_summaries(get(&values, "summaries")?)?,
    })
}

fn decode_parameter(form: &Form) -> InterfaceResult<ParameterInterface> {
    let values = strict_map(
        form,
        &[
            "id",
            "canonical",
            "type",
            "has-default",
            "variadic",
            "aliases",
            "metadata",
        ],
    )?;
    Ok(ParameterInterface {
        id: expect_string(get(&values, "id")?, "parameter id")?,
        canonical: expect_string(get(&values, "canonical")?, "parameter name")?,
        ty: decode_type(get(&values, "type")?)?,
        has_default: expect_bool(get(&values, "has-default")?, "has-default")?,
        variadic: expect_bool(get(&values, "variadic")?, "variadic")?,
        aliases: decode_strings(get(&values, "aliases")?, "parameter aliases")?,
        metadata: decode_metadata(get(&values, "metadata")?)?,
    })
}

fn decode_struct(form: &Form) -> InterfaceResult<StructInterface> {
    let values = strict_map(
        form,
        &[
            "binding",
            "type-parameters",
            "fields",
            "invariant-count",
            "doc",
        ],
    )?;
    Ok(StructInterface {
        binding: expect_string(get(&values, "binding")?, "struct binding")?,
        type_parameters: decode_strings(get(&values, "type-parameters")?, "type parameters")?,
        fields: decode_vector(get(&values, "fields")?, decode_field)?,
        invariant_count: expect_usize(get(&values, "invariant-count")?, "invariant count")?,
        doc: decode_optional_string(get(&values, "doc")?, "doc")?,
    })
}

fn decode_field(form: &Form) -> InterfaceResult<FieldInterface> {
    let values = strict_map(
        form,
        &[
            "id",
            "canonical",
            "type",
            "has-default",
            "aliases",
            "metadata",
        ],
    )?;
    Ok(FieldInterface {
        id: expect_string(get(&values, "id")?, "field id")?,
        canonical: expect_string(get(&values, "canonical")?, "field name")?,
        ty: decode_type(get(&values, "type")?)?,
        has_default: expect_bool(get(&values, "has-default")?, "has-default")?,
        aliases: decode_strings(get(&values, "aliases")?, "field aliases")?,
        metadata: decode_metadata(get(&values, "metadata")?)?,
    })
}

fn decode_operator_instance(form: &Form) -> InterfaceResult<OperatorInstance> {
    let values = strict_map(
        form,
        &[
            "id",
            "binding",
            "owner-binding",
            "operator",
            "operands",
            "result",
            "summaries",
        ],
    )?;
    let operator_name = expect_keyword(get(&values, "operator")?, "operator")?;
    let operator = ScalarOperator::from_stable_name(operator_name).ok_or_else(|| {
        InterfaceError::new(
            "OSR-I0068",
            format!("unknown static operator `{operator_name}`"),
        )
    })?;
    if operator.stable_name() != operator_name {
        return Err(InterfaceError::new(
            "OSR-I0068",
            format!("operator `{operator_name}` is not in canonical wire form"),
        ));
    }
    Ok(OperatorInstance {
        id: expect_string(get(&values, "id")?, "operator instance id")?,
        binding: expect_string(get(&values, "binding")?, "operator binding")?,
        owner_binding: expect_string(get(&values, "owner-binding")?, "operator owner binding")?,
        operator,
        operands: decode_vector(get(&values, "operands")?, decode_type)?,
        result: decode_type(get(&values, "result")?)?,
        summaries: decode_summaries(get(&values, "summaries")?)?,
    })
}

fn decode_macro_interface(form: &Form) -> InterfaceResult<MacroInterface> {
    let values = strict_map(
        form,
        &[
            "id",
            "canonical",
            "phase",
            "visibility",
            "parameters",
            "minimum-arity",
            "variadic",
            "helper-bindings",
            "phase-1-ir",
        ],
    )?;
    require_public(get(&values, "visibility")?)?;
    if expect_keyword(get(&values, "phase")?, "macro phase")? != "macro" {
        return Err(InterfaceError::new(
            "OSR-I0059",
            "public macro has an invalid phase",
        ));
    }
    Ok(MacroInterface {
        id: expect_string(get(&values, "id")?, "macro id")?,
        canonical: expect_string(get(&values, "canonical")?, "macro name")?,
        parameters: get(&values, "parameters")?.clone(),
        minimum_arity: expect_usize(get(&values, "minimum-arity")?, "macro minimum arity")?,
        variadic: expect_bool(get(&values, "variadic")?, "macro variadic")?,
        helper_bindings: decode_strings(get(&values, "helper-bindings")?, "helper bindings")?,
        phase_ir: get(&values, "phase-1-ir")?.clone(),
    })
}

fn decode_phase_helper(form: &Form) -> InterfaceResult<PhaseHelperInterface> {
    let values = strict_map(
        form,
        &["id", "canonical", "phase", "visibility", "phase-1-ir"],
    )?;
    require_private(get(&values, "visibility")?)?;
    if expect_keyword(get(&values, "phase")?, "helper phase")? != "syntax" {
        return Err(InterfaceError::new(
            "OSR-I0060",
            "phase-1 helper has an invalid phase",
        ));
    }
    Ok(PhaseHelperInterface {
        id: expect_string(get(&values, "id")?, "phase-1 helper id")?,
        canonical: expect_string(get(&values, "canonical")?, "phase-1 helper name")?,
        phase_ir: get(&values, "phase-1-ir")?.clone(),
    })
}

fn decode_static_schema(form: &Form, module: &str) -> InterfaceResult<StaticSchema> {
    let values = strict_map(
        form,
        &[
            "binding",
            "name",
            "schema-id",
            "version",
            "fields",
            "indexes",
            "body-hash",
            "visibility",
        ],
    )?;
    require_public(get(&values, "visibility")?)?;
    let name = expect_string(get(&values, "name")?, "static schema name")?;
    let binding = expect_string(get(&values, "binding")?, "static schema binding")?;
    let expected_binding = BindingId::new(module, &name, BindingKind::Type);
    if binding != expected_binding.as_str() {
        return Err(InterfaceError::new(
            "OSR-I0056",
            format!("static schema `{name}` has an invalid binding id"),
        ));
    }
    Ok(StaticSchema {
        name,
        schema_id: expect_string(get(&values, "schema-id")?, "schema id")?,
        version: expect_u64(get(&values, "version")?, "schema version")?,
        fields: decode_vector(get(&values, "fields")?, decode_static_schema_field)?,
        indexes: decode_vector(get(&values, "indexes")?, decode_static_schema_index)?,
        body_hash: expect_hash(get(&values, "body-hash")?)?,
    })
}

fn decode_static_schema_field(form: &Form) -> InterfaceResult<records::SchemaField> {
    let values = strict_map(form, &["name", "type", "required", "default"])?;
    let defaults = expect_vector(get(&values, "default")?, "static field default")?;
    if defaults.len() > 1 {
        return Err(InterfaceError::new(
            "OSR-I0056",
            "static field default must contain zero or one datum",
        ));
    }
    Ok(records::SchemaField {
        name: expect_string(get(&values, "name")?, "static field name")?,
        datum_type: decode_static_type(get(&values, "type")?)?,
        required: expect_bool(get(&values, "required")?, "static field required")?,
        default: defaults.first().map(decode_static_datum).transpose()?,
    })
}

fn decode_static_schema_index(form: &Form) -> InterfaceResult<records::SchemaIndex> {
    let values = strict_map(form, &["id", "scope", "projections"])?;
    Ok(records::SchemaIndex {
        id: expect_string(get(&values, "id")?, "static index id")?,
        scope: expect_string(get(&values, "scope")?, "static index scope")?,
        projections: decode_vector(get(&values, "projections")?, |form| {
            let projection = strict_map(form, &["kind", "field", "role"])?;
            let kind = match expect_keyword(get(&projection, "kind")?, "projection kind")? {
                "field" => ProjectionKind::Field,
                "each" => ProjectionKind::Each,
                _ => {
                    return Err(InterfaceError::new(
                        "OSR-I0056",
                        "unknown static index projection kind",
                    ));
                }
            };
            Ok(records::IndexProjection {
                kind,
                field: expect_string(get(&projection, "field")?, "projection field")?,
                role: expect_string(get(&projection, "role")?, "projection role")?,
            })
        })?,
    })
}

fn decode_static_type(form: &Form) -> InterfaceResult<StaticType> {
    if let FormKind::Keyword(_) = &form.kind {
        return match expect_keyword(form, "static type")? {
            "any" => Ok(StaticType::Any),
            "none" => Ok(StaticType::None),
            "bool" => Ok(StaticType::Bool),
            "int" => Ok(StaticType::Int),
            "float" => Ok(StaticType::Float),
            "str" => Ok(StaticType::Str),
            "keyword" => Ok(StaticType::Keyword),
            "symbol" => Ok(StaticType::Symbol),
            _ => Err(InterfaceError::new("OSR-I0056", "unknown static type")),
        };
    }
    let values = expect_vector(form, "static type")?;
    let Some(tag) = values.first() else {
        return Err(InterfaceError::new("OSR-I0056", "empty static type"));
    };
    match expect_keyword(tag, "static type tag")? {
        "list" | "vector" | "set" | "optional" if values.len() == 2 => {
            let inner = Box::new(decode_static_type(&values[1])?);
            Ok(match expect_keyword(tag, "static type tag")? {
                "list" => StaticType::List(inner),
                "vector" => StaticType::Vector(inner),
                "set" => StaticType::Set(inner),
                _ => StaticType::Optional(inner),
            })
        }
        "map" if values.len() == 3 => Ok(StaticType::Map(
            Box::new(decode_static_type(&values[1])?),
            Box::new(decode_static_type(&values[2])?),
        )),
        "one-of" if values.len() == 2 => Ok(StaticType::OneOf(
            expect_vector(&values[1], "OneOf values")?
                .iter()
                .map(decode_static_datum)
                .collect::<InterfaceResult<_>>()?,
        )),
        _ => Err(InterfaceError::new(
            "OSR-I0056",
            "invalid static type constructor",
        )),
    }
}

fn decode_static_record(form: &Form) -> InterfaceResult<ValidatedRecord> {
    let values = strict_map(
        form,
        &[
            "schema",
            "owner-binding-id",
            "owner-name",
            "module",
            "visibility",
            "stable-record-id",
            "record-body-hash",
            "fields",
            "index-claims",
            "origin",
        ],
    )?;
    require_public(get(&values, "visibility")?)?;
    let fields = decode_vector(get(&values, "fields")?, |form| {
        let pair = expect_vector(form, "static record field")?;
        if pair.len() != 2 {
            return Err(InterfaceError::new(
                "OSR-I0057",
                "static record field must be a pair",
            ));
        }
        Ok((
            expect_string(&pair[0], "static record field name")?,
            decode_static_datum(&pair[1])?,
        ))
    })?;
    Ok(ValidatedRecord {
        schema: decode_schema_identity(get(&values, "schema")?)?,
        owner_binding_id: expect_string(
            get(&values, "owner-binding-id")?,
            "record owner binding id",
        )?,
        owner_name: expect_string(get(&values, "owner-name")?, "record owner name")?,
        module: expect_string(get(&values, "module")?, "record module")?,
        public: true,
        stable_record_id: expect_hash(get(&values, "stable-record-id")?)?,
        record_body_hash: expect_hash(get(&values, "record-body-hash")?)?,
        fields,
        index_claims: decode_vector(get(&values, "index-claims")?, decode_index_claim)?,
        origin: decode_record_origin(get(&values, "origin")?)?,
    })
}

fn decode_schema_identity(form: &Form) -> InterfaceResult<records::SchemaIdentity> {
    let values = strict_map(form, &["binding-id", "schema-id", "version", "body-hash"])?;
    Ok(records::SchemaIdentity {
        binding_id: expect_string(get(&values, "binding-id")?, "schema binding id")?,
        schema_id: expect_string(get(&values, "schema-id")?, "schema id")?,
        version: expect_u64(get(&values, "version")?, "schema version")?,
        body_hash: expect_hash(get(&values, "body-hash")?)?,
    })
}

fn decode_index_claim(form: &Form) -> InterfaceResult<records::IndexClaim> {
    let values = strict_map(
        form,
        &[
            "index-id",
            "projection-field",
            "projection-role",
            "key",
            "normalized-key",
            "raw-spelling",
        ],
    )?;
    Ok(records::IndexClaim {
        index_id: expect_string(get(&values, "index-id")?, "index claim id")?,
        projection_field: expect_string(
            get(&values, "projection-field")?,
            "index projection field",
        )?,
        projection_role: expect_string(get(&values, "projection-role")?, "index projection role")?,
        key: decode_static_datum(get(&values, "key")?)?,
        normalized_key: expect_string(get(&values, "normalized-key")?, "normalized index key")?,
        raw_spelling: decode_optional_string(get(&values, "raw-spelling")?, "raw spelling")?,
    })
}

fn decode_record_origin(form: &Form) -> InterfaceResult<records::RecordOrigin> {
    let values = strict_map(form, &["module", "span", "macro-origin"])?;
    let span = expect_vector(get(&values, "span")?, "record origin span")?;
    if span.len() != 2 {
        return Err(InterfaceError::new(
            "OSR-I0057",
            "record origin span must have two offsets",
        ));
    }
    let start = expect_usize(&span[0], "record origin start")?;
    let end = expect_usize(&span[1], "record origin end")?;
    if start > end {
        return Err(InterfaceError::new(
            "OSR-I0057",
            "record origin span is reversed",
        ));
    }
    Ok(records::RecordOrigin {
        module: expect_string(get(&values, "module")?, "record origin module")?,
        span: Span::new(start, end),
        macro_origin: decode_optional_string(get(&values, "macro-origin")?, "record macro origin")?,
    })
}

fn decode_static_datum(form: &Form) -> InterfaceResult<StaticDatum> {
    let values = expect_vector(form, "static datum")?;
    let Some(tag) = values.first() else {
        return Err(InterfaceError::new("OSR-I0058", "empty static datum"));
    };
    let tag = expect_keyword(tag, "static datum tag")?;
    let datum = match (tag, values.len()) {
        ("none", 1) => StaticDatum::None,
        ("bool", 2) => StaticDatum::Bool(expect_bool(&values[1], "static bool")?),
        ("int", 2) => StaticDatum::Int(expect_string(&values[1], "static integer")?),
        ("float", 2) => {
            let bits = expect_string(&values[1], "static float bits")?;
            if bits.len() != 16
                || !bits
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            {
                return Err(InterfaceError::new(
                    "OSR-I0058",
                    "static float requires 16 lowercase hexadecimal bits",
                ));
            }
            StaticDatum::Float(
                u64::from_str_radix(&bits, 16)
                    .map_err(|_| InterfaceError::new("OSR-I0058", "invalid static float bits"))?,
            )
        }
        ("string", 2) => StaticDatum::Str(expect_string(&values[1], "static string")?),
        ("keyword", 2) => StaticDatum::Keyword(expect_string(&values[1], "static keyword")?),
        ("symbol", 3) => StaticDatum::Symbol {
            spelling: expect_string(&values[1], "static symbol")?,
            binding_id: decode_optional_string(&values[2], "static symbol binding")?,
        },
        ("list" | "vector" | "set", 2) => {
            let items = expect_vector(&values[1], "static datum items")?
                .iter()
                .map(decode_static_datum)
                .collect::<InterfaceResult<Vec<_>>>()?;
            match tag {
                "list" => StaticDatum::List(items),
                "vector" => StaticDatum::Vector(items),
                _ => StaticDatum::Set(items),
            }
        }
        ("map", 2) => {
            let entries = expect_vector(&values[1], "static map entries")?
                .iter()
                .map(|form| {
                    let pair = expect_vector(form, "static map entry")?;
                    if pair.len() != 2 {
                        return Err(InterfaceError::new(
                            "OSR-I0058",
                            "static map entry must be a pair",
                        ));
                    }
                    Ok((
                        decode_static_datum(&pair[0])?,
                        decode_static_datum(&pair[1])?,
                    ))
                })
                .collect::<InterfaceResult<Vec<_>>>()?;
            StaticDatum::Map(entries)
        }
        _ => {
            return Err(InterfaceError::new(
                "OSR-I0058",
                "invalid static datum encoding",
            ));
        }
    };
    datum.canonicalize().map_err(|error| {
        InterfaceError::new(
            "OSR-I0058",
            format!("invalid static datum: {}", error.message),
        )
    })
}

fn decode_type(form: &Form) -> InterfaceResult<Type> {
    if let FormKind::Keyword(name) = &form.kind {
        return match name.canonical.trim_start_matches(':') {
            "bool" => Ok(Type::Bool),
            "int" => Ok(Type::Int),
            "float" => Ok(Type::Float),
            "str" => Ok(Type::Str),
            "bytes" => Ok(Type::Bytes),
            "none" => Ok(Type::None),
            "any" => Ok(Type::Any),
            "never" => Ok(Type::Never),
            "unknown" => Ok(Type::Unknown),
            "error" => Ok(Type::Error),
            tag => Err(InterfaceError::new(
                "OSR-I0031",
                format!("unknown type tag `{tag}`"),
            )),
        };
    }
    let parts = expect_vector(form, "type")?;
    let tag = parts
        .first()
        .ok_or_else(|| InterfaceError::new("OSR-I0031", "empty type"))?;
    match expect_keyword(tag, "type tag")? {
        "option" if parts.len() == 2 => Ok(Type::option(decode_type(&parts[1])?)),
        "union" => Ok(Type::union(decode_types(&parts[1..])?)),
        "tuple" => Ok(Type::Tuple(decode_types(&parts[1..])?)),
        "list" if parts.len() == 2 => Ok(Type::List(Box::new(decode_type(&parts[1])?))),
        "vector" if parts.len() == 2 => Ok(Type::Vector(Box::new(decode_type(&parts[1])?))),
        "set" if parts.len() == 2 => Ok(Type::Set(Box::new(decode_type(&parts[1])?))),
        "map" if parts.len() == 3 => Ok(Type::Map(
            Box::new(decode_type(&parts[1])?),
            Box::new(decode_type(&parts[2])?),
        )),
        "fn" if parts.len() == 4 => Ok(Type::Fn(FunctionType {
            parameters: expect_vector(&parts[1], "function parameters")?
                .iter()
                .map(decode_type)
                .collect::<InterfaceResult<_>>()?,
            return_type: Box::new(decode_type(&parts[2])?),
            summaries: decode_summaries(&parts[3])?,
        })),
        "nominal" if parts.len() >= 2 => Ok(Type::Nominal {
            binding: expect_string(&parts[1], "nominal binding")?,
            args: decode_types(&parts[2..])?,
        }),
        "literal" if parts.len() == 2 => Ok(Type::Literal(decode_type_literal(&parts[1])?)),
        "type-var" if parts.len() == 2 => Ok(Type::TypeVar(TypeVarId(expect_u32(
            &parts[1],
            "type variable",
        )?))),
        tag => Err(InterfaceError::new(
            "OSR-I0031",
            format!("invalid type encoding `{tag}`"),
        )),
    }
}

fn decode_type_literal(form: &Form) -> InterfaceResult<TypeLiteral> {
    let values = expect_vector(form, "type literal")?;
    let Some(tag) = values.first() else {
        return Err(InterfaceError::new("OSR-I0031", "empty type literal"));
    };
    let tag = expect_keyword(tag, "type literal tag")?;
    let literal = match (tag, values.len()) {
        ("none", 1) => TypeLiteral::None,
        ("bool", 2) => TypeLiteral::Bool(expect_bool(&values[1], "literal bool")?),
        ("integer", 2) => TypeLiteral::Integer(expect_string(&values[1], "literal integer")?),
        ("float", 2) => {
            let bits = expect_string(&values[1], "literal float bits")?;
            if bits.len() != 16
                || !bits
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            {
                return Err(InterfaceError::new(
                    "OSR-I0031",
                    "literal float requires 16 lowercase hexadecimal bits",
                ));
            }
            TypeLiteral::Float(
                u64::from_str_radix(&bits, 16)
                    .map_err(|_| InterfaceError::new("OSR-I0031", "invalid literal float bits"))?,
            )
        }
        ("string", 2) => TypeLiteral::String(expect_string(&values[1], "literal string")?),
        ("keyword", 2) => TypeLiteral::Keyword(expect_string(&values[1], "literal keyword")?),
        ("symbol", 2) => TypeLiteral::Symbol(expect_string(&values[1], "literal symbol")?),
        ("list" | "vector" | "set", 2) => {
            let items = expect_vector(&values[1], "type literal items")?
                .iter()
                .map(decode_type_literal)
                .collect::<InterfaceResult<Vec<_>>>()?;
            match tag {
                "list" => TypeLiteral::List(items),
                "vector" => TypeLiteral::Vector(items),
                _ => TypeLiteral::Set(items),
            }
        }
        ("map", 2) => {
            let entries = expect_vector(&values[1], "type literal map entries")?
                .iter()
                .map(|form| {
                    let pair = expect_vector(form, "type literal map entry")?;
                    if pair.len() != 2 {
                        return Err(InterfaceError::new(
                            "OSR-I0031",
                            "type literal map entry must be a pair",
                        ));
                    }
                    Ok((
                        decode_type_literal(&pair[0])?,
                        decode_type_literal(&pair[1])?,
                    ))
                })
                .collect::<InterfaceResult<Vec<_>>>()?;
            TypeLiteral::Map(entries)
        }
        _ => {
            return Err(InterfaceError::new(
                "OSR-I0031",
                "invalid type literal encoding",
            ));
        }
    };
    literal.canonicalize().map_err(|error| {
        InterfaceError::new(
            "OSR-I0031",
            format!("invalid type literal: {}", error.message()),
        )
    })
}

fn decode_types(forms: &[Form]) -> InterfaceResult<Vec<Type>> {
    forms.iter().map(decode_type).collect()
}

fn decode_summaries(form: &Form) -> InterfaceResult<CallSummaries> {
    let values = strict_map(form, &["effects", "temporal", "data"])?;
    Ok(CallSummaries {
        effects: decode_effects(get(&values, "effects")?)?,
        temporal: decode_temporal(get(&values, "temporal")?)?,
        data: decode_data(get(&values, "data")?)?,
    })
}

fn decode_effects(form: &Form) -> InterfaceResult<EffectRow> {
    let values = strict_map(form, &["open", "items"])?;
    let mut effects = BTreeSet::new();
    for value in expect_vector(get(&values, "items")?, "effects")? {
        let effect = match &value.kind {
            FormKind::Keyword(name) => match name.canonical.trim_start_matches(':') {
                "io" => Effect::Io,
                "throw" => Effect::Throw,
                "mutation" => Effect::Mutation,
                "hidden-state" => Effect::HiddenState,
                "python-dynamic" => Effect::PythonDynamic,
                tag => {
                    return Err(InterfaceError::new(
                        "OSR-I0032",
                        format!("unknown effect `{tag}`"),
                    ));
                }
            },
            FormKind::Vector(parts)
                if parts.len() == 2 && expect_keyword(&parts[0], "effect")? == "custom" =>
            {
                Effect::Custom(expect_string(&parts[1], "custom effect")?)
            }
            _ => return Err(InterfaceError::new("OSR-I0032", "invalid effect")),
        };
        if !effects.insert(effect) {
            return Err(InterfaceError::new("OSR-I0033", "duplicate effect"));
        }
    }
    Ok(EffectRow {
        effects,
        open: expect_bool(get(&values, "open")?, "effect open")?,
    })
}

fn decode_temporal(form: &Form) -> InterfaceResult<TemporalSummary> {
    let values = strict_map(form, &["past", "future", "availability"])?;
    Ok(TemporalSummary {
        past: decode_bound(get(&values, "past")?)?,
        future: decode_bound(get(&values, "future")?)?,
        availability: decode_availability(get(&values, "availability")?)?,
    })
}

fn decode_bound(form: &Form) -> InterfaceResult<TemporalBound> {
    if let FormKind::Keyword(name) = &form.kind {
        return match name.canonical.trim_start_matches(':') {
            "unknown" => Ok(TemporalBound::Unknown),
            "unbounded" => Ok(TemporalBound::Unbounded),
            _ => Err(InterfaceError::new("OSR-I0034", "invalid temporal bound")),
        };
    }
    let parts = expect_vector(form, "temporal bound")?;
    if parts.len() != 2 {
        return Err(InterfaceError::new("OSR-I0034", "invalid temporal bound"));
    }
    match expect_keyword(&parts[0], "temporal bound")? {
        "finite" => Ok(TemporalBound::Finite(expect_u64(
            &parts[1],
            "finite bound",
        )?)),
        "symbolic" => Ok(TemporalBound::Symbolic(expect_string(
            &parts[1],
            "symbolic bound",
        )?)),
        _ => Err(InterfaceError::new("OSR-I0034", "invalid temporal bound")),
    }
}

fn decode_availability(form: &Form) -> InterfaceResult<Availability> {
    if let FormKind::Keyword(name) = &form.kind {
        return match name.canonical.trim_start_matches(':') {
            "immediate" => Ok(Availability::Immediate),
            "unknown" => Ok(Availability::Unknown),
            _ => Err(InterfaceError::new("OSR-I0035", "invalid availability")),
        };
    }
    let parts = expect_vector(form, "availability")?;
    if parts.len() == 2 && expect_keyword(&parts[0], "availability")? == "named" {
        Ok(Availability::Named(expect_string(
            &parts[1],
            "availability",
        )?))
    } else {
        Err(InterfaceError::new("OSR-I0035", "invalid availability"))
    }
}

fn decode_data(form: &Form) -> InterfaceResult<DataProperties> {
    let values = strict_map(
        form,
        &[
            "schema",
            "axes",
            "alignment",
            "ordered-by",
            "unique-by",
            "preserves-length",
            "materializes",
            "reshapes",
            "nulls-possible",
            "nan-possible",
            "nonfinite-possible",
            "nonfinite-policy",
        ],
    )?;
    let alignment = match expect_keyword(get(&values, "alignment")?, "alignment")? {
        "positional" => Alignment::Positional,
        "labelled" => Alignment::Labelled,
        "as-of" => Alignment::AsOf,
        "unknown" => Alignment::Unknown,
        _ => return Err(InterfaceError::new("OSR-I0036", "invalid alignment")),
    };
    Ok(DataProperties {
        schema: decode_optional_string(get(&values, "schema")?, "schema")?,
        axes: if is_none(get(&values, "axes")?) {
            None
        } else {
            Some(decode_strings(get(&values, "axes")?, "axes")?)
        },
        alignment,
        ordered_by: if is_none(get(&values, "ordered-by")?) {
            None
        } else {
            Some(decode_strings(get(&values, "ordered-by")?, "ordered-by")?)
        },
        unique_by: if is_none(get(&values, "unique-by")?) {
            None
        } else {
            Some(decode_strings(get(&values, "unique-by")?, "unique-by")?)
        },
        preserves_length: decode_optional_bool(get(&values, "preserves-length")?)?,
        materializes: decode_optional_bool(get(&values, "materializes")?)?,
        reshapes: decode_optional_bool(get(&values, "reshapes")?)?,
        nulls_possible: decode_optional_bool(get(&values, "nulls-possible")?)?,
        nan_possible: decode_optional_bool(get(&values, "nan-possible")?)?,
        nonfinite_possible: decode_optional_bool(get(&values, "nonfinite-possible")?)?,
        nonfinite_policy: decode_optional_string(
            get(&values, "nonfinite-policy")?,
            "nonfinite-policy",
        )?,
    })
}

fn normalize_model(interface: &mut Interface) -> InterfaceResult<()> {
    // Validate before recursive normalization so direct API callers and
    // forged interfaces cannot bypass metadata limits.
    validate_interface_metadata_resources(interface)?;
    interface.metadata = normalize_metadata(&interface.metadata)?;
    for binding in &mut interface.bindings {
        binding.metadata = normalize_metadata(&binding.metadata)?;
    }
    for function in &mut interface.functions {
        for parameter in &mut function.parameters {
            parameter.metadata = normalize_metadata(&parameter.metadata)?;
        }
    }
    for structure in &mut interface.structs {
        for field in &mut structure.fields {
            field.metadata = normalize_metadata(&field.metadata)?;
        }
    }
    for macro_ in &mut interface.macros {
        macro_.parameters = normalize_form(&macro_.parameters);
        macro_.phase_ir = normalize_form(&macro_.phase_ir);
    }
    for helper in &mut interface.phase_helpers {
        helper.phase_ir = normalize_form(&helper.phase_ir);
    }
    Ok(())
}

fn metadata_aliases(canonical: &str, metadata: &[MetadataEntry]) -> Vec<String> {
    let mut names = BTreeSet::new();
    for entry in metadata {
        if form_name(&entry.key).is_some_and(|name| name.trim_start_matches(':') == "osiris/names")
        {
            collect_names(&entry.value, &mut names);
        }
    }
    names.remove(canonical);
    names.into_iter().collect()
}

fn collect_names(form: &Form, names: &mut BTreeSet<String>) {
    let FormKind::Map(entries) = &form.kind else {
        return;
    };
    for pair in entries.chunks_exact(2) {
        match form_name(&pair[0])
            .unwrap_or_default()
            .trim_start_matches(':')
        {
            "preferred" => {
                if let FormKind::Symbol(name) = &pair[1].kind {
                    names.insert(name.canonical.clone());
                }
            }
            "aliases" => {
                if let FormKind::Vector(values) = &pair[1].kind {
                    for value in values {
                        if let FormKind::Symbol(name) = &value.kind {
                            names.insert(name.canonical.clone());
                        }
                    }
                }
            }
            _ => collect_names(&pair[1], names),
        }
    }
}

fn project_metadata(
    metadata: &[MetadataEntry],
    projection: MetadataProjection,
) -> Vec<MetadataEntry> {
    match projection {
        MetadataProjection::Full => metadata.to_vec(),
        MetadataProjection::Semantic => metadata
            .iter()
            .filter(|entry| {
                let key = form_name(&entry.key)
                    .unwrap_or_default()
                    .trim_start_matches(':');
                !(key == "doc"
                    || key == "since"
                    || key == "deprecated"
                    || key == "replacement"
                    || key.starts_with("agent/")
                    || key.starts_with("render/"))
            })
            .cloned()
            .collect(),
    }
}

fn normalize_metadata(metadata: &[MetadataEntry]) -> InterfaceResult<Vec<MetadataEntry>> {
    validate_metadata_target(metadata, "metadata target")?;
    let mut values = metadata
        .iter()
        .map(|entry| MetadataEntry {
            key: normalize_form(&entry.key),
            value: normalize_form(&entry.value),
        })
        .collect::<Vec<_>>();
    values.sort_by_cached_key(|entry| form_text(&entry.key));
    for pair in values.windows(2) {
        if form_text(&pair[0].key) == form_text(&pair[1].key) {
            return Err(InterfaceError::new("OSR-I0040", "duplicate metadata key"));
        }
    }
    Ok(values)
}

fn normalize_form(form: &Form) -> Form {
    let kind = match &form.kind {
        FormKind::None => FormKind::None,
        FormKind::Bool(value) => FormKind::Bool(*value),
        FormKind::Integer(value) => FormKind::Integer(value.clone()),
        FormKind::Float(value) => FormKind::Float(value.clone()),
        FormKind::String(value) => FormKind::String(value.clone()),
        FormKind::Keyword(name) => FormKind::Keyword(normalize_name(name)),
        FormKind::Symbol(name) => FormKind::Symbol(normalize_name(name)),
        FormKind::List(values) => FormKind::List(values.iter().map(normalize_form).collect()),
        FormKind::Vector(values) => FormKind::Vector(values.iter().map(normalize_form).collect()),
        FormKind::Map(values) => {
            let mut pairs = values
                .chunks_exact(2)
                .map(|pair| (normalize_form(&pair[0]), normalize_form(&pair[1])))
                .collect::<Vec<_>>();
            pairs.sort_by_cached_key(|(key, _)| form_text(key));
            FormKind::Map(
                pairs
                    .into_iter()
                    .flat_map(|(key, value)| [key, value])
                    .collect(),
            )
        }
        FormKind::Set(values) => {
            let mut values = values.iter().map(normalize_form).collect::<Vec<_>>();
            values.sort_by_cached_key(form_text);
            FormKind::Set(values)
        }
        FormKind::ReaderMacro { macro_kind, form } => FormKind::ReaderMacro {
            macro_kind: *macro_kind,
            form: Box::new(normalize_form(form)),
        },
        FormKind::Error(message) => FormKind::Error(message.clone()),
    };
    let mut result = form_node(kind);
    result.metadata = normalize_metadata(&form.metadata).unwrap_or_default();
    result
}

fn normalize_name(name: &Name) -> Name {
    Name {
        spelling: name.canonical.clone(),
        canonical: name.canonical.clone(),
    }
}

fn strict_map(form: &Form, expected: &[&str]) -> InterfaceResult<BTreeMap<String, Form>> {
    let FormKind::Map(entries) = &form.kind else {
        return Err(InterfaceError::new("OSR-I0041", "expected interface map"));
    };
    if entries.len() % 2 != 0 {
        return Err(InterfaceError::new("OSR-I0041", "unmatched map key"));
    }
    let allowed = expected.iter().copied().collect::<BTreeSet<_>>();
    let mut values = BTreeMap::new();
    for pair in entries.chunks_exact(2) {
        let key = expect_keyword(&pair[0], "map key")?.to_owned();
        if !allowed.contains(key.as_str()) {
            return Err(InterfaceError::new(
                "OSR-I0042",
                format!("unknown interface field `:{key}`"),
            ));
        }
        if values.insert(key.clone(), pair[1].clone()).is_some() {
            return Err(InterfaceError::new(
                "OSR-I0043",
                format!("duplicate interface field `:{key}`"),
            ));
        }
    }
    for key in expected {
        if !values.contains_key(*key) {
            return Err(InterfaceError::new(
                "OSR-I0044",
                format!("missing interface field `:{key}`"),
            ));
        }
    }
    Ok(values)
}

fn reject_duplicate_maps(form: &Form) -> InterfaceResult<()> {
    match &form.kind {
        FormKind::Map(entries) => {
            if entries.len() % 2 != 0 {
                return Err(InterfaceError::new("OSR-I0043", "unmatched map key"));
            }
            let mut keys = BTreeSet::new();
            for pair in entries.chunks_exact(2) {
                let key = form_text(&normalize_form(&pair[0]));
                if !keys.insert(key.clone()) {
                    return Err(InterfaceError::new(
                        "OSR-I0043",
                        format!("duplicate map key `{key}`"),
                    ));
                }
                reject_duplicate_maps(&pair[0])?;
                reject_duplicate_maps(&pair[1])?;
            }
        }
        FormKind::List(values) | FormKind::Vector(values) | FormKind::Set(values) => {
            for value in values {
                reject_duplicate_maps(value)?;
            }
        }
        FormKind::ReaderMacro { form, .. } => reject_duplicate_maps(form)?,
        _ => {}
    }
    Ok(())
}

fn unwrap<'a>(form: &'a Form, expected: &str) -> InterfaceResult<&'a Form> {
    let FormKind::List(values) = &form.kind else {
        return Err(InterfaceError::new(
            "OSR-I0045",
            "expected interface section",
        ));
    };
    if values.len() != 2 || form_name(&values[0]) != Some(expected) {
        return Err(InterfaceError::new(
            "OSR-I0045",
            format!("expected `({expected} ...)`"),
        ));
    }
    Ok(&values[1])
}

fn get<'a>(map: &'a BTreeMap<String, Form>, key: &str) -> InterfaceResult<&'a Form> {
    map.get(key)
        .ok_or_else(|| InterfaceError::new("OSR-I0044", format!("missing `:{key}`")))
}

fn decode_vector<T>(
    form: &Form,
    decode: impl Fn(&Form) -> InterfaceResult<T>,
) -> InterfaceResult<Vec<T>> {
    expect_vector(form, "section")?.iter().map(decode).collect()
}

fn decode_strings(form: &Form, context: &str) -> InterfaceResult<Vec<String>> {
    expect_vector(form, context)?
        .iter()
        .map(|value| expect_string(value, context))
        .collect()
}

fn decode_metadata(form: &Form) -> InterfaceResult<Vec<MetadataEntry>> {
    let FormKind::Map(entries) = &form.kind else {
        return Err(InterfaceError::new("OSR-I0046", "metadata must be a map"));
    };
    let entry_count = entries.len() / 2;
    if entry_count > METADATA_TARGET_LIMITS.max_entries {
        return Err(metadata_resource_error(
            "metadata target",
            "syntax target",
            MetadataLimitExceeded {
                resource: "entry count",
                actual: entry_count,
                limit: METADATA_TARGET_LIMITS.max_entries,
            },
        ));
    }
    normalize_metadata(
        &entries
            .chunks_exact(2)
            .map(|pair| MetadataEntry {
                key: pair[0].clone(),
                value: pair[1].clone(),
            })
            .collect::<Vec<_>>(),
    )
}

fn expect_vector<'a>(form: &'a Form, context: &str) -> InterfaceResult<&'a [Form]> {
    match &form.kind {
        FormKind::Vector(values) => Ok(values),
        _ => Err(InterfaceError::new(
            "OSR-I0047",
            format!("{context} must be a vector"),
        )),
    }
}

fn expect_string(form: &Form, context: &str) -> InterfaceResult<String> {
    match &form.kind {
        FormKind::String(value) => Ok(value.clone()),
        _ => Err(InterfaceError::new(
            "OSR-I0048",
            format!("{context} must be a string"),
        )),
    }
}

fn expect_keyword<'a>(form: &'a Form, context: &str) -> InterfaceResult<&'a str> {
    match &form.kind {
        FormKind::Keyword(name) => Ok(name.canonical.trim_start_matches(':')),
        _ => Err(InterfaceError::new(
            "OSR-I0049",
            format!("{context} must be a keyword"),
        )),
    }
}

fn expect_bool(form: &Form, context: &str) -> InterfaceResult<bool> {
    match form.kind {
        FormKind::Bool(value) => Ok(value),
        _ => Err(InterfaceError::new(
            "OSR-I0050",
            format!("{context} must be a boolean"),
        )),
    }
}

fn expect_u32(form: &Form, context: &str) -> InterfaceResult<u32> {
    expect_integer(form, context)?
        .parse()
        .map_err(|_| InterfaceError::new("OSR-I0051", format!("{context} must fit u32")))
}

fn expect_u64(form: &Form, context: &str) -> InterfaceResult<u64> {
    expect_integer(form, context)?
        .parse()
        .map_err(|_| InterfaceError::new("OSR-I0051", format!("{context} must fit u64")))
}

fn expect_usize(form: &Form, context: &str) -> InterfaceResult<usize> {
    expect_integer(form, context)?
        .parse()
        .map_err(|_| InterfaceError::new("OSR-I0051", format!("{context} must fit usize")))
}

fn expect_integer<'a>(form: &'a Form, context: &str) -> InterfaceResult<&'a str> {
    match &form.kind {
        FormKind::Integer(value) => Ok(value),
        _ => Err(InterfaceError::new(
            "OSR-I0051",
            format!("{context} must be an integer"),
        )),
    }
}

fn expect_hash(form: &Form) -> InterfaceResult<String> {
    let value = expect_string(form, "hash")?;
    let Some(digest) = value.strip_prefix("sha256:") else {
        return Err(InterfaceError::new("OSR-I0052", "hash must use SHA-256"));
    };
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(InterfaceError::new("OSR-I0052", "invalid SHA-256 hash"));
    }
    Ok(value)
}

fn decode_optional_string(form: &Form, context: &str) -> InterfaceResult<Option<String>> {
    if is_none(form) {
        Ok(None)
    } else {
        expect_string(form, context).map(Some)
    }
}

fn decode_optional_bool(form: &Form) -> InterfaceResult<Option<bool>> {
    if is_none(form) {
        Ok(None)
    } else {
        expect_bool(form, "optional bool").map(Some)
    }
}

fn require_public(form: &Form) -> InterfaceResult<()> {
    if expect_keyword(form, "visibility")? == "public" {
        Ok(())
    } else {
        Err(InterfaceError::new(
            "OSR-I0053",
            "private declaration leaked into interface",
        ))
    }
}

fn require_private(form: &Form) -> InterfaceResult<()> {
    if expect_keyword(form, "visibility")? == "private" {
        Ok(())
    } else {
        Err(InterfaceError::new(
            "OSR-I0060",
            "phase-1 helper closure member must be private",
        ))
    }
}

fn decode_binding_kind(form: &Form) -> InterfaceResult<BindingKind> {
    match expect_keyword(form, "binding kind")? {
        "module" => Ok(BindingKind::Module),
        "value" => Ok(BindingKind::Value),
        "function" => Ok(BindingKind::Function),
        "type" => Ok(BindingKind::Type),
        "field" => Ok(BindingKind::Field),
        "parameter" => Ok(BindingKind::Parameter),
        "macro" => Ok(BindingKind::Macro),
        "python-module" => Ok(BindingKind::PythonModule),
        _ => Err(InterfaceError::new("OSR-I0054", "unknown binding kind")),
    }
}

fn binding_kind_name(kind: BindingKind) -> &'static str {
    match kind {
        BindingKind::Module => "module",
        BindingKind::Value => "value",
        BindingKind::Function => "function",
        BindingKind::Type => "type",
        BindingKind::Field => "field",
        BindingKind::Parameter => "parameter",
        BindingKind::Macro => "macro",
        BindingKind::PythonModule => "python-module",
    }
}

fn metadata_form(metadata: &[MetadataEntry]) -> Form {
    form_node(FormKind::Map(
        metadata
            .iter()
            .flat_map(|entry| [entry.key.clone(), entry.value.clone()])
            .collect(),
    ))
}

fn strings_form(values: &[String]) -> Form {
    vector(values.iter().map(|value| string(value)).collect())
}

fn optional_string(value: Option<&str>) -> Form {
    value.map_or_else(none, string)
}

fn optional_bool(value: Option<bool>) -> Form {
    value.map_or_else(none, boolean)
}

fn wrap(head: &str, value: Form) -> Form {
    form_node(FormKind::List(vec![symbol(head), value]))
}

fn map(entries: Vec<(&str, Form)>) -> Form {
    form_node(FormKind::Map(
        entries
            .into_iter()
            .flat_map(|(key, value)| [keyword(key), value])
            .collect(),
    ))
}

fn vector(values: Vec<Form>) -> Form {
    form_node(FormKind::Vector(values))
}

fn none() -> Form {
    form_node(FormKind::None)
}

fn boolean(value: bool) -> Form {
    form_node(FormKind::Bool(value))
}

fn integer(value: u32) -> Form {
    form_node(FormKind::Integer(value.to_string()))
}

fn integer_u64(value: u64) -> Form {
    form_node(FormKind::Integer(value.to_string()))
}

fn integer_usize(value: usize) -> Form {
    form_node(FormKind::Integer(value.to_string()))
}

fn string(value: &str) -> Form {
    form_node(FormKind::String(value.to_owned()))
}

fn keyword(value: &str) -> Form {
    let value = format!(":{}", value.trim_start_matches(':'));
    form_node(FormKind::Keyword(Name {
        spelling: value.clone(),
        canonical: value,
    }))
}

fn symbol(value: &str) -> Form {
    form_node(FormKind::Symbol(Name {
        spelling: value.to_owned(),
        canonical: value.to_owned(),
    }))
}

fn form_node(kind: FormKind) -> Form {
    Form::new(kind, Span::default())
}

fn is_none(form: &Form) -> bool {
    matches!(form.kind, FormKind::None)
}

fn form_name(form: &Form) -> Option<&str> {
    match &form.kind {
        FormKind::Keyword(name) | FormKind::Symbol(name) => Some(&name.canonical),
        _ => None,
    }
}

fn render_forms(forms: &[Form]) -> String {
    render_document_text(&Document {
        format_version: 1,
        source_len: 0,
        tokens: Vec::new(),
        forms: forms.to_vec(),
        nodes: Vec::new(),
        diagnostics: Vec::new(),
    })
}

fn form_text(form: &Form) -> String {
    render_forms(std::slice::from_ref(form))
        .trim_end_matches('\n')
        .to_owned()
}

fn hash_form(form: &Form) -> String {
    hash_text(&form_text(form))
}

fn hash_text(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut output = String::with_capacity(71);
    output.push_str("sha256:");
    for byte in digest {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{
        build, emit, integer, keyword, read, refresh_standalone_hashes, render, string,
        validate_interface_metadata_resources,
    };
    use crate::{
        ast, hir, macro_expand, reader as source_reader,
        syntax::{METADATA_INTERFACE_LIMITS, METADATA_TARGET_LIMITS, MetadataEntry},
        types::{Availability, TemporalBound, Type},
    };

    const SOURCE: &str = r#"
        (module sample.core)

        ^{:doc "distance" :osiris/names {"zh-CN" {:preferred 距离}}}
        (defn distance
          [^{:osiris/names {"zh-CN" {:preferred 点位}}} [point Float]]
          -> Float
          point)

        (def metre 1)
        (alias 米 metre)

        (defstruct (Range T)
          "closed range"
          [min T]
          ^{:osiris/names {"zh-CN" {:preferred 最大值}}} [max T])

        (def private-value 9)
        (export [distance metre 米 Range])
    "#;

    const STATIC_SOURCE: &str = r#"
        (module sample.records)

        (defstatic-schema Descriptor
          :schema-id "sample/descriptor"
          :version 1
          :fields
          {:id {:type Str :required true}
           :aliases {:type (Vector Str) :default []}}
          :indexes
          [{:id "sample/runtime-id"
            :scope :effective-dependency-graph
            :keys [{:field :id :role :canonical}]}])

        (defstatic-schema PrivateSchema
          :schema-id "sample/private"
          :version 1
          :fields {:value {:type Int :required true}})

        (def public-owner 1)
        (static-record Descriptor public-owner {:id "alpha"})
        (def private-owner 2)
        (static-record Descriptor private-owner {:id "private"})
        (export [Descriptor public-owner])
    "#;

    const MACRO_SOURCE: &str = r#"
        (module sample.macros)

        (defn-for-syntax helper [value]
          (list 'inc value))
        (defn-for-syntax helper-two [value]
          (helper value))
        (defn-for-syntax unused-helper [value]
          (list 'ignore value))
        (defmacro public-pipeline [value & steps]
          (helper-two value))
        (defmacro hidden-macro [value]
          (helper value))
        (export [public-pipeline])
    "#;

    const OPERATOR_SOURCE: &str = r#"
        (module sample.operators)

        (defstruct (Series T)
          [values (Vector T)])

        ^{:osiris/operator :multiply}
        (defn multiply-series
          [[series (Series Float)] [multiplier Float]]
          -> (Series Float)
          series)

        (export [Series multiply-series])
    "#;

    fn modules() -> (ast::Module, hir::Module) {
        let surface = ast::lower_document(&source_reader::read(SOURCE));
        assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
        let typed = hir::lower_module(&surface.module, "sample.core");
        assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
        (surface.module, typed.module)
    }

    fn static_modules() -> (ast::Module, hir::Module) {
        let surface = ast::lower_document(&source_reader::read(STATIC_SOURCE));
        assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
        let typed = hir::lower_module(&surface.module, "sample.records");
        // Static schemas are represented by the surface/static pass.  The HIR
        // module remains sufficient for the exported record owner here.
        (surface.module, typed.module)
    }

    fn macro_modules(source: &str) -> (ast::Module, hir::Module) {
        let surface = ast::lower_document(&source_reader::read(source));
        assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
        let typed = hir::lower_module(&surface.module, "sample.macros");
        assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
        (surface.module, typed.module)
    }

    fn operator_modules(source: &str) -> (ast::Module, hir::Module) {
        let surface = ast::lower_document(&source_reader::read(source));
        assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
        let typed = hir::lower_module(&surface.module, "sample.operators");
        assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
        (surface.module, typed.module)
    }

    fn metadata_with_normalized_bytes(bytes: usize) -> Vec<MetadataEntry> {
        vec![MetadataEntry {
            key: keyword("x"),
            value: string(&"x".repeat(bytes.saturating_sub(7))),
        }]
    }

    fn metadata_map_source_with_normalized_bytes(bytes: usize) -> String {
        format!("{{:x \"{}\"}}", "x".repeat(bytes.saturating_sub(7)))
    }

    fn metadata_entries(count: usize) -> Vec<MetadataEntry> {
        (0..count)
            .map(|index| MetadataEntry {
                key: keyword(&format!("k{index}")),
                value: integer(u32::try_from(index).expect("small test metadata index")),
            })
            .collect()
    }

    fn metadata_map_source_entries(count: usize) -> String {
        let entries = (0..count)
            .map(|index| format!(":k{index} {index}"))
            .collect::<Vec<_>>()
            .join(" ");
        format!("{{{entries}}}")
    }

    fn clear_interface_metadata(interface: &mut super::Interface) {
        interface.metadata.clear();
        for binding in &mut interface.bindings {
            binding.metadata.clear();
        }
        for function in &mut interface.functions {
            for parameter in &mut function.parameters {
                parameter.metadata.clear();
            }
        }
        for structure in &mut interface.structs {
            for field in &mut structure.fields {
                field.metadata.clear();
            }
        }
    }

    fn set_function_metadata_target_count(
        interface: &mut super::Interface,
        metadata: &[MetadataEntry],
        target_count: usize,
    ) {
        let function = interface.functions.first_mut().expect("sample function");
        interface
            .bindings
            .iter_mut()
            .find(|binding| binding.id == function.binding)
            .expect("sample function binding")
            .metadata = metadata.to_vec();
        let mut template = function
            .parameters
            .first()
            .cloned()
            .expect("sample parameter");
        function.parameters.clear();
        for index in 0..target_count.saturating_sub(1) {
            template.id = format!("{}::resource-{index}", function.binding);
            template.canonical = format!("resource-{index}");
            template.metadata = metadata.to_vec();
            function.parameters.push(template.clone());
        }
    }

    fn set_binding_metadata_target_count(
        interface: &mut super::Interface,
        metadata: &[MetadataEntry],
        target_count: usize,
    ) {
        let template = interface.bindings.first().cloned().expect("sample binding");
        interface.bindings = (0..target_count)
            .map(|index| {
                let mut binding = template.clone();
                binding.id = format!("sample.core::value::resource-{index}");
                binding.canonical = format!("resource-{index}");
                binding.python = format!("resource_{index}");
                binding.metadata = metadata.to_vec();
                binding
            })
            .collect();
        interface.aliases.clear();
        interface.functions.clear();
        interface.structs.clear();
        interface.operator_instances.clear();
        interface.macros.clear();
        interface.phase_helpers.clear();
    }

    fn emit_source(source: &str, module: &str) -> String {
        let surface = ast::lower_document(&source_reader::read(source));
        assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
        let typed = hir::lower_module(&surface.module, module);
        assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
        emit(&typed.module, &surface.module).expect("test interface emits")
    }

    #[test]
    fn canonical_interface_round_trips() {
        let (surface, typed) = modules();
        let encoded = emit(&typed, &surface).expect("interface should emit");
        let decoded = read(&encoded).expect("interface should read");
        assert_eq!(render(&decoded).unwrap(), encoded);
        assert!(
            decoded
                .aliases
                .iter()
                .any(|alias| alias.canonical == "距离")
        );
        assert!(decoded.aliases.iter().any(|alias| alias.canonical == "米"));
        assert_eq!(decoded.functions[0].parameters[0].aliases, ["点位"]);
        assert_eq!(decoded.structs[0].type_parameters, ["T"]);
        assert_eq!(decoded.structs[0].fields[1].aliases, ["最大值"]);
        assert!(!encoded.contains("private-value"));
    }

    #[test]
    fn nominal_binding_identity_round_trips_and_legacy_short_ids_fail_closed() {
        let (surface, typed) = modules();
        let encoded = emit(&typed, &surface).expect("interface should emit");
        let decoded = read(&encoded).expect("interface should read");
        let range = decoded
            .bindings
            .iter()
            .find(|binding| binding.canonical == "Range")
            .expect("public Range binding");
        assert!(matches!(
            &range.ty,
            Type::Nominal { binding, .. } if binding == "sample.core::type::Range"
        ));
        assert!(
            encoded.contains("[:nominal \"sample.core::type::Range\""),
            "{encoded}"
        );

        let legacy = encoded.replacen(
            "[:nominal \"sample.core::type::Range\"",
            "[:nominal \"Range\"",
            1,
        );
        let error = read(&legacy).expect_err("legacy short nominal identity must be rejected");
        assert_eq!(error.code, "OSR-I0084");
    }

    #[test]
    fn public_signature_cannot_leak_a_private_local_nominal_type() {
        let source = r#"
            (module sample.private-nominal)
            (defstruct Hidden [value Int])
            (defn expose [[value Hidden]] -> Hidden value)
            (export [expose])
        "#;
        let surface = ast::lower_document(&source_reader::read(source));
        assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
        let typed = hir::lower_module(&surface.module, "sample.private-nominal");
        assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
        let error = build(&typed.module, &surface.module)
            .expect_err("private nominal type must not leak through a public signature");
        assert_eq!(error.code, "OSR-I0084");
        assert!(error.message.contains("private or missing local type"));
    }

    #[test]
    fn interface_metadata_target_boundary_is_accepted_and_overflow_fails_closed() {
        let (surface, typed) = modules();
        let mut interface = build(&typed, &surface).expect("base interface");
        interface.metadata =
            metadata_with_normalized_bytes(METADATA_TARGET_LIMITS.max_normalized_bytes);
        refresh_standalone_hashes(&mut interface).expect("refresh boundary hashes");
        render(&interface).expect("metadata byte boundary must be publishable");

        interface.metadata =
            metadata_with_normalized_bytes(METADATA_TARGET_LIMITS.max_normalized_bytes + 1);
        let error = render(&interface).expect_err("direct model must enforce target limit");
        assert_eq!(error.code, "OSR-I0082");
        assert!(error.message.contains("syntax target normalized byte size"));

        let encoded = emit(&typed, &surface).expect("valid base interface");
        let oversized = metadata_map_source_with_normalized_bytes(
            METADATA_TARGET_LIMITS.max_normalized_bytes + 1,
        );
        let forged = encoded.replacen(":metadata {}", &format!(":metadata {oversized}"), 1);
        let error = read(&forged).expect_err("forged interface must enforce target limit");
        assert_eq!(error.code, "OSR-I0082");
    }

    #[test]
    fn interface_metadata_aggregate_boundaries_are_enforced() {
        let entry_target = metadata_entries(METADATA_TARGET_LIMITS.max_entries);

        let (surface, typed) = modules();
        let mut declaration = build(&typed, &surface).expect("base interface");
        clear_interface_metadata(&mut declaration);
        set_function_metadata_target_count(&mut declaration, &entry_target, 4);
        validate_interface_metadata_resources(&declaration)
            .expect("four full targets equal the declaration entry boundary");

        set_function_metadata_target_count(&mut declaration, &entry_target, 5);
        let error = render(&declaration).expect_err("declaration aggregate must fail closed");
        assert_eq!(error.code, "OSR-I0082");
        assert!(error.message.contains("metadata declaration entry count"));

        let mut interface = build(&typed, &surface).expect("base interface");
        clear_interface_metadata(&mut interface);
        set_binding_metadata_target_count(&mut interface, &entry_target, 32);
        assert_eq!(
            32 * METADATA_TARGET_LIMITS.max_entries,
            METADATA_INTERFACE_LIMITS.max_entries
        );
        validate_interface_metadata_resources(&interface)
            .expect("32 full targets equal the interface entry boundary");

        set_binding_metadata_target_count(&mut interface, &entry_target, 33);
        let error = render(&interface).expect_err("interface aggregate must fail closed");
        assert_eq!(error.code, "OSR-I0082");
        assert!(error.message.contains("metadata interface entry count"));
    }

    #[test]
    fn forged_interface_cannot_bypass_declaration_or_interface_totals() {
        let metadata = metadata_map_source_entries(METADATA_TARGET_LIMITS.max_entries);

        let parameters = (0..5)
            .map(|index| format!("[p{index} Int]"))
            .collect::<Vec<_>>()
            .join(" ");
        let declaration_source = format!(
            "(module sample.metadata-declaration)\n\
             (defn f [{parameters}] -> Int p0)\n\
             (export [f])"
        );
        let declaration_encoded = emit_source(&declaration_source, "sample.metadata-declaration");
        let declaration_forged =
            declaration_encoded.replace(":metadata {}", &format!(":metadata {metadata}"));
        let error = read(&declaration_forged)
            .expect_err("forged declaration aggregate must be rejected before hashes");
        assert_eq!(error.code, "OSR-I0082");
        assert!(error.message.contains("metadata declaration entry count"));

        let definitions = (0..32)
            .map(|index| format!("(def value{index} {index})"))
            .collect::<Vec<_>>()
            .join("\n");
        let exports = (0..32)
            .map(|index| format!("value{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        let interface_source =
            format!("(module sample.metadata-interface)\n{definitions}\n(export [{exports}])");
        let interface_encoded = emit_source(&interface_source, "sample.metadata-interface");
        let interface_forged =
            interface_encoded.replace(":metadata {}", &format!(":metadata {metadata}"));
        let error = read(&interface_forged)
            .expect_err("forged interface aggregate must be rejected before hashes");
        assert_eq!(error.code, "OSR-I0082");
        assert!(error.message.contains("metadata interface entry count"));
    }

    #[test]
    fn literal_type_arguments_round_trip_and_change_semantic_hashes() {
        let source = r#"
            (module sample.literal-types)
            (defstruct (Array T Axes) [values Any])
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
              frame)
            (export [Array Frame array-id frame-id])
        "#;
        let surface = ast::lower_document(&source_reader::read(source));
        assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
        let typed = hir::lower_module(&surface.module, "sample.literal-types");
        assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
        let encoded = emit(&typed.module, &surface.module).expect("literal interface emits");
        let decoded = read(&encoded).expect("literal interface reads");
        assert_eq!(
            render(&decoded).expect("literal interface renders"),
            encoded
        );
        assert!(encoded.contains(":literal"), "{encoded}");
        assert_eq!(
            decoded.functions[0].parameters[0].ty,
            decoded.functions[0].return_type
        );
        assert_eq!(
            decoded.functions[1].parameters[0].ty,
            decoded.functions[1].return_type
        );

        let changed_source = source.replace(":feature", ":channel");
        let changed_surface = ast::lower_document(&source_reader::read(&changed_source));
        let changed_typed = hir::lower_module(&changed_surface.module, "sample.literal-types");
        let changed = read(
            &emit(&changed_typed.module, &changed_surface.module)
                .expect("changed literal interface emits"),
        )
        .expect("changed literal interface reads");
        assert_ne!(decoded.hashes.semantic_body, changed.hashes.semantic_body);
        assert_ne!(
            decoded.semantic_interface_hash(),
            changed.semantic_interface_hash()
        );
    }

    #[test]
    fn exported_extern_contract_round_trips_without_gaining_trust() {
        let source = r#"
            (module sample.externs)
            (defstruct Series [values Any])
            (extern python "host.series"
              (defn rolling [[values Series] [window Int]] -> Series
                :contract
                {:id "host.series/rolling-v1"
                 :effects :pure
                 :temporal {:past "2*(window-1)"
                            :future 0
                            :availability :published}
                 :data {:axes [:time]
                        :alignment :labelled
                        :preserves-length true}})
              (defn dynamic [[value Int]] -> Int))
            (export [Series rolling dynamic])
        "#;
        let surface = ast::lower_document(&source_reader::read(source));
        assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
        let typed = hir::lower_module(&surface.module, "sample.externs");
        assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
        let encoded = emit(&typed.module, &surface.module).expect("interface should emit");
        let decoded = read(&encoded).expect("interface should read");

        let rolling = decoded
            .functions
            .iter()
            .find(|function| function.contract_id.as_deref() == Some("host.series/rolling-v1"))
            .expect("declared extern contract");
        assert_eq!(
            rolling.summaries.temporal.past,
            TemporalBound::Symbolic("2*(window-1)".to_owned())
        );
        assert_eq!(rolling.summaries.temporal.future, TemporalBound::Finite(0));
        assert_eq!(
            rolling.summaries.temporal.availability,
            Availability::Named("published".to_owned())
        );
        assert_eq!(rolling.summaries.data.preserves_length, Some(true));

        let dynamic = decoded
            .functions
            .iter()
            .find(|function| function.contract_id.is_none())
            .expect("uncontracted extern remains represented");
        assert!(dynamic.summaries.effects.open);
        assert_eq!(dynamic.summaries.temporal.future, TemporalBound::Unknown);
        assert_eq!(render(&decoded).unwrap(), encoded);
    }

    #[test]
    fn emission_is_byte_deterministic() {
        let (surface, typed) = modules();
        let first = emit(&typed, &surface).unwrap();
        let second = emit(&typed, &surface).unwrap();
        assert_eq!(first.as_bytes(), second.as_bytes());
        assert_eq!(first.lines().count(), 4);
    }

    #[test]
    fn content_tampering_is_rejected() {
        let (surface, typed) = modules();
        let encoded = emit(&typed, &surface).unwrap();
        let tampered = encoded.replacen("\"distance\"", "\"changed\"", 1);
        assert!(matches!(
            read(&tampered).unwrap_err().code,
            "OSR-I0015" | "OSR-I0073" | "OSR-I0084"
        ));
    }

    #[test]
    fn graph_envelope_tampering_is_rejected() {
        let (surface, typed) = modules();
        let encoded = emit(&typed, &surface).unwrap();
        let tampered = encoded.replacen(":group-id \"sample.core\"", ":group-id \"changed\"", 1);
        assert_eq!(read(&tampered).unwrap_err().code, "OSR-I0073");
    }

    #[test]
    fn public_static_schema_and_owned_record_round_trip() {
        let (surface, typed) = static_modules();
        let encoded = emit(&typed, &surface).expect("static interface should emit");
        let decoded = read(&encoded).expect("static interface should read");

        assert_eq!(decoded.static_schemas.len(), 1);
        assert_eq!(decoded.static_schemas[0].name, "Descriptor");
        assert_eq!(decoded.owned_records.len(), 1);
        assert_eq!(decoded.owned_records[0].owner_name, "public-owner");
        assert_eq!(render(&decoded).unwrap(), encoded);
        assert!(encoded.contains(":static-schemas"));
        assert!(encoded.contains(":owned-records"));

        // Distribution/provider records remain sidecar data. The compilation
        // interface graph hashes are published in a separate, non-recursive
        // section and therefore do not alter the semantic body hash.
        assert!(!encoded.contains(":distribution"));
        assert!(!encoded.contains(":interface-member-id"));
        assert!(encoded.contains(":semantic-interface-hash"));
    }

    #[test]
    fn private_static_declarations_are_filtered() {
        let (surface, typed) = static_modules();
        let encoded = emit(&typed, &surface).unwrap();
        let decoded = read(&encoded).unwrap();

        assert!(
            decoded
                .static_schemas
                .iter()
                .all(|schema| schema.name != "PrivateSchema")
        );
        assert!(
            decoded
                .owned_records
                .iter()
                .all(|record| record.owner_name != "private-owner")
        );
        assert!(!encoded.contains("sample/private"));
        assert!(!encoded.contains("private-owner"));
    }

    #[test]
    fn static_payload_tampering_is_rejected() {
        let (surface, typed) = static_modules();
        let encoded = emit(&typed, &surface).unwrap();

        let schema_tamper = encoded.replacen("sample/descriptor", "sample/changed", 1);
        assert!(matches!(
            read(&schema_tamper).unwrap_err().code,
            "OSR-I0015" | "OSR-I0056" | "OSR-I0057"
        ));

        let record_tamper = encoded.replacen("\"alpha\"", "\"omega\"", 1);
        assert!(matches!(
            read(&record_tamper).unwrap_err().code,
            "OSR-I0015" | "OSR-I0057"
        ));

        let changed_source = STATIC_SOURCE.replacen("\"alpha\"", "\"omega\"", 1);
        let changed_surface = ast::lower_document(&source_reader::read(&changed_source));
        assert!(changed_surface.diagnostics.is_empty());
        let changed_typed = hir::lower_module(&changed_surface.module, "sample.records");
        let changed = read(
            &emit(&changed_typed.module, &changed_surface.module)
                .expect("changed static interface should emit"),
        )
        .unwrap();
        let original = read(&encoded).unwrap();
        assert_ne!(
            changed.hashes.interface_body,
            original.hashes.interface_body
        );
        assert_ne!(changed.hashes.semantic_body, original.hashes.semantic_body);
    }

    #[test]
    fn public_macro_ir_round_trips_and_replays() {
        let (surface, typed) = macro_modules(MACRO_SOURCE);
        let encoded = emit(&typed, &surface).expect("macro interface should emit");
        let decoded = read(&encoded).expect("macro interface should read");

        assert_eq!(decoded.macros.len(), 1);
        assert_eq!(decoded.macros[0].canonical, "public-pipeline");
        assert_eq!(decoded.macros[0].minimum_arity, 1);
        assert!(decoded.macros[0].variadic);
        assert_eq!(decoded.phase_helpers.len(), 2);
        assert_eq!(
            decoded
                .phase_helpers
                .iter()
                .map(|helper| helper.canonical.as_str())
                .collect::<Vec<_>>(),
            ["helper", "helper-two"]
        );
        assert!(!encoded.contains("unused-helper"));
        assert!(!encoded.contains("hidden-macro"));
        assert!(!encoded.contains("/home/"));
        assert_eq!(render(&decoded).unwrap(), encoded);

        let imported = decoded.imported_phase_forms();
        let input = source_reader::read("(public-pipeline 1)");
        let expanded = macro_expand::expand_with_imported_phase_forms(
            &input,
            &imported,
            macro_expand::ExpansionOptions::default(),
        );
        assert!(
            expanded.document.diagnostics.is_empty(),
            "{:?}",
            expanded.document.diagnostics
        );
        let rendered = crate::printer::render_document_text(&expanded.document);
        assert!(rendered.contains("inc"), "{rendered}");
    }

    #[test]
    fn macro_ir_tampering_is_rejected_and_changes_semantic_hash() {
        let (surface, typed) = macro_modules(MACRO_SOURCE);
        let encoded = emit(&typed, &surface).unwrap();
        let tampered = encoded.replacen("helper-two", "missing-helper", 1);
        assert!(matches!(
            read(&tampered).unwrap_err().code,
            "OSR-I0059" | "OSR-I0060" | "OSR-I0015"
        ));

        let changed_source = MACRO_SOURCE.replacen("'inc", "'dec", 1);
        let (changed_surface, changed_typed) = macro_modules(&changed_source);
        let changed = read(&emit(&changed_typed, &changed_surface).unwrap()).unwrap();
        let original = read(&encoded).unwrap();
        assert_ne!(
            changed.hashes.interface_body,
            original.hashes.interface_body
        );
        assert_ne!(changed.hashes.semantic_body, original.hashes.semantic_body);
    }

    #[test]
    fn operator_instance_round_trips_and_is_semantic() {
        let (surface, typed) = operator_modules(OPERATOR_SOURCE);
        let encoded = emit(&typed, &surface).expect("operator interface should emit");
        let decoded = read(&encoded).expect("operator interface should read");

        assert_eq!(decoded.operator_instances.len(), 1);
        let instance = &decoded.operator_instances[0];
        assert_eq!(instance.operator, crate::types::ScalarOperator::Multiply);
        assert_eq!(instance.operands.len(), 2);
        assert_eq!(render(&decoded).unwrap(), encoded);
        assert!(encoded.contains(":operator-instances"));

        let changed_source = OPERATOR_SOURCE.replace(":multiply", ":subtract");
        let (changed_surface, changed_typed) = operator_modules(&changed_source);
        let changed = read(&emit(&changed_typed, &changed_surface).unwrap()).unwrap();
        assert_ne!(changed.hashes.interface_body, decoded.hashes.interface_body);
        assert_ne!(changed.hashes.semantic_body, decoded.hashes.semantic_body);
    }

    #[test]
    fn operator_instance_tampering_is_rejected_before_hash_acceptance() {
        let (surface, typed) = operator_modules(OPERATOR_SOURCE);
        let encoded = emit(&typed, &surface).unwrap();

        let invalid_id = encoded.replacen("::operator::multiply\"", "::operator::subtract\"", 1);
        assert_eq!(read(&invalid_id).unwrap_err().code, "OSR-I0068");

        let invalid_signature = encoded.replacen(":float] :result", ":any] :result", 1);
        assert!(matches!(
            read(&invalid_signature).unwrap_err().code,
            "OSR-I0069" | "OSR-I0071"
        ));
    }

    #[test]
    fn operator_instance_requires_an_owned_nominal_operand() {
        let source = r#"
            (module sample.operators)
            ^{:osiris/operator :add}
            (defn add-scalars [[left Float] [right Float]] -> Float left)
            (export [add-scalars])
        "#;
        let (surface, typed) = operator_modules(source);
        assert_eq!(emit(&typed, &surface).unwrap_err().code, "OSR-I0064");
    }

    #[test]
    fn duplicate_operator_operand_tuple_is_rejected() {
        let source = OPERATOR_SOURCE.replace(
            "(export [Series multiply-series])",
            r#"
            ^{:osiris/operator :multiply}
            (defn multiply-series-again
              [[series (Series Float)] [multiplier Float]]
              -> (Series Float)
              series)
            (export [Series multiply-series multiply-series-again])
            "#,
        );
        let (surface, typed) = operator_modules(&source);
        assert_eq!(emit(&typed, &surface).unwrap_err().code, "OSR-I0065");
    }

    #[test]
    fn duplicate_and_private_entries_are_rejected() {
        let (surface, typed) = modules();
        let encoded = emit(&typed, &surface).unwrap();
        let duplicate = encoded.replacen(
            ":module \"sample.core\"",
            ":module \"sample.core\" :module \"duplicate\"",
            1,
        );
        assert_eq!(read(&duplicate).unwrap_err().code, "OSR-I0043");

        let private = encoded.replacen(":visibility :public", ":visibility :private", 1);
        assert_eq!(read(&private).unwrap_err().code, "OSR-I0053");
    }

    #[test]
    fn incompatible_abi_is_rejected() {
        let (surface, typed) = modules();
        let encoded = emit(&typed, &surface).unwrap();
        let incompatible =
            encoded.replacen("\"osiris-compiler-v0\"", "\"osiris-compiler-v999\"", 1);
        assert_eq!(read(&incompatible).unwrap_err().code, "OSR-I0013");
    }

    #[test]
    fn reading_runtime_locator_does_not_import_python() {
        let (surface, mut typed) = modules();
        let binding = typed
            .bindings
            .iter_mut()
            .find(|binding| binding.name.canonical == "distance")
            .unwrap();
        binding.runtime = Some(hir::RuntimeBinding {
            module: "module_that_cannot_exist_for_osiris_test".to_owned(),
            name: "distance".to_owned(),
            python_module: true,
        });
        let encoded = emit(&typed, &surface).unwrap();
        let decoded = read(&encoded).unwrap();
        assert_eq!(
            decoded.bindings[0]
                .runtime
                .as_ref()
                .map(|runtime| runtime.module.as_str()),
            Some("module_that_cannot_exist_for_osiris_test")
        );
    }
}
