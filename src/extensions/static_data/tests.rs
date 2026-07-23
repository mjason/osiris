use std::collections::BTreeMap;

use super::*;
use crate::{ast::lower_document, hir, interface, reader::read};

fn lower(source: &str) -> ast::Module {
    let document = read(source);
    let result = lower_document(&document);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    result.module
}

fn sample_module() -> ast::Module {
    lower(
        r#"(module example)
               (export [owner S])
               (defstatic-schema S
                 :schema-id "example/schema"
                 :version 1
                 :fields {:id {:type Str :required true}
                          :tags {:type (Vector Str) :default []}}
                 :indexes [{:id "example/id"
                            :keys [{:field :id :role :canonical}]}])
               (def owner none)
               (static-record S owner {:id "alpha"})"#,
    )
}

fn dependency_interface_named(module_name: &str) -> interface::Interface {
    let source = format!(
        r#"(module {module_name})
               (defstatic-schema Descriptor
                 :schema-id "dep/descriptor"
                 :version 1
                 :fields {{:id {{:type Str :required true}}}})
               (alias SchemaAlias Descriptor)
               (def owner none)
               (export [Descriptor SchemaAlias])"#
    );
    let surface = lower(&source);
    let typed = hir::lower_module(&surface, module_name);
    assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
    let encoded = interface::emit(&typed.module, &surface).expect("dependency .osri");
    interface::read(&encoded).expect("dependency .osri should validate")
}

fn dependency_interface() -> interface::Interface {
    dependency_interface_named("dep.schemas")
}

fn analyze_imported(source: &str) -> StaticModuleData {
    let module = lower(source);
    let dependency = dependency_interface();
    let interfaces = BTreeMap::from([(dependency.module.clone(), dependency)]);
    analyze_module_with_interfaces(&module, &interfaces)
}

#[test]
fn datum_encoding_is_tagged_and_preserves_float_bits() {
    let datum = StaticDatum::Float((-0.0f64).to_bits());
    assert_eq!(
        String::from_utf8(datum.canonical_bytes()).expect("JSON"),
        r#"{"$osiris":"float","value":"8000000000000000"}"#
    );
    let integer = StaticDatum::Int("0007".to_owned()).canonicalize().unwrap();
    assert_eq!(integer, StaticDatum::Int("7".to_owned()));
}

#[test]
fn duplicate_map_keys_and_set_items_are_rejected() {
    let map = StaticDatum::Map(vec![
        (
            StaticDatum::Keyword(":a".to_owned()),
            StaticDatum::Int("1".to_owned()),
        ),
        (
            StaticDatum::Keyword(":a".to_owned()),
            StaticDatum::Int("2".to_owned()),
        ),
    ]);
    assert!(map.canonicalize().is_err());
    let set = StaticDatum::Set(vec![StaticDatum::Bool(true), StaticDatum::Bool(true)]);
    assert!(set.canonicalize().is_err());
}

#[test]
fn runtime_call_is_not_static_data() {
    let module = lower("(foo 1)");
    let ast::ItemKind::Expr(expression) = &module.items[0].kind else {
        panic!("expected expression");
    };
    let error = StaticDatum::from_expr(expression).expect_err("call must fail");
    assert_eq!(error.code, RECORD_INVALID_DATUM);
}

#[test]
fn schema_defaults_types_and_index_claims_validate() {
    let module = sample_module();
    let data = analyze_module(&module);
    assert!(data.diagnostics.is_empty(), "{:?}", data.diagnostics);
    assert_eq!(data.schemas.len(), 1);
    assert_eq!(data.records.len(), 1);
    let record = &data.records[0];
    assert!(record.public);
    assert_eq!(record.fields.len(), 2, "default should be materialized");
    assert_eq!(record.index_claims[0].normalized_key, "alpha");
}

#[test]
fn imported_qualified_schema_uses_provider_binding_identity() {
    let data = analyze_imported(
        r#"(module app.records)
               (import dep.schemas :as dep)
               (def owner none)
               (export [owner])
               (static-record dep/Descriptor owner {:id "alpha"})"#,
    );
    assert!(data.diagnostics.is_empty(), "{:?}", data.diagnostics);
    assert_eq!(data.records.len(), 1);
    assert_eq!(
        data.records[0].schema.binding_id,
        "dep.schemas::type::Descriptor"
    );
    assert!(data.records[0].public);
}

#[test]
fn imported_schema_alias_and_refer_resolve_to_the_same_identity() {
    let data = analyze_imported(
        r#"(module app.records)
               (import dep.schemas :refer [SchemaAlias])
               (def owner none)
               (static-record SchemaAlias owner {:id "alpha"})"#,
    );
    assert!(data.diagnostics.is_empty(), "{:?}", data.diagnostics);
    assert_eq!(data.records.len(), 1);
    assert_eq!(
        data.records[0].schema.binding_id,
        "dep.schemas::type::Descriptor"
    );

    let qualified = analyze_imported(
        r#"(module app.records)
               (import dep.schemas :as dep)
               (def owner none)
               (static-record dep/SchemaAlias owner {:id "alpha"})"#,
    );
    assert!(
        qualified.diagnostics.is_empty(),
        "{:?}",
        qualified.diagnostics
    );
    assert_eq!(
        qualified.records[0].schema.binding_id,
        data.records[0].schema.binding_id
    );
}

#[test]
fn imported_missing_or_private_schema_fails_closed() {
    let missing = analyze_imported(
        r#"(module app.records)
               (import dep.schemas :as dep)
               (def owner none)
               (static-record dep/Missing owner {:id "alpha"})"#,
    );
    assert!(missing.records.is_empty());
    assert!(missing.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == RECORD_RECORD_SHAPE && diagnostic.message.contains("unresolved schema")
    }));

    let private = analyze_imported(
        r#"(module app.records)
               (import dep.schemas :as dep)
               (def owner none)
               (static-record dep/owner owner {:id "alpha"})"#,
    );
    assert!(private.records.is_empty());
    assert!(!private.diagnostics.is_empty());
}

#[test]
fn conflicting_referred_schemas_are_rejected() {
    let first = dependency_interface();
    let second = dependency_interface_named("other.schemas");

    let module = lower(
        r#"(module app.records)
               (import dep.schemas :refer [Descriptor])
               (import other.schemas :refer [Descriptor])
               (def owner none)
               (static-record Descriptor owner {:id "alpha"})"#,
    );
    let interfaces = BTreeMap::from([
        (first.module.clone(), first),
        (second.module.clone(), second),
    ]);
    let data = analyze_module_with_interfaces(&module, &interfaces);
    assert!(data.records.is_empty());
    assert!(data.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == RECORD_RECORD_SHAPE && diagnostic.message.contains("conflicting")
    }));
}

#[test]
fn conflicting_module_aliases_are_rejected_even_for_disjoint_schema_names() {
    let first = dependency_interface();
    let second = dependency_interface_named("other.schemas");
    let module = lower(
        r#"(module app.records)
               (import dep.schemas :as dep)
               (import other.schemas :as dep)
               (def owner none)
               (static-record dep/Descriptor owner {:id "alpha"})"#,
    );
    let interfaces = BTreeMap::from([
        (first.module.clone(), first),
        (second.module.clone(), second),
    ]);
    let data = analyze_module_with_interfaces(&module, &interfaces);
    assert!(data.records.is_empty());
    assert!(data.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == RECORD_RECORD_SHAPE && diagnostic.message.contains("conflicting")
    }));
}

#[test]
fn imported_schema_cannot_use_an_imported_owner() {
    let data = analyze_imported(
        r#"(module app.records)
               (import dep.schemas :as dep)
               (def owner none)
               (static-record dep/Descriptor dep/owner {:id "alpha"})"#,
    );
    assert!(data.records.is_empty());
    assert!(data.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("owner `dep/owner` is not a top-level declaration")
    }));
}

#[test]
fn private_owner_is_filtered_from_public_records() {
    let module = lower(
        r#"(module example)
               (defstatic-schema S :schema-id "example/schema" :version 1
                 :fields {:id {:type Str :required true}})
               (def owner none)
               (static-record S owner {:id "alpha"})"#,
    );
    let data = analyze_module(&module);
    assert!(data.diagnostics.is_empty(), "{:?}", data.diagnostics);
    assert_eq!(data.public_records().len(), 0);
}

fn fake_indexed(distribution: &str, key: &str, path: &[&str]) -> IndexedRecord {
    let schema = StaticSchema {
        name: "S".to_owned(),
        schema_id: "example/schema".to_owned(),
        version: 1,
        fields: Vec::new(),
        indexes: Vec::new(),
        body_hash: "sha256:schema".to_owned(),
    };
    let claim = IndexClaim {
        index_id: "example/index".to_owned(),
        projection_field: ":id".to_owned(),
        projection_role: "canonical".to_owned(),
        key: StaticDatum::Str(key.to_owned()),
        normalized_key: key.to_owned(),
        raw_spelling: Some(key.to_owned()),
    };
    let record = ValidatedRecord {
        schema: schema.identity("example::type::S"),
        owner_binding_id: format!("example::value::{distribution}"),
        owner_name: distribution.to_owned(),
        module: "example".to_owned(),
        public: true,
        stable_record_id: format!("sha256:{distribution}"),
        record_body_hash: format!("sha256:body-{distribution}"),
        fields: Vec::new(),
        index_claims: vec![claim],
        origin: RecordOrigin {
            module: "example".to_owned(),
            span: Span::default(),
            macro_origin: None,
        },
    };
    IndexedRecord {
        occurrence: record.occurrence("pkg", "1", distribution, "sha256:iface"),
        record,
        dependency_path: path.iter().map(|value| (*value).to_owned()).collect(),
    }
}

#[test]
fn index_merge_deduplicates_exact_diamond_occurrence() {
    let first = fake_indexed("owner", "alpha", &["root", "a"]);
    let mut second = first.clone();
    second.dependency_path = vec!["root".to_owned(), "b".to_owned()];
    let merged = merge_unique_indexes(vec![second, first]).expect("same occurrence dedupes");
    assert_eq!(merged.claims.len(), 1);
    assert!(merged.effective_record_index_hash.starts_with("sha256:"));
}

#[test]
fn index_merge_reports_conflicts_independent_of_traversal_order() {
    let first = fake_indexed("owner-a", "alpha", &["z"]);
    let second = fake_indexed("owner-b", "alpha", &["a"]);
    let left = merge_unique_indexes(vec![first.clone(), second.clone()]).expect_err("conflict");
    let right = merge_unique_indexes(vec![second, first]).expect_err("conflict");
    assert_eq!(left[0].message, right[0].message);
    assert_eq!(left[0].code, RECORD_INDEX_CONFLICT);
}

#[test]
fn sidecar_round_trip_and_tamper_check() {
    let module = sample_module();
    let data = analyze_module(&module);
    let record = data.records[0].clone();
    let indexed = IndexedRecord {
        occurrence: record.occurrence("example", "0.1", "example::owner", "sha256:iface"),
        record,
        dependency_path: Vec::new(),
    };
    let encoded = encode_sidecar(vec!["sha256:iface".to_owned()], vec![indexed.clone()]).unwrap();
    let decoded = decode_sidecar(&encoded.bytes, Some(&encoded.records_hash)).unwrap();
    assert_eq!(decoded, encoded.sidecar);
    let mut tampered = encoded.bytes.clone();
    let index = tampered.iter().position(|byte| *byte == b'a').unwrap();
    tampered[index] = b'b';
    assert!(decode_sidecar(&tampered, Some(&encoded.records_hash)).is_err());
    verify_sidecar_against_records(
        &encoded.bytes,
        Some(&encoded.records_hash),
        &["sha256:iface".to_owned()],
        &[indexed],
    )
    .unwrap();
}

#[test]
fn duplicate_json_member_is_rejected_before_hash_validation() {
    let json = br#"{"format-version":1,"format-version":1,"interface-semantic-hashes":[],"record-identities":[],"record-set-hash":"sha256:00","records":[]}"#;
    let error = decode_sidecar(json, None).expect_err("duplicate member");
    assert_eq!(error.code, RECORD_SIDECAR);
    assert!(error.message.contains("duplicate"));
}
