#[test]
fn thread_first_inserts_after_each_call_head() {
    assert_eq!(expanded("(-> value (f 1) g)"), "(g (f value 1))\n");
}

#[test]
fn thread_last_inserts_at_the_end() {
    assert_eq!(
        expanded("(->> values (map f) (reduce +))"),
        "(reduce + (map f values))\n"
    );
}

#[test]
fn phase_one_reduce_honors_reduced_values() {
    let output = expanded(
        r#"(defmacro prefix [& values]
                  (reduce
                    (fn [result value]
                      (if (= value :stop)
                        (reduced result)
                        (conj result value)))
                    []
                    values))
                (prefix 1 2 :stop 3 4)"#,
    );
    assert!(output.ends_with("[1 2]\n"), "{output}");

    let helpers = expanded(
        r#"(defmacro marker-state []
                  (let [marker (reduced :done)]
                    [(reduced? marker) (unreduced marker)]))
                (marker-state)"#,
    );
    assert!(helpers.ends_with("[true :done]\n"), "{helpers}");
}

#[test]
fn cond_thread_uses_generated_let_bindings() {
    let output = expanded("(cond-> value ready? (normalize 1) final? finish)");
    assert!(output.contains("thread__osr_g0"));
    assert!(output.contains("thread__osr_g1"));
    assert!(output.contains(
        "(if (osiris.prelude/truthy* ready?) (normalize thread__osr_g0 1) thread__osr_g0)"
    ));
    assert!(
        output.contains(
            "(if (osiris.prelude/truthy* final?) (finish thread__osr_g1) thread__osr_g1)"
        )
    );
}

#[test]
fn extended_threading_and_object_macros_preserve_single_evaluation() {
    let conditional = expanded("(cond->> (load) ready? (map normalize) final? (reduce combine))");
    assert_eq!(conditional.matches("(load)").count(), 1);
    assert!(conditional.contains("(map normalize thread__osr_g0)"));
    assert!(conditional.contains("(reduce combine thread__osr_g1)"));

    let named = expanded("(as-> (load) value (normalize value) (combine value value))");
    assert_eq!(named.matches("(load)").count(), 1);
    assert!(named.contains("(let [value (load)]"));
    assert!(named.contains("(let [value (normalize value)] (combine value value))"));

    let doto = expanded("(doto (builder) (configure 1) finish)");
    assert_eq!(doto.matches("(builder)").count(), 1);
    assert!(doto.contains("(configure doto__osr_g0 1)"));
    assert!(doto.contains("(finish doto__osr_g0)"));
    assert!(doto.ends_with("doto__osr_g0))\n"));
}

#[test]
fn defn_dash_lowers_to_defn_with_private_authored_metadata() {
    let output = expanded("(defn- helper [value] value)");
    assert!(output.contains(":private true"), "{output}");
    assert!(output.contains("(defn helper [value] value)"), "{output}");
}

#[test]
fn negative_and_presence_binding_macros_expand_structurally() {
    assert_eq!(
        expanded("(if-not ready? (wait) (run))"),
        "(if (not (osiris.prelude/truthy* ready?)) (wait) (run))\n"
    );
    assert_eq!(
        expanded("(when-not ready? (prepare) (wait))"),
        "(if (not (osiris.prelude/truthy* ready?)) (do (prepare) (wait)) none)\n"
    );

    let if_some = expanded("(if-some [value (lookup)] (consume value) :missing)");
    assert_eq!(if_some.matches("(lookup)").count(), 1);
    assert!(if_some.contains("(osiris.prelude/nil* if-some__osr_g0)"));
    assert!(
        if_some.contains("(let [value (osiris.prelude/present* if-some__osr_g0)] (consume value))")
    );

    let when_some = expanded("(when-some [{:keys [id]} (lookup)] (consume id))");
    assert!(when_some.contains("(osiris.prelude/nil* if-some__osr_g0)"));
    assert!(when_some.contains("(let [{:keys [id]} (osiris.prelude/present* if-some__osr_g0)]"));
}

#[test]
fn throw_and_comment_lower_to_existing_core_forms() {
    assert_eq!(expanded("(throw error)"), "(raise error)\n");
    assert_eq!(expanded("(comment (cond invalid))"), "none\n");
}

#[test]
fn control_prelude_forms_expand_to_the_small_runtime_core() {
    assert_eq!(
        expanded("(when ready? (prepare) (run))"),
        "(if (osiris.prelude/truthy* ready?) (do (prepare) (run)) none)\n"
    );
    assert_eq!(
        expanded("(cond first? 1 second? 2 :else 3)"),
        "(if (osiris.prelude/truthy* first?) 1 (if (osiris.prelude/truthy* second?) 2 3))\n"
    );
    let condp = expanded("(condp = (classify) 1 :one 2 :two :else :other)");
    assert_eq!(condp.matches("(classify)").count(), 1);
    assert!(condp.contains("osiris.prelude/truthy*"));
    assert!(condp.contains("(= 1 condp-value__osr_g0)"));
    let condp_handler = expanded("(condp = value 1 :>> render :else :missing)");
    assert_eq!(
        condp_handler.matches("(= 1 condp-value__osr_g0)").count(),
        1
    );
    assert!(condp_handler.contains("(render (osiris.prelude/present* condp-result__osr_g1))"));
    assert_eq!(
        expanded("(for [item items] (normalize item))"),
        "(osiris.prelude/mapv (fn [item] (do (normalize item))) items)\n"
    );
    let destructured = expanded("(for [{:keys [value]} items] value)");
    assert!(destructured.contains("(fn [item__osr_g0]"));
    assert!(destructured.contains("(let [{:keys [value]} item__osr_g0]"));
}

#[test]
fn binding_and_nil_thread_macros_are_hygienic_and_short_circuiting() {
    let if_let = expanded("(if-let [{:keys [value]} (lookup)] value :missing)");
    assert_eq!(if_let.matches("(lookup)").count(), 1);
    assert!(if_let.contains("(osiris.prelude/truthy* if-let__osr_g0)"));
    assert!(
        if_let.contains("(let [{:keys [value]} (osiris.prelude/present* if-let__osr_g0)] value)")
    );

    let when_let = expanded("(when-let [value (lookup)] (consume value))");
    assert_eq!(when_let.matches("(lookup)").count(), 1);
    assert!(
        when_let.contains(
            "(let [value (osiris.prelude/present* if-let__osr_g0)] (do (consume value)))"
        )
    );

    let first = expanded("(some-> (lookup) (normalize 1) finish)");
    assert_eq!(first.matches("(lookup)").count(), 1);
    assert_eq!(first.matches("osiris.prelude/nil*").count(), 2);
    assert!(first.contains("(normalize (osiris.prelude/present* some-thread__osr_g0) 1)"));
    assert!(first.contains("(finish (osiris.prelude/present* some-thread__osr_g1))"));

    let last = expanded("(some->> (lookup) (map normalize) (reduce combine))");
    assert_eq!(last.matches("(lookup)").count(), 1);
    assert!(last.contains("(map normalize (osiris.prelude/present* some-thread__osr_g0))"));
    assert!(last.contains("(reduce combine (osiris.prelude/present* some-thread__osr_g1))"));
}

#[test]
fn case_evaluates_the_dispatch_once_and_supports_constant_groups() {
    let output = expanded("(case (classify) (1 2) :small 3 :three :other)");
    assert_eq!(output.matches("(classify)").count(), 1);
    assert!(output.contains("(let [case__osr_g0 (classify)]"));
    assert!(output.contains("(if (= case__osr_g0 1) true (= case__osr_g0 2))"));
    assert!(output.contains("(if (= case__osr_g0 3) :three :other)"));
}

#[test]
fn statement_loops_expand_through_existing_for_and_loop_primitives() {
    let doseq = expanded("(doseq [value values :when (positive? value)] (emit value))");
    assert!(doseq.contains("osiris.prelude/doseq*"));
    assert!(doseq.contains("(if (osiris.prelude/truthy* (positive? value))"));
    assert!(!doseq.contains("osiris.prelude/mapv"));
    assert!(!doseq.contains("osiris.prelude/mapcatv"));

    let when_first = expanded("(when-first [[left right] pairs] (+ left right))");
    assert!(when_first.contains("(seq pairs)"));
    assert!(when_first.contains("osiris.prelude/nil*"));
    assert!(
        when_first
            .contains("(let [[left right] (nth (osiris.prelude/present* when-first__osr_g0) 0)]")
    );

    let dotimes = expanded("(dotimes [index (limit)] (emit index))");
    assert_eq!(dotimes.matches("(limit)").count(), 1);
    assert!(dotimes.contains("osiris.prelude/loop*"));
    assert!(dotimes.contains("osiris.prelude/recur*"));

    let while_loop = expanded("(while (ready?) (step))");
    assert!(while_loop.contains("osiris.prelude/loop*"));
    assert!(while_loop.contains("osiris.prelude/recur*"));
    assert!(while_loop.contains("(osiris.prelude/truthy* (ready?))"));
    assert_eq!(while_loop.matches("(ready?)").count(), 1);
}

#[test]
fn parallel_forms_expand_through_future_and_ordered_deref() {
    let pmap = expanded("(pmap normalize values)");
    assert!(pmap.contains("osiris.prelude/future-call*"), "{pmap}");
    assert!(pmap.contains("osiris.prelude/deref*"), "{pmap}");
    assert_eq!(pmap.matches("normalize").count(), 1, "{pmap}");

    let multi = expanded("(pmap combine left right)");
    assert!(multi.contains("(fn [pmap-item__osr_g"), "{multi}");
    assert!(multi.contains("(pmap-function__osr_g"), "{multi}");

    let calls = expanded("(pcalls first second)");
    assert!(calls.contains("pcall-function__osr_g"), "{calls}");
    assert!(calls.contains("osiris.prelude/deref*"), "{calls}");
    assert_eq!(calls.matches("first").count(), 1, "{calls}");

    let values = expanded("(pvalues (slow-one) (slow-two))");
    assert!(values.contains("pvalues-future__osr_g"), "{values}");
    assert_eq!(values.matches("(slow-one)").count(), 1, "{values}");
    assert_eq!(values.matches("(slow-two)").count(), 1, "{values}");
    assert_eq!(expanded("(pvalues)"), "[]\n");
    assert_eq!(expanded("(pcalls)"), "[]\n");
}

#[test]
fn dynamic_binding_expands_to_the_context_runtime_boundary() {
    let binding = expanded("(binding [*value* (next-value)] (consume *value*))");
    assert_eq!(binding.matches("(next-value)").count(), 1);
    assert!(binding.contains("osiris.prelude/binding*"));
    assert!(binding.contains("[*value* (next-value)]"));
    assert!(binding.contains("(fn [] (do (consume *value*)))"));
}

#[test]
fn collection_while_clauses_stop_the_nearest_runtime_loop() {
    let comprehension = expanded("(for [value values :while (< value 3)] value)");
    assert!(comprehension.contains("osiris.prelude/mapcatv"));
    assert!(comprehension.contains("(osiris.prelude/for-stop*)"));
    assert_eq!(comprehension.matches("(< value 3)").count(), 1);

    let nested =
        expanded("(doseq [left lefts right rights :while (< right left)] (emit left right))");
    assert_eq!(nested.matches("osiris.prelude/doseq*").count(), 2);
    assert_eq!(nested.matches("osiris.prelude/for-stop*").count(), 1);
    assert!(nested.contains("(if (osiris.prelude/truthy* (< right left))"));
}

#[test]
fn malformed_binding_and_case_macros_report_specific_diagnostics() {
    for (source, expected) in [
        (
            "(if-let [value] value)",
            "if-let requires exactly one binding pair",
        ),
        (
            "(if-let [1 value] value)",
            "if-let binding pattern must be a symbol, vector, or map",
        ),
        (
            "(if-let [value true] value none :extra)",
            "if-let accepts at most one else expression",
        ),
        (
            "(when-first [value] value)",
            "when-first requires exactly one binding pair",
        ),
        (
            "(case value 1 :one)",
            "case requires an explicit default expression",
        ),
        (
            "(case value (1 2) :small 2 :two :other)",
            "case test constants must be unique",
        ),
        (
            "(case value () :never :other)",
            "case test group cannot be empty",
        ),
        (
            "(dotimes [index] index)",
            "dotimes requires exactly one name/count pair",
        ),
        ("(as-> value 1 value)", "as-> requires a symbol binding"),
        (
            "(if-not ready? 1 2 3)",
            "if-not accepts at most one else expression",
        ),
        (
            "(if-some [value] value)",
            "if-some requires exactly one binding pair",
        ),
        (
            "(if-some [value true] value none :extra)",
            "if-some accepts at most one else expression",
        ),
    ] {
        let result = expand(&read(source), ExpansionOptions::default());
        assert!(
            result.document.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "OSR-M0007" && diagnostic.message == expected
            }),
            "expected `{expected}` for {source}, got: {:?}",
            result.document.diagnostics
        );
    }
}
