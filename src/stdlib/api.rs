use std::collections::BTreeMap;

use oxilangtag::LanguageTag;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    name::BindingKind,
    semantic::SemanticDocumentation,
    syntax::{FormKind, MetadataEntry},
    types::{Type, TypeVarId},
};

use super::{NAMESPACES, StandardBinding, exports};

const API_SCHEMA: &str = "osiris.standard-api/v1";

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StandardSourceLocation {
    pub uri: String,
    pub line: u32,
    pub column: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StandardApiRecord {
    pub schema: &'static str,
    pub binding_id: String,
    pub namespace: &'static str,
    pub canonical: &'static str,
    pub kind: BindingKind,
    pub call_shapes: Vec<String>,
    pub signature: String,
    pub evaluation: &'static str,
    pub effects: serde_json::Value,
    pub exceptions: Vec<&'static str>,
    pub since: String,
    pub deprecated: bool,
    pub documentation: SemanticDocumentation,
    pub source: StandardSourceLocation,
    pub semantic_hash: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StandardApiSelection {
    #[serde(flatten)]
    pub api: StandardApiRecord,
    pub requested_locale: Option<String>,
    pub resolved_locale: Option<String>,
    pub label: String,
    pub selected_documentation: String,
    pub available_locales: Vec<String>,
    pub provenance: &'static str,
}

#[must_use]
pub fn api_catalog() -> Vec<StandardApiRecord> {
    NAMESPACES
        .iter()
        .flat_map(|namespace| exports(namespace))
        .map(api_record)
        .collect()
}

#[must_use]
pub fn api_record(binding: StandardBinding) -> StandardApiRecord {
    let call_shapes = call_shapes(binding);
    let interface = super::interface_artifact(binding.namespace).ok();
    let public = interface.as_ref().and_then(|interface| {
        interface
            .bindings
            .iter()
            .find(|public| public.id == binding.id().as_str())
    });
    let function = interface.as_ref().and_then(|interface| {
        interface
            .functions
            .iter()
            .find(|function| function.binding == binding.id().as_str())
    });
    let signature_text = public
        .and_then(|public| match &public.ty {
            Type::Fn(signature) => Some(TypeDisplay::function(
                signature,
                &generic_names(&public.metadata, signature),
            )),
            _ => None,
        })
        .unwrap_or_else(|| call_shapes.join(" | "));
    let effects = function.map_or_else(
        || serde_json::json!({"derivedFromExpansion": true}),
        |function| {
            serde_json::to_value(&function.summaries)
                .unwrap_or_else(|_| serde_json::json!({"unknown": true}))
        },
    );
    let documentation = documentation(binding);
    let source = source_location(binding);
    let evaluation = evaluation(binding);
    let exceptions = exceptions(binding);
    let mut hasher = Sha256::new();
    hasher.update(binding.id().as_str().as_bytes());
    hasher.update(signature_text.as_bytes());
    hasher.update(evaluation.as_bytes());
    for shape in &call_shapes {
        hasher.update(shape.as_bytes());
        hasher.update([0]);
    }
    for exception in &exceptions {
        hasher.update(exception.as_bytes());
        hasher.update([0]);
    }
    StandardApiRecord {
        schema: API_SCHEMA,
        binding_id: binding.id().as_str().to_owned(),
        namespace: binding.namespace,
        canonical: binding.canonical,
        kind: binding.kind,
        call_shapes,
        signature: signature_text,
        evaluation,
        effects,
        exceptions,
        since: public
            .and_then(|public| metadata_string(&public.metadata, "since"))
            .unwrap_or_default()
            .to_owned(),
        deprecated: public
            .and_then(|public| metadata_bool(&public.metadata, "deprecated"))
            .unwrap_or(false),
        documentation,
        source,
        semantic_hash: format!("sha256:{:x}", hasher.finalize()),
    }
}

#[must_use]
pub fn query_api(query: &str, locale: Option<&str>) -> Vec<StandardApiSelection> {
    let mut records = api_catalog()
        .into_iter()
        .filter(|record| {
            record.binding_id == query
                || record.canonical == query
                || format!("{}/{}", record.namespace, record.canonical) == query
                || format!("{}.{}", record.namespace, record.canonical) == query
        })
        .map(|api| select_locale(api, locale))
        .collect::<Vec<_>>();
    records.sort_by(|left, right| left.api.binding_id.cmp(&right.api.binding_id));
    records
}

fn select_locale(api: StandardApiRecord, requested: Option<&str>) -> StandardApiSelection {
    let requested_locale = requested.and_then(normalize_locale);
    let resolved_locale = requested_locale
        .as_deref()
        .and_then(|locale| lookup_locale(&api.documentation.translations, locale));
    let selected_documentation = resolved_locale
        .as_ref()
        .and_then(|locale| api.documentation.translations.get(locale))
        .cloned()
        .unwrap_or_else(|| api.documentation.default.clone().unwrap_or_default());
    let available_locales = api.documentation.translations.keys().cloned().collect();
    let label = api.canonical.to_owned();
    StandardApiSelection {
        api,
        requested_locale,
        resolved_locale,
        label,
        selected_documentation,
        available_locales,
        provenance: "compiler-embedded-standard-interface",
    }
}

fn normalize_locale(locale: &str) -> Option<String> {
    LanguageTag::parse_and_normalize(locale)
        .ok()
        .map(|tag| tag.to_string())
}

fn lookup_locale(values: &BTreeMap<String, String>, requested: &str) -> Option<String> {
    let mut candidate = requested.to_owned();
    loop {
        if values.contains_key(&candidate) {
            return Some(candidate);
        }
        let (parent, _) = candidate.rsplit_once('-')?;
        candidate.truncate(parent.len());
    }
}

fn documentation(binding: StandardBinding) -> SemanticDocumentation {
    super::artifacts::binding_metadata(binding)
        .map(|metadata| crate::semantic::documentation(&metadata))
        .unwrap_or_default()
}

fn source_location(binding: StandardBinding) -> StandardSourceLocation {
    super::artifacts::binding_source_location(binding)
}

fn evaluation(binding: StandardBinding) -> &'static str {
    if binding.kind == BindingKind::Macro {
        return "macro";
    }
    if matches!(
        binding.canonical,
        "map"
            | "mapcat"
            | "filter"
            | "remove"
            | "range"
            | "repeat"
            | "repeatedly"
            | "iterate"
            | "cycle"
            | "sequence"
            | "take"
            | "drop"
            | "take-while"
            | "drop-while"
            | "take-last"
            | "drop-last"
            | "partition"
            | "partition-all"
            | "partition-by"
            | "interleave"
            | "interpose"
            | "distinct"
            | "dedupe"
            | "flatten"
            | "keep"
            | "keep-indexed"
            | "map-indexed"
            | "concat"
            | "cons"
            | "reductions"
    ) {
        return "lazy";
    }
    if matches!(
        binding.canonical,
        "reduce"
            | "fold"
            | "count"
            | "run!"
            | "doall"
            | "dorun"
            | "some"
            | "every?"
            | "not-every?"
            | "not-any?"
            | "group-by"
            | "frequencies"
    ) {
        return "consumer";
    }
    "eager"
}

fn exceptions(binding: StandardBinding) -> Vec<&'static str> {
    if binding.kind == BindingKind::Macro {
        return vec!["compile-time syntax or expansion diagnostic"];
    }
    match binding.canonical {
        "index-by" | "rename-keys" | "update-keys" | "invert" => {
            vec![
                "ValueError on duplicate logical output keys",
                "callback exceptions propagate",
            ]
        }
        "nth" => vec![
            "IndexError without a not-found value",
            "TypeError at dynamic boundaries",
        ],
        "split" => vec!["ValueError for an empty separator or invalid limit"],
        "deref" => vec!["TimeoutError or the deferred computation exception"],
        "future-call" | "pmap" => vec!["task and callback exceptions propagate on dereference"],
        _ => vec!["typed callback and Python boundary exceptions propagate"],
    }
}

fn call_shapes(binding: StandardBinding) -> Vec<String> {
    if binding.kind == BindingKind::Macro {
        return macro_shapes(binding.canonical)
            .iter()
            .map(ToString::to_string)
            .collect();
    }
    if matches!(binding.kind, BindingKind::Value | BindingKind::Type) {
        return vec![binding.canonical.to_owned()];
    }
    if let Some(shapes) = source_dispatched_call_shapes(binding.canonical) {
        return shapes.iter().map(ToString::to_string).collect();
    }
    let Ok(interface) = super::interface_artifact(binding.namespace) else {
        return vec![format!("({} ...)", binding.canonical)];
    };
    let Some(function) = interface
        .functions
        .iter()
        .find(|function| function.binding == binding.id().as_str())
    else {
        return vec![format!("({} ...)", binding.canonical)];
    };
    let mut parts = vec![binding.canonical.to_owned()];
    for parameter in &function.parameters {
        let name = if parameter.variadic {
            format!("{}...", parameter.canonical)
        } else if !parameter.has_default {
            parameter.canonical.clone()
        } else {
            format!("[{}]", parameter.canonical)
        };
        parts.push(name);
    }
    vec![format!("({})", parts.join(" "))]
}

fn source_dispatched_call_shapes(canonical: &str) -> Option<&'static [&'static str]> {
    match canonical {
        "nth" => Some(&["(nth collection index)", "(nth collection index not-found)"]),
        "range" => Some(&["(range end)", "(range start end)", "(range start end step)"]),
        "repeat" => Some(&["(repeat value)", "(repeat count value)"]),
        "repeatedly" => Some(&["(repeatedly function)", "(repeatedly count function)"]),
        "drop-last" => Some(&["(drop-last collection)", "(drop-last count collection)"]),
        "partition" => Some(&[
            "(partition size collection)",
            "(partition size step collection)",
            "(partition size step padding collection)",
        ]),
        "partition-all" => Some(&[
            "(partition-all size collection)",
            "(partition-all size step collection)",
        ]),
        "reduce" => Some(&[
            "(reduce function collection)",
            "(reduce function initial collection)",
        ]),
        "reductions" => Some(&[
            "(reductions function collection)",
            "(reductions function initial collection)",
        ]),
        "doall" => Some(&["(doall collection)", "(doall count collection)"]),
        "dorun" => Some(&["(dorun collection)", "(dorun count collection)"]),
        _ => None,
    }
}

fn macro_shapes(name: &str) -> &'static [&'static str] {
    match name {
        "and" => &["(and form...)"],
        "or" => &["(or form...)"],
        "when" => &["(when test body...)"],
        "when-not" => &["(when-not test body...)"],
        "if-not" => &["(if-not test then [else])"],
        "cond" => &["(cond test result ... [:else result])"],
        "case" => &["(case value test result ... default)"],
        "condp" => &["(condp predicate value test result ... :else result)"],
        "if-let" => &["(if-let [pattern value] then [else])"],
        "if-some" => &["(if-some [pattern value] then [else])"],
        "when-let" => &["(when-let [pattern value] body...)"],
        "when-some" => &["(when-some [pattern value] body...)"],
        "when-first" => &["(when-first [pattern collection] body...)"],
        "->" => &["(-> value step...)"],
        "->>" => &["(->> value step...)"],
        "some->" => &["(some-> value step...)"],
        "some->>" => &["(some->> value step...)"],
        "cond->" => &["(cond-> value test step ...)"],
        "cond->>" => &["(cond->> value test step ...)"],
        "as->" => &["(as-> value name form...)"],
        "doto" => &["(doto value call...)"],
        "defn-" => &["(defn- name parameters body...)"],
        "letfn" => &["(letfn [(name parameters body...) ...] body...)"],
        "loop" => &["(loop [pattern initial ...] body...)"],
        "recur" => &["(recur value...)"],
        "for" => &["(for [clauses...] body...)"],
        "forv" => &["(forv [clauses...] body...)"],
        "doseq" => &["(doseq [clauses...] body...)"],
        "dotimes" => &["(dotimes [name count] body...)"],
        "while" => &["(while test body...)"],
        "trampoline" => &["(trampoline function argument...)"],
        "lazy-seq" => &["(lazy-seq body...)"],
        "lazy-cat" => &["(lazy-cat collection...)"],
        "delay" => &["(delay body...)"],
        "force" => &["(force value)"],
        "realized?" => &["(realized? value)"],
        "deref" => &["(deref value [timeout-ms timeout-value])"],
        "binding" => &["(binding [dynamic-var value ...] body...)"],
        "with-open" => &["(with-open [name resource ...] body...)"],
        "assert" => &["(assert test [message])"],
        "throw" => &["(throw value)"],
        "comment" => &["(comment form...)"],
        "time" => &["(time body...)"],
        "future" => &["(future body...)"],
        "pvalues" => &["(pvalues form...)"],
        "pcalls" => &["(pcalls function...)"],
        "locking" => &["(locking lock body...)"],
        _ => &["(macro form...)"],
    }
}

struct TypeDisplay;

impl TypeDisplay {
    fn function(
        function: &crate::types::FunctionType,
        generic_names: &[(String, TypeVarId)],
    ) -> String {
        let mut display = crate::types::Type::Fn(function.clone()).to_string();
        for (name, variable) in generic_names {
            display = display.replace(&format!("?{}", variable.0), name);
        }
        display
    }
}

fn metadata_value<'a>(metadata: &'a [MetadataEntry], key: &str) -> Option<&'a FormKind> {
    metadata.iter().find_map(|entry| {
        let FormKind::Keyword(name) = &entry.key.kind else {
            return None;
        };
        (name.canonical.trim_start_matches(':') == key).then_some(&entry.value.kind)
    })
}

fn metadata_string<'a>(metadata: &'a [MetadataEntry], key: &str) -> Option<&'a str> {
    match metadata_value(metadata, key)? {
        FormKind::String(value) => Some(value),
        _ => None,
    }
}

fn metadata_bool(metadata: &[MetadataEntry], key: &str) -> Option<bool> {
    match metadata_value(metadata, key)? {
        FormKind::Bool(value) => Some(*value),
        _ => None,
    }
}

fn generic_names(
    metadata: &[MetadataEntry],
    signature: &crate::types::FunctionType,
) -> Vec<(String, TypeVarId)> {
    let names = match metadata_value(metadata, "type-params") {
        Some(FormKind::Vector(values)) => values
            .iter()
            .filter_map(|value| match &value.kind {
                FormKind::Symbol(name) => Some(name.canonical.clone()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    let mut variables = BTreeMap::<u32, TypeVarId>::new();
    for parameter in &signature.parameters {
        collect_type_variables(parameter, &mut variables);
    }
    collect_type_variables(&signature.return_type, &mut variables);
    names.into_iter().zip(variables.into_values()).collect()
}

fn collect_type_variables(ty: &Type, variables: &mut BTreeMap<u32, TypeVarId>) {
    match ty {
        Type::TypeVar(variable) => {
            variables.insert(variable.0, *variable);
        }
        Type::Option(inner) | Type::List(inner) | Type::Vector(inner) | Type::Set(inner) => {
            collect_type_variables(inner, variables);
        }
        Type::Map(key, value) => {
            collect_type_variables(key, variables);
            collect_type_variables(value, variables);
        }
        Type::Union(items) | Type::Tuple(items) => {
            for item in items {
                collect_type_variables(item, variables);
            }
        }
        Type::Fn(function) => {
            for parameter in &function.parameters {
                collect_type_variables(parameter, variables);
            }
            collect_type_variables(&function.return_type, variables);
        }
        Type::Nominal { args, .. } => {
            for argument in args {
                collect_type_variables(argument, variables);
            }
        }
        _ => {}
    }
}
