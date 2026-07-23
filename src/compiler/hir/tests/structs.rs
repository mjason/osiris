#[test]
fn struct_constructor_checks_fields_and_defaults() {
    let result = lower(
        "(defstruct Point [x Int] [y Int = 0])
             (def point (Point :x 1))",
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let ItemKind::Value(value) = &result.module.items[1].kind else {
        panic!("expected value");
    };
    let expression = value.value.as_ref().expect("constructor expression");
    assert_eq!(
        expression.ty,
        Type::Nominal {
            binding: "example::type::Point".to_owned(),
            args: Vec::new()
        }
    );
    let super::ExprKind::Call { arguments, .. } = &expression.kind else {
        panic!("expected constructor call");
    };
    assert!(matches!(
        &arguments[0],
        super::CallArgument::Keyword { name, .. } if name == "x"
    ));
}

#[test]
fn struct_field_access_keeps_declared_type_and_summary() {
    let result = lower(
        r#"(defstruct Point [x Int] [y Float])
               (defn distance [[point Point]] -> Float (+ point.x point.y))"#,
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
        .expect("expected distance function");
    assert_eq!(function.body.ty, Type::Float);
    let ExprKind::Operator { operands, .. } = &function.body.kind else {
        panic!("expected scalar operator")
    };
    assert_eq!(operands[0].ty, Type::Int);
    assert_eq!(operands[1].ty, Type::Float);
    assert!(matches!(
        &operands[0].kind,
        ExprKind::Attribute { attribute, .. } if attribute == "x"
    ));
}

#[test]
fn imported_struct_field_access_uses_interface_field_type() {
    let document = read(
        r#"(module dep.fields)
               (defstruct (Series T)
                 ^{:osiris/names {"zh-CN" {:preferred 值}}}
                 [values (Vector T)])
               (export [Series])"#,
    );
    let surface = lower_document(&document);
    assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
    let typed = lower_module(&surface.module, "dep.fields");
    assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
    let dependency = interface::build(&typed.module, &surface.module).expect("interface");
    let interfaces = BTreeMap::from([(dependency.module.clone(), dependency)]);
    let caller = read(
        r#"(module app)
               (import dep.fields)
               (defn values [[series (Series Float)]] -> (Vector Float) series.值)"#,
    );
    let caller_surface = lower_document(&caller);
    assert!(
        caller_surface.diagnostics.is_empty(),
        "{:?}",
        caller_surface.diagnostics
    );
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
        .expect("expected values function");
    assert_eq!(function.body.ty, Type::Vector(Box::new(Type::Float)));
}

#[test]
fn unknown_declared_struct_field_is_rejected() {
    let result = lower("(defstruct Point [x Int]) (defn bad [[point Point]] -> Int point.missing)");
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-T0016")
    );
}

#[test]
fn generic_struct_constructor_instantiates_type_parameters() {
    let result = lower(
        "(defstruct (Range T) [min T] [max T = 1])
             (def range (Range 0 1))",
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let ItemKind::Value(value) = &result.module.items[1].kind else {
        panic!("expected value");
    };
    let expression = value.value.as_ref().expect("constructor expression");
    assert_eq!(
        expression.ty,
        Type::Nominal {
            binding: "example::type::Range".to_owned(),
            args: vec![Type::Int]
        }
    );
}

#[test]
fn literal_type_arguments_reach_typed_hir_without_error_types() {
    let result = lower(
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
    assert_eq!(functions.len(), 2);
    let Type::Nominal { args, .. } = &functions[0].parameters[0].ty else {
        panic!("array parameter is nominal")
    };
    assert_eq!(
        args[1],
        Type::Literal(TypeLiteral::Vector(vec![
            TypeLiteral::Keyword(":time".to_owned()),
            TypeLiteral::Keyword(":feature".to_owned()),
        ]))
    );
    let Type::Nominal { args, .. } = &functions[1].return_type else {
        panic!("frame return is nominal")
    };
    let Type::Literal(schema) = &args[0] else {
        panic!("frame schema is literal")
    };
    assert_eq!(
        schema.canonical_text(),
        "{:category Str :time Datetime :value Float}"
    );
}

#[test]
fn unknown_nominal_type_is_not_fabricated_in_typed_hir() {
    let result = lower("(defn typo [[value Typo]] -> Typo value)");
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "OSR-T0021" && diagnostic.message == "unknown nominal type `Typo`"
    }));
    let binding = result
        .module
        .bindings
        .iter()
        .find(|binding| binding.name.canonical == "typo")
        .expect("function binding remains recoverable");
    let Type::Fn(signature) = &binding.ty else {
        panic!("function remains typed")
    };
    assert_eq!(signature.parameters, [Type::Error]);
    assert_eq!(signature.return_type.as_ref(), &Type::Error);
}

#[test]
fn struct_check_keeps_typed_message_and_throw_summary() {
    let result = lower(
        "(defstruct Checked [value Int]
               (check (> value 0) \"value must be positive\"))
             (def checked (Checked 1))",
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let ItemKind::Struct(structure) = &result.module.items[0].kind else {
        panic!("expected struct");
    };
    assert_eq!(structure.checks.len(), 1);
    assert_eq!(
        structure.checks[0]
            .message
            .as_ref()
            .map(|message| &message.ty),
        Some(&Type::Str)
    );
    assert!(structure.checks[0].condition.ty == Type::Bool);
}

#[test]
fn parameter_aliases_are_canonicalized_and_type_checked() {
    let result = lower(
        "(defn f [^{:osiris/names {\"zh-CN\" {:preferred 周期 :aliases [时长]}}}
                       [window Int]] -> Int window)
             (f :时长 2)",
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let ItemKind::Expr(expression) = &result.module.items[1].kind else {
        panic!("expected call expression");
    };
    let super::ExprKind::Call { arguments, .. } = &expression.kind else {
        panic!("expected call");
    };
    assert!(matches!(
        &arguments[0],
        super::CallArgument::Keyword { name, .. } if name == "window"
    ));
}

#[test]
fn phase_one_names_do_not_collide_with_runtime_names() {
    let result = lower(
        "(defn-for-syntax helper [] -> Int 1)
             (defn helper [] -> Int 2)
             (helper)",
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result
            .module
            .bindings
            .iter()
            .filter(|binding| binding.name.canonical == "helper")
            .count(),
        1
    );
}

#[test]
fn exporting_an_alias_requires_an_explicit_canonical_export() {
    let rejected = lower(
        "(defn canonical [] -> Int 1)
             (alias 本地名 canonical)
             (export [本地名])",
    );
    assert!(
        rejected
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-N0015")
    );
    assert!(rejected.module.exports.is_empty());
    assert!(!rejected.module.aliases[0].public);

    let accepted = lower(
        "(defn canonical [] -> Int 1)
             (alias 本地名 canonical)
             (export [canonical 本地名])",
    );
    assert!(
        accepted.diagnostics.is_empty(),
        "{:?}",
        accepted.diagnostics
    );
    assert_eq!(accepted.module.exports.len(), 1);
    assert!(accepted.module.aliases[0].public);
}

#[test]
fn rejects_local_python_identifier_collisions() {
    let result = lower("(defn collision [[a-b Int] [a_b Int]] -> Int a-b)");
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-N0002")
    );
}
