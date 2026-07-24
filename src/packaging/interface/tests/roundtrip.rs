use super::{
    build, emit, integer, keyword, read, refresh_standalone_hashes, render, string,
    validate_interface_metadata_resources,
};
use crate::{
    ast, hir, macro_expand, reader as source_reader,
    syntax::{METADATA_INTERFACE_LIMITS, METADATA_TARGET_LIMITS, MetadataEntry},
    types::{Availability, TemporalBound, Type},
};

const SOURCE: &str = r#"
        (module sample.core)

        ^{:doc "distance" :osiris/names {"zh-CN" {:preferred 距离}}}
        (defn ^Float distance [^{:type Float :osiris/names {"zh-CN" {:preferred 点位}}} point]
          point)

        ^{:doc "one metre"}
        (def ^Int metre 1)
        (alias 米 metre)

        ^{:doc "closed range"}
        (defstruct (Range T)
          "closed range"
          [min T]
          ^{:osiris/names {"zh-CN" {:preferred 最大值}}} [max T])

        (def private-value 9)
        (export [distance metre 米 Range])
    "#;

const STATIC_SOURCE: &str = r#"
        (module sample.records)

        ^{:doc "Descriptor schema."}
        (defstatic-schema Descriptor
          :schema-id "sample/descriptor"
          :version 1
          :fields
          {:id {:type Str :required true}
           :aliases {:type (Vector Str) :default []}}
          :indexes
          [{:id "sample/runtime-id"
            :scope :effective-dependency-graph
            :keys [{:field :id :role :canonical}]}])

        (defstatic-schema PrivateSchema
          :schema-id "sample/private"
          :version 1
          :fields {:value {:type Int :required true}})

        ^{:doc "Public record owner."}
        (def ^Int public-owner 1)
        (static-record Descriptor public-owner {:id "alpha"})
        (def private-owner 2)
        (static-record Descriptor private-owner {:id "private"})
        (export [Descriptor public-owner])
    "#;

const MACRO_SOURCE: &str = r#"
        (module sample.macros)

        (defn-for-syntax helper [value]
          (list 'inc value))
        (defn-for-syntax helper-two [value]
          (helper value))
        (defn-for-syntax unused-helper [value]
          (list 'ignore value))
        ^{:doc "Build a public pipeline."}
        (defmacro public-pipeline [value & steps]
          (helper-two value))
        (defmacro hidden-macro [value]
          (helper value))
        (export [public-pipeline])
    "#;

const OPERATOR_SOURCE: &str = r#"
        (module sample.operators)

        ^{:doc "A generic series fixture."}
        (defstruct (Series T)
          [values (Vector T)])

        ^{:doc "Multiply a series fixture." :osiris/operator :multiply}
        (defn ^{:type (Series Float)} multiply-series [^{:type (Series Float)} series ^Float multiplier]
          series)

        (export [Series multiply-series])
    "#;

fn modules() -> (ast::Module, hir::Module) {
    let surface = ast::lower_document(&source_reader::read(SOURCE));
    assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
    let typed = hir::lower_module(&surface.module, "sample.core");
    assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
    (surface.module, typed.module)
}

fn static_modules() -> (ast::Module, hir::Module) {
    let surface = ast::lower_document(&source_reader::read(STATIC_SOURCE));
    assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
    let typed = hir::lower_module(&surface.module, "sample.records");
    // Static schemas are represented by the surface/static pass.  The HIR
    // module remains sufficient for the exported record owner here.
    (surface.module, typed.module)
}

fn macro_modules(source: &str) -> (ast::Module, hir::Module) {
    let surface = ast::lower_document(&source_reader::read(source));
    assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
    let typed = hir::lower_module(&surface.module, "sample.macros");
    assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
    (surface.module, typed.module)
}

fn operator_modules(source: &str) -> (ast::Module, hir::Module) {
    let surface = ast::lower_document(&source_reader::read(source));
    assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
    let typed = hir::lower_module(&surface.module, "sample.operators");
    assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
    (surface.module, typed.module)
}

fn metadata_with_normalized_bytes(bytes: usize) -> Vec<MetadataEntry> {
    vec![MetadataEntry {
        key: keyword("x"),
        value: string(&"x".repeat(bytes.saturating_sub(7))),
    }]
}

fn metadata_map_source_with_normalized_bytes(bytes: usize) -> String {
    format!("{{:x \"{}\"}}", "x".repeat(bytes.saturating_sub(7)))
}

fn metadata_entries(count: usize) -> Vec<MetadataEntry> {
    (0..count)
        .map(|index| MetadataEntry {
            key: keyword(&format!("k{index}")),
            value: integer(u32::try_from(index).expect("small test metadata index")),
        })
        .collect()
}

fn metadata_map_source_entries(count: usize) -> String {
    let entries = (0..count)
        .map(|index| format!(":k{index} {index}"))
        .collect::<Vec<_>>()
        .join(" ");
    format!("{{{entries}}}")
}

fn clear_interface_metadata(interface: &mut super::Interface) {
    interface.metadata.clear();
    for binding in &mut interface.bindings {
        binding.metadata.clear();
    }
    for function in &mut interface.functions {
        for parameter in &mut function.parameters {
            parameter.metadata.clear();
        }
    }
    for structure in &mut interface.structs {
        for field in &mut structure.fields {
            field.metadata.clear();
        }
    }
}

fn set_function_metadata_target_count(
    interface: &mut super::Interface,
    metadata: &[MetadataEntry],
    target_count: usize,
) {
    let function = interface.functions.first_mut().expect("sample function");
    interface
        .bindings
        .iter_mut()
        .find(|binding| binding.id == function.binding)
        .expect("sample function binding")
        .metadata = metadata.to_vec();
    let mut template = function
        .parameters
        .first()
        .cloned()
        .expect("sample parameter");
    function.parameters.clear();
    for index in 0..target_count.saturating_sub(1) {
        template.id = format!("{}::resource-{index}", function.binding);
        template.canonical = format!("resource-{index}");
        template.metadata = metadata.to_vec();
        function.parameters.push(template.clone());
    }
}

fn set_binding_metadata_target_count(
    interface: &mut super::Interface,
    metadata: &[MetadataEntry],
    target_count: usize,
) {
    let template = interface.bindings.first().cloned().expect("sample binding");
    interface.bindings = (0..target_count)
        .map(|index| {
            let mut binding = template.clone();
            binding.id = format!("sample.core::value::resource-{index}");
            binding.canonical = format!("resource-{index}");
            binding.python = format!("resource_{index}");
            binding.metadata = metadata.to_vec();
            binding
        })
        .collect();
    interface.aliases.clear();
    interface.functions.clear();
    interface.structs.clear();
    interface.operator_instances.clear();
    interface.macros.clear();
    interface.phase_helpers.clear();
}

fn emit_source(source: &str, module: &str) -> String {
    let surface = ast::lower_document(&source_reader::read(source));
    assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
    let typed = hir::lower_module(&surface.module, module);
    assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
    emit(&typed.module, &surface.module).expect("test interface emits")
}

#[test]
fn canonical_interface_round_trips() {
    let (surface, typed) = modules();
    let encoded = emit(&typed, &surface).expect("interface should emit");
    let decoded = read(&encoded).expect("interface should read");
    assert_eq!(render(&decoded).unwrap(), encoded);
    assert!(
        decoded
            .aliases
            .iter()
            .any(|alias| alias.canonical == "距离")
    );
    assert!(decoded.aliases.iter().any(|alias| alias.canonical == "米"));
    assert_eq!(decoded.functions[0].parameters[0].aliases, ["点位"]);
    assert_eq!(decoded.structs[0].type_parameters, ["T"]);
    assert_eq!(decoded.structs[0].fields[1].aliases, ["最大值"]);
    assert!(!encoded.contains("private-value"));
}

#[test]
fn generic_function_metadata_survives_provisional_and_final_interfaces() {
    let source = r#"
            (module sample.generics)
            ^{:doc "Return an optional generic value." :type-params [A]}
            (defn ^{:type (Option A)} optional-id
              [[^{:type (Option A)} value = none]]
              value)
            (export [optional-id])
        "#;
    let surface = ast::lower_document(&source_reader::read(source));
    assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
    let provisional = super::build_provisional(&surface.module).expect("provisional interface");
    let typed = hir::lower_module(&surface.module, "sample.generics");
    assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
    let final_model = build(&typed.module, &surface.module).expect("final interface");
    super::validate_provisional_shape(&provisional, &final_model)
        .expect("generic provisional and final shapes agree");

    let encoded = render(&final_model).expect("generic interface renders");
    let decoded = read(&encoded).expect("generic interface reads");
    assert_eq!(render(&decoded).unwrap(), encoded);
    let binding = decoded
        .bindings
        .iter()
        .find(|binding| binding.canonical == "optional-id")
        .expect("generic binding");
    let Type::Fn(signature) = &binding.ty else {
        panic!("expected function type");
    };
    let [parameter] = signature.parameters.as_slice() else {
        panic!("expected one parameter");
    };
    assert!(matches!(
        (parameter, signature.return_type.as_ref()),
        (
            Type::Option(parameter),
            Type::Option(returned)
        ) if matches!(
            (parameter.as_ref(), returned.as_ref()),
            (Type::TypeVar(left), Type::TypeVar(right)) if left == right
        )
    ));
    assert!(encoded.contains(":type-params [A]"), "{encoded}");
}

#[test]
fn nominal_binding_identity_round_trips_and_legacy_short_ids_fail_closed() {
    let (surface, typed) = modules();
    let encoded = emit(&typed, &surface).expect("interface should emit");
    let decoded = read(&encoded).expect("interface should read");
    let range = decoded
        .bindings
        .iter()
        .find(|binding| binding.canonical == "Range")
        .expect("public Range binding");
    assert!(matches!(
        &range.ty,
        Type::Nominal { binding, .. } if binding == "sample.core::type::Range"
    ));
    assert!(
        encoded.contains("[:nominal \"sample.core::type::Range\""),
        "{encoded}"
    );

    let legacy = encoded.replacen(
        "[:nominal \"sample.core::type::Range\"",
        "[:nominal \"Range\"",
        1,
    );
    let error = read(&legacy).expect_err("legacy short nominal identity must be rejected");
    assert_eq!(error.code, "OSR-I0084");
}

#[test]
fn public_signature_cannot_leak_a_private_local_nominal_type() {
    let source = r#"
            (module sample.private-nominal)
            (defstruct Hidden [value Int])
            ^{:doc "Expose a hidden type."}
            (defn ^Hidden expose [^Hidden value] value)
            (export [expose])
        "#;
    let surface = ast::lower_document(&source_reader::read(source));
    assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
    let typed = hir::lower_module(&surface.module, "sample.private-nominal");
    assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
    let error = build(&typed.module, &surface.module)
        .expect_err("private nominal type must not leak through a public signature");
    assert_eq!(error.code, "OSR-I0084");
    assert!(error.message.contains("private or missing local type"));
}

#[test]
fn interface_metadata_target_boundary_is_accepted_and_overflow_fails_closed() {
    let (surface, typed) = modules();
    let mut interface = build(&typed, &surface).expect("base interface");
    interface.metadata =
        metadata_with_normalized_bytes(METADATA_TARGET_LIMITS.max_normalized_bytes);
    refresh_standalone_hashes(&mut interface).expect("refresh boundary hashes");
    render(&interface).expect("metadata byte boundary must be publishable");

    interface.metadata =
        metadata_with_normalized_bytes(METADATA_TARGET_LIMITS.max_normalized_bytes + 1);
    let error = render(&interface).expect_err("direct model must enforce target limit");
    assert_eq!(error.code, "OSR-I0082");
    assert!(error.message.contains("syntax target normalized byte size"));

    let encoded = emit(&typed, &surface).expect("valid base interface");
    let oversized =
        metadata_map_source_with_normalized_bytes(METADATA_TARGET_LIMITS.max_normalized_bytes + 1);
    let forged = encoded.replacen(":metadata {}", &format!(":metadata {oversized}"), 1);
    let error = read(&forged).expect_err("forged interface must enforce target limit");
    assert_eq!(error.code, "OSR-I0082");
}

#[test]
fn interface_metadata_aggregate_boundaries_are_enforced() {
    let entry_target = metadata_entries(METADATA_TARGET_LIMITS.max_entries);

    let (surface, typed) = modules();
    let mut declaration = build(&typed, &surface).expect("base interface");
    clear_interface_metadata(&mut declaration);
    set_function_metadata_target_count(&mut declaration, &entry_target, 4);
    validate_interface_metadata_resources(&declaration)
        .expect("four full targets equal the declaration entry boundary");

    set_function_metadata_target_count(&mut declaration, &entry_target, 5);
    let error = render(&declaration).expect_err("declaration aggregate must fail closed");
    assert_eq!(error.code, "OSR-I0082");
    assert!(error.message.contains("metadata declaration entry count"));

    let mut interface = build(&typed, &surface).expect("base interface");
    clear_interface_metadata(&mut interface);
    set_binding_metadata_target_count(&mut interface, &entry_target, 32);
    assert_eq!(
        32 * METADATA_TARGET_LIMITS.max_entries,
        METADATA_INTERFACE_LIMITS.max_entries
    );
    validate_interface_metadata_resources(&interface)
        .expect("32 full targets equal the interface entry boundary");

    set_binding_metadata_target_count(&mut interface, &entry_target, 33);
    let error = render(&interface).expect_err("interface aggregate must fail closed");
    assert_eq!(error.code, "OSR-I0082");
    assert!(error.message.contains("metadata interface entry count"));
}

#[test]
fn forged_interface_cannot_bypass_declaration_or_interface_totals() {
    let metadata = metadata_map_source_entries(METADATA_TARGET_LIMITS.max_entries);

    let parameters = (0..5)
        .map(|index| format!("^Int p{index}"))
        .collect::<Vec<_>>()
        .join(" ");
    let declaration_source = format!(
        "(module sample.metadata-declaration)\n\
             ^{{:doc \"Metadata declaration fixture.\"}}\n\
             (defn ^Int f [{parameters}] p0)\n\
             (export [f])"
    );
    let declaration_encoded = emit_source(&declaration_source, "sample.metadata-declaration");
    let declaration_forged = declaration_encoded.replace(
        ":metadata {:tag Int}",
        &format!(":metadata {metadata}"),
    );
    let error = read(&declaration_forged)
        .expect_err("forged declaration aggregate must be rejected before hashes");
    assert_eq!(error.code, "OSR-I0082");
    assert!(error.message.contains("metadata declaration entry count"));

    let definitions = (0..33)
        .map(|index| {
            format!("^{{:doc \"Metadata value.\"}} (def ^Int value{index} {index})")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let exports = (0..33)
        .map(|index| format!("value{index}"))
        .collect::<Vec<_>>()
        .join(" ");
    let interface_source =
        format!("(module sample.metadata-interface)\n{definitions}\n(export [{exports}])");
    let interface_encoded = emit_source(&interface_source, "sample.metadata-interface");
    let interface_forged = interface_encoded.replace(
        ":metadata {:doc \"Metadata value.\" :tag Int}",
        &format!(":metadata {metadata}"),
    );
    let error = read(&interface_forged)
        .expect_err("forged interface aggregate must be rejected before hashes");
    assert_eq!(error.code, "OSR-I0082");
    assert!(error.message.contains("metadata interface entry count"));
}

#[test]
fn literal_type_arguments_round_trip_and_change_semantic_hashes() {
    let source = r#"
            (module sample.literal-types)
            ^{:doc "An array fixture."}
            (defstruct (Array T Axes) [values Any])
            ^{:doc "A frame fixture."}
            (defstruct (Frame Schema KeyMarker KeyValue OrderMarker OrderValue)
              [values Any])
            ^{:doc "Return an array unchanged."}
            (defn ^{:type (Array Float [:time :feature])} array-id [^{:type (Array Float [:time :feature])} values]
              values)
            ^{:doc "Return a frame unchanged."}
            (defn ^{:type (Frame {:category Str :value Float :time Datetime}
                        :key [:time :category]
                        :order [:time])} frame-id [^{:type (Frame {:value Float :time Datetime :category Str}
                             :key [:time :category]
                             :order [:time])} frame]
              frame)
            (export [Array Frame array-id frame-id])
        "#;
    let surface = ast::lower_document(&source_reader::read(source));
    assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
    let typed = hir::lower_module(&surface.module, "sample.literal-types");
    assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
    let encoded = emit(&typed.module, &surface.module).expect("literal interface emits");
    let decoded = read(&encoded).expect("literal interface reads");
    assert_eq!(
        render(&decoded).expect("literal interface renders"),
        encoded
    );
    assert!(encoded.contains(":literal"), "{encoded}");
    assert_eq!(
        decoded.functions[0].parameters[0].ty,
        decoded.functions[0].return_type
    );
    assert_eq!(
        decoded.functions[1].parameters[0].ty,
        decoded.functions[1].return_type
    );

    let changed_source = source.replace(":feature", ":channel");
    let changed_surface = ast::lower_document(&source_reader::read(&changed_source));
    let changed_typed = hir::lower_module(&changed_surface.module, "sample.literal-types");
    let changed = read(
        &emit(&changed_typed.module, &changed_surface.module)
            .expect("changed literal interface emits"),
    )
    .expect("changed literal interface reads");
    assert_ne!(decoded.hashes.semantic_body, changed.hashes.semantic_body);
    assert_ne!(
        decoded.semantic_interface_hash(),
        changed.semantic_interface_hash()
    );
}
