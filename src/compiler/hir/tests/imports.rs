#[test]
fn symbolic_temporal_contracts_specialize_and_compose_across_calls() {
    let result = lower(
        r#"(extern python "host.ops"
                  (defn ^Int rolling [^Int value ^Int window]
                    :contract
                    {:id "host.ops/rolling-v1"
                     :effects :pure
                     :temporal {:past "window-1"
                                :future 0
                                :availability :published}}))
               (defn ^Int twice [^Int value ^Int n]
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
                    "host.series/rolling-v1".to_owned()
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
                    "host.series/rolling-v1".to_owned()
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
                    "host.series/rolling-v1".to_owned()
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
             (defn ^Int call [] (dep/add 1 :值 2))",
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
fn ordinary_import_refer_all_applies_exclusion_and_rename() {
    let result = lower_with_dependency(
        "(module app)\n\
         (import dep.core :as dep :refer :all :exclude [sum] :rename {add plus})\n\
         (defn ^Int local-call [] (plus 1 2))\n\
         (defn ^Int qualified-call [] (dep/add 3 4))",
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert!(result.module.aliases.iter().any(|alias| {
        alias.canonical == "plus" && alias.target.as_str() == "dep.core::function::add"
    }));
    assert!(result.module.items.iter().filter_map(|item| match &item.kind {
        ItemKind::Function(function) => Some(&function.body),
        _ => None,
    }).all(|body| matches!(
        &body.kind,
        ExprKind::Call { callee, .. }
            if matches!(&callee.kind, ExprKind::Binding(binding) if binding.as_str() == "dep.core::function::add")
    )));
}

#[test]
fn ordinary_import_refer_all_validates_exclusions_and_renames() {
    let result = lower_with_dependency(
        "(module app)\n\
         (import dep.core :refer :all :exclude [add missing] :rename {add plus nope absent})",
    );
    for code in ["OSR-H0011", "OSR-H0014"] {
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == code),
            "missing {code}: {:?}",
            result.diagnostics
        );
    }
}

#[test]
fn qualified_imported_targets_can_be_local_aliases_without_new_binding() {
    let result = lower_with_dependency(
        "(module app)
             (import dep.core :as dep)
             (alias 加法 dep/add)
             (alias 求和 dep/sum)
             (defn ^Int call [] (加法 1 2))",
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
fn python_decorators_resolve_aliases_to_local_generated_bindings() {
    let result = lower(
        "(py/import host.runtime :as host)\n\
         (defn ^Int publish [] 1)\n\
         (alias 发布 publish)\n\
         (py/decorate 发布 host.register)",
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
        .expect("decorated function");
    assert_eq!(function.decorators.len(), 1);
    assert!(matches!(
        function.decorators[0].kind,
        ExprKind::Attribute { .. }
    ));
}

#[test]
fn python_decorators_reject_unknown_non_generated_and_duplicate_targets() {
    let result = lower(
        "(py/import host.runtime :as host)\n\
         (def ^Int value 1)\n\
         (defn ^Int publish [] 1)\n\
         (py/decorate missing host.register)\n\
         (py/decorate value host.register)\n\
         (py/decorate publish host.first)\n\
         (py/decorate publish host.second)",
    );
    for code in ["OSR-H0030", "OSR-H0031", "OSR-H0032"] {
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == code),
            "missing {code}: {:?}",
            result.diagnostics
        );
    }
}

#[test]
fn implicit_core_functions_keep_their_facade_identity() {
    let result = lower("(def result (mapv (fn [value] (+ value 1)) [1 2]))");
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let call = result
        .module
        .items
        .iter()
        .find_map(|item| match &item.kind {
            ItemKind::Value(value) => value.value.as_ref(),
            _ => None,
        })
        .expect("lowered value");
    let ExprKind::Call { callee, .. } = &call.kind else {
        panic!("mapv should remain a call");
    };
    let ExprKind::Binding(binding) = &callee.kind else {
        panic!("mapv should resolve to a stable binding");
    };
    assert_eq!(binding.as_str(), "osiris.core::function::mapv");

    let restricted = lower(
        "(import osiris.core :refer [reduce])\n\
         (def result (mapv (fn [value] (+ value 1)) [1 2]))",
    );
    assert!(restricted.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "OSR-N0012" && diagnostic.message.contains("mapv")
    }));
}

#[test]
fn core_import_applies_exclusion_before_rename_and_supports_qualified_calls() {
    let renamed = lower(
        "(import osiris.core :as core :refer :all :exclude [map] :rename {reduce fold-left})\n\
         (def result (fold-left (fn [left right] (+ left right)) 0 [1 2]))\n\
         (def first-value (core/first [1 2]))",
    );
    assert!(renamed.diagnostics.is_empty(), "{:?}", renamed.diagnostics);
    assert!(renamed.module.bindings.iter().any(|binding| {
        binding.name.id.as_str() == "osiris.core::function::reduce"
    }));
    assert!(!renamed.module.bindings.iter().any(|binding| {
        binding.name.id.as_str() == "osiris.core::function::map"
    }));

    let invalid = lower(
        "(import osiris.core :refer :all :exclude [map] :rename {map mapped nope missing})",
    );
    for code in ["OSR-H0023", "OSR-H0025"] {
        assert!(
            invalid
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == code),
            "missing {code}: {:?}",
            invalid.diagnostics
        );
    }
}

#[test]
fn local_names_are_not_selected_as_standard_intrinsics_by_spelling() {
    let result = lower(
        "(def result (let [mapv (fn [value] value)] (mapv 1)))",
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let call = result
        .module
        .items
        .iter()
        .find_map(|item| match &item.kind {
            ItemKind::Value(value) => value.value.as_ref(),
            _ => None,
        })
        .expect("lowered value");
    let ExprKind::Let { body, .. } = &call.kind else {
        panic!("result should remain a let expression");
    };
    let ExprKind::Call { callee, .. } = &body.kind else {
        panic!("let body should remain a call");
    };
    let ExprKind::Binding(binding) = &callee.kind else {
        panic!("local mapv should resolve to a binding");
    };
    assert_ne!(binding.as_str(), "osiris.core::function::mapv");
}

#[test]
fn top_level_declarations_shadow_only_implicit_core_names() {
    let result = lower(
        "(defn ^Int mapv [^Int value] value)\n\
         (def local-result (mapv 1))\n\
         (def core-result (osiris.core/mapv (fn [value] value) [1]))",
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);

    let local_id = result
        .module
        .bindings
        .iter()
        .find(|binding| {
            binding.source_spelling == "mapv"
                && binding.name.id.as_str().starts_with("example::function::")
        })
        .map(|binding| binding.name.id.clone())
        .expect("local mapv binding");
    let callees = result
        .module
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::Value(value) => value.value.as_ref(),
            _ => None,
        })
        .filter_map(|value| match &value.kind {
            ExprKind::Call { callee, .. } => match &callee.kind {
                ExprKind::Binding(binding) => Some(binding.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(callees.contains(&local_id.as_str()));
    assert!(callees.contains(&"osiris.core::function::mapv"));
}
