use super::{
    AST_WRONG_SHAPE, ExprKind, FunctionPhase, ItemKind, OperatorMetadataError, PatternKind,
    TypeExpr, TypeExprKind, lower_document, metadata_key, operator_declaration,
};
use crate::{
    reader::read,
    syntax::FormKind,
    types::{Alignment, Availability, Effect, TemporalBound},
};

#[test]
fn lowers_module_header_and_declarations() {
    let document = read(
        "(module analytics.transforms.normalize)
             (import data.series :as series)
             (import-for-syntax osiris.syntax :as syntax)
             (py/import numpy :as np)
             (export [normalize])
             (alias 窗口均值 series/moving-average)
             (def ^Float scale 0.5)
             (defn ^Float normalize [^Frame values [^PositiveInt window = 8]]
               (let [x 1 y (+ x 2)] y))",
    );
    assert!(
        document.diagnostics.is_empty(),
        "{:?}",
        document.diagnostics
    );
    let lowered = lower_document(&document);
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    assert_eq!(
        lowered
            .module
            .name
            .as_ref()
            .map(|name| name.canonical.as_str()),
        Some("analytics.transforms.normalize")
    );
    assert_eq!(lowered.module.items.len(), 7);
    assert!(matches!(lowered.module.items[0].kind, ItemKind::Import(_)));
    assert!(matches!(
        lowered.module.items[1].kind,
        ItemKind::ImportForSyntax(_)
    ));
    assert!(matches!(
        lowered.module.items[2].kind,
        ItemKind::PyImport(_)
    ));
    let function = match &lowered.module.items[6].kind {
        ItemKind::Defn(function) => function,
        other => panic!("expected defn, got {other:?}"),
    };
    assert_eq!(function.params.len(), 2);
    assert_eq!(
        function.params[1].default.as_ref().map(|value| &value.kind),
        Some(&ExprKind::Integer("8".to_owned()))
    );
    assert_eq!(function.phase, FunctionPhase::Runtime);
}

#[test]
fn lowers_clojure_parameter_patterns_without_confusing_type_annotations() {
    let lowered = lower_document(&read(
        r#"(fn [{:keys [value]} ^Any [left right] ^{:type (Vector Int)} [first second] ^Int plain]
                 value)"#,
    ));
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let ItemKind::Expr(expression) = &lowered.module.items[0].kind else {
        panic!("expected expression");
    };
    let ExprKind::Fn(function) = &expression.kind else {
        panic!("expected fn");
    };
    assert_eq!(function.params.len(), 4);
    assert!(matches!(
        function.params[0]
            .pattern
            .as_ref()
            .map(|pattern| &pattern.kind),
        Some(PatternKind::Map(_))
    ));
    assert!(matches!(
        function.params[1]
            .pattern
            .as_ref()
            .map(|pattern| &pattern.kind),
        Some(PatternKind::Vector(_))
    ));
    assert!(function.params[1].type_annotation.is_some());
    assert!(matches!(
        function.params[2]
            .pattern
            .as_ref()
            .map(|pattern| &pattern.kind),
        Some(PatternKind::Vector(_))
    ));
    assert!(function.params[2].type_annotation.is_some());
    assert!(function.params[3].pattern.is_none());
    assert!(function.params[3].type_annotation.is_some());
}

#[test]
fn runtime_parameter_types_do_not_depend_on_case_or_script() {
    let lowered = lower_document(&read(
        "(fn [^right left ^{:type 中文类型} 参数 ^pkg/lower qualified] left)",
    ));
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let ItemKind::Expr(expression) = &lowered.module.items[0].kind else {
        panic!("expected expression");
    };
    let ExprKind::Fn(function) = &expression.kind else {
        panic!("expected fn");
    };

    let expected = ["right", "中文类型", "pkg/lower"];
    for (parameter, expected_type) in function.params.iter().zip(expected) {
        assert!(parameter.pattern.is_none());
        assert!(matches!(
            parameter.type_annotation.as_ref().map(|ty| &ty.kind),
            Some(TypeExprKind::Name(name)) if name.canonical == expected_type
        ));
    }
}

#[test]
fn lowers_type_and_tag_metadata_for_parameters_returns_and_locals() {
    let lowered = lower_document(&read(
        "(defn ^{:type (Vector Int)} increment-all [^{:type (Vector Int)} values]
               (let [^{:type Int} offset 1] values))
             (defn ^Vector tagged [^Int value] value)",
    ));
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);

    let ItemKind::Defn(function) = &lowered.module.items[0].kind else {
        panic!("expected defn");
    };
    assert!(matches!(
        function.params[0]
            .type_annotation
            .as_ref()
            .map(|ty| &ty.kind),
        Some(TypeExprKind::Apply { constructor, args })
            if matches!(&constructor.kind, TypeExprKind::Name(name) if name.canonical == "Vector")
                && args.len() == 1
    ));
    assert!(matches!(
        function.return_type.as_ref().map(|ty| &ty.kind),
        Some(TypeExprKind::Apply { constructor, args })
            if matches!(&constructor.kind, TypeExprKind::Name(name) if name.canonical == "Vector")
                && args.len() == 1
    ));
    assert_eq!(
        function
            .return_type
            .as_ref()
            .map_or(0, |ty| ty.metadata.len()),
        1
    );
    let ExprKind::Let { bindings, .. } = &function.body[0].kind else {
        panic!("expected let");
    };
    assert!(matches!(
        bindings[0].type_annotation.as_ref().map(|ty| &ty.kind),
        Some(TypeExprKind::Name(name)) if name.canonical == "Int"
    ));

    let ItemKind::Defn(tagged) = &lowered.module.items[1].kind else {
        panic!("expected tagged defn");
    };
    assert!(matches!(
        tagged.params[0]
            .type_annotation
            .as_ref()
            .map(|ty| &ty.kind),
        Some(TypeExprKind::Name(name)) if name.canonical == "Int"
    ));
    assert!(matches!(
        tagged.return_type.as_ref().map(|ty| &ty.kind),
        Some(TypeExprKind::Apply { constructor, args })
            if matches!(&constructor.kind, TypeExprKind::Name(name) if name.canonical == "Vector")
                && matches!(args.as_slice(), [TypeExpr { kind: TypeExprKind::Name(name), .. }]
                    if name.canonical == "Any")
    ));
}

#[test]
fn raw_core_container_metadata_defaults_parameters_to_any() {
    let lowered = lower_document(&read(
        "(defn raw [^Vector vector ^List list ^Set set ^Option option ^Map mapping] vector)
             (fn [^Vector strict] strict)",
    ));
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let ItemKind::Defn(function) = &lowered.module.items[0].kind else {
        panic!("expected defn");
    };
    for (parameter, (constructor_name, arity)) in function.params.iter().zip([
        ("Vector", 1),
        ("List", 1),
        ("Set", 1),
        ("Option", 1),
        ("Map", 2),
    ]) {
        let Some(TypeExpr {
            kind: TypeExprKind::Apply { constructor, args },
            ..
        }) = &parameter.type_annotation
        else {
            panic!("expected raw container metadata to become an application");
        };
        assert!(matches!(
            &constructor.kind,
            TypeExprKind::Name(name) if name.canonical == constructor_name
        ));
        assert_eq!(args.len(), arity);
        assert!(args.iter().all(|argument| matches!(
            &argument.kind,
            TypeExprKind::Name(name) if name.canonical == "Any"
        )));
    }

    let ItemKind::Expr(expression) = &lowered.module.items[1].kind else {
        panic!("expected fn expression");
    };
    let ExprKind::Fn(function) = &expression.kind else {
        panic!("expected fn expression");
    };
    assert!(matches!(
        function.params[0]
            .type_annotation
            .as_ref()
            .map(|ty| &ty.kind),
        Some(TypeExprKind::Apply { constructor, args })
            if matches!(&constructor.kind, TypeExprKind::Name(name) if name.canonical == "Vector")
                && args.len() == 1
    ));
}

#[test]
fn detached_type_marker_is_not_a_parameter_annotation() {
    let lowered = lower_document(&read(
        "(defn increment-all [^:type (Vector Int) values] values)",
    ));
    assert!(lowered.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == AST_WRONG_SHAPE || diagnostic.code == super::AST_INVALID_NAME
    }));
}

#[test]
fn old_adjacent_parameter_and_arrow_types_are_rejected() {
    let lowered = lower_document(&read(
        "(defn ^Int choose [[value Float]] -> Float value)",
    ));
    assert!(!lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let ItemKind::Defn(function) = &lowered.module.items[0].kind else {
        panic!("expected defn");
    };
    assert!(matches!(
        function.return_type.as_ref().map(|ty| &ty.kind),
        Some(TypeExprKind::Name(name)) if name.canonical == "Int"
    ));
}

#[test]
fn type_metadata_wins_over_tag_metadata_with_a_diagnostic() {
    let lowered = lower_document(&read("(defn ^{:type Int :tag Float} choose [value] value)"));
    assert_eq!(
        lowered
            .diagnostics
            .iter()
            .filter(|diagnostic| { diagnostic.code == super::AST_CONFLICTING_TYPE_ANNOTATION })
            .count(),
        1,
        "{:?}",
        lowered.diagnostics
    );
    let ItemKind::Defn(function) = &lowered.module.items[0].kind else {
        panic!("expected defn");
    };
    assert!(matches!(
        function.return_type.as_ref().map(|ty| &ty.kind),
        Some(TypeExprKind::Name(name)) if name.canonical == "Int"
    ));
}

#[test]
fn malformed_type_metadata_is_recoverable() {
    let lowered = lower_document(&read(
        "(defn invalid [^{:type 1} value] value)\n\
             (defn incomplete [^:type Int] 1)\n\
             (defn okay [value] value)",
    ));
    assert!(
        lowered
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == super::AST_INVALID_TYPE_METADATA)
            .count()
            >= 2,
        "{:?}",
        lowered.diagnostics
    );
    assert!(matches!(lowered.module.items[2].kind, ItemKind::Defn(_)));
}

#[test]
fn phase_one_vector_parameters_are_patterns_regardless_of_spelling() {
    let lowered = lower_document(&read(
        "(defmacro choose [[left Type]] left)\n\
             (defn-for-syntax helper [[参数 中文类型]] 参数)",
    ));
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);

    let ItemKind::Defmacro(macro_) = &lowered.module.items[0].kind else {
        panic!("expected macro");
    };
    assert!(matches!(
        macro_.params[0]
            .pattern
            .as_ref()
            .map(|pattern| &pattern.kind),
        Some(PatternKind::Vector(_))
    ));

    let ItemKind::DefnForSyntax(helper) = &lowered.module.items[1].kind else {
        panic!("expected syntax helper");
    };
    assert!(matches!(
        helper.params[0]
            .pattern
            .as_ref()
            .map(|pattern| &pattern.kind),
        Some(PatternKind::Vector(_))
    ));
}

#[test]
fn runtime_vector_pattern_type_must_annotate_the_pattern() {
    let lowered = lower_document(&read("(fn [[^right left extra]] left)"));
    assert!(lowered.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == AST_WRONG_SHAPE && diagnostic.message.contains("Rich Metadata type")
    }));
}

#[test]
fn runtime_vector_pattern_requires_an_explicit_type() {
    let lowered = lower_document(&read("(fn [[[left right]]] left)"));
    assert!(lowered.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == AST_WRONG_SHAPE
            && diagnostic
                .message
                .contains("runtime vector destructuring requires an explicit Rich Metadata type")
    }));
}
