//! Structured Python backend for the typed HIR.
//!
//! The backend intentionally has no source-string templates for generated
//! Python.  It lowers HIR into [`crate::python_ast`] first and delegates all
//! syntax, escaping, and precedence decisions to that module's printer.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::{
    hir::{self, ExprKind, ItemKind, Operator},
    name::python_identifier,
    python_ast as py,
    source::Span,
    types::{PythonVersion, Type, nominal_short_name, python_builtin_exception_from_binding},
};

/// A fully rendered backend result.  Keeping the AST alongside its rendering
/// lets the compiler, source-map writer, and tests inspect the same result.
#[derive(Clone, Debug, PartialEq)]
pub struct GeneratedPython {
    pub module: py::Module,
    pub source: String,
}

/// An error raised while lowering a semantically valid HIR to Python.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackendError {
    pub message: String,
    pub span: Option<Span>,
}

impl BackendError {
    fn new(message: impl Into<String>, span: Option<Span>) -> Self {
        Self {
            message: message.into(),
            span,
        }
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    #[must_use]
    pub const fn span(&self) -> Option<Span> {
        self.span
    }
}

impl fmt::Display for BackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for BackendError {}

/// Lower a typed HIR module to a deterministic Python module and source.
pub fn compile_module(
    module: &hir::Module,
    target: impl Into<PythonVersion>,
) -> Result<GeneratedPython, BackendError> {
    let mut backend = Backend::new(module, target.into());
    let body = backend.lower_items(module)?;
    let mut imports = backend.imports();
    imports.extend(backend.typing_imports());

    // Future annotations keeps nominal/generic references readable and makes
    // forward references between generated declarations legal on Python 3.9.
    let mut final_body = Vec::with_capacity(imports.len() + body.len());
    final_body.push(py::Stmt::Import(py::Import::From {
        module: Some("__future__".to_owned()),
        names: vec![py::ImportAlias::new("annotations")],
        level: 0,
    }));
    final_body.extend(imports);
    final_body.extend(backend.typevar_declarations());
    final_body.extend(body);
    let python_module = py::Module::new(final_body);
    let source = python_module
        .to_source()
        .map_err(|error| BackendError::new(error.to_string(), None))?;
    Ok(GeneratedPython {
        module: python_module,
        source,
    })
}

/// Alias kept explicit for callers that prefer the verb used by other
/// backends.  It also makes the public API pleasant to discover in docs.
pub fn emit_module(
    module: &hir::Module,
    target: impl Into<PythonVersion>,
) -> Result<GeneratedPython, BackendError> {
    compile_module(module, target)
}

struct Backend<'hir> {
    target: PythonVersion,
    bindings: BTreeMap<crate::name::BindingId, &'hir hir::Binding>,
    names: BTreeMap<crate::name::BindingId, String>,
    reserved_names: BTreeSet<String>,
    temporary_counter: usize,
    helper_counter: usize,
    direct_imports: BTreeMap<String, Option<String>>,
    from_imports: BTreeMap<String, BTreeMap<String, Option<String>>>,
    typing: BTreeSet<String>,
    need_dataclass: bool,
    need_dataclass_field: bool,
    typevars: BTreeMap<String, String>,
    typevar_names: BTreeMap<crate::types::TypeVarId, String>,
    active_type_parameters: BTreeMap<String, String>,
    binding_overrides: Vec<BTreeMap<crate::name::BindingId, py::Expr>>,
}

impl<'hir> Backend<'hir> {
    fn new(hir: &'hir hir::Module, target: PythonVersion) -> Self {
        let bindings = hir
            .bindings
            .iter()
            .map(|binding| (binding.name.id.clone(), binding))
            .collect::<BTreeMap<_, _>>();
        let mut reserved_names = BTreeSet::new();
        let mut names = BTreeMap::new();
        // HIR has already checked global Python collisions.  Local bindings
        // can repeat across lexical scopes, which is legal in separate Python
        // functions, so retain their canonical spelling here for readability.
        let mut global_bindings = hir
            .bindings
            .iter()
            .filter(|binding| {
                matches!(
                    binding.name.kind,
                    crate::name::BindingKind::Module
                        | crate::name::BindingKind::PythonModule
                        | crate::name::BindingKind::Value
                        | crate::name::BindingKind::Function
                        | crate::name::BindingKind::Type
                )
            })
            .collect::<Vec<_>>();
        global_bindings.sort_by_key(|binding| {
            (
                !binding
                    .name
                    .id
                    .as_str()
                    .starts_with(&format!("{}::", hir.name)),
                binding.name.id.as_str(),
            )
        });
        for binding in global_bindings {
            let base = binding.name.python.clone();
            let mut python = base.clone();
            let mut suffix = 2_usize;
            while reserved_names.contains(&python) {
                python = format!("{base}_{suffix}");
                suffix += 1;
            }
            reserved_names.insert(python.clone());
            names.insert(binding.name.id.clone(), python);
        }
        for binding in &hir.bindings {
            reserved_names.insert(binding.name.python.clone());
            names
                .entry(binding.name.id.clone())
                .or_insert_with(|| binding.name.python.clone());
        }
        Self {
            target,
            bindings,
            names,
            reserved_names,
            temporary_counter: 0,
            helper_counter: 0,
            direct_imports: BTreeMap::new(),
            from_imports: BTreeMap::new(),
            typing: BTreeSet::new(),
            need_dataclass: false,
            need_dataclass_field: false,
            typevars: BTreeMap::new(),
            typevar_names: BTreeMap::new(),
            active_type_parameters: BTreeMap::new(),
            binding_overrides: Vec::new(),
        }
    }

    fn lower_items(&mut self, module: &hir::Module) -> Result<Vec<py::Stmt>, BackendError> {
        let mut body = Vec::new();
        for item in &module.items {
            match &item.kind {
                ItemKind::Import(import) => self.register_item_import(import),
                ItemKind::Value(value) => {
                    let target = self.binding_target(&value.binding)?;
                    let binding = self.binding(&value.binding)?;
                    let annotation = self.annotation(&binding.ty, Some(item.span))?;
                    match &value.value {
                        Some(expression) => {
                            let lowered = self.lower_value(expression)?;
                            if !lowered.prefix.is_empty() {
                                body.push(py::Stmt::AnnAssign(py::AnnAssign {
                                    target: target.clone(),
                                    annotation,
                                    value: None,
                                }));
                                body.extend(lowered.prefix);
                                let result = lowered.value.ok_or_else(|| {
                                    self.error(
                                        "value definition terminates before producing a value",
                                        Some(expression.span),
                                    )
                                })?;
                                body.push(py::Stmt::Assign(py::Assign {
                                    targets: vec![target],
                                    value: result,
                                }));
                            } else {
                                body.push(py::Stmt::AnnAssign(py::AnnAssign {
                                    target,
                                    annotation,
                                    value: Some(lowered.value.ok_or_else(|| {
                                        self.error(
                                            "value definition does not produce a value",
                                            Some(expression.span),
                                        )
                                    })?),
                                }));
                            }
                        }
                        None => body.push(py::Stmt::AnnAssign(py::AnnAssign {
                            target,
                            annotation,
                            value: None,
                        })),
                    }
                }
                ItemKind::Function(function) => {
                    let lowered = self.lower_function(function)?;
                    body.push(lowered);
                }
                ItemKind::Struct(structure) => {
                    body.extend(self.lower_struct(structure)?);
                }
                ItemKind::Expr(expression) => {
                    let lowered = self.lower_value(expression)?;
                    body.extend(lowered.prefix);
                    if let Some(value) = lowered.value {
                        body.push(py::Stmt::Expr(value));
                    }
                }
                ItemKind::StaticSchema(_) | ItemKind::StaticRecord(_) => {
                    // Static interface data belongs in .osri/records artifacts;
                    // it has no runtime Python statement.
                }
            }
        }
        Ok(body)
    }

    fn register_runtime_binding(&mut self, id: &crate::name::BindingId) {
        let Some(binding) = self.bindings.get(id).copied() else {
            return;
        };
        let Some(runtime) = &binding.runtime else {
            return;
        };
        if binding.name.kind == crate::name::BindingKind::PythonModule
            || binding.name.kind == crate::name::BindingKind::Module
        {
            return;
        }
        let local = self.python_name(&binding.name.id).to_owned();
        self.from_imports
            .entry(runtime.module.replace('/', "."))
            .or_default()
            .insert(runtime.name.clone(), Some(local));
    }

    fn register_runtime_type(&mut self, nominal_binding: &str) {
        // Nominal types in HIR use the stable defining type BindingId. A
        // static schema has no corresponding runtime struct and therefore is
        // never registered here; it remains a records/.osri-only declaration.
        let binding_id = self.bindings.iter().find_map(|(id, binding)| {
            (binding.name.kind == crate::name::BindingKind::Type && id.as_str() == nominal_binding)
                .then_some(id.clone())
        });
        if let Some(id) = binding_id {
            self.register_runtime_binding(&id);
        }
    }

    fn register_item_import(&mut self, import: &hir::Import) {
        let local = self
            .names
            .get(&import.binding)
            .cloned()
            .unwrap_or_else(|| python_identifier(&import.module));
        let module = import.module.replace('/', ".");
        let default_local = module.rsplit('.').next().unwrap_or(&module);
        let alias = (local != default_local).then_some(local);
        self.direct_imports.insert(module, alias);
    }

    fn imports(&self) -> Vec<py::Stmt> {
        let mut result = Vec::new();
        if self.need_dataclass {
            let mut names = vec![py::ImportAlias::new("dataclass")];
            if self.need_dataclass_field {
                names.push(py::ImportAlias::new("field"));
            }
            result.push(py::Stmt::Import(py::Import::From {
                module: Some("dataclasses".to_owned()),
                names,
                level: 0,
            }));
        }
        for (module, names) in &self.from_imports {
            let aliases = names
                .iter()
                .map(|(name, alias)| match alias {
                    Some(alias) if alias != name => py::ImportAlias::renamed(name, alias),
                    _ => py::ImportAlias::new(name),
                })
                .collect();
            result.push(py::Stmt::Import(py::Import::From {
                module: Some(module.clone()),
                names: aliases,
                level: 0,
            }));
        }
        for (module, alias) in &self.direct_imports {
            result.push(py::Stmt::Import(py::Import::Direct(vec![match alias {
                Some(alias) => py::ImportAlias::renamed(module, alias),
                None => py::ImportAlias::new(module),
            }])))
        }
        for name in &self.typing {
            // Added below as one grouped statement; this loop only keeps
            // deterministic ordering obvious to readers of the backend.
            let _ = name;
        }
        if !self.typing.is_empty() {
            result.insert(
                usize::from(self.need_dataclass),
                py::Stmt::Import(py::Import::From {
                    module: Some("typing".to_owned()),
                    names: self.typing.iter().map(py::ImportAlias::new).collect(),
                    level: 0,
                }),
            );
        }
        result
    }

    fn typing_imports(&self) -> Vec<py::Stmt> {
        // Imports are emitted in `imports`; this hook is kept separate so the
        // final assembly can remain stable if more typing backends are added.
        Vec::new()
    }

    fn typevar_declarations(&self) -> Vec<py::Stmt> {
        self.typevars
            .iter()
            .map(|(source, python)| {
                py::Stmt::Assign(py::Assign {
                    targets: vec![py::Expr::name(python.clone())],
                    value: py::Expr::call(
                        py::Expr::name("TypeVar"),
                        vec![py::CallArgument::Positional(py::Expr::string(
                            source.clone(),
                        ))],
                    ),
                })
            })
            .collect()
    }

    fn lower_function(&mut self, function: &hir::Function) -> Result<py::Stmt, BackendError> {
        let binding_name = self.python_name(&function.binding).to_owned();
        let mut parameters = py::Parameters::default();
        for (parameter_index, parameter) in function.parameters.iter().enumerate() {
            let name = self.python_name(&parameter.binding).to_owned();
            let annotation = Some(self.annotation(&parameter.ty, Some(function.body.span))?);
            let default = match &parameter.default {
                Some(default) => {
                    let lowered = self.lower_value(default)?;
                    if !lowered.prefix.is_empty() {
                        return Err(self.error(
                            "function parameter defaults must be expression-only",
                            Some(default.span),
                        ));
                    }
                    Some(lowered.value.ok_or_else(|| {
                        self.error(
                            "parameter default does not produce a value",
                            Some(default.span),
                        )
                    })?)
                }
                None => None,
            };
            let parameter = py::Parameter {
                name,
                annotation,
                default,
            };
            if function.parameters[parameter_index].variadic {
                parameters.vararg = Some(py::Parameter {
                    name: parameter.name,
                    annotation: parameter.annotation,
                    default: None,
                });
            } else {
                parameters.positional.push(parameter);
            }
        }
        let returns = Some(self.annotation(&function.return_type, Some(function.body.span))?);
        let mut body = self.lower_tail(&function.body)?;
        if body.is_empty() {
            body.push(py::Stmt::Pass);
        }
        Ok(py::Stmt::FunctionDef(Box::new(py::FunctionDef {
            name: binding_name,
            parameters,
            returns,
            decorators: Vec::new(),
            body,
            is_async: false,
        })))
    }

    fn lower_struct(&mut self, structure: &hir::Struct) -> Result<Vec<py::Stmt>, BackendError> {
        self.need_dataclass = true;
        let struct_name = self.python_name(&structure.binding).to_owned();
        let previous = std::mem::take(&mut self.active_type_parameters);
        if let Ok(binding) = self.binding(&structure.binding) {
            if let Type::Nominal { args, .. } = &binding.ty {
                for (parameter, argument) in structure.type_parameters.iter().zip(args) {
                    if let Type::TypeVar(variable) = argument {
                        self.typevar_names.insert(*variable, parameter.clone());
                    }
                }
            }
        }
        for parameter in &structure.type_parameters {
            let python = self
                .typevars
                .entry(parameter.clone())
                .or_insert_with(|| parameter.clone())
                .clone();
            self.active_type_parameters
                .insert(parameter.clone(), python);
        }
        let mut body = Vec::new();
        if let Some(doc) = &structure.doc {
            body.push(py::Stmt::Expr(py::Expr::string(doc.clone())));
        }
        for field in &structure.fields {
            let target = py::Expr::name(self.python_name(&field.binding).to_owned());
            let annotation = self.annotation(&field.ty, Some(Span::default()))?;
            let value = match &field.default {
                None => None,
                Some(default) => {
                    let lowered = self.lower_value(default)?;
                    let value = lowered.value.ok_or_else(|| {
                        self.error(
                            "struct field default does not produce a value",
                            Some(default.span),
                        )
                    })?;
                    if lowered.prefix.is_empty() && is_safe_dataclass_default(&value) {
                        Some(value)
                    } else if lowered.prefix.is_empty() {
                        self.need_dataclass_field = true;
                        Some(py::Expr::call(
                            py::Expr::name("field"),
                            vec![py::CallArgument::Keyword(py::KeywordArgument::Named {
                                name: "default_factory".to_owned(),
                                value: py::Expr::Lambda {
                                    parameters: Box::new(py::Parameters::default()),
                                    body: Box::new(value),
                                },
                            })],
                        ))
                    } else {
                        // A helper function preserves both complex control
                        // flow and per-instance default evaluation.
                        self.need_dataclass_field = true;
                        let helper = self.fresh_helper("_osr_default");
                        let mut helper_body = lowered.prefix;
                        helper_body.push(py::Stmt::Return(Some(value)));
                        body.push(py::Stmt::FunctionDef(Box::new(py::FunctionDef {
                            name: helper.clone(),
                            parameters: py::Parameters::default(),
                            returns: None,
                            decorators: Vec::new(),
                            body: helper_body,
                            is_async: false,
                        })));
                        Some(py::Expr::call(
                            py::Expr::name("field"),
                            vec![py::CallArgument::Keyword(py::KeywordArgument::Named {
                                name: "default_factory".to_owned(),
                                value: py::Expr::name(helper),
                            })],
                        ))
                    }
                }
            };
            body.push(py::Stmt::AnnAssign(py::AnnAssign {
                target,
                annotation,
                value,
            }));
        }
        if !structure.checks.is_empty() {
            let self_name = py::Expr::name("self");
            let mut overrides = BTreeMap::new();
            for field in &structure.fields {
                overrides.insert(
                    field.binding.clone(),
                    py::Expr::Attribute {
                        value: Box::new(self_name.clone()),
                        attr: self.python_name(&field.binding).to_owned(),
                    },
                );
            }
            self.binding_overrides.push(overrides);
            let mut checks = Vec::new();
            for check in &structure.checks {
                let lowered = self.lower_value(&check.condition)?;
                let condition = lowered.value.ok_or_else(|| {
                    self.error(
                        "struct check does not produce a boolean value",
                        Some(check.condition.span),
                    )
                })?;
                let message = check
                    .message
                    .as_ref()
                    .map(|message| self.lower_value(message))
                    .transpose()?;
                checks.push((lowered.prefix, condition, message));
            }
            self.binding_overrides.pop();
            let mut check_body = Vec::new();
            for (prefix, condition, message) in checks {
                check_body.extend(prefix);
                let mut failure_body = Vec::new();
                let message = if let Some(message) = message {
                    failure_body.extend(message.prefix);
                    message.value.ok_or_else(|| {
                        self.error("struct check message does not produce a value", None)
                    })?
                } else {
                    py::Expr::string(format!("invariant failed for {struct_name}"))
                };
                failure_body.push(py::Stmt::Raise(py::Raise {
                    exception: Some(py::Expr::call(
                        py::Expr::name("ValueError"),
                        vec![py::CallArgument::Positional(message)],
                    )),
                    cause: None,
                }));
                check_body.push(py::Stmt::If(py::IfStmt {
                    test: py::Expr::UnaryOp {
                        op: py::UnaryOp::Not,
                        operand: Box::new(condition),
                    },
                    body: failure_body,
                    orelse: Vec::new(),
                }));
            }
            body.push(py::Stmt::FunctionDef(Box::new(py::FunctionDef {
                name: "__post_init__".to_owned(),
                parameters: py::Parameters {
                    positional: vec![py::Parameter::new("self")],
                    ..py::Parameters::default()
                },
                returns: Some(py::Expr::name("None")),
                decorators: Vec::new(),
                body: check_body,
                is_async: false,
            })));
        }
        let mut bases = Vec::new();
        if !structure.type_parameters.is_empty() {
            // Generic and TypeVar are runtime names on Python 3.9-3.11.  A
            // type parameter need not occur in a field annotation (for
            // example an extension marker struct can intentionally leave its
            // payload as Any), so register both imports from the declaration
            // itself rather than relying on annotation traversal.
            self.typing.insert("Generic".to_owned());
            self.typing.insert("TypeVar".to_owned());
            bases.push(py::Expr::Subscript {
                value: Box::new(py::Expr::name("Generic")),
                slice: Box::new(if structure.type_parameters.len() == 1 {
                    py::Expr::name(
                        self.active_type_parameters
                            .get(&structure.type_parameters[0])
                            .cloned()
                            .unwrap_or_else(|| structure.type_parameters[0].clone()),
                    )
                } else {
                    py::Expr::Tuple(
                        structure
                            .type_parameters
                            .iter()
                            .map(|parameter| {
                                py::Expr::name(
                                    self.active_type_parameters
                                        .get(parameter)
                                        .cloned()
                                        .unwrap_or_else(|| parameter.clone()),
                                )
                            })
                            .collect(),
                    )
                }),
            });
        }
        self.active_type_parameters = previous;
        Ok(vec![py::Stmt::ClassDef(py::ClassDef {
            name: struct_name,
            bases,
            keywords: Vec::new(),
            decorators: vec![py::Expr::call(
                py::Expr::name("dataclass"),
                vec![py::CallArgument::Keyword(py::KeywordArgument::Named {
                    name: "frozen".to_owned(),
                    value: py::Expr::Literal(py::Literal::Bool(true)),
                })],
            )],
            body,
        })])
    }

    fn lower_tail(&mut self, expression: &hir::Expr) -> Result<Vec<py::Stmt>, BackendError> {
        match &expression.kind {
            ExprKind::Let { bindings, body } => {
                let mut result = Vec::new();
                for binding in bindings {
                    let lowered = self.lower_value(&binding.value)?;
                    result.extend(lowered.prefix);
                    let value = lowered.value.ok_or_else(|| {
                        self.error(
                            "let binding does not produce a value",
                            Some(binding.value.span),
                        )
                    })?;
                    result.push(py::Stmt::Assign(py::Assign {
                        targets: vec![self.binding_target(&binding.binding)?],
                        value,
                    }));
                }
                result.extend(self.lower_tail(body)?);
                Ok(result)
            }
            ExprKind::Do(expressions) => {
                let mut result = Vec::new();
                for expression in expressions.iter().take(expressions.len().saturating_sub(1)) {
                    let lowered = self.lower_value(expression)?;
                    result.extend(lowered.prefix);
                    if let Some(value) = lowered.value {
                        result.push(py::Stmt::Expr(value));
                    } else {
                        return Ok(result);
                    }
                }
                if let Some(last) = expressions.last() {
                    result.extend(self.lower_tail(last)?);
                }
                Ok(result)
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let condition = self.lower_value(condition)?;
                let condition_value = condition.value.ok_or_else(|| {
                    self.error(
                        "if condition does not produce a value",
                        Some(expression.span),
                    )
                })?;
                let then_body = self.lower_tail(then_branch)?;
                let else_body = self.lower_tail(else_branch)?;
                let mut result = condition.prefix;
                result.push(py::Stmt::If(py::IfStmt {
                    test: condition_value,
                    body: then_body,
                    orelse: else_body,
                }));
                Ok(result)
            }
            ExprKind::Try {
                body,
                catches,
                finally_body,
            } => {
                // Clojure permits a body-only `try`; Python requires at
                // least one handler or a finally suite.  In the body-only
                // case the construct has no observable exception boundary,
                // so preserve the expression directly instead of emitting
                // invalid Python syntax.
                if catches.is_empty() && finally_body.is_none() {
                    return self.lower_tail(body);
                }
                let mut handlers = Vec::new();
                for catch in catches {
                    let exception_type = catch
                        .exception_type
                        .as_ref()
                        .map(|ty| self.annotation(ty, Some(catch.body.span)))
                        .transpose()?;
                    let name = catch
                        .binding
                        .as_ref()
                        .map(|binding| self.python_name(binding).to_owned());
                    handlers.push(py::ExceptHandler {
                        exception_type,
                        name,
                        body: self.lower_tail(&catch.body)?,
                    });
                }
                let mut prefix = Vec::new();
                let body_stmts = self.lower_tail(body)?;
                let finally_stmts = finally_body
                    .as_ref()
                    .map(|body| self.lower_discard(body))
                    .transpose()?
                    .unwrap_or_default();
                prefix.push(py::Stmt::Try(py::Try {
                    body: body_stmts,
                    handlers,
                    orelse: Vec::new(),
                    finalbody: finally_stmts,
                }));
                Ok(prefix)
            }
            ExprKind::Raise(value) => {
                let mut result = Vec::new();
                let exception = value
                    .as_ref()
                    .map(|value| self.lower_value(value))
                    .transpose()?;
                if let Some(lowered) = exception {
                    result.extend(lowered.prefix);
                    result.push(py::Stmt::Raise(py::Raise {
                        exception: Some(lowered.value.ok_or_else(|| {
                            self.error(
                                "raise expression does not produce a value",
                                value.as_ref().map(|value| value.span),
                            )
                        })?),
                        cause: None,
                    }));
                } else {
                    result.push(py::Stmt::Raise(py::Raise {
                        exception: None,
                        cause: None,
                    }));
                }
                Ok(result)
            }
            _ => {
                let lowered = self.lower_value(expression)?;
                let mut result = lowered.prefix;
                if let Some(value) = lowered.value {
                    result.push(py::Stmt::Return(Some(value)));
                }
                Ok(result)
            }
        }
    }

    fn lower_discard(&mut self, expression: &hir::Expr) -> Result<Vec<py::Stmt>, BackendError> {
        let lowered = self.lower_value(expression)?;
        let mut result = lowered.prefix;
        if let Some(value) = lowered.value {
            result.push(py::Stmt::Expr(value));
        }
        Ok(result)
    }

    fn lower_value(&mut self, expression: &hir::Expr) -> Result<Lowered, BackendError> {
        let span = expression.span;
        let result = match &expression.kind {
            ExprKind::None => py::Expr::Literal(py::Literal::None),
            ExprKind::Bool(value) => py::Expr::Literal(py::Literal::Bool(*value)),
            ExprKind::Integer(value) => {
                return Ok(Lowered::value(py::Expr::Literal(py::Literal::IntegerText(
                    value.clone(),
                ))));
            }
            ExprKind::Float(value) => py::Expr::Literal(py::Literal::Float(
                value
                    .parse::<f64>()
                    .map_err(|_| self.error("invalid float literal", Some(span)))?,
            )),
            ExprKind::String(value) => py::Expr::Literal(py::Literal::String(value.clone())),
            ExprKind::Binding(binding) => return Ok(Lowered::value(self.binding_expr(binding)?)),
            ExprKind::List(items) => return self.lower_sequence(items, false),
            ExprKind::Vector(items) => return self.lower_sequence(items, true),
            ExprKind::Map(entries) => {
                let mut prefix = Vec::new();
                let mut pairs = Vec::new();
                for (key_expression, value_expression) in entries {
                    let key = self.lower_value(key_expression)?;
                    prefix.extend(key.prefix);
                    let value_key = key.value.ok_or_else(|| {
                        self.error(
                            "map key does not produce a value",
                            Some(key_expression.span),
                        )
                    })?;
                    let value = self.lower_value(value_expression)?;
                    prefix.extend(value.prefix);
                    let value_value = value.value.ok_or_else(|| {
                        self.error(
                            "map value does not produce a value",
                            Some(value_expression.span),
                        )
                    })?;
                    pairs.push(py::DictItem::Pair {
                        key: value_key,
                        value: value_value,
                    });
                }
                return Ok(Lowered {
                    prefix,
                    value: Some(py::Expr::Dict(pairs)),
                });
            }
            ExprKind::Set(items) => return self.lower_sequence(items, false)?.with_set(),
            ExprKind::Call { callee, arguments } => {
                let callee_expression = callee;
                let callee = self.lower_value(callee_expression)?;
                let mut prefix = callee.prefix;
                let function = callee.value.ok_or_else(|| {
                    self.error(
                        "call target does not produce a value",
                        Some(callee_expression.span),
                    )
                })?;
                let mut args = Vec::new();
                for argument in arguments {
                    match argument {
                        hir::CallArgument::Positional(value_expression) => {
                            let value = self.lower_value(value_expression)?;
                            prefix.extend(value.prefix);
                            args.push(py::CallArgument::Positional(value.value.ok_or_else(
                                || {
                                    self.error(
                                        "call argument does not produce a value",
                                        Some(value_expression.span),
                                    )
                                },
                            )?));
                        }
                        hir::CallArgument::Keyword {
                            name,
                            value: value_expression,
                        } => {
                            let value = self.lower_value(value_expression)?;
                            prefix.extend(value.prefix);
                            args.push(py::CallArgument::Keyword(py::KeywordArgument::Named {
                                name: python_identifier(name),
                                value: value.value.ok_or_else(|| {
                                    self.error(
                                        "keyword argument does not produce a value",
                                        Some(value_expression.span),
                                    )
                                })?,
                            }));
                        }
                    }
                }
                return Ok(Lowered {
                    prefix,
                    value: Some(py::Expr::call(function, args)),
                });
            }
            ExprKind::Operator { operator, operands } => {
                return self.lower_operator(*operator, operands);
            }
            ExprKind::Attribute { value, attribute } => {
                let value_expression = value;
                let value = self.lower_value(value_expression)?;
                let prefix = value.prefix;
                let base = value.value.ok_or_else(|| {
                    self.error(
                        "attribute base does not produce a value",
                        Some(value_expression.span),
                    )
                })?;
                return Ok(Lowered {
                    prefix,
                    value: Some(py::Expr::Attribute {
                        value: Box::new(base),
                        attr: python_identifier(attribute),
                    }),
                });
            }
            ExprKind::Index { value, index } => {
                let value_expression = value;
                let index_expression = index;
                let value = self.lower_value(value_expression)?;
                let mut prefix = value.prefix;
                let base = value.value.ok_or_else(|| {
                    self.error(
                        "index base does not produce a value",
                        Some(value_expression.span),
                    )
                })?;
                let index = self.lower_value(index_expression)?;
                prefix.extend(index.prefix);
                let index = index.value.ok_or_else(|| {
                    self.error(
                        "index does not produce a value",
                        Some(index_expression.span),
                    )
                })?;
                return Ok(Lowered {
                    prefix,
                    value: Some(py::Expr::Subscript {
                        value: Box::new(base),
                        slice: Box::new(index),
                    }),
                });
            }
            ExprKind::Let { .. } | ExprKind::Do(_) | ExprKind::If { .. } | ExprKind::Try { .. } => {
                let temporary = self.fresh_temporary();
                let statements = self.lower_value_block(expression, &temporary)?;
                return Ok(Lowered {
                    prefix: statements,
                    value: Some(py::Expr::name(temporary)),
                });
            }
            ExprKind::Lambda { parameters, body } => return self.lower_lambda(parameters, body),
            ExprKind::Raise(_) => {
                return Ok(Lowered {
                    prefix: self.lower_tail(expression)?,
                    value: None,
                });
            }
            ExprKind::Error => {
                return Err(self.error("cannot generate Python for erroneous HIR", Some(span)));
            }
        };
        Ok(Lowered::value(result))
    }

    fn lower_sequence(
        &mut self,
        items: &[hir::Expr],
        tuple: bool,
    ) -> Result<Lowered, BackendError> {
        let mut prefix = Vec::new();
        let mut values = Vec::new();
        for item in items {
            let lowered = self.lower_value(item)?;
            prefix.extend(lowered.prefix);
            values.push(lowered.value.ok_or_else(|| {
                self.error("collection item does not produce a value", Some(item.span))
            })?);
        }
        Ok(Lowered {
            prefix,
            value: Some(if tuple {
                py::Expr::Tuple(values)
            } else {
                py::Expr::List(values)
            }),
        })
    }

    fn lower_operator(
        &mut self,
        operator: Operator,
        operands: &[hir::Expr],
    ) -> Result<Lowered, BackendError> {
        let mut prefix = Vec::new();
        let mut values = Vec::new();
        for operand in operands {
            let lowered = self.lower_value(operand)?;
            prefix.extend(lowered.prefix);
            values.push(lowered.value.ok_or_else(|| {
                self.error(
                    "operator operand does not produce a value",
                    Some(operand.span),
                )
            })?);
        }
        let value = match operator {
            Operator::And | Operator::Or => {
                if values.len() < 2 {
                    return Ok(Lowered {
                        prefix,
                        value: values.pop(),
                    });
                }
                py::Expr::BoolOp {
                    op: if operator == Operator::And {
                        py::BooleanOp::And
                    } else {
                        py::BooleanOp::Or
                    },
                    values,
                }
            }
            Operator::Not => unary(values, py::UnaryOp::Not, &mut prefix, "not")?,
            Operator::Negate => unary(values, py::UnaryOp::Negative, &mut prefix, "negate")?,
            Operator::Positive => unary(values, py::UnaryOp::Positive, &mut prefix, "positive")?,
            Operator::Equal
            | Operator::NotEqual
            | Operator::Less
            | Operator::LessEqual
            | Operator::Greater
            | Operator::GreaterEqual => {
                if values.len() < 2 {
                    return Err(self.error("comparison needs at least two operands", None));
                }
                let op = match operator {
                    Operator::Equal => py::CompareOp::Equal,
                    Operator::NotEqual => py::CompareOp::NotEqual,
                    Operator::Less => py::CompareOp::Less,
                    Operator::LessEqual => py::CompareOp::LessEqual,
                    Operator::Greater => py::CompareOp::Greater,
                    Operator::GreaterEqual => py::CompareOp::GreaterEqual,
                    _ => unreachable!(),
                };
                let left = values.remove(0);
                py::Expr::Compare {
                    left: Box::new(left),
                    comparisons: values.into_iter().map(|value| (op, value)).collect(),
                }
            }
            _ => {
                if values.is_empty() {
                    return Err(self.error("operator needs an operand", None));
                }
                let op = match operator {
                    Operator::Add => py::BinaryOp::Add,
                    Operator::Subtract => py::BinaryOp::Subtract,
                    Operator::Multiply => py::BinaryOp::Multiply,
                    Operator::Divide => py::BinaryOp::Divide,
                    Operator::FloorDivide => py::BinaryOp::FloorDivide,
                    Operator::Remainder => py::BinaryOp::Modulo,
                    _ => return Err(self.error("unsupported operator", None)),
                };
                let mut result = values.remove(0);
                for right in values {
                    result = py::Expr::BinOp {
                        left: Box::new(result),
                        op,
                        right: Box::new(right),
                    };
                }
                result
            }
        };
        Ok(Lowered {
            prefix,
            value: Some(value),
        })
    }

    fn lower_lambda(
        &mut self,
        parameters: &[hir::Parameter],
        body: &hir::Expr,
    ) -> Result<Lowered, BackendError> {
        let mut py_parameters = py::Parameters::default();
        for parameter in parameters {
            let default = match parameter.default.as_ref() {
                Some(default_expression) => {
                    let lowered = self.lower_value(default_expression)?;
                    if !lowered.prefix.is_empty() {
                        return Err(self.error(
                            "lambda parameter defaults must be expression-only",
                            Some(default_expression.span),
                        ));
                    }
                    Some(lowered.value.ok_or_else(|| {
                        self.error(
                            "lambda parameter default does not produce a value",
                            Some(default_expression.span),
                        )
                    })?)
                }
                None => None,
            };
            let py_parameter = py::Parameter {
                name: self.python_name(&parameter.binding).to_owned(),
                annotation: None,
                default,
            };
            if parameter.variadic {
                py_parameters.vararg = Some(py_parameter);
            } else {
                py_parameters.positional.push(py_parameter);
            }
        }
        let lowered = self.lower_value(body)?;
        if lowered.prefix.is_empty() {
            return Ok(Lowered::value(py::Expr::Lambda {
                parameters: Box::new(py_parameters),
                body: Box::new(lowered.value.ok_or_else(|| {
                    self.error("lambda body does not produce a value", Some(body.span))
                })?),
            }));
        }
        let helper = self.fresh_helper("_osr_lambda");
        let mut helper_body = lowered.prefix;
        helper_body.push(py::Stmt::Return(lowered.value));
        // Keep the helper in the expression prefix instead of a module-level
        // queue.  A complex lambda may close over a function-local binding;
        // emitting its helper at module scope would make that binding
        // unresolved at runtime.  Prefix statements are emitted by each
        // enclosing lowering context, so this preserves the lambda's lexical
        // scope for both direct and nested closures.
        Ok(Lowered {
            prefix: vec![py::Stmt::FunctionDef(Box::new(py::FunctionDef {
                name: helper.clone(),
                parameters: py_parameters,
                returns: None,
                decorators: Vec::new(),
                body: helper_body,
                is_async: false,
            }))],
            value: Some(py::Expr::name(helper)),
        })
    }

    fn lower_value_block(
        &mut self,
        expression: &hir::Expr,
        temporary: &str,
    ) -> Result<Vec<py::Stmt>, BackendError> {
        match &expression.kind {
            ExprKind::Let { bindings, body } => {
                let mut statements = Vec::new();
                for binding in bindings {
                    let lowered = self.lower_value(&binding.value)?;
                    statements.extend(lowered.prefix);
                    let value = lowered.value.ok_or_else(|| {
                        self.error(
                            "let binding does not produce a value",
                            Some(binding.value.span),
                        )
                    })?;
                    statements.push(py::Stmt::Assign(py::Assign {
                        targets: vec![self.binding_target(&binding.binding)?],
                        value,
                    }));
                }
                statements.extend(self.lower_value_block(body, temporary)?);
                Ok(statements)
            }
            ExprKind::Do(expressions) => {
                let mut statements = Vec::new();
                for expression in expressions.iter().take(expressions.len().saturating_sub(1)) {
                    statements.extend(self.lower_discard(expression)?);
                }
                if let Some(last) = expressions.last() {
                    statements.extend(self.lower_value_block(last, temporary)?);
                }
                Ok(statements)
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let condition = self.lower_value(condition)?;
                let condition_value = condition.value.ok_or_else(|| {
                    self.error(
                        "if condition does not produce a value",
                        Some(expression.span),
                    )
                })?;
                let mut statements = condition.prefix;
                statements.push(py::Stmt::If(py::IfStmt {
                    test: condition_value,
                    body: self.lower_value_block(then_branch, temporary)?,
                    orelse: self.lower_value_block(else_branch, temporary)?,
                }));
                Ok(statements)
            }
            ExprKind::Try {
                body,
                catches,
                finally_body,
            } => {
                if catches.is_empty() && finally_body.is_none() {
                    return self.lower_value_block(body, temporary);
                }
                let handlers = catches
                    .iter()
                    .map(|catch| {
                        Ok(py::ExceptHandler {
                            exception_type: catch
                                .exception_type
                                .as_ref()
                                .map(|ty| self.annotation(ty, Some(catch.body.span)))
                                .transpose()?,
                            name: catch
                                .binding
                                .as_ref()
                                .map(|binding| self.python_name(binding).to_owned()),
                            body: self.lower_value_block(&catch.body, temporary)?,
                        })
                    })
                    .collect::<Result<Vec<_>, BackendError>>()?;
                let finalbody = finally_body
                    .as_ref()
                    .map(|body| self.lower_discard(body))
                    .transpose()?
                    .unwrap_or_default();
                Ok(vec![py::Stmt::Try(py::Try {
                    body: self.lower_value_block(body, temporary)?,
                    handlers,
                    orelse: Vec::new(),
                    finalbody,
                })])
            }
            ExprKind::Raise(_) => self.lower_tail(expression),
            _ => {
                let lowered = self.lower_value(expression)?;
                let mut statements = lowered.prefix;
                if let Some(value) = lowered.value {
                    statements.push(py::Stmt::Assign(py::Assign {
                        targets: vec![py::Expr::name(temporary)],
                        value,
                    }));
                }
                Ok(statements)
            }
        }
    }

    fn binding(&self, id: &crate::name::BindingId) -> Result<&'hir hir::Binding, BackendError> {
        self.bindings
            .get(id)
            .copied()
            .ok_or_else(|| self.error("HIR references an unknown binding", None))
    }

    fn binding_expr(&mut self, id: &crate::name::BindingId) -> Result<py::Expr, BackendError> {
        for overrides in self.binding_overrides.iter().rev() {
            if let Some(expression) = overrides.get(id) {
                return Ok(expression.clone());
            }
        }
        self.register_runtime_binding(id);
        Ok(py::Expr::name(self.python_name(id).to_owned()))
    }

    fn binding_target(&self, id: &crate::name::BindingId) -> Result<py::Expr, BackendError> {
        Ok(py::Expr::name(self.python_name(id).to_owned()))
    }

    fn python_name(&self, id: &crate::name::BindingId) -> &str {
        self.names
            .get(id)
            .map(String::as_str)
            .unwrap_or("_osr_unknown")
    }

    fn annotation(&mut self, ty: &Type, span: Option<Span>) -> Result<py::Expr, BackendError> {
        let expression = match ty {
            Type::Bool => py::Expr::name("bool"),
            Type::Int => py::Expr::name("int"),
            Type::Float => py::Expr::name("float"),
            Type::Str => py::Expr::name("str"),
            Type::Bytes => py::Expr::name("bytes"),
            Type::None => py::Expr::name("None"),
            Type::Any => {
                self.typing.insert("Any".to_owned());
                py::Expr::name("Any")
            }
            Type::Never => {
                self.typing.insert(
                    if self.target.at_least(3, 11) {
                        "Never"
                    } else {
                        "NoReturn"
                    }
                    .to_owned(),
                );
                py::Expr::name(if self.target.at_least(3, 11) {
                    "Never"
                } else {
                    "NoReturn"
                })
            }
            Type::Unknown | Type::Error => {
                return Err(self.error(
                    "unresolved type cannot be emitted as a Python annotation",
                    span,
                ));
            }
            Type::Option(inner) => {
                self.typing.insert("Optional".to_owned());
                py::Expr::Subscript {
                    value: Box::new(py::Expr::name("Optional")),
                    slice: Box::new(self.annotation(inner, span)?),
                }
            }
            Type::Union(members) => {
                self.typing.insert("Union".to_owned());
                py::Expr::Subscript {
                    value: Box::new(py::Expr::name("Union")),
                    slice: Box::new(py::Expr::Tuple(
                        members
                            .iter()
                            .map(|member| self.annotation(member, span))
                            .collect::<Result<_, _>>()?,
                    )),
                }
            }
            Type::Tuple(members) => py::Expr::Subscript {
                value: Box::new(py::Expr::name("tuple")),
                slice: Box::new(py::Expr::Tuple(
                    members
                        .iter()
                        .map(|member| self.annotation(member, span))
                        .collect::<Result<_, _>>()?,
                )),
            },
            Type::List(item) => py::Expr::Subscript {
                value: Box::new(py::Expr::name("list")),
                slice: Box::new(self.annotation(item, span)?),
            },
            Type::Vector(item) => py::Expr::Subscript {
                value: Box::new(py::Expr::name("tuple")),
                slice: Box::new(py::Expr::Tuple(vec![
                    self.annotation(item, span)?,
                    py::Expr::Literal(py::Literal::Ellipsis),
                ])),
            },
            Type::Map(key, value) => py::Expr::Subscript {
                value: Box::new(py::Expr::name("dict")),
                slice: Box::new(py::Expr::Tuple(vec![
                    self.annotation(key, span)?,
                    self.annotation(value, span)?,
                ])),
            },
            Type::Set(item) => py::Expr::Subscript {
                value: Box::new(py::Expr::name("set")),
                slice: Box::new(self.annotation(item, span)?),
            },
            Type::Fn(function) => {
                self.typing.insert("Callable".to_owned());
                py::Expr::Subscript {
                    value: Box::new(py::Expr::name("Callable")),
                    slice: Box::new(py::Expr::Tuple(vec![
                        py::Expr::List(
                            function
                                .parameters
                                .iter()
                                .map(|parameter| self.annotation(parameter, span))
                                .collect::<Result<_, _>>()?,
                        ),
                        self.annotation(&function.return_type, span)?,
                    ])),
                }
            }
            Type::Nominal { binding, args } => {
                self.register_runtime_type(binding);
                let name = self.nominal_name(binding);
                if args.is_empty() {
                    name
                } else {
                    py::Expr::Subscript {
                        value: Box::new(name),
                        slice: Box::new(if args.len() == 1 {
                            self.annotation(&args[0], span)?
                        } else {
                            py::Expr::Tuple(
                                args.iter()
                                    .map(|arg| self.annotation(arg, span))
                                    .collect::<Result<_, _>>()?,
                            )
                        }),
                    }
                }
            }
            Type::Literal(value) => {
                self.typing.insert("Literal".to_owned());
                py::Expr::Subscript {
                    value: Box::new(py::Expr::name("Literal")),
                    slice: Box::new(py::Expr::string(value.canonical_text())),
                }
            }
            Type::TypeVar(variable) => {
                self.typing.insert("TypeVar".to_owned());
                let source = self
                    .typevar_names
                    .get(variable)
                    .cloned()
                    .unwrap_or_else(|| format!("_T{}", variable.0));
                let python = self
                    .typevars
                    .entry(source.clone())
                    .or_insert(source)
                    .clone();
                py::Expr::name(python)
            }
        };
        Ok(expression)
    }

    fn nominal_name(&self, binding: &str) -> py::Expr {
        if let Some(name) = python_builtin_exception_from_binding(binding) {
            return py::Expr::name(name);
        }
        if let Some((id, _)) = self.bindings.iter().find(|(id, binding_name)| {
            id.as_str() == binding && binding_name.name.kind == crate::name::BindingKind::Type
        }) {
            return py::Expr::name(self.python_name(id).to_owned());
        }
        let name = nominal_short_name(binding);
        if let Some(mapped) = self.active_type_parameters.get(name) {
            return py::Expr::name(mapped.clone());
        }
        let mut parts = name
            .split('/')
            .flat_map(|part| part.split('.'))
            .map(python_identifier);
        let Some(first) = parts.next() else {
            return py::Expr::name("Any");
        };
        parts.fold(py::Expr::name(first), |value, attr| py::Expr::Attribute {
            value: Box::new(value),
            attr,
        })
    }

    fn fresh_temporary(&mut self) -> String {
        loop {
            let name = format!("_osr_value_{}", self.temporary_counter);
            self.temporary_counter += 1;
            if self.reserved_names.insert(name.clone()) {
                return name;
            }
        }
    }
    fn fresh_helper(&mut self, prefix: &str) -> String {
        loop {
            let name = format!("{}_{}", prefix, self.helper_counter);
            self.helper_counter += 1;
            if self.reserved_names.insert(name.clone()) {
                return name;
            }
        }
    }
    fn error(&self, message: impl Into<String>, span: Option<Span>) -> BackendError {
        BackendError::new(message, span)
    }
}

#[derive(Clone, Debug)]
struct Lowered {
    prefix: Vec<py::Stmt>,
    value: Option<py::Expr>,
}

impl Lowered {
    fn value(value: py::Expr) -> Self {
        Self {
            prefix: Vec::new(),
            value: Some(value),
        }
    }
}

impl Lowered {
    fn with_set(self) -> Result<Self, BackendError> {
        let Some(value) = self.value else {
            return Ok(self);
        };
        let py::Expr::List(items) = value else {
            return Ok(Self {
                prefix: self.prefix,
                value: Some(value),
            });
        };
        Ok(Self {
            prefix: self.prefix,
            value: Some(py::Expr::Set(items)),
        })
    }
}

fn unary(
    mut values: Vec<py::Expr>,
    op: py::UnaryOp,
    _prefix: &mut Vec<py::Stmt>,
    name: &str,
) -> Result<py::Expr, BackendError> {
    if values.len() != 1 {
        return Err(BackendError::new(
            format!("{name} expects one operand"),
            None,
        ));
    }
    Ok(py::Expr::UnaryOp {
        op,
        operand: Box::new(values.remove(0)),
    })
}

fn is_safe_dataclass_default(value: &py::Expr) -> bool {
    match value {
        py::Expr::Literal(
            py::Literal::None
            | py::Literal::Bool(_)
            | py::Literal::Integer(_)
            | py::Literal::Float(_)
            | py::Literal::String(_),
        ) => true,
        py::Expr::Tuple(items) => items.iter().all(is_safe_dataclass_default),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::compile_module;
    use crate::{ast::lower_document, hir::lower_module, reader::read, types::PythonVersion};

    fn compile(source: &str) -> String {
        let document = read(source);
        let ast = lower_document(&document);
        let result = lower_module(&ast.module, "example");
        assert!(
            document.diagnostics.is_empty(),
            "{:?}",
            document.diagnostics
        );
        assert!(ast.diagnostics.is_empty(), "{:?}", ast.diagnostics);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        compile_module(&result.module, PythonVersion::PYTHON_3_9)
            .expect("backend should compile")
            .source
    }

    #[test]
    fn emits_readable_typed_function_and_value() {
        let source =
            compile("(defn square [[x Float]] -> Float (* x x)) (def answer Float (square 3.0))");
        assert!(
            source.contains("def square(x: float) -> float:"),
            "{source}"
        );
        assert!(source.contains("return x * x"), "{source}");
        assert!(source.contains("answer: float = square(3.0)"), "{source}");
    }

    #[test]
    fn lowers_control_flow_and_structured_collections() {
        let source = compile("(defn choose [[x Int]] -> Int (let [y (+ x 1)] (if (> y 0) y 0)))");
        assert!(source.contains("y = x + 1"), "{source}");
        assert!(source.contains("if y > 0:"), "{source}");
        assert!(source.contains("return y"), "{source}");
    }

    #[test]
    fn lowers_nested_runtime_destructuring_to_readable_assignments() {
        let source = compile(
            r#"(defn total [[entry (Map Str Int)]] -> Int
                 (let [{:keys [left right] :or {right 5} :as whole} entry
                       [first second] [left right]]
                   (+ first second)))"#,
        );
        assert!(source.contains("[\"left\"]"), "{source}");
        assert!(source.contains(".get(\"right\", 5)"), "{source}");
        assert!(source.contains("whole ="), "{source}");
        assert!(source.contains("first ="), "{source}");
        assert!(source.contains("second ="), "{source}");
        assert!(source.contains("return first + second"), "{source}");

        let structure = compile(
            r#"(defstruct Point [x Int] [y Int])
               (defn point-total [[point Point]] -> Int
                 (let [{:keys [x y]} point] (+ x y)))"#,
        );
        assert!(structure.contains(".x"), "{structure}");
        assert!(structure.contains(".y"), "{structure}");

        let parameters = compile(
            r#"(defn entry-total [[{:keys [left right]} (Map Str Int)]] -> Int
                 (+ left right))
               (defn pair-total [[[left right] (Vector Int)]] -> Int
                 (+ left right))"#,
        );
        assert!(
            parameters.contains("def entry_total(_u0_arg0: dict[str, int]) -> int:"),
            "{parameters}"
        );
        assert!(
            parameters.contains("def pair_total(_u0_arg1: tuple[int, ...]) -> int:"),
            "{parameters}"
        );
        assert!(parameters.contains("[\"left\"]"), "{parameters}");
        assert!(parameters.contains("[0]"), "{parameters}");
    }

    #[test]
    fn emits_frozen_struct_with_invariant_and_factory_default() {
        let source = compile(
            "(defstruct Point [x Int] [child Any = (+ 1 2)] (check (> x 0) \"x must be positive\"))\n             (def point Point (Point :x 1))",
        );
        assert!(
            source.contains("from dataclasses import dataclass, field"),
            "{source}"
        );
        assert!(source.contains("@dataclass(frozen=True)"), "{source}");
        assert!(source.contains("class Point:"), "{source}");
        assert!(source.contains("default_factory=lambda"), "{source}");
        assert!(
            source.contains("def __post_init__(self) -> None:"),
            "{source}"
        );
        assert!(source.contains("x must be positive"), "{source}");
    }

    #[test]
    fn maps_struct_type_variables_to_python_generic_parameters() {
        let source = compile("(defstruct (Box T) [value T])");
        assert!(source.contains("T = TypeVar(\"T\")"), "{source}");
        assert!(
            source.contains("from typing import Generic, TypeVar"),
            "{source}"
        );
        assert!(source.contains("class Box(Generic[T]):"), "{source}");
        assert!(source.contains("value: T"), "{source}");
        let output = Command::new("python3")
            .arg("-c")
            .arg(&source)
            .output()
            .expect("python3 should execute generated generic struct");
        assert!(
            output.status.success(),
            "generated generic struct failed: {}\n{}",
            String::from_utf8_lossy(&output.stderr),
            source
        );
    }

    #[test]
    fn emits_parseable_literal_annotations_for_axes_and_frame_schema() {
        let source = compile(
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
        assert!(
            source.lines().any(|line| {
                line.strip_prefix("from typing import ")
                    .is_some_and(|names| names.split(", ").any(|name| name == "Literal"))
            }),
            "{source}"
        );
        assert!(source.contains("Literal[\"[:time :feature]\"]"), "{source}");
        assert!(
            source.contains("Literal[\"{:category Str :time Datetime :value Float}\"]"),
            "{source}"
        );
        let output = Command::new("python3")
            .arg("-c")
            .arg(&source)
            .output()
            .expect("python3 should parse generated literal annotations");
        assert!(
            output.status.success(),
            "generated Python failed: {}\n{}",
            String::from_utf8_lossy(&output.stderr),
            source
        );
    }

    #[test]
    fn keeps_complex_lambda_helpers_in_their_closure_scope() {
        let source = compile(
            "(defn make [[base Int]] -> Any\n \
                 (fn [[x Int]] (let [y (+ base x)] y)))\n \
             (def result Any ((make 2) 3))",
        );
        // The helper must be nested under `make`; placing it after the module
        // definition would leave `base` unresolved when the callback runs.
        assert!(
            source.contains("def make(base: int) -> Any:\n    def _osr_lambda_"),
            "{source}"
        );
        let script = format!("{source}\nprint(result)\n");
        let output = Command::new("python3")
            .arg("-c")
            .arg(script)
            .output()
            .expect("python3 should execute generated closure");
        assert!(
            output.status.success(),
            "generated Python failed: {}\n{}",
            String::from_utf8_lossy(&output.stderr),
            source
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "5");
    }
}
