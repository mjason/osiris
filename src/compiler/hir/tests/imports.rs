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
fn python_decorators_resolve_aliases_to_local_generated_bindings() {
    let result = lower(
        "(py/import host.runtime :as host)\n\
         (defn publish [] -> Int 1)\n\
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
         (def value Int 1)\n\
         (defn publish [] -> Int 1)\n\
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
