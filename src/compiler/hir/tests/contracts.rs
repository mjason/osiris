use std::collections::BTreeMap;

use crate::{
    ast::lower_document,
    interface,
    reader::read,
    types::{Alignment, Availability, Effect, TemporalBound, TemporalSummary, Type, TypeLiteral},
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
