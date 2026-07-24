#[test]
fn keeps_call_keyword_order_and_duplicates() {
    let document = read("(f first :周期 3 :周期 4 tail)");
    let lowered = lower_document(&document);
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let item = &lowered.module.items[0];
    let call = match &item.kind {
        ItemKind::Expr(expr) => match &expr.kind {
            ExprKind::Call(call) => call,
            other => panic!("expected call, got {other:?}"),
        },
        other => panic!("expected expression, got {other:?}"),
    };
    assert_eq!(call.args.len(), 4);
    assert_eq!(call.positional.len(), 2);
    assert_eq!(call.keywords.len(), 2);
    assert_eq!(call.keywords[0].key.canonical, ":周期");
    assert_eq!(call.keywords[1].key.canonical, ":周期");
}

#[test]
fn malformed_keyword_and_bindings_recover_following_top_level_form() {
    let document = read("(f :missing) (let [x] x) (defn okay [value] value)");
    let lowered = lower_document(&document);
    assert!(
        lowered
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == super::AST_EXPECTED_PAIR)
    );
    assert_eq!(lowered.module.items.len(), 3);
    assert!(matches!(lowered.module.items[2].kind, ItemKind::Defn(_)));
}

#[test]
fn lowers_special_forms_and_reader_quotes() {
    let document = read(
        "(fn [x] (if true (do x) (raise \"bad\")))
             '(quoted value)
             \x60(let [tmp ~x] ~@items)",
    );
    let lowered = lower_document(&document);
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let first = match &lowered.module.items[0].kind {
        ItemKind::Expr(expr) => expr,
        _ => panic!("expected expression"),
    };
    assert!(matches!(first.kind, ExprKind::Fn(_)));
    let quoted = match &lowered.module.items[1].kind {
        ItemKind::Expr(expr) => expr,
        _ => panic!("expected expression"),
    };
    assert!(matches!(quoted.kind, ExprKind::Quote(_)));
    let syntax_quoted = match &lowered.module.items[2].kind {
        ItemKind::Expr(expr) => expr,
        _ => panic!("expected expression"),
    };
    assert!(matches!(syntax_quoted.kind, ExprKind::SyntaxQuote(_)));
}

#[test]
fn lowers_struct_generics_fields_and_checks() {
    let document = read(
        "(defstruct (Range T)
               \"closed range\"
               [min T]
               [max T = 1]
               (check (<= min max) \"ordered\"))",
    );
    let lowered = lower_document(&document);
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let structure = match &lowered.module.items[0].kind {
        ItemKind::Defstruct(structure) => structure,
        other => panic!("expected defstruct, got {other:?}"),
    };
    assert_eq!(structure.type_params.len(), 1);
    assert_eq!(structure.fields.len(), 2);
    assert_eq!(structure.checks.len(), 1);
    assert!(matches!(
        structure.checks[0].condition.kind,
        ExprKind::Call(_)
    ));
    assert!(matches!(
        structure.checks[0]
            .message
            .as_ref()
            .map(|message| &message.kind),
        Some(ExprKind::String(message)) if message == "ordered"
    ));
    assert_eq!(structure.doc.as_deref(), Some("closed range"));
}

#[test]
fn lowers_function_type_parameters_from_rich_metadata() {
    let document = read(
        "^{:type-params [A B]}\n\
         (defn ^{:type B} transform [^{:type A} value ^{:type (Fn [A] -> B)} function]\n\
           (function value))",
    );
    let lowered = lower_document(&document);
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let ItemKind::Defn(function) = &lowered.module.items[0].kind else {
        panic!("expected defn");
    };
    assert_eq!(
        function
            .type_params
            .iter()
            .map(|parameter| parameter.canonical.as_str())
            .collect::<Vec<_>>(),
        ["A", "B"]
    );
}

#[test]
fn type_expression_is_structured_but_keeps_extension_literals() {
    let document = read("(defn ^{:type (Union Int Float)} f [^{:type (Array Float [:time])} value] value)");
    let lowered = lower_document(&document);
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let function = match &lowered.module.items[0].kind {
        ItemKind::Defn(function) => function,
        _ => panic!("expected defn"),
    };
    assert!(matches!(
        function.params[0]
            .type_annotation
            .as_ref()
            .map(|type_expr| &type_expr.kind),
        Some(super::TypeExprKind::Apply { .. })
    ));
    assert!(matches!(
        function
            .return_type
            .as_ref()
            .map(|type_expr| &type_expr.kind),
        Some(super::TypeExprKind::Union(_))
    ));
}

#[test]
fn non_list_top_level_forms_are_surface_expressions() {
    let document = read("42");
    let lowered = lower_document(&document);
    assert!(lowered.diagnostics.is_empty());
    assert!(matches!(
        &lowered.module.items[0].kind,
        ItemKind::Expr(expr)
            if matches!(expr.kind, ExprKind::Integer(ref value) if value == "42")
    ));
}

#[test]
fn metadata_and_spans_are_copied_to_ast_nodes() {
    let document = read("^{:doc \"x\"} (def ^:private value 1)");
    let lowered = lower_document(&document);
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    assert_eq!(lowered.module.items[0].metadata.len(), 1);
    let definition = match &lowered.module.items[0].kind {
        ItemKind::Def(definition) => definition,
        _ => panic!("expected def"),
    };
    assert_eq!(definition.name.canonical, "value");
    assert_eq!(definition.metadata.len(), 2);
    assert!(definition.span.end > definition.span.start);
}

#[test]
fn def_name_metadata_is_preserved_with_name_precedence() {
    let lowered = lower_document(&read(
        "^{:doc \"outer\" :dynamic false} (def ^{:doc \"name\" :dynamic true} *value* 1)",
    ));
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let ItemKind::Def(definition) = &lowered.module.items[0].kind else {
        panic!("expected def");
    };
    assert_eq!(definition.metadata.len(), 2);
    assert!(definition.metadata.iter().any(|entry| {
        metadata_key(&entry.key) == Some("doc")
            && matches!(&entry.value.kind, FormKind::String(value) if value == "name")
    }));
    assert!(definition.metadata.iter().any(|entry| {
        metadata_key(&entry.key) == Some("dynamic")
            && matches!(entry.value.kind, FormKind::Bool(true))
    }));
}

#[test]
fn exposes_closed_operator_metadata_without_interpreting_it() {
    let lowered = lower_document(&read(
        "^{:osiris/operator :multiply} (defn ^Float scale [^Float value] value)",
    ));
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let function = match &lowered.module.items[0].kind {
        ItemKind::Defn(function) => function,
        other => panic!("expected defn, got {other:?}"),
    };
    assert_eq!(
        operator_declaration(&function.metadata),
        Ok(Some("multiply".to_owned()))
    );

    let malformed = lower_document(&read(
        "^{:osiris/operator [:multiply]} (defn ^Float scale [^Float value] value)",
    ));
    let malformed_function = match &malformed.module.items[0].kind {
        ItemKind::Defn(function) => function,
        other => panic!("expected defn, got {other:?}"),
    };
    assert_eq!(
        operator_declaration(&malformed_function.metadata),
        Err(OperatorMetadataError::ExpectedName)
    );

    let mut duplicate = function.metadata.clone();
    duplicate.push(function.metadata[0].clone());
    assert_eq!(
        operator_declaration(&duplicate),
        Err(OperatorMetadataError::Duplicate)
    );
}
