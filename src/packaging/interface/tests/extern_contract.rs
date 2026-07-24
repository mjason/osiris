#[test]
fn exported_extern_contract_round_trips_without_gaining_trust() {
    let source = r#"
            (module sample.externs)
            ^{:doc "A series fixture."}
            (defstruct Series [values Any])
            (extern python "host.series"
              ^{:doc "Apply a rolling operation."}
              (defn ^Series rolling [^Series values ^Int window]
                :contract
                {:id "host.series/rolling-v1"
                 :effects :pure
                 :temporal {:past "2*(window-1)"
                            :future 0
                            :availability :published}
                 :data {:axes [:time]
                        :alignment :labelled
                        :preserves-length true}})
              ^{:doc "Call a dynamic operation."}
              (defn ^Int dynamic [^Int value]))
            (export [Series rolling dynamic])
        "#;
    let surface = ast::lower_document(&source_reader::read(source));
    assert!(surface.diagnostics.is_empty(), "{:?}", surface.diagnostics);
    let typed = hir::lower_module(&surface.module, "sample.externs");
    assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
    let encoded = emit(&typed.module, &surface.module).expect("interface should emit");
    let decoded = read(&encoded).expect("interface should read");

    let rolling = decoded
        .functions
        .iter()
        .find(|function| function.contract_id.as_deref() == Some("host.series/rolling-v1"))
        .expect("declared extern contract");
    assert_eq!(
        rolling.summaries.temporal.past,
        TemporalBound::Symbolic("2*(window-1)".to_owned())
    );
    assert_eq!(rolling.summaries.temporal.future, TemporalBound::Finite(0));
    assert_eq!(
        rolling.summaries.temporal.availability,
        Availability::Named("published".to_owned())
    );
    assert_eq!(rolling.summaries.data.preserves_length, Some(true));

    let dynamic = decoded
        .functions
        .iter()
        .find(|function| function.contract_id.is_none())
        .expect("uncontracted extern remains represented");
    assert!(dynamic.summaries.effects.open);
    assert_eq!(dynamic.summaries.temporal.future, TemporalBound::Unknown);
    assert_eq!(render(&decoded).unwrap(), encoded);
}
