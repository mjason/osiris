use super::record::*;
use super::schema::*;
use super::*;

#[derive(Clone, Debug, Default)]
pub struct StaticModuleData {
    pub schemas: Vec<StaticSchema>,
    pub records: Vec<ValidatedRecord>,
    pub diagnostics: Vec<Diagnostic>,
}

impl StaticModuleData {
    #[must_use]
    pub fn public_records(&self) -> Vec<&ValidatedRecord> {
        self.records.iter().filter(|record| record.public).collect()
    }
}

#[derive(Clone, Debug)]
struct ImportedSchema<'a> {
    module: String,
    schema: &'a StaticSchema,
    binding_id: String,
}

#[derive(Clone, Debug)]
enum ImportedName<'a> {
    Schema(ImportedSchema<'a>),
    /// A public binding which is not a static schema.  Keeping this state in
    /// the lookup table lets a later `static-record` report a wrong-kind
    /// reference instead of silently treating it as unresolved/local.
    NonSchema,
    /// A malformed/private schema or a missing alias target.  Interfaces read
    /// from `.osri` should already reject these, but direct API callers still
    /// need fail-closed behavior.
    Invalid,
    /// Two imports assign different identities to the same local spelling.
    Conflict,
}

#[derive(Clone, Debug, Default)]
struct ImportedSchemaScope<'a> {
    qualified: BTreeMap<String, ImportedName<'a>>,
    referred: BTreeMap<String, ImportedName<'a>>,
    conflicting_qualifiers: BTreeSet<String>,
}

#[derive(Clone, Debug)]
enum ResolvedSchema {
    Local(StaticSchema),
    Imported {
        schema: StaticSchema,
        binding_id: String,
    },
    Missing,
    NonSchema,
    Invalid,
    Conflict,
}

fn insert_imported_name<'a>(
    names: &mut BTreeMap<String, ImportedName<'a>>,
    key: impl Into<String>,
    value: ImportedName<'a>,
) {
    let key = key.into();
    let Some(existing) = names.get_mut(&key) else {
        names.insert(key, value);
        return;
    };
    if matches!(existing, ImportedName::Conflict) {
        return;
    }
    let same_schema = matches!(
        (&*existing, &value),
        (ImportedName::Schema(left), ImportedName::Schema(right))
            if left.module == right.module && left.binding_id == right.binding_id
    );
    if !same_schema {
        *existing = ImportedName::Conflict;
    }
}

fn public_schema_names<'a>(
    module_name: &str,
    interface: &'a crate::interface::Interface,
) -> BTreeMap<String, ImportedName<'a>> {
    let mut by_binding = BTreeMap::<String, ImportedName<'a>>::new();
    let mut schema_names = BTreeMap::<String, ImportedName<'a>>::new();

    for schema in &interface.static_schemas {
        let binding_id = BindingId::new(module_name, &schema.name, BindingKind::Type)
            .as_str()
            .to_owned();
        let valid_binding = interface.bindings.iter().any(|binding| {
            binding.id == binding_id
                && binding.kind == BindingKind::Type
                && binding.canonical == schema.name
        });
        let valid_schema = valid_binding && schema.verify_integrity().is_ok();
        let entry = if valid_schema {
            ImportedName::Schema(ImportedSchema {
                module: module_name.to_owned(),
                schema,
                binding_id: binding_id.clone(),
            })
        } else {
            ImportedName::Invalid
        };
        insert_imported_name(&mut by_binding, binding_id.clone(), entry);
        insert_imported_name(
            &mut schema_names,
            schema.name.clone(),
            by_binding[&binding_id].clone(),
        );
    }

    // A public type binding without a matching static schema is deliberately
    // marked invalid: accepting it as a record schema would make the result
    // depend on an interface producer's omitted payload.
    for binding in &interface.bindings {
        if binding.kind != BindingKind::Type {
            insert_imported_name(
                &mut schema_names,
                binding.canonical.clone(),
                ImportedName::NonSchema,
            );
            continue;
        }
        let entry = by_binding
            .get(&binding.id)
            .cloned()
            .unwrap_or(ImportedName::Invalid);
        insert_imported_name(&mut schema_names, binding.canonical.clone(), entry);
    }

    // Public aliases are resolved by their stable target binding id.  An alias
    // to a function/value remains a non-schema entry so `static-record` can
    // issue a precise wrong-kind diagnostic.
    for alias in &interface.aliases {
        let entry = by_binding
            .get(&alias.target)
            .cloned()
            .unwrap_or(ImportedName::Invalid);
        insert_imported_name(&mut schema_names, alias.canonical.clone(), entry.clone());
        insert_imported_name(&mut schema_names, alias.spelling.clone(), entry);
    }

    schema_names
}

fn imported_schema_scope<'a>(
    module: &Module,
    interfaces: &'a BTreeMap<String, crate::interface::Interface>,
) -> ImportedSchemaScope<'a> {
    let mut scope = ImportedSchemaScope::default();
    let mut qualifier_modules = BTreeMap::<String, String>::new();
    for item in &module.items {
        let ItemKind::Import(import) = &item.kind else {
            continue;
        };
        let module_name = import.module.canonical.as_str();
        let Some(interface) = interfaces.get(module_name) else {
            // Missing interfaces are reported when a static-record actually
            // names one of their members; ordinary runtime imports remain the
            // responsibility of the module/HIR dependency checker.
            continue;
        };
        if interface.module != module_name {
            continue;
        }
        let names = public_schema_names(module_name, interface);
        let base = import
            .alias
            .as_ref()
            .map_or(module_name, |alias| alias.canonical.as_str());
        for qualifier in [base, module_name] {
            if let Some(previous) =
                qualifier_modules.insert(qualifier.to_owned(), module_name.to_owned())
                && previous != module_name
            {
                scope.conflicting_qualifiers.insert(qualifier.to_owned());
            }
            for (name, entry) in &names {
                insert_imported_name(
                    &mut scope.qualified,
                    format!("{qualifier}/{name}"),
                    entry.clone(),
                );
                insert_imported_name(
                    &mut scope.qualified,
                    format!("{qualifier}.{name}"),
                    entry.clone(),
                );
            }
        }
        for member in &import.members {
            let entry = names
                .get(&member.canonical)
                .or_else(|| names.get(&member.spelling))
                .cloned()
                .unwrap_or(ImportedName::Invalid);
            insert_imported_name(&mut scope.referred, member.canonical.clone(), entry.clone());
            insert_imported_name(&mut scope.referred, member.spelling.clone(), entry);
        }
    }
    scope
}

fn resolve_imported_name<'scope, 'interface>(
    scope: &'scope ImportedSchemaScope<'interface>,
    name: &str,
) -> Option<&'scope ImportedName<'interface>> {
    if name.contains('/') || name.contains('.') {
        scope.qualified.get(name)
    } else {
        scope.referred.get(name)
    }
}

fn resolve_schema(
    name: &str,
    local: &BTreeMap<String, StaticSchema>,
    scope: &ImportedSchemaScope<'_>,
) -> ResolvedSchema {
    if (name.contains('/') || name.contains('.'))
        && name
            .split_once('/')
            .or_else(|| name.split_once('.'))
            .is_some_and(|(base, _)| scope.conflicting_qualifiers.contains(base))
    {
        return ResolvedSchema::Conflict;
    }
    let imported = resolve_imported_name(scope, name);
    if let Some(schema) = local.get(name) {
        // A local declaration and an imported `:refer`/qualified entry with
        // the same spelling are ambiguous, even though the local map could
        // technically win by insertion order.
        if imported.is_some() {
            return ResolvedSchema::Conflict;
        }
        return ResolvedSchema::Local(schema.clone());
    }
    let Some(imported) = imported else {
        return ResolvedSchema::Missing;
    };
    match imported {
        ImportedName::Schema(schema) => ResolvedSchema::Imported {
            schema: schema.schema.clone(),
            binding_id: schema.binding_id.clone(),
        },
        ImportedName::NonSchema => ResolvedSchema::NonSchema,
        ImportedName::Invalid => ResolvedSchema::Invalid,
        ImportedName::Conflict => ResolvedSchema::Conflict,
    }
}

/// Parse and validate the static declarations that can be resolved inside one
/// module.  Imported schemas can be supplied later through
/// [`validate_record_with_schema_binding`]; unresolved qualified names are
/// reported rather than guessed.
pub fn analyze_module(module: &Module) -> StaticModuleData {
    let interfaces: BTreeMap<String, crate::interface::Interface> = BTreeMap::new();
    analyze_module_with_interfaces(module, &interfaces)
}

/// Parse and validate static declarations with an explicit, read-only map of
/// imported compilation interfaces.  Only public type bindings which have a
/// matching public static schema are made available to a record; this keeps
/// static validation independent from the Python runtime and fails closed on
/// missing, private, malformed, or ambiguous imports.
pub fn analyze_module_with_interfaces(
    module: &Module,
    interfaces: &BTreeMap<String, crate::interface::Interface>,
) -> StaticModuleData {
    let module_name = module
        .name
        .as_ref()
        .map_or_else(|| "<anonymous>".to_owned(), |name| name.canonical.clone());
    let mut result = StaticModuleData::default();
    let mut schemas = BTreeMap::<String, StaticSchema>::new();
    for item in &module.items {
        if let ItemKind::DefstaticSchema(declaration) = &item.kind {
            match parse_schema(declaration) {
                Ok(schema) => {
                    if schemas
                        .insert(declaration.name.canonical.clone(), schema.clone())
                        .is_some()
                    {
                        result.diagnostics.push(Diagnostic::error(
                            RECORD_SCHEMA_SHAPE,
                            format!("duplicate schema `{}`", declaration.name.canonical),
                            declaration.span,
                        ));
                    } else {
                        result.schemas.push(schema);
                    }
                }
                Err(diagnostics) => result.diagnostics.extend(diagnostics),
            }
        }
    }

    let mut declarations = BTreeMap::<String, BindingKind>::new();
    let mut aliases = BTreeMap::<String, String>::new();
    let mut exports = BTreeSet::new();
    for item in &module.items {
        match &item.kind {
            ItemKind::Def(declaration) => {
                declarations.insert(declaration.name.canonical.clone(), BindingKind::Value);
            }
            ItemKind::Defn(function) => {
                if let Some(name) = &function.name {
                    declarations.insert(name.canonical.clone(), BindingKind::Function);
                }
            }
            ItemKind::Defstruct(declaration) => {
                declarations.insert(declaration.name.canonical.clone(), BindingKind::Type);
            }
            ItemKind::Extern(external) => {
                for nested in &external.items {
                    match &nested.kind {
                        ItemKind::Def(declaration) => {
                            declarations
                                .insert(declaration.name.canonical.clone(), BindingKind::Value);
                        }
                        ItemKind::Defn(function) => {
                            if let Some(name) = &function.name {
                                declarations.insert(name.canonical.clone(), BindingKind::Function);
                            }
                        }
                        _ => {}
                    }
                }
            }
            ItemKind::Alias(alias) => {
                aliases.insert(
                    alias.local.canonical.clone(),
                    alias.target.canonical.clone(),
                );
            }
            ItemKind::Export(export) => {
                exports.extend(export.names.iter().map(|name| name.canonical.clone()));
            }
            _ => {}
        }
    }
    for schema in &result.schemas {
        declarations.insert(schema.name.clone(), BindingKind::Type);
    }

    let resolve_alias = |name: &str, aliases: &BTreeMap<String, String>| {
        let mut current = name.to_owned();
        let mut visited = BTreeSet::new();
        while let Some(target) = aliases.get(&current) {
            if !visited.insert(current.clone()) {
                break;
            }
            current = target.clone();
        }
        current
    };
    let is_exported = |name: &str| {
        let canonical = resolve_alias(name, &aliases);
        exports.contains(name) || exports.contains(&canonical)
    };
    let imported_scope = imported_schema_scope(module, interfaces);

    let mut seen_owner_schema = BTreeSet::new();
    for item in &module.items {
        let ItemKind::StaticRecord(record) = &item.kind else {
            continue;
        };
        let (schema, schema_binding, schema_is_public) = match resolve_schema(
            &record.schema.canonical,
            &schemas,
            &imported_scope,
        ) {
            ResolvedSchema::Local(schema) => {
                let binding = BindingId::new(&module_name, &schema.name, BindingKind::Type)
                    .as_str()
                    .to_owned();
                let public = is_exported(&schema.name);
                (schema, binding, public)
            }
            ResolvedSchema::Imported { schema, binding_id } => (schema, binding_id, true),
            ResolvedSchema::Missing => {
                result.diagnostics.push(Diagnostic::error(
                    RECORD_RECORD_SHAPE,
                    format!(
                        "static-record references unresolved schema `{}`; schema must be local or imported from a public interface",
                        record.schema.canonical
                    ),
                    record.span,
                ));
                continue;
            }
            ResolvedSchema::NonSchema => {
                result.diagnostics.push(Diagnostic::error(
                    RECORD_RECORD_SHAPE,
                    format!(
                        "static-record schema `{}` resolves to a non-schema export",
                        record.schema.canonical
                    ),
                    record.span,
                ));
                continue;
            }
            ResolvedSchema::Invalid => {
                result.diagnostics.push(Diagnostic::error(
                    RECORD_RECORD_SHAPE,
                    format!(
                        "static-record schema `{}` references a missing, private, or invalid imported schema",
                        record.schema.canonical
                    ),
                    record.span,
                ));
                continue;
            }
            ResolvedSchema::Conflict => {
                result.diagnostics.push(Diagnostic::error(
                    RECORD_RECORD_SHAPE,
                    format!(
                        "static-record schema `{}` has conflicting local or imported bindings",
                        record.schema.canonical
                    ),
                    record.span,
                ));
                continue;
            }
        };
        let owner_name = resolve_alias(&record.owner.canonical, &aliases);
        let Some(owner_kind) = declarations.get(&owner_name).copied() else {
            result.diagnostics.push(Diagnostic::error(
                RECORD_RECORD_SHAPE,
                format!(
                    "static-record owner `{}` is not a top-level declaration",
                    record.owner.canonical
                ),
                record.span,
            ));
            continue;
        };
        let owner_binding = BindingId::new(&module_name, &owner_name, owner_kind)
            .as_str()
            .to_owned();
        let public = is_exported(&owner_name);
        if public && !schema_is_public {
            result.diagnostics.push(Diagnostic::error(
                RECORD_RECORD_SHAPE,
                format!(
                    "public record owner requires exported schema `{}`",
                    schema.name
                ),
                record.span,
            ));
        }
        let pair = (
            schema.schema_id.clone(),
            schema.version,
            owner_binding.clone(),
        );
        if !seen_owner_schema.insert(pair) {
            result.diagnostics.push(Diagnostic::error(
                RECORD_RECORD_SHAPE,
                format!(
                    "owner `{}` has more than one record for schema `{}`",
                    owner_name, schema.schema_id
                ),
                record.span,
            ));
            continue;
        }
        match validate_record_with_schema_binding(
            &schema,
            record,
            schema_binding,
            owner_binding,
            public,
            module_name.clone(),
        ) {
            Ok(record) => result.records.push(record),
            Err(diagnostics) => result.diagnostics.extend(diagnostics),
        }
    }
    result
        .schemas
        .sort_by(|left, right| left.name.cmp(&right.name));
    result
        .records
        .sort_by(|left, right| left.stable_record_id.cmp(&right.stable_record_id));
    result
}
