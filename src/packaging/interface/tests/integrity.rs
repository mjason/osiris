#[test]
fn emission_is_byte_deterministic() {
    let (surface, typed) = modules();
    let first = emit(&typed, &surface).unwrap();
    let second = emit(&typed, &surface).unwrap();
    assert_eq!(first.as_bytes(), second.as_bytes());
    assert_eq!(first.lines().count(), 4);
}

#[test]
fn content_tampering_is_rejected() {
    let (surface, typed) = modules();
    let encoded = emit(&typed, &surface).unwrap();
    let tampered = encoded.replacen("\"distance\"", "\"changed\"", 1);
    assert!(matches!(
        read(&tampered).unwrap_err().code,
        "OSR-I0015" | "OSR-I0073" | "OSR-I0084"
    ));
}

#[test]
fn graph_envelope_tampering_is_rejected() {
    let (surface, typed) = modules();
    let encoded = emit(&typed, &surface).unwrap();
    let tampered = encoded.replacen(":group-id \"sample.core\"", ":group-id \"changed\"", 1);
    assert_eq!(read(&tampered).unwrap_err().code, "OSR-I0073");
}

#[test]
fn public_static_schema_and_owned_record_round_trip() {
    let (surface, typed) = static_modules();
    let encoded = emit(&typed, &surface).expect("static interface should emit");
    let decoded = read(&encoded).expect("static interface should read");

    assert_eq!(decoded.static_schemas.len(), 1);
    assert_eq!(decoded.static_schemas[0].name, "Descriptor");
    assert_eq!(decoded.owned_records.len(), 1);
    assert_eq!(decoded.owned_records[0].owner_name, "public-owner");
    assert_eq!(render(&decoded).unwrap(), encoded);
    assert!(encoded.contains(":static-schemas"));
    assert!(encoded.contains(":owned-records"));

    // Distribution/provider records remain sidecar data. The compilation
    // interface graph hashes are published in a separate, non-recursive
    // section and therefore do not alter the semantic body hash.
    assert!(!encoded.contains(":distribution"));
    assert!(!encoded.contains(":interface-member-id"));
    assert!(encoded.contains(":semantic-interface-hash"));
}

#[test]
fn private_static_declarations_are_filtered() {
    let (surface, typed) = static_modules();
    let encoded = emit(&typed, &surface).unwrap();
    let decoded = read(&encoded).unwrap();

    assert!(
        decoded
            .static_schemas
            .iter()
            .all(|schema| schema.name != "PrivateSchema")
    );
    assert!(
        decoded
            .owned_records
            .iter()
            .all(|record| record.owner_name != "private-owner")
    );
    assert!(!encoded.contains("sample/private"));
    assert!(!encoded.contains("private-owner"));
}

#[test]
fn static_payload_tampering_is_rejected() {
    let (surface, typed) = static_modules();
    let encoded = emit(&typed, &surface).unwrap();

    let schema_tamper = encoded.replacen("sample/descriptor", "sample/changed", 1);
    assert!(matches!(
        read(&schema_tamper).unwrap_err().code,
        "OSR-I0015" | "OSR-I0056" | "OSR-I0057"
    ));

    let record_tamper = encoded.replacen("\"alpha\"", "\"omega\"", 1);
    assert!(matches!(
        read(&record_tamper).unwrap_err().code,
        "OSR-I0015" | "OSR-I0057"
    ));

    let changed_source = STATIC_SOURCE.replacen("\"alpha\"", "\"omega\"", 1);
    let changed_surface = ast::lower_document(&source_reader::read(&changed_source));
    assert!(changed_surface.diagnostics.is_empty());
    let changed_typed = hir::lower_module(&changed_surface.module, "sample.records");
    let changed = read(
        &emit(&changed_typed.module, &changed_surface.module)
            .expect("changed static interface should emit"),
    )
    .unwrap();
    let original = read(&encoded).unwrap();
    assert_ne!(
        changed.hashes.interface_body,
        original.hashes.interface_body
    );
    assert_ne!(changed.hashes.semantic_body, original.hashes.semantic_body);
}
