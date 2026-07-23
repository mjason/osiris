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
