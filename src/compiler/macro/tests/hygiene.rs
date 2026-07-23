#[test]
fn phase_one_metadata_limits_accept_boundary_and_recover_overflow() {
    let boundary_payload = "x".repeat(
        METADATA_TARGET_LIMITS
            .max_normalized_bytes
            .saturating_sub(7),
    );
    let boundary = format!(
        "(defmacro annotate [target] (with-meta target {{:x \"{boundary_payload}\"}}))\n\
             (annotate value)"
    );
    let result = expand(&read(&boundary), ExpansionOptions::default());
    assert!(
        result.document.diagnostics.is_empty(),
        "{:?}",
        result.document.diagnostics
    );
    assert_eq!(result.document.forms[1].metadata.len(), 1);

    let overflow_payload = format!("{boundary_payload}x");
    let overflow = format!(
        "(defmacro annotate [target] (with-meta target {{:x \"{overflow_payload}\"}}))\n\
             (annotate value)\n\
             (def after-overflow 1)"
    );
    let result = expand(&read(&overflow), ExpansionOptions::default());
    assert!(result.document.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "OSR-M0009" && diagnostic.message.contains("normalized byte size")
    }));
    assert!(result.document.forms.iter().any(|form| {
            matches!(
                &form.kind,
                FormKind::List(items)
                    if items.get(1).is_some_and(|name| {
                        matches!(&name.kind, FormKind::Symbol(name) if name.canonical == "after-overflow")
                    })
            )
        }));
}

#[test]
fn expands_custom_variadic_macro_with_splicing() {
    let source = "(defmacro unless [condition & body]\n  `(if (not ~condition) (do ~@body) none))\n(unless ready? (run) (log))";
    let output = expanded(source);
    assert!(output.ends_with("(if (not ready?) (do (run) (log)) none)\n"));
}

#[test]
fn supports_vector_destructuring_in_macro_parameters() {
    let source = "(defmacro swap [[left right]] `[~right ~left])\n(swap [1 2])";
    assert!(expanded(source).ends_with("[2 1]\n"));
}

#[test]
fn supports_clojure_map_destructuring_and_evaluated_defaults() {
    let source = "(defmacro configure [{:keys [name missing] :or {missing (+ 1 2)} :as all}]\n  `(result ~name ~missing ~(count all)))\n(configure {:name 7})";
    assert!(expanded(source).ends_with("(result 7 3 1)\n"));
}

#[test]
fn supports_nested_and_key_typed_map_destructuring() {
    let source = "(defmacro unpack [{{:keys [value]} :payload\n                         :strs [label]\n                         :syms [token]\n                         :keys [component/id]}]\n  `(result ~value ~label ~token ~id))\n(unpack {:payload {:value 9} \"label\" 2 token 3 :component/id 4})";
    assert!(expanded(source).ends_with("(result 9 2 3 4)\n"));
}

#[test]
fn rejects_defaults_for_unknown_map_bindings() {
    let source = "(defmacro broken [{:keys [known] :or {typo 1}}] known)\n(broken {:known 2})";
    let result = expand(&read(source), ExpansionOptions::default());
    assert!(result.document.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "OSR-M0003"
            && diagnostic
                .message
                .contains("unknown destructured binding `typo`")
    }));
}

#[test]
fn phase_functions_are_recursive_and_deterministic() {
    let source = "(defn-for-syntax repeat-forms [n form]\n  (if (= n 0) '() (cons form (repeat-forms (- n 1) form))))\n(defmacro repeat [n form] `(do ~@(repeat-forms n form)))\n(repeat 3 (tick))";
    let output = expanded(source);
    assert!(
        output.ends_with("(do (tick) (tick) (tick))\n"),
        "unexpected expansion:\n{output}"
    );
}

#[test]
fn auto_gensym_has_stable_spelling_and_unforgeable_identity() {
    let source = "(defmacro twice [expr]\n  `(let [value# ~expr] (+ value# value#)))\n(twice value__osr_g0)\n(twice value__osr_g0)";
    let result = expand(&read(source), ExpansionOptions::default());
    assert!(result.document.diagnostics.is_empty());
    let first = gensym_occurrences(&result.document.forms[1]);
    let second = gensym_occurrences(&result.document.forms[2]);
    assert_eq!(first.len(), 3);
    assert!(first.iter().all(|name| name == &first[0]));
    assert!(second.iter().all(|name| name == &second[0]));
    assert_ne!(first[0], second[0]);
    assert!(first[0].starts_with('\0'));

    let FormKind::List(first_call) = &result.document.forms[1].kind else {
        panic!("macro should expand to let");
    };
    let FormKind::Vector(bindings) = &first_call[1].kind else {
        panic!("let should contain bindings");
    };
    let FormKind::Symbol(caller_name) = &bindings[1].kind else {
        panic!("caller expression should remain a symbol");
    };
    assert_eq!(caller_name.canonical, "value__osr_g0");
    assert_ne!(caller_name.canonical, first[0]);
}

#[test]
fn explicit_gensym_is_shared_through_unquotes() {
    let source = "(defmacro hold [expr]\n  (let [binding (gensym \"held\")]\n    `(let [~binding ~expr] ~binding)))\n(hold value)";
    let result = expand(&read(source), ExpansionOptions::default());
    assert!(result.document.diagnostics.is_empty());
    let names = gensym_occurrences(&result.document.forms[1]);
    assert_eq!(names.len(), 2);
    assert_eq!(names[0], names[1]);
}

#[test]
fn rich_metadata_is_visible_and_purely_updated_at_phase_one() {
    let source = "(defmacro mark []\n  (vary-meta `(def generated 1) assoc :expanded true))\n^:caller (mark)";
    let result = expand(&read(source), ExpansionOptions::default());
    assert!(result.document.diagnostics.is_empty());
    let metadata = &result.document.forms[1].metadata;
    assert!(metadata.iter().any(|entry| {
        matches!(&entry.key.kind, FormKind::Keyword(name) if name.canonical == ":expanded")
            && matches!(entry.value.kind, FormKind::Bool(true))
    }));
    assert!(metadata.iter().any(|entry| {
        matches!(&entry.key.kind, FormKind::Keyword(name) if name.canonical == ":caller")
    }));
}

#[test]
fn ampersand_form_exposes_call_metadata() {
    let source = "(defmacro copy-call-meta []\n  (with-meta `(def generated 1) (meta &form)))\n^{:agent/view \"中文\"} (copy-call-meta)";
    let result = expand(&read(source), ExpansionOptions::default());
    assert!(result.document.diagnostics.is_empty());
    assert!(result.document.forms[1].metadata.iter().any(|entry| {
        matches!(&entry.key.kind, FormKind::Keyword(name) if name.canonical == ":agent/view")
            && matches!(&entry.value.kind, FormKind::String(value) if value == "中文")
    }));
}

#[test]
fn syntax_error_uses_a_stable_macro_diagnostic() {
    let source =
        "(defmacro reject [] (syntax-error &form \"rejected by macro\"))\n(reject)\n(def okay 1)";
    let result = expand(&read(source), ExpansionOptions::default());
    assert!(result.document.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "OSR-M0007" && diagnostic.message == "rejected by macro"
    }));
    assert_eq!(result.document.forms.len(), 3);
}

#[test]
fn phase_recursion_limit_is_recoverable() {
    let source = "(defn-for-syntax forever [] (forever))\n(defmacro broken [] (forever))\n(broken)\n(def okay 1)";
    let result = expand(&read(source), ExpansionOptions::default());
    assert!(
        result
            .document
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-M0005")
    );
    assert_eq!(result.document.forms.len(), 4);
}

#[test]
fn nested_macro_trace_preserves_the_origin_chain() {
    let source = "(defmacro inner [] `(+ 1 2))\n(defmacro outer [] `(inner))\n(outer)";
    let result = expand(&read(source), ExpansionOptions::default());
    assert!(result.document.diagnostics.is_empty());
    assert_eq!(
        result
            .traces
            .iter()
            .map(|trace| trace.macro_name.as_str())
            .collect::<Vec<_>>(),
        vec!["outer", "inner"]
    );
    assert_eq!(result.traces[0].origin.len(), 1);
    assert_eq!(result.traces[1].origin.len(), 2);
}

#[test]
fn hygienic_macro_output_reaches_readable_python_codegen() {
    let source = "(module macro-demo)\n(defmacro twice [expr]\n  `(let [value# ~expr] (+ value# value#)))\n(defn twice-value [[value Int]] -> Int\n  (twice value))";
    let result = compile(
        source,
        &CompileOptions::new("macro-demo", PythonVersion::default()),
    );
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let generated = result.python.expect("macro should compile");
    let python = &generated.source;
    assert!(python.contains("def twice_value(value: int) -> int:"));
    assert!(python.contains("_u0_osr_gensym"));
}

#[test]
fn control_prelude_reaches_typed_readable_python() {
    let source = r#"(module control-demo)
            (defn choose [[first Bool] [second Bool]] -> Bool
              (cond first true second false :else (and first second)))
            (defn maybe [[ready Bool]] -> (Option Bool)
              (when ready true))"#;
    let result = compile(
        source,
        &CompileOptions::new("control-demo", PythonVersion::default()),
    );
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let python = &result
        .python
        .expect("control prelude should compile")
        .source;
    assert!(python.contains("def choose(first: bool, second: bool) -> bool:"));
    assert!(python.contains("def maybe(ready: bool) -> Optional[bool]:"));
    assert!(python.contains("if _u0_osiris_truthy(first):"));
    assert!(python.contains("if _u0_osiris_truthy(ready):"));
}

#[test]
fn for_macro_reaches_typed_runtime_mapv() {
    let source = r#"(module control-map)
            (defn increment-all [[items (Vector Int)]] -> (Vector Int)
              (for [item items] (+ item 1)))"#;
    let result = compile(
        source,
        &CompileOptions::new("control-map", PythonVersion::default()),
    );
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let python = &result
        .python
        .expect("for should compile through mapv")
        .source;
    assert!(python.contains("from osiris.prelude import mapv as _u0_osiris_mapv"));
    assert!(python.contains("def increment_all(items: tuple[int, ...]) -> tuple[int, ...]:"));
    assert!(python.contains("_u0_osiris_mapv(lambda item: item + 1, items)"));

    let destructured = compile(
        r#"(module control-map-pattern)
               (defn values [[items (Vector (Map Str Int))]] -> (Vector Int)
                 (for [{:keys [value]} items] value))"#,
        &CompileOptions::new("control-map-pattern", PythonVersion::default()),
    );
    assert!(
        destructured.analysis.diagnostics.is_empty(),
        "{:?}",
        destructured.analysis.diagnostics
    );
    let python = &destructured
        .python
        .expect("destructured for should compile")
        .source;
    assert!(python.contains("[\"value\"]"));
    assert!(python.contains("_u0_osiris_mapv"));
}

#[test]
fn multi_clause_for_reaches_typed_runtime_collections() {
    let source = r#"(module control-comprehension)
            (defn cartesian-sums [[lefts (Vector Int)] [rights (Vector Int)]] -> (Vector Int)
              (for [left lefts right rights]
                (+ left right)))
            (defn selected-sums [[lefts (Vector Int)] [rights (Vector Int)]] -> (Vector Int)
              (for [left lefts
                    right rights
                    :let [sum (+ left right)]
                    :when (> sum 2)]
                sum))"#;
    let result = compile(
        source,
        &CompileOptions::new("control-comprehension", PythonVersion::default()),
    );
    assert!(
        result.analysis.diagnostics.is_empty(),
        "{:?}",
        result.analysis.diagnostics
    );
    let python = &result
        .python
        .expect("multi-clause for should compile")
        .source;
    assert!(python.contains("mapcatv as _u0_osiris_mapcatv"), "{python}");
    assert!(python.contains("mapv as _u0_osiris_mapv"), "{python}");
    assert!(python.matches("_u0_osiris_mapcatv(").count() >= 3);
    assert!(python.contains("_u0_osiris_mapv("));
}

fn gensym_occurrences(form: &Form) -> Vec<String> {
    let mut names = Vec::new();
    collect_gensyms(form, &mut names);
    names
}

fn collect_gensyms(form: &Form, names: &mut Vec<String>) {
    match &form.kind {
        FormKind::Symbol(name) if name.canonical.starts_with('\0') => {
            names.push(name.canonical.clone());
        }
        FormKind::List(items)
        | FormKind::Vector(items)
        | FormKind::Map(items)
        | FormKind::Set(items) => {
            for item in items {
                collect_gensyms(item, names);
            }
        }
        FormKind::ReaderMacro { form, .. } => collect_gensyms(form, names),
        _ => {}
    }
}
