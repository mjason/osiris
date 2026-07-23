use std::collections::{BTreeMap, BTreeSet};

use super::{
    Alignment, Availability, CallSummaries, DataProperties, Effect, EffectRow, FunctionType,
    PythonTypingImport, PythonVersion, ScalarOperator, TemporalBound, TemporalSummary, Type,
    TypeContext, TypeErrorKind, TypeLiteral, TypeVarId, parse_type,
    python_builtin_exception_binding, python_builtin_exception_from_binding,
    python_builtin_exception_name, scalar_operator_signatures,
};
use crate::{
    reader::read,
    syntax::{Form, FormKind},
};

fn read_one(source: &str) -> Form {
    let document = read(source);
    assert!(document.diagnostics.is_empty(), "reader diagnostics");
    assert_eq!(document.forms.len(), 1);
    document.forms.into_iter().next().expect("one form")
}

#[test]
fn infers_a_generic_collection_element() {
    let mut context = TypeContext::new();
    let variable = context.fresh_var();
    let expected = Type::List(Box::new(variable.clone()));

    let unified = context
        .unify(&expected, &Type::List(Box::new(Type::Int)))
        .expect("types unify");

    assert_eq!(unified, Type::List(Box::new(Type::Int)));
    let Type::TypeVar(variable) = variable else {
        panic!("fresh type variable")
    };
    assert_eq!(context.substitution(variable), Some(Type::Int));
}

#[test]
fn occurs_check_rejects_an_infinite_type_without_leaking_a_binding() {
    let mut context = TypeContext::new();
    let variable = context.fresh_var();
    let recursive = Type::List(Box::new(variable.clone()));

    let error = context
        .unify(&variable, &recursive)
        .expect_err("infinite type is rejected");

    assert!(matches!(error.kind, TypeErrorKind::OccursCheck { .. }));
    let Type::TypeVar(variable) = variable else {
        panic!("fresh type variable")
    };
    assert_eq!(context.substitution(variable), None);
}

#[test]
fn any_is_a_one_way_explicit_boundary() {
    let context = TypeContext::new();

    assert!(context.is_assignable(&Type::Int, &Type::Any));
    assert!(!context.is_assignable(&Type::Any, &Type::Int));
    let error = TypeContext::new()
        .unify(&Type::Any, &Type::Int)
        .expect_err("Any cannot silently become Int");
    assert_eq!(error.kind, TypeErrorKind::AnyRequiresExplicitCast);
}

#[test]
fn nominal_identity_is_the_defining_type_binding_not_the_short_name() {
    let left = Type::Nominal {
        binding: "dep.alpha::type::X".to_owned(),
        args: Vec::new(),
    };
    let right = Type::Nominal {
        binding: "dep.beta::type::X".to_owned(),
        args: Vec::new(),
    };
    let context = TypeContext::new();

    assert!(!context.is_assignable(&left, &right));
    assert!(TypeContext::new().unify(&left, &right).is_err());
    assert_eq!(
        Type::union([left, right]),
        Type::Union(vec![
            Type::Nominal {
                binding: "dep.alpha::type::X".to_owned(),
                args: Vec::new(),
            },
            Type::Nominal {
                binding: "dep.beta::type::X".to_owned(),
                args: Vec::new(),
            },
        ])
    );
}

#[test]
fn options_and_unions_are_canonical_and_assignable() {
    let context = TypeContext::new();
    let option = Type::union([Type::None, Type::Int, Type::Int]);

    assert_eq!(option, Type::Option(Box::new(Type::Int)));
    assert!(context.is_assignable(&Type::None, &option));
    assert!(context.is_assignable(&Type::Int, &option));
    assert!(!context.is_assignable(&Type::Str, &option));
    assert_eq!(context.join(&Type::None, &Type::Int), option);
    assert_eq!(
        Type::union([Type::Str, Type::Int]),
        Type::Union(vec![Type::Int, Type::Str])
    );
}

#[test]
fn joins_collection_branches_elementwise() {
    let context = TypeContext::new();
    assert_eq!(
        context.join(
            &Type::Vector(Box::new(Type::Int)),
            &Type::Vector(Box::new(Type::Never)),
        ),
        Type::Vector(Box::new(Type::Int))
    );
    assert_eq!(
        context.join(
            &Type::Map(Box::new(Type::Str), Box::new(Type::Int)),
            &Type::Map(Box::new(Type::Str), Box::new(Type::Float)),
        ),
        Type::Map(Box::new(Type::Str), Box::new(Type::Float))
    );
}

#[test]
fn function_assignment_is_contravariant_and_summary_aware() {
    let context = TypeContext::new();
    let broad_parameter = Type::Fn(FunctionType::new(vec![Type::Float], Type::Int));
    let narrow_parameter = Type::Fn(FunctionType::new(vec![Type::Int], Type::Float));

    assert!(context.is_assignable(&broad_parameter, &narrow_parameter));
    assert!(!context.is_assignable(&narrow_parameter, &broad_parameter));

    let throwing = Type::Fn(
        FunctionType::new(vec![], Type::Int).with_summaries(CallSummaries {
            effects: EffectRow::singleton(Effect::Throw),
            ..CallSummaries::pure_scalar()
        }),
    );
    let pure = Type::Fn(FunctionType::new(vec![], Type::Int));
    assert!(!context.is_assignable(&throwing, &pure));
    assert!(context.is_assignable(&pure, &throwing));

    let source_unspecified = parse_type(&read_one("(Fn [] -> Int)"), &BTreeMap::new())
        .expect("source function type parses");
    let rich_callback = Type::Fn(FunctionType::new(vec![], Type::Int).with_summaries(
        CallSummaries {
            effects: EffectRow::singleton(Effect::Mutation),
            temporal: TemporalSummary {
                past: TemporalBound::Finite(2),
                future: TemporalBound::Finite(1),
                availability: Availability::Named("published".to_owned()),
            },
            data: DataProperties {
                axes: Some(vec!["time".to_owned()]),
                alignment: Alignment::Labelled,
                preserves_length: Some(true),
                ..DataProperties::unknown()
            },
        },
    ));
    assert!(context.is_assignable(&rich_callback, &source_unspecified));
    assert!(!context.is_assignable(&rich_callback, &pure));
}

#[test]
fn pointwise_temporal_facts_are_join_identities() {
    let declared = TemporalSummary {
        past: TemporalBound::Symbolic("window".to_owned()),
        future: TemporalBound::Finite(0),
        availability: Availability::Named("published".to_owned()),
    };

    assert_eq!(declared.join(&TemporalSummary::pointwise()), declared);
}

#[test]
fn rolling_temporal_bounds_compose_and_join_symbolically() {
    let rolling = TemporalSummary {
        past: TemporalBound::Symbolic("n-1".to_owned()),
        future: TemporalBound::Finite(0),
        availability: Availability::Named("published".to_owned()),
    };

    let twice = rolling.compose(&rolling);
    assert_eq!(twice.past, TemporalBound::Symbolic("2*(n-1)".to_owned()));
    assert_eq!(rolling.join(&twice), twice);

    let specialized = rolling.substitute(&BTreeMap::from([("n".to_owned(), "window".to_owned())]));
    assert_eq!(
        specialized.past,
        TemporalBound::Symbolic("window-1".to_owned())
    );
    let literal = rolling.substitute(&BTreeMap::from([("n".to_owned(), "96".to_owned())]));
    assert_eq!(literal.past, TemporalBound::Finite(95));
}

#[test]
fn parses_explicit_generic_and_function_types() {
    let generic = parse_type(
        &read_one("(Map Str (Option T))"),
        &BTreeMap::from([("T".to_owned(), TypeVarId(7))]),
    )
    .expect("generic type parses");
    assert_eq!(
        generic,
        Type::Map(
            Box::new(Type::Str),
            Box::new(Type::Option(Box::new(Type::TypeVar(TypeVarId(7)))))
        )
    );

    let function = parse_type(
        &read_one("(Fn [Int (List T)] -> (Option T))"),
        &BTreeMap::from([("T".to_owned(), TypeVarId(7))]),
    )
    .expect("function type parses");
    assert_eq!(
        function,
        Type::Fn(
            FunctionType::new(
                vec![Type::Int, Type::List(Box::new(Type::TypeVar(TypeVarId(7))))],
                Type::Option(Box::new(Type::TypeVar(TypeVarId(7))))
            )
            .with_summaries(CallSummaries::unknown())
        )
    );
}

#[test]
fn parses_canonical_literal_type_arguments_for_axes_and_frame_schema() {
    let array = parse_type(
        &read_one("(Array Float [:time :feature])"),
        &BTreeMap::new(),
    )
    .expect("array type parses");
    assert_eq!(
        array,
        Type::Nominal {
            binding: "Array".to_owned(),
            args: vec![
                Type::Float,
                Type::Literal(TypeLiteral::Vector(vec![
                    TypeLiteral::Keyword(":time".to_owned()),
                    TypeLiteral::Keyword(":feature".to_owned()),
                ])),
            ],
        }
    );
    assert_eq!(
        array.to_python_annotation(PythonVersion::PYTHON_3_9),
        Ok("Array[float, Literal[\"[:time :feature]\"]]".to_owned())
    );
    assert!(
        array
            .python_typing_imports(PythonVersion::PYTHON_3_9)
            .contains(&PythonTypingImport::typing("Literal"))
    );
    let annotated_axes = parse_type(
        &read_one("(Array Float ^{:doc \"display only\"} [:time :feature])"),
        &BTreeMap::new(),
    )
    .expect("metadata-bearing axes parse");
    assert_eq!(array, annotated_axes, "metadata is not type identity");
    let other_axes = parse_type(
        &read_one("(Array Float [:time :channel])"),
        &BTreeMap::new(),
    )
    .expect("second array type parses");
    let context = TypeContext::new();
    assert!(context.is_assignable(&array, &array));
    assert!(!context.is_assignable(&array, &other_axes));

    let frame = parse_type(
        &read_one(
            "(Frame {:value Float :time Datetime :category Str} \
                         :key [:time :category] :order [:time])",
        ),
        &BTreeMap::new(),
    )
    .expect("frame type parses");
    let Type::Nominal { args, .. } = frame else {
        panic!("frame is nominal")
    };
    assert_eq!(args.len(), 5);
    let Type::Literal(schema) = &args[0] else {
        panic!("frame schema is literal")
    };
    assert_eq!(
        schema.canonical_text(),
        "{:category Str :time Datetime :value Float}"
    );
    assert_eq!(
        args[1],
        Type::Literal(TypeLiteral::Keyword(":key".to_owned()))
    );
    assert_eq!(
        args[3],
        Type::Literal(TypeLiteral::Keyword(":order".to_owned()))
    );
}

#[test]
fn numeric_unification_and_scalar_signatures_promote_to_float() {
    assert_eq!(
        TypeContext::new().unify(&Type::Int, &Type::Float),
        Ok(Type::Float)
    );
    let division = scalar_operator_signatures(ScalarOperator::TrueDivide);
    assert_eq!(division.len(), 4);
    assert!(
        division
            .iter()
            .all(|signature| signature.result == Type::Float)
    );
    let addition = scalar_operator_signatures(ScalarOperator::Add);
    assert!(addition.iter().any(|signature| {
        signature.operands == vec![Type::Int, Type::Float] && signature.result == Type::Float
    }));
}

#[test]
fn reports_typing_imports_and_readable_annotations() {
    let ty = Type::Fn(FunctionType::new(
        vec![
            Type::Nominal {
                binding: "data.series::type::Series".to_owned(),
                args: vec![Type::Float],
            },
            Type::Never,
        ],
        Type::option(Type::Int),
    ));
    assert_eq!(
        ty.python_typing_imports(PythonVersion::PYTHON_3_9),
        BTreeSet::from([
            PythonTypingImport::typing("Callable"),
            PythonTypingImport::typing("NoReturn"),
            PythonTypingImport::typing("Optional"),
        ])
    );
    assert_eq!(
        ty.to_python_annotation(PythonVersion::PYTHON_3_9),
        Ok("Callable[[Series[float], NoReturn], Optional[int]]".to_owned())
    );
}

#[test]
fn malformed_function_annotation_has_a_source_span() {
    let form = read_one("(Fn Int Str)");
    let error = parse_type(&form, &BTreeMap::new()).expect_err("parameter vector required");
    assert!(matches!(
        error.kind,
        super::TypeParseErrorKind::FunctionParameters
    ));
    let FormKind::List(items) = form.kind else {
        panic!("list")
    };
    assert_eq!(error.span, items[1].span);
}

#[test]
fn builtin_exception_type_whitelist_is_closed_and_roundtrips() {
    assert_eq!(
        python_builtin_exception_name("Exception"),
        Some("Exception")
    );
    assert_eq!(
        python_builtin_exception_name("builtins/ValueError"),
        Some("ValueError")
    );
    assert_eq!(python_builtin_exception_name("custom/Exception"), None);
    let binding = python_builtin_exception_binding("TypeError").expect("known exception");
    assert_eq!(
        python_builtin_exception_from_binding(&binding),
        Some("TypeError")
    );
    assert_eq!(python_builtin_exception_from_binding("Exception"), None);
}
