fn interface_error(source: &str) -> super::InterfaceError {
    let surface = ast::lower_document(&source_reader::read(source));
    assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
    let typed = hir::lower_module(&surface.module, "metadata.contract");
    assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
    build(&typed.module, &surface.module).expect_err("metadata contract must fail")
}

fn emitted(source: &str) -> super::Interface {
    let surface = ast::lower_document(&source_reader::read(source));
    assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
    let typed = hir::lower_module(&surface.module, "metadata.contract");
    assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
    read(&emit(&typed.module, &surface.module).expect("metadata interface emits"))
        .expect("metadata interface reads")
}

#[test]
fn exported_declarations_require_authored_documentation() {
    let error = interface_error(
        "(module metadata.contract) (defn ^Int value [] 1) (export [value])",
    );
    assert_eq!(error.code, "OSR-I0087");
    assert!(error.message.contains("exported declaration `value`"));
}

#[test]
fn documentation_maps_require_non_empty_default_and_canonical_locales() {
    for (metadata, message) in [
        (r#"{:doc ""}"#, "must not be empty"),
        (
            r#"{:doc {"zh-CN" "文档"}}"#,
            "requires a `:default` entry",
        ),
        (
            r#"{:doc {:default "Value." "not_a_locale" "Invalid."}}"#,
            "well-formed BCP 47",
        ),
        (
            r#"{:doc {:default "Value." "en-us" "One." "en-US" "Two."}}"#,
            "after BCP 47 normalization",
        ),
    ] {
        let source = format!(
            "(module metadata.contract) ^{metadata} (defn ^Int value [] 1) (export [value])"
        );
        let error = interface_error(&source);
        assert_eq!(error.code, "OSR-I0085");
        assert!(error.message.contains(message), "{}", error.message);
    }
}

#[test]
fn localized_name_tables_reject_malformed_and_duplicate_names() {
    for (names, message) in [
        (
            r#"{:default {:preferred value-localized}}"#,
            "keys must be BCP 47 locale strings",
        ),
        (
            r#"{"en" {:preferred value-localized :unknown []}}"#,
            "permits only",
        ),
        (
            r#"{"en" {:preferred repeated :aliases [repeated]}}"#,
            "duplicated after NFC normalization",
        ),
        (
            r#"{"fr" {:preferred value}}"#,
            "repeats canonical name",
        ),
        (
            "{\"en\" {:preferred e\u{301}} \"fr\" {:preferred é}}",
            "duplicated after NFC normalization",
        ),
    ] {
        let source = format!(
            "(module metadata.contract) ^{{:doc \"Value.\" :osiris/names {names}}} (defn ^Int value [] 1) (export [value])"
        );
        let error = interface_error(&source);
        assert_eq!(error.code, "OSR-I0086");
        assert!(error.message.contains(message), "{}", error.message);
    }
}

#[test]
fn locale_keys_are_canonicalized_in_published_interfaces() {
    let interface = emitted(
        r#"(module metadata.contract)
           ^{:doc {:default "Value." "zh-cn" "值。"}
             :osiris/names {"en-us" {:preferred localized-value}}}
           (defn ^Int value [] 1)
           (export [value])"#,
    );
    let rendered = super::render(&interface).expect("canonical interface renders");
    assert!(rendered.contains(r#""zh-CN" "值。""#), "{rendered}");
    assert!(rendered.contains(r#""en-US""#), "{rendered}");
    assert!(!rendered.contains("zh-cn"), "{rendered}");
    assert!(!rendered.contains("en-us"), "{rendered}");
}

#[test]
fn documentation_translations_are_tooling_only_but_names_are_semantic() {
    let base = emitted(
        r#"(module metadata.contract)
           ^{:doc "Value."} (defn ^Int value [] 1)
           (export [value])"#,
    );
    let translated = emitted(
        r#"(module metadata.contract)
           ^{:doc {:default "Value." "zh-CN" "值。"}} (defn ^Int value [] 1)
           (export [value])"#,
    );
    let localized = emitted(
        r#"(module metadata.contract)
           ^{:doc "Value." :osiris/names {"zh-CN" {:preferred 值}}}
           (defn ^Int value [] 1)
           (export [value])"#,
    );
    assert_eq!(base.hashes.semantic_body, translated.hashes.semantic_body);
    assert_ne!(base.hashes.tooling_body, translated.hashes.tooling_body);
    assert_ne!(base.hashes.semantic_body, localized.hashes.semantic_body);
}
