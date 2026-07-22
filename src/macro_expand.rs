//! Hygienic surface-form macro expansion.
//!
//! Macros transform reader forms before surface AST lowering. The standard
//! threading forms live here as prelude macros rather than HIR special cases,
//! keeping the runtime language and backend small.

use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

use serde::Serialize;

use crate::{
    diagnostic::Diagnostic,
    name::{BindingId, BindingKind},
    source::Span,
    syntax::{
        Document, Form, FormKind, METADATA_TARGET_LIMITS, MetadataEntry, Name, ReaderMacroKind,
        check_metadata_resources, metadata_datum_is_serializable,
    },
};

const DEFAULT_MAX_EXPANSIONS: usize = 1_024;
const DEFAULT_MAX_EVAL_STEPS: usize = 100_000;
const DEFAULT_MAX_EVAL_DEPTH: usize = 128;
const DEFAULT_MAX_RESULT_NODES: usize = 65_536;

// The standard macros are ordinary phase-1 definitions. Keeping their source
// here bootstraps the prelude without introducing a second expansion engine;
// packaged preludes can later provide the same definitions through `.osri`.
const BOOTSTRAP_PRELUDE: &str = r#"
(defn-for-syntax osiris.prelude/thread-first-step [value step]
  (if (list? step)
    (if (empty? step)
      (syntax-error step "threading step cannot be an empty list")
      (with-meta
        (concat (list (first step) value) (rest step))
        (meta step)))
    (if (symbol? step)
      (list step value)
      (syntax-error step "threading step must be a symbol or non-empty list"))))

(defn-for-syntax osiris.prelude/thread-last-step [value step]
  (if (list? step)
    (if (empty? step)
      (syntax-error step "threading step cannot be an empty list")
      (with-meta (concat step (list value)) (meta step)))
    (if (symbol? step)
      (list step value)
      (syntax-error step "threading step must be a symbol or non-empty list"))))

(defn-for-syntax osiris.prelude/cond-thread [call threader value clauses]
  (if (empty? clauses)
    value
    (if (empty? (rest clauses))
      (syntax-error call "conditional threading requires condition and step pairs")
      (let [condition (first clauses)
            step (nth clauses 1)
            temporary (gensym "thread")
            threaded (threader temporary step)]
        (osiris.prelude/cond-thread
          call
          threader
          `(let [~temporary ~value]
             (if (osiris.prelude/truthy* ~condition)
               ~threaded
               ~temporary))
          (rest (rest clauses)))))))

(defmacro -> [value & steps]
  (reduce osiris.prelude/thread-first-step value steps))

(defmacro ->> [value & steps]
  (reduce osiris.prelude/thread-last-step value steps))

(defmacro cond-> [value & clauses]
  (osiris.prelude/cond-thread
    &form osiris.prelude/thread-first-step value clauses))

(defmacro cond->> [value & clauses]
  (osiris.prelude/cond-thread
    &form osiris.prelude/thread-last-step value clauses))

(defn-for-syntax osiris.prelude/as-thread [name value forms]
  (if (empty? forms)
    value
    `(let [~name ~value]
       ~(osiris.prelude/as-thread name (first forms) (rest forms)))))

(defmacro as-> [value name & forms]
  (if (not (symbol? name))
    (syntax-error name "as-> requires a symbol binding")
    (osiris.prelude/as-thread name value forms)))

(defn-for-syntax osiris.prelude/doto-calls [value forms]
  (if (empty? forms)
    '()
    (cons
      (osiris.prelude/thread-first-step value (first forms))
      (osiris.prelude/doto-calls value (rest forms)))))

(defmacro doto [value & forms]
  (let [temporary (gensym "doto")]
    `(let [~temporary ~value]
      (do
        ~@(osiris.prelude/doto-calls temporary forms)
        ~temporary))))

;; `defn-` is the Clojure spelling for a private function declaration.  Osiris
;; already makes every non-exported binding private at the interface boundary,
;; so the macro only preserves the authored intent as Rich Metadata while
;; lowering to the ordinary compiler-owned `defn` declaration.
(defmacro defn- [name & declaration]
  (if (not (symbol? name))
    (syntax-error name "defn- requires a symbol name")
    (let [definition (cons 'defn (cons name declaration))]
      (with-meta definition (assoc (meta definition) :private true)))))

(defn-for-syntax osiris.prelude/and-form [forms]
  (if (empty? forms)
    true
    (if (empty? (rest forms))
      (first forms)
      (let [temporary (gensym "and")]
        `(let [~temporary ~(first forms)]
           (if (osiris.prelude/truthy* ~temporary)
             ~(osiris.prelude/and-form (rest forms))
             ~temporary))))))

(defn-for-syntax osiris.prelude/or-form [forms]
  (if (empty? forms)
    none
    (if (empty? (rest forms))
      (first forms)
      (let [temporary (gensym "or")]
        `(let [~temporary ~(first forms)]
           (if (osiris.prelude/truthy* ~temporary)
             ~temporary
             ~(osiris.prelude/or-form (rest forms))))))))

(defn-for-syntax osiris.prelude/cond-form [call clauses]
  (if (empty? clauses)
    none
    (if (empty? (rest clauses))
      (syntax-error call "cond requires condition/result pairs")
      (let [condition (first clauses)
            result (nth clauses 1)
            remaining (rest (rest clauses))]
        (if (= condition :else)
          (if (empty? remaining)
            result
            (syntax-error condition ":else must be the final cond condition"))
          `(if (osiris.prelude/truthy* ~condition)
             ~result
             ~(osiris.prelude/cond-form call remaining)))))))

(defmacro and [& forms]
  (osiris.prelude/and-form forms))

(defmacro or [& forms]
  (osiris.prelude/or-form forms))

(defmacro when [condition & body]
  (if (empty? body)
    `(if (osiris.prelude/truthy* ~condition) none none)
    `(if (osiris.prelude/truthy* ~condition) (do ~@body) none)))

(defmacro if-not [condition then & else]
  (if (> (count else) 1)
    (syntax-error &form "if-not accepts at most one else expression")
    `(if (not (osiris.prelude/truthy* ~condition))
       ~then
       ~(if (empty? else) none (first else)))))

(defmacro when-not [condition & body]
  `(if (not (osiris.prelude/truthy* ~condition))
     ~(if (empty? body) none `(do ~@body))
     none))

(defmacro cond [& clauses]
  (osiris.prelude/cond-form &form clauses))

(defn-for-syntax osiris.prelude/control-binding? [bindings]
  (if (not (vector? bindings))
    false
    (= (count bindings) 2)))

(defn-for-syntax osiris.prelude/control-binding-pattern? [pattern]
  (if (symbol? pattern)
    true
    (if (vector? pattern)
      true
      (map? pattern))))

(defn-for-syntax osiris.prelude/if-let-form [call bindings then else]
  (if (not (osiris.prelude/control-binding? bindings))
    (syntax-error bindings "if-let requires exactly one binding pair")
    (let [pattern (first bindings)]
      (if (not (osiris.prelude/control-binding-pattern? pattern))
        (syntax-error pattern "if-let binding pattern must be a symbol, vector, or map")
        (let [temporary (gensym "if-let")]
          `(let [~temporary ~(nth bindings 1)]
             (if (osiris.prelude/truthy* ~temporary)
               (let [~pattern (osiris.prelude/present* ~temporary)] ~then)
               ~else)))))))

(defmacro if-let [bindings then & else]
  (if (> (count else) 1)
    (syntax-error &form "if-let accepts at most one else expression")
    (osiris.prelude/if-let-form
      &form bindings then (if (empty? else) none (first else)))))

(defmacro when-let [bindings & body]
  (osiris.prelude/if-let-form
    &form bindings (if (empty? body) none `(do ~@body)) none))

(defn-for-syntax osiris.prelude/if-some-form [call bindings then else]
  (if (not (osiris.prelude/control-binding? bindings))
    (syntax-error bindings "if-some requires exactly one binding pair")
    (let [pattern (first bindings)]
      (if (not (osiris.prelude/control-binding-pattern? pattern))
        (syntax-error pattern "if-some binding pattern must be a symbol, vector, or map")
        (let [temporary (gensym "if-some")]
          `(let [~temporary ~(nth bindings 1)]
             (if (osiris.prelude/nil* ~temporary)
               ~else
               (let [~pattern (osiris.prelude/present* ~temporary)] ~then))))))))

(defmacro if-some [bindings then & else]
  (if (> (count else) 1)
    (syntax-error &form "if-some accepts at most one else expression")
    (osiris.prelude/if-some-form
      &form bindings then (if (empty? else) none (first else)))))

(defmacro when-some [bindings & body]
  (osiris.prelude/if-some-form
    &form bindings (if (empty? body) none `(do ~@body)) none))

;; Runtime nil predicates are kept as tiny macros so callers do not need to
;; know the private intrinsic spelling.  The phase-1 evaluator still uses its
;; own `nil?`/`some?` builtins when inspecting syntax data.
(defmacro nil? [value]
  `(osiris.prelude/nil* ~value))

(defmacro some? [value]
  `(not (osiris.prelude/nil* ~value)))

(defn-for-syntax osiris.prelude/some-thread-form [threader value steps]
  (if (empty? steps)
    value
    (let [temporary (gensym "some-thread")
          threaded (threader `(osiris.prelude/present* ~temporary) (first steps))]
      `(let [~temporary ~value]
         (if (osiris.prelude/nil* ~temporary)
           none
           ~(osiris.prelude/some-thread-form
              threader threaded (rest steps)))))))

(defmacro some-> [value & steps]
  (osiris.prelude/some-thread-form
    osiris.prelude/thread-first-step value steps))

(defmacro some->> [value & steps]
  (osiris.prelude/some-thread-form
    osiris.prelude/thread-last-step value steps))

;; `condp` keeps the predicate and dispatch expression single-evaluation while
;; leaving each test/result branch lazy.  `:>>` applies its following function
;; to the dispatch value when the predicate succeeds.
(defn-for-syntax osiris.prelude/condp-operator? [predicate]
  (if (not (symbol? predicate))
    false
    (or (= predicate '=)
        (= predicate '==)
        (= predicate '!=)
        (= predicate 'not=)
        (= predicate '<)
        (= predicate '<=)
        (= predicate '>)
        (= predicate '>=))))

(defn-for-syntax osiris.prelude/condp-form [call predicate value clauses]
  (if (empty? clauses)
    (syntax-error call "condp requires an explicit :else clause")
    (if (= (first clauses) :else)
      (if (= (count clauses) 2)
        (nth clauses 1)
        (syntax-error (first clauses) ":else must be the final condp clause"))
      (if (empty? (rest clauses))
        (syntax-error (first clauses) "condp test requires a result expression")
        (let [test (first clauses)
              result (nth clauses 1)
              remaining (rest (rest clauses))]
          (if (= result :>>)
            (if (empty? remaining)
              (syntax-error result "condp :>> requires a function expression")
              (let [function (first remaining)
                    tail (rest remaining)
                    predicate-result (gensym "condp-result")]
                `(let [~predicate-result (~predicate ~test ~value)]
                   (if (osiris.prelude/truthy* ~predicate-result)
                     (~function (osiris.prelude/present* ~predicate-result))
                     ~(osiris.prelude/condp-form call predicate value tail)))))
            `(if (osiris.prelude/truthy* (~predicate ~test ~value))
               ~result
               ~(osiris.prelude/condp-form call predicate value remaining))))))))

(defmacro condp [predicate value & clauses]
  (let [value* (gensym "condp-value")]
    (if (osiris.prelude/condp-operator? predicate)
      `(let [~value* ~value]
         ~(osiris.prelude/condp-form &form predicate value* clauses))
      (let [predicate* (gensym "condp-predicate")]
        `(let [~predicate* ~predicate
               ~value* ~value]
           ~(osiris.prelude/condp-form &form predicate* value* clauses))))))

(defn-for-syntax osiris.prelude/case-register-tests [call seen tests]
  (if (empty? tests)
    seen
    (let [test (first tests)]
      (if (contains? seen test)
        (syntax-error test "case test constants must be unique")
        (osiris.prelude/case-register-tests
          call (conj seen test) (rest tests))))))

(defn-for-syntax osiris.prelude/case-register-test [call seen test]
  (if (list? test)
    (if (empty? test)
      (syntax-error test "case test group cannot be empty")
      (osiris.prelude/case-register-tests call seen test))
    (if (symbol? test)
      (syntax-error test "case symbol constants are not runtime values; use a keyword or string")
      (if (vector? test)
        (syntax-error test "case vector constants are not supported in v0")
        (if (map? test)
          (syntax-error test "case map constants are not supported in v0")
          (osiris.prelude/case-register-tests call seen (list test)))))))

(defn-for-syntax osiris.prelude/validate-case-clauses [call clauses seen]
  (if (< (count clauses) 2)
    seen
    (osiris.prelude/validate-case-clauses
      call
      (rest (rest clauses))
      (osiris.prelude/case-register-test call seen (first clauses)))))

(defn-for-syntax osiris.prelude/case-group-condition [value tests]
  (if (empty? (rest tests))
    `(= ~value ~(first tests))
    `(if (= ~value ~(first tests))
       true
       ~(osiris.prelude/case-group-condition value (rest tests)))))

(defn-for-syntax osiris.prelude/case-condition [value test]
  (if (list? test)
    (osiris.prelude/case-group-condition value test)
    `(= ~value ~test)))

(defn-for-syntax osiris.prelude/case-form [call value clauses]
  (if (empty? clauses)
    (syntax-error call "case requires an explicit default expression")
    (if (empty? (rest clauses))
      (first clauses)
      `(if ~(osiris.prelude/case-condition value (first clauses))
         ~(nth clauses 1)
         ~(osiris.prelude/case-form call value (rest (rest clauses)))))))

(defmacro case [value & clauses]
  (let [valid (osiris.prelude/validate-case-clauses &form clauses #{})
        temporary (gensym "case")]
    `(let [~temporary ~value]
       ~(osiris.prelude/case-form &form temporary clauses))))

(defn-for-syntax osiris.prelude/for-binding-pattern? [pattern]
  (if (symbol? pattern)
    true
    (if (vector? pattern)
      true
      (map? pattern))))

(defn-for-syntax osiris.prelude/for-even-bindings? [bindings]
  (if (empty? bindings)
    true
    (if (empty? (rest bindings))
      false
      (osiris.prelude/for-even-bindings? (rest (rest bindings))))))

(defn-for-syntax osiris.prelude/validate-for-let-bindings [bindings]
  (if (empty? bindings)
    true
    (let [pattern (first bindings)]
      (if (not (osiris.prelude/for-binding-pattern? pattern))
        (syntax-error pattern "for :let binding pattern must be a symbol, vector, or map")
        (osiris.prelude/validate-for-let-bindings (rest (rest bindings)))))))

(defn-for-syntax osiris.prelude/for-flatten? [clauses]
  (if (empty? clauses)
    false
    (let [clause (first clauses)
          remaining (rest clauses)]
      (if (keyword? clause)
        (if (= clause :when)
          true
          (if (= clause :let)
            (if (empty? remaining)
              true
              (osiris.prelude/for-flatten? (rest remaining)))
            true))
        true))))

(defn-for-syntax osiris.prelude/for-terminal [call clauses body]
  (if (empty? clauses)
    `(do ~@body)
    (let [clause (first clauses)
          remaining (rest clauses)]
      (if (= clause :let)
        (if (empty? remaining)
          (syntax-error clause "for :let requires a binding vector")
          (let [bindings (first remaining)]
            (if (not (vector? bindings))
              (syntax-error bindings "for :let requires a binding vector")
              (if (not (osiris.prelude/for-even-bindings? bindings))
                (syntax-error bindings "for :let requires an even number of binding forms")
                (let [valid (osiris.prelude/validate-for-let-bindings bindings)]
                  `(let ~bindings
                     ~(osiris.prelude/for-terminal call (rest remaining) body)))))))
        (syntax-error call "invalid terminal for clause")))))

(defn-for-syntax osiris.prelude/for-tail [call clauses body]
  (if (empty? clauses)
    `[(do ~@body)]
    (let [clause (first clauses)
          remaining (rest clauses)]
      (if (keyword? clause)
        (if (= clause :let)
          (if (empty? remaining)
            (syntax-error clause "for :let requires a binding vector")
            (let [bindings (first remaining)]
              (if (not (vector? bindings))
                (syntax-error bindings "for :let requires a binding vector")
                (if (not (osiris.prelude/for-even-bindings? bindings))
                  (syntax-error bindings "for :let requires an even number of binding forms")
                  (let [valid (osiris.prelude/validate-for-let-bindings bindings)]
                    `(let ~bindings
                       ~(osiris.prelude/for-tail call (rest remaining) body)))))))
          (if (= clause :when)
            (if (empty? remaining)
              (syntax-error clause "for :when requires a predicate expression")
              `(if (osiris.prelude/truthy* ~(first remaining))
                 ~(osiris.prelude/for-tail call (rest remaining) body)
                 []))
            (if (= clause :while)
              (if (empty? remaining)
                (syntax-error clause "for :while requires a predicate expression")
                `(if (osiris.prelude/truthy* ~(first remaining))
                   ~(osiris.prelude/for-tail call (rest remaining) body)
                   (osiris.prelude/for-stop*)))
              (syntax-error
                clause
                (str "unsupported for modifier " clause "; expected :let, :when, or :while")))))
        (osiris.prelude/for-bindings call clauses body)))))

(defn-for-syntax osiris.prelude/for-bindings [call clauses body]
  (if (empty? clauses)
    (syntax-error call "for requires at least one pattern/collection pair")
    (let [pattern (first clauses)
          remaining (rest clauses)]
      (if (keyword? pattern)
        (syntax-error pattern "for binding vector must start with a pattern/collection pair")
        (if (not (osiris.prelude/for-binding-pattern? pattern))
          (syntax-error pattern "for binding pattern must be a symbol, vector, or map")
          (if (empty? remaining)
            (syntax-error pattern "for binding pattern requires a collection expression")
            (let [collection (first remaining)
                  tail (rest remaining)
                  item (if (symbol? pattern) none (gensym "item"))
                  flatten (osiris.prelude/for-flatten? tail)
                  continuation (if flatten
                                 (osiris.prelude/for-tail call tail body)
                                 (osiris.prelude/for-terminal call tail body))]
              (if flatten
                (if (symbol? pattern)
                  `(osiris.prelude/mapcatv
                     (fn [~pattern] ~continuation)
                     ~collection)
                  `(osiris.prelude/mapcatv
                     (fn [~item]
                       (let [~pattern ~item]
                         ~continuation))
                     ~collection))
                (if (symbol? pattern)
                  `(osiris.prelude/mapv
                     (fn [~pattern] ~continuation)
                     ~collection)
                  `(osiris.prelude/mapv
                     (fn [~item]
                       (let [~pattern ~item]
                         ~continuation))
                     ~collection))))))))))

(defmacro for [bindings & body]
  (if (not (vector? bindings))
    (syntax-error bindings "for requires a binding vector")
    (if (empty? body)
      (syntax-error &form "for requires a body")
      (osiris.prelude/for-bindings &form bindings body))))

(defn-for-syntax osiris.prelude/validate-doseq-let-bindings [bindings]
  (if (empty? bindings)
    true
    (let [pattern (first bindings)]
      (if (not (osiris.prelude/for-binding-pattern? pattern))
        (syntax-error pattern "doseq :let binding pattern must be a symbol, vector, or map")
        (osiris.prelude/validate-doseq-let-bindings (rest (rest bindings)))))))

(defn-for-syntax osiris.prelude/doseq-tail [call clauses body]
  (if (empty? clauses)
    `(do ~@body none)
    (let [clause (first clauses)
          remaining (rest clauses)]
      (if (keyword? clause)
        (if (= clause :let)
          (if (empty? remaining)
            (syntax-error clause "doseq :let requires a binding vector")
            (let [bindings (first remaining)]
              (if (not (vector? bindings))
                (syntax-error bindings "doseq :let requires a binding vector")
                (if (not (osiris.prelude/for-even-bindings? bindings))
                  (syntax-error bindings "doseq :let requires an even number of binding forms")
                  (let [valid (osiris.prelude/validate-doseq-let-bindings bindings)]
                    `(let ~bindings
                       ~(osiris.prelude/doseq-tail call (rest remaining) body)))))))
          (if (= clause :when)
            (if (empty? remaining)
              (syntax-error clause "doseq :when requires a predicate expression")
              `(if (osiris.prelude/truthy* ~(first remaining))
                 ~(osiris.prelude/doseq-tail call (rest remaining) body)
                 none))
            (if (= clause :while)
              (if (empty? remaining)
                (syntax-error clause "doseq :while requires a predicate expression")
                `(if (osiris.prelude/truthy* ~(first remaining))
                   ~(osiris.prelude/doseq-tail call (rest remaining) body)
                   (osiris.prelude/for-stop*)))
              (syntax-error
                clause
                (str "unsupported doseq modifier " clause "; expected :let, :when, or :while")))))
        (osiris.prelude/doseq-bindings call clauses body)))))

(defn-for-syntax osiris.prelude/doseq-bindings [call clauses body]
  (if (empty? clauses)
    (syntax-error call "doseq requires at least one pattern/collection pair")
    (let [pattern (first clauses)
          remaining (rest clauses)]
      (if (keyword? pattern)
        (syntax-error pattern "doseq binding vector must start with a pattern/collection pair")
        (if (not (osiris.prelude/for-binding-pattern? pattern))
          (syntax-error pattern "doseq binding pattern must be a symbol, vector, or map")
          (if (empty? remaining)
            (syntax-error pattern "doseq binding pattern requires a collection expression")
            (let [collection (first remaining)
                  tail (rest remaining)
                  item (if (symbol? pattern) none (gensym "doseq-item"))
                  continuation (osiris.prelude/doseq-tail call tail body)]
              (if (symbol? pattern)
                `(osiris.prelude/doseq*
                   (fn [~pattern] ~continuation)
                   ~collection)
                `(osiris.prelude/doseq*
                   (fn [~item]
                     (let [~pattern ~item]
                       ~continuation))
                   ~collection)))))))))

(defmacro doseq [bindings & body]
  (if (not (vector? bindings))
    (syntax-error bindings "doseq requires a binding vector")
    (osiris.prelude/doseq-bindings &form bindings body)))

(defmacro when-first [bindings & body]
  (if (not (osiris.prelude/control-binding? bindings))
    (syntax-error bindings "when-first requires exactly one binding pair")
    (let [pattern (first bindings)]
      (if (not (osiris.prelude/control-binding-pattern? pattern))
        (syntax-error pattern "when-first binding pattern must be a symbol, vector, or map")
        (let [sequence (gensym "when-first")]
          `(let [~sequence (seq ~(nth bindings 1))]
             (if (osiris.prelude/nil* ~sequence)
               none
               (let [~pattern (nth (osiris.prelude/present* ~sequence) 0)]
                 (do ~@body)))))))))

;; `loop`/`recur` stay ordinary surface macros. The runtime primitive receives
;; one callback and the current state values; a `recur` result is a private
;; control token consumed by that primitive. This keeps the reader and core
;; AST free of a second looping syntax while still giving Python an O(1)-stack
;; implementation.
(defn-for-syntax osiris.prelude/loop-valid-bindings? [bindings]
  (if (empty? bindings)
    true
    (if (empty? (rest bindings))
      false
      (osiris.prelude/loop-valid-bindings? (rest (rest bindings))))))

(defn-for-syntax osiris.prelude/loop-params [bindings]
  (if (empty? bindings)
    '()
    (let [pattern (first bindings)]
      (cons
        (if (symbol? pattern) pattern (gensym "loop-state"))
        (osiris.prelude/loop-params (rest (rest bindings)))))))

(defn-for-syntax osiris.prelude/loop-inits [bindings]
  (if (empty? bindings)
    '()
    (cons
      (nth bindings 1)
      (osiris.prelude/loop-inits (rest (rest bindings))))))

(defn-for-syntax osiris.prelude/loop-destructure [bindings params]
  (if (empty? bindings)
    '()
    (let [pattern (first bindings)
          parameter (first params)
          remaining (osiris.prelude/loop-destructure
                      (rest (rest bindings))
                      (rest params))]
      (if (symbol? pattern)
        remaining
        (concat (list pattern parameter) remaining)))))

(defmacro loop [bindings & body]
  (if (not (vector? bindings))
    (syntax-error bindings "loop requires a binding vector")
    (if (not (osiris.prelude/loop-valid-bindings? bindings))
      (syntax-error bindings "loop bindings require pattern/value pairs")
      (if (empty? body)
        (syntax-error &form "loop requires a body")
        (let [params (osiris.prelude/loop-params bindings)
              inits (osiris.prelude/loop-inits bindings)
              destructure (osiris.prelude/loop-destructure bindings params)
              callback-body (if (empty? destructure)
                              `(do ~@body)
                              `(let [~@destructure] (do ~@body)))]
          `(osiris.prelude/loop*
             (fn [~@params] ~callback-body)
             ~@inits))))))

(defmacro recur [& values]
  `(osiris.prelude/recur* ~@values))

(defmacro dotimes [bindings & body]
  (if (not (vector? bindings))
    (syntax-error bindings "dotimes requires a binding vector")
    (if (not (= (count bindings) 2))
      (syntax-error bindings "dotimes requires exactly one name/count pair")
      (let [binding (first bindings)]
        (if (not (symbol? binding))
          (syntax-error binding "dotimes binding must be a symbol")
          (let [limit (gensym "dotimes-limit")
                index (gensym "dotimes-index")]
            `(let [~limit ~(nth bindings 1)]
               (loop [~index 0]
                 (if (< ~index ~limit)
                   (do
                     (let [~binding ~index] (do ~@body))
                     (recur (+ ~index 1)))
                   none)))))))))

(defmacro while [condition & body]
  `(loop []
     (if (osiris.prelude/truthy* ~condition)
       (do ~@body (recur))
       none)))

;; `letfn` needs a compiler-owned lexical frame so all names are visible while
;; their lambda bodies are lowered.  The `letfn*` target is intentionally
;; private; its surface shape remains ordinary Clojure-style bindings.
(defn-for-syntax osiris.prelude/letfn-normalize [entries]
  (if (empty? entries)
    '()
    (let [entry (first entries)
          remaining (rest entries)]
      (if (list? entry)
        (if (empty? entry)
          (syntax-error entry "letfn function binding cannot be empty")
          (cons
            (first entry)
            (cons `(fn ~@(rest entry))
                  (osiris.prelude/letfn-normalize remaining))))
        (if (empty? remaining)
          (syntax-error entry "letfn requires name/function pairs")
          (cons
            entry
            (cons
              (first remaining)
              (osiris.prelude/letfn-normalize (rest remaining)))))))))

(defmacro letfn [bindings & body]
  (if (not (vector? bindings))
    (syntax-error bindings "letfn requires a binding vector")
    (if (empty? body)
      (syntax-error &form "letfn requires a body")
      `(osiris.prelude/letfn*
         ~(apply vector (osiris.prelude/letfn-normalize bindings))
         (do ~@body)))))

(defmacro trampoline [function & args]
  `(osiris.prelude/trampoline* ~function ~@args))

(defmacro lazy-seq [& body]
  `(osiris.prelude/lazy-seq* (fn [] (do ~@body))))

;; `lazy-cat` keeps each collection expression lazy and delegates traversal to
;; the memoized sequence runtime.  Unlike `concat`, its surface form accepts
;; arbitrary body expressions and therefore mirrors the Clojure macro shape.
(defmacro lazy-cat [& forms]
  `(osiris.prelude/lazy-seq*
     (fn [] (osiris.prelude/concat ~@forms))))

;; Delay/force are deliberately expressed as ordinary surface macros.  The
;; compiler only owns the tiny runtime ABI; the delayed body remains a normal
;; lexical function and therefore keeps source maps, metadata, and callback
;; summaries intact.
(defmacro delay [& body]
  `(osiris.prelude/delay* (fn [] (do ~@body))))

(defmacro force [value]
  `(osiris.prelude/force* ~value))

(defmacro realized? [value]
  `(osiris.prelude/realized* ~value))

(defmacro deref [value & options]
  (if (or (= (count options) 0) (= (count options) 2))
    `(osiris.prelude/deref* ~value ~@options)
    (syntax-error &form "deref accepts one argument or value/timeout/default")))

;; Concurrency forms intentionally remain ordinary macros.  The compiler owns
;; only the small, typed runtime entry points below; callback bodies continue
;; through the regular lexical lowering path and therefore retain source maps
;; and effect summaries.
(defmacro future [& body]
  `(osiris.prelude/future-call*
     (fn [] ~(if (empty? body) none `(do ~@body)))))

(defmacro future-call [function]
  `(osiris.prelude/future-call* ~function))

(defmacro future-done? [value]
  `(osiris.prelude/future-done* ~value))

(defmacro future-cancelled? [value]
  `(osiris.prelude/future-cancelled* ~value))

(defmacro future-cancel [value]
  `(osiris.prelude/future-cancel* ~value))

;; Parallel collection/control helpers are intentionally expressed in terms
;; of the existing Future ABI.  They submit all work eagerly, then deref in
;; source/input order, which keeps the result deterministic while making the
;; concurrency boundary explicit in generated Osiris/Python code.  A failed
;; deref propagates its first observed exception; already-submitted work is
;; not implicitly cancelled.
(defn-for-syntax osiris.prelude/parallel-params [prefix forms]
  (if (empty? forms)
    '()
    (cons
      (gensym prefix)
      (osiris.prelude/parallel-params prefix (rest forms)))))

(defn-for-syntax osiris.prelude/parallel-bindings [names forms]
  (if (empty? names)
    '()
    (cons
      (first names)
      (cons
        (first forms)
        (osiris.prelude/parallel-bindings (rest names) (rest forms))))))

(defn-for-syntax osiris.prelude/parallel-deref-forms [names]
  (if (empty? names)
    '()
    (cons
      `(deref ~(first names))
      (osiris.prelude/parallel-deref-forms (rest names)))))

(defn-for-syntax osiris.prelude/parallel-call-forms [names]
  (if (empty? names)
    '()
    (cons
      `(future-call (fn [] (~(first names))))
      (osiris.prelude/parallel-call-forms (rest names)))))

(defn-for-syntax osiris.prelude/pvalues-future-expressions [forms]
  (if (empty? forms)
    '()
    (cons
      `(future-call (fn [] ~(first forms)))
      (osiris.prelude/pvalues-future-expressions (rest forms)))))

(defmacro pmap [function & collections]
  (if (empty? collections)
    (syntax-error &form "pmap requires at least one collection")
    (let [function-name (gensym "pmap-function")
          parameters (osiris.prelude/parallel-params "pmap-item" collections)
          futures-name (gensym "pmap-futures")
          task-name (gensym "pmap-task")
          invocation (cons function-name parameters)]
      `(let [~function-name ~function
             ~futures-name
             (mapv
               (fn [~@parameters]
                 (future-call (fn [] ~invocation)))
               ~@collections)]
         (mapv (fn [~task-name] (deref ~task-name)) ~futures-name)))))

(defmacro pvalues [& forms]
  (if (empty? forms)
    []
    (let [future-names (osiris.prelude/parallel-params "pvalues-future" forms)
          future-expressions (osiris.prelude/pvalues-future-expressions forms)]
      `(let [~@(osiris.prelude/parallel-bindings future-names future-expressions)]
         [~@(osiris.prelude/parallel-deref-forms future-names)]))))

(defmacro pcalls [& functions]
  (if (empty? functions)
    []
    (let [function-names (osiris.prelude/parallel-params "pcall-function" functions)
          future-names (osiris.prelude/parallel-params "pcall-future" functions)
          function-bindings (osiris.prelude/parallel-bindings function-names functions)
          future-expressions (osiris.prelude/parallel-call-forms function-names)
          future-bindings (osiris.prelude/parallel-bindings future-names future-expressions)]
      `(let [~@function-bindings
             ~@future-bindings]
         [~@(osiris.prelude/parallel-deref-forms future-names)]))))

(defmacro promise [& args]
  (if (empty? args)
    `(osiris.prelude/promise*)
    (syntax-error &form "promise does not accept arguments")))

(defmacro deliver [value result]
  `(osiris.prelude/deliver* ~value ~result))

(defmacro lock [& args]
  (if (empty? args)
    `(osiris.prelude/lock*)
    (syntax-error &form "lock does not accept arguments")))

(defmacro locking [value & body]
  `(osiris.prelude/locking* ~value
     (fn [] ~(if (empty? body) none `(do ~@body)))))

;; Keep timing as a surface macro so the measured expression remains ordinary
;; typed code with its lexical scope and source map intact.  The runtime owns
;; only clock access and the Clojure-compatible reporting format.
(defmacro time [& expressions]
  (if (empty? expressions)
    (syntax-error &form "time requires at least one expression")
    `(osiris.prelude/time* (fn [] (do ~@expressions)))))

(defn-for-syntax osiris.prelude/validate-binding-targets [bindings]
  (if (empty? bindings)
    true
    (let [target (first bindings)]
      (if (not (symbol? target))
        (syntax-error target "binding targets must be dynamic Var symbols")
        (osiris.prelude/validate-binding-targets (rest (rest bindings)))))))

(defmacro binding [bindings & body]
  (if (not (vector? bindings))
    (syntax-error bindings "binding requires a binding vector")
    (if (not (osiris.prelude/for-even-bindings? bindings))
      (syntax-error bindings "binding requires target/value pairs")
      (let [valid (osiris.prelude/validate-binding-targets bindings)]
        `(osiris.prelude/binding*
           ~bindings
           (fn [] ~(if (empty? body) none `(do ~@body))))))))

(defn-for-syntax osiris.prelude/with-open-form [call bindings body]
  (if (empty? bindings)
    `(do ~@body)
    (if (empty? (rest bindings))
      (syntax-error call "with-open requires resource name/value pairs")
      (let [name (first bindings)
            value (nth bindings 1)
            remaining (rest (rest bindings))]
        (if (not (symbol? name))
          (syntax-error name "with-open resource names must be symbols")
          `(let [~name ~value]
             (try
               ~(osiris.prelude/with-open-form call remaining body)
               (finally (osiris.prelude/close* ~name)))))))))

(defmacro with-open [bindings & body]
  (if (not (vector? bindings))
    (syntax-error bindings "with-open requires a binding vector")
    (osiris.prelude/with-open-form &form bindings body)))

(defmacro throw [value]
  `(raise ~value))

(defmacro assert [condition & message]
  (if (> (count message) 1)
    (syntax-error &form "assert accepts at most one message expression")
    (if (empty? message)
      `(if (osiris.prelude/truthy* ~condition)
         none
         (osiris.prelude/assert* false))
      `(if (osiris.prelude/truthy* ~condition)
         none
         (osiris.prelude/assert* false ~(first message))))))

(defmacro comment [& forms]
  none)
"#;

#[derive(Clone, Copy, Debug)]
pub struct ExpansionOptions {
    pub once: bool,
    pub max_expansions: usize,
}

impl Default for ExpansionOptions {
    fn default() -> Self {
        Self {
            once: false,
            max_expansions: DEFAULT_MAX_EXPANSIONS,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ExpansionTrace {
    pub macro_name: String,
    pub macro_binding_id: String,
    pub call_span: Span,
    pub expansion_span: Span,
    pub depth: usize,
    pub origin: Vec<Span>,
}

#[derive(Clone, Debug)]
pub struct ExpansionResult {
    pub document: Document,
    pub traces: Vec<ExpansionTrace>,
}

/// One data-only phase-1 interface import.
///
/// `namespace` is the stable definition namespace used to isolate private
/// helpers. `macro_names` maps each caller-visible spelling (for example
/// `q/pipeline` or a referred `pipeline`) to the canonical macro declaration
/// name contained in `forms`. No imported macro is callable unless it appears
/// in this map.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportedPhaseModule {
    pub namespace: String,
    pub forms: Vec<Form>,
    pub macro_names: BTreeMap<String, String>,
    /// Definition-site names that syntax quote may resolve into stable,
    /// module-qualified symbols. Values are canonical exported names.
    pub definition_names: BTreeMap<String, String>,
}

impl ImportedPhaseModule {
    #[must_use]
    pub fn new(
        namespace: impl Into<String>,
        forms: Vec<Form>,
        macro_names: BTreeMap<String, String>,
    ) -> Self {
        Self {
            namespace: namespace.into(),
            forms,
            macro_names,
            definition_names: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_definition_names(mut self, definition_names: BTreeMap<String, String>) -> Self {
        self.definition_names = definition_names;
        self
    }
}

#[derive(Clone, Debug)]
struct FunctionDef {
    name: String,
    source_name: String,
    macro_binding_id: Option<String>,
    namespace: Option<String>,
    imported: bool,
    params: Parameters,
    body: Vec<Form>,
    span: Span,
}

#[derive(Clone, Debug)]
struct Parameters {
    fixed: Vec<Pattern>,
    rest: Option<Box<Pattern>>,
}

#[derive(Clone, Debug)]
enum Pattern {
    Bind(String),
    Ignore,
    Vector(Parameters),
    Map(MapPattern),
}

#[derive(Clone, Debug)]
struct MapPattern {
    entries: Vec<MapPatternEntry>,
    defaults: BTreeMap<String, Form>,
    whole: Option<Box<Pattern>>,
}

#[derive(Clone, Debug)]
struct MapPatternEntry {
    binding: Pattern,
    lookup: Form,
}

#[derive(Clone, Debug)]
struct Lambda {
    params: Parameters,
    body: Vec<Form>,
    closure: Environment,
    namespace: Option<String>,
}

#[derive(Clone, Debug)]
enum Callable {
    Builtin(&'static str),
    User(String),
    Lambda(Rc<Lambda>),
}

#[derive(Clone, Debug)]
enum Value {
    Data(Form),
    Callable(Callable),
    Reduced(Box<Value>),
}

impl Value {
    fn into_data(self, span: Span) -> Result<Form, EvalError> {
        match self {
            Self::Data(form) => Ok(form),
            Self::Callable(_) => Err(EvalError::evaluation(
                "a phase-1 function cannot be used as syntax data",
                span,
            )),
            Self::Reduced(_) => Err(EvalError::evaluation(
                "a reduced phase-1 value must be consumed by `reduce` or `unreduced`",
                span,
            )),
        }
    }
}

type Environment = BTreeMap<String, Value>;

#[derive(Clone, Copy, Debug, Default)]
struct EvalBudget {
    steps: usize,
}

#[derive(Clone, Debug)]
struct EvalError {
    code: &'static str,
    message: String,
    span: Span,
}

impl EvalError {
    fn new(code: &'static str, message: impl Into<String>, span: Span) -> Self {
        Self {
            code,
            message: message.into(),
            span,
        }
    }

    fn evaluation(message: impl Into<String>, span: Span) -> Self {
        Self::new("OSR-M0004", message, span)
    }
}

/// Expands prelude and user macros in a recovered reader document.
#[must_use]
pub fn expand(document: &Document, options: ExpansionOptions) -> ExpansionResult {
    expand_with_imported_phase_forms(document, &[], options)
}

/// Expands a document after loading data-only phase-1 declarations from
/// dependency interfaces. Imported forms use the same parser, evaluator,
/// budgets, hygiene machinery, and diagnostics as local declarations.
#[must_use]
pub fn expand_with_imported_phase_forms(
    document: &Document,
    imported_phase_forms: &[Form],
    options: ExpansionOptions,
) -> ExpansionResult {
    let module_name = document_module_name(document).unwrap_or("osiris.anonymous");
    let mut expander = Expander::new(options, module_name);
    collect_bootstrap_prelude(&mut expander);
    expander.collect_phase_one_declarations(imported_phase_forms);
    expand_document(document, expander)
}

/// Expands a document after loading isolated phase-1 modules from dependency
/// interfaces. Private helpers stay inside their definition namespace and
/// imported macros are visible only through `macro_names` entries.
#[must_use]
pub fn expand_with_imported_phase_modules(
    document: &Document,
    imported_phase_modules: &[ImportedPhaseModule],
    options: ExpansionOptions,
) -> ExpansionResult {
    expand_with_imported_phase_modules_for_module(
        document,
        imported_phase_modules,
        "osiris.anonymous",
        options,
    )
}

/// Expands isolated phase-1 modules while assigning local macros the same
/// stable module identity used by later compiler passes.
#[must_use]
pub fn expand_with_imported_phase_modules_for_module(
    document: &Document,
    imported_phase_modules: &[ImportedPhaseModule],
    fallback_module_name: &str,
    options: ExpansionOptions,
) -> ExpansionResult {
    let module_name = document_module_name(document).unwrap_or(fallback_module_name);
    let mut expander = Expander::new(options, module_name);
    collect_bootstrap_prelude(&mut expander);
    expander.collect_imported_phase_modules(imported_phase_modules);
    expand_document(document, expander)
}

fn collect_bootstrap_prelude(expander: &mut Expander) {
    let prelude = crate::reader::read(BOOTSTRAP_PRELUDE);
    debug_assert!(
        prelude.diagnostics.is_empty(),
        "bootstrap prelude must remain valid Osiris source: {:?}",
        prelude.diagnostics
    );
    expander.collect_phase_one_declarations_in_module(&prelude.forms, "osiris.prelude");
}

fn document_module_name(document: &Document) -> Option<&str> {
    document.forms.iter().find_map(|form| {
        let FormKind::List(items) = &form.kind else {
            return None;
        };
        (items.first().and_then(symbol_canonical) == Some("module"))
            .then(|| items.get(1).and_then(symbol_canonical))
            .flatten()
    })
}

fn expand_document(document: &Document, mut expander: Expander) -> ExpansionResult {
    expander.collect_phase_one_declarations(&document.forms);
    let forms = document
        .forms
        .iter()
        .flat_map(|form| expander.expand_top_level_forms(form))
        .collect();
    let mut diagnostics = document.diagnostics.clone();
    diagnostics.append(&mut expander.diagnostics);
    diagnostics
        .sort_by_key(|diagnostic| (diagnostic.span.start, diagnostic.span.end, diagnostic.code));

    ExpansionResult {
        document: Document {
            format_version: document.format_version,
            source_len: document.source_len,
            tokens: document.tokens.clone(),
            forms,
            nodes: Vec::new(),
            diagnostics,
        },
        traces: expander.traces,
    }
}

/// Validate replayable declarations with the evaluator's own declaration
/// parser. Callers remain responsible for rejecting non-phase forms.
#[must_use]
pub fn validate_phase_forms(forms: &[Form]) -> Vec<Diagnostic> {
    let mut expander = Expander::new(ExpansionOptions::default(), "osiris.validation");
    expander.collect_phase_one_declarations(forms);
    expander.diagnostics
}

struct Expander {
    options: ExpansionOptions,
    local_module_name: String,
    expansions: usize,
    next_generated_name: u64,
    macros: BTreeMap<String, FunctionDef>,
    macro_exports: BTreeMap<String, String>,
    phase_functions: BTreeMap<String, FunctionDef>,
    definition_names: BTreeMap<String, BTreeMap<String, String>>,
    active_phase_namespace: Option<String>,
    active_origins: Vec<Span>,
    diagnostics: Vec<Diagnostic>,
    traces: Vec<ExpansionTrace>,
}

impl Expander {
    fn new(options: ExpansionOptions, local_module_name: impl Into<String>) -> Self {
        Self {
            options,
            local_module_name: local_module_name.into(),
            expansions: 0,
            next_generated_name: 0,
            macros: BTreeMap::new(),
            macro_exports: BTreeMap::new(),
            phase_functions: BTreeMap::new(),
            definition_names: BTreeMap::new(),
            active_phase_namespace: None,
            active_origins: Vec::new(),
            diagnostics: Vec::new(),
            traces: Vec::new(),
        }
    }

    fn collect_phase_one_declarations(&mut self, forms: &[Form]) {
        let module_name = self.local_module_name.clone();
        self.collect_phase_one_declarations_scoped(forms, None, &module_name);
    }

    fn collect_phase_one_declarations_in_module(&mut self, forms: &[Form], module_name: &str) {
        self.collect_phase_one_declarations_scoped(forms, None, module_name);
    }

    fn collect_imported_phase_modules(&mut self, modules: &[ImportedPhaseModule]) {
        let mut grouped = BTreeMap::<&str, Vec<&ImportedPhaseModule>>::new();
        for module in modules {
            if module.namespace.trim().is_empty() {
                self.diagnostics.push(Diagnostic::error(
                    "OSR-M0003",
                    "imported phase-1 module requires a non-empty namespace",
                    module
                        .forms
                        .first()
                        .map_or(Span::default(), |form| form.span),
                ));
                continue;
            }
            grouped
                .entry(module.namespace.as_str())
                .or_default()
                .push(module);
        }

        for (namespace, group) in grouped {
            let span = group
                .iter()
                .flat_map(|module| module.forms.iter())
                .map(|form| form.span)
                .min_by_key(|span| (span.start, span.end))
                .unwrap_or_default();
            let forms = group[0].forms.as_slice();
            if group
                .iter()
                .skip(1)
                .any(|module| module.forms.as_slice() != forms)
            {
                self.diagnostics.push(Diagnostic::error(
                    "OSR-M0003",
                    format!(
                        "imported phase-1 namespace `{namespace}` was loaded with inconsistent declarations"
                    ),
                    span,
                ));
                continue;
            }
            let definition_names = group[0].definition_names.clone();
            if group
                .iter()
                .skip(1)
                .any(|module| module.definition_names != definition_names)
            {
                self.diagnostics.push(Diagnostic::error(
                    "OSR-M0003",
                    format!(
                        "imported phase-1 namespace `{namespace}` was loaded with inconsistent definition names"
                    ),
                    span,
                ));
                continue;
            }
            self.definition_names
                .insert(namespace.to_owned(), definition_names);

            let mut macro_names = BTreeMap::<String, String>::new();
            let mut conflicting_names = BTreeSet::new();
            for module in group {
                for (visible, target) in &module.macro_names {
                    match macro_names.get(visible) {
                        Some(existing) if existing != target => {
                            conflicting_names.insert(visible.clone());
                        }
                        Some(_) => {}
                        None => {
                            macro_names.insert(visible.clone(), target.clone());
                        }
                    }
                }
            }
            for name in conflicting_names {
                macro_names.remove(&name);
                self.diagnostics.push(Diagnostic::error(
                    "OSR-M0003",
                    format!(
                        "imported phase-1 namespace `{namespace}` exposes `{name}` with conflicting targets"
                    ),
                    span,
                ));
            }

            let definitions =
                self.collect_phase_one_declarations_scoped(forms, Some(namespace), namespace);
            for (visible, target) in macro_names {
                let internal = definitions.get(&target).cloned().or_else(|| {
                    let target_short = target.rsplit('/').next().unwrap_or(&target);
                    let mut matches = definitions
                        .iter()
                        .filter(|(source, _)| {
                            source.rsplit('/').next().unwrap_or(source) == target_short
                        })
                        .map(|(_, internal)| internal.clone());
                    let first = matches.next()?;
                    matches.next().is_none().then_some(first)
                });
                let Some(internal) = internal else {
                    self.diagnostics.push(Diagnostic::error(
                        "OSR-M0003",
                        format!(
                            "imported phase-1 namespace `{namespace}` exposes unknown or ambiguous macro `{target}`"
                        ),
                        span,
                    ));
                    continue;
                };
                if let Some(existing) = self.macro_exports.get(&visible) {
                    if existing != &internal {
                        self.diagnostics.push(Diagnostic::error(
                            "OSR-M0003",
                            format!("imported macro name `{visible}` has conflicting definitions"),
                            span,
                        ));
                    }
                    continue;
                }
                if self
                    .macros
                    .get(&visible)
                    .is_some_and(|definition| !definition.imported)
                {
                    self.diagnostics.push(Diagnostic::error(
                        "OSR-M0003",
                        format!("imported macro name `{visible}` conflicts with a local macro"),
                        span,
                    ));
                    continue;
                }
                self.macro_exports.insert(visible, internal);
            }
        }
    }

    fn collect_phase_one_declarations_scoped(
        &mut self,
        forms: &[Form],
        namespace: Option<&str>,
        binding_module: &str,
    ) -> BTreeMap<String, String> {
        let mut imported_macros = BTreeMap::new();
        for form in forms {
            let FormKind::List(items) = &form.kind else {
                continue;
            };
            let Some(head) = items.first().and_then(symbol_canonical) else {
                continue;
            };
            let kind = match head {
                "defmacro" => PhaseDeclarationKind::Macro,
                "defn-for-syntax" => PhaseDeclarationKind::Function,
                _ => continue,
            };
            let mut definition = match parse_phase_declaration(form, kind) {
                Ok(definition) => definition,
                Err(message) => {
                    self.diagnostics
                        .push(Diagnostic::error("OSR-M0003", message, form.span));
                    continue;
                }
            };
            if let Some(namespace) = namespace {
                definition.name = scoped_phase_name(namespace, &definition.source_name);
                definition.namespace = Some(namespace.to_owned());
                definition.imported = true;
            }
            if matches!(kind, PhaseDeclarationKind::Macro) {
                definition.macro_binding_id = Some(
                    BindingId::new(binding_module, &definition.source_name, BindingKind::Macro)
                        .as_str()
                        .to_owned(),
                );
            }
            if self.macros.contains_key(&definition.name)
                || self.phase_functions.contains_key(&definition.name)
            {
                self.diagnostics.push(Diagnostic::error(
                    "OSR-M0003",
                    format!("duplicate phase-1 declaration `{}`", definition.name),
                    definition.span,
                ));
                continue;
            }
            match kind {
                PhaseDeclarationKind::Macro => {
                    imported_macros.insert(definition.source_name.clone(), definition.name.clone());
                    self.macros.insert(definition.name.clone(), definition);
                }
                PhaseDeclarationKind::Function => {
                    self.phase_functions
                        .insert(definition.name.clone(), definition);
                }
            }
        }
        imported_macros
    }

    fn expand_form(&mut self, form: &Form, depth: usize) -> Form {
        match &form.kind {
            FormKind::List(items) => self.expand_list(form, items, depth),
            FormKind::Vector(items) => Self::with_kind(
                form,
                FormKind::Vector(
                    items
                        .iter()
                        .map(|item| self.expand_form(item, depth))
                        .collect(),
                ),
            ),
            FormKind::Map(items) => Self::with_kind(
                form,
                FormKind::Map(
                    items
                        .iter()
                        .map(|item| self.expand_form(item, depth))
                        .collect(),
                ),
            ),
            FormKind::Set(items) => Self::with_kind(
                form,
                FormKind::Set(
                    items
                        .iter()
                        .map(|item| self.expand_form(item, depth))
                        .collect(),
                ),
            ),
            // Quoted data and phase-1 templates are not runtime macro calls.
            FormKind::ReaderMacro {
                macro_kind: ReaderMacroKind::Quote | ReaderMacroKind::SyntaxQuote,
                ..
            } => form.clone(),
            FormKind::ReaderMacro {
                macro_kind,
                form: body,
            } => Self::with_kind(
                form,
                FormKind::ReaderMacro {
                    macro_kind: *macro_kind,
                    form: Box::new(self.expand_form(body, depth)),
                },
            ),
            _ => form.clone(),
        }
    }

    /// Expand one module-level form while preserving the dependency graph
    /// boundary. Header and phase declarations are authored compiler inputs;
    /// allowing a macro to create one after graph construction would make
    /// imports and phase-1 bindings differ between passes.
    fn expand_top_level_forms(&mut self, form: &Form) -> Vec<Form> {
        if top_level_boundary_head(form).is_some() {
            return vec![form.clone()];
        }

        let trace_start = self.traces.len();
        let expanded = self.expand_form(form, 0);
        if self.traces.len() > trace_start {
            if let Some(head) = generated_top_level_boundary_head(&expanded) {
                self.diagnostics.push(Diagnostic::error(
                    "OSR-M0008",
                    format!("macro expansion cannot generate top-level declaration `{head}`"),
                    form.span,
                ));
                return vec![error_form(
                    "macro-generated top-level declaration",
                    form.span,
                )];
            }
            if let Some(declarations) = generated_declaration_sequence(&expanded) {
                return declarations;
            }
        }
        vec![expanded]
    }

    fn expand_list(&mut self, form: &Form, items: &[Form], depth: usize) -> Form {
        let Some(head) = items.first().and_then(symbol_canonical) else {
            return self.expand_list_children(form, items, depth);
        };

        if is_phase_one_declaration(head) {
            return form.clone();
        }

        let short_name = head.rsplit('/').next().unwrap_or(head);
        let user_macro = self
            .macros
            .get(head)
            .filter(|definition| !definition.imported)
            .or_else(|| {
                self.macros
                    .get(short_name)
                    .filter(|definition| !definition.imported)
            })
            .or_else(|| {
                self.macro_exports
                    .get(head)
                    .and_then(|target| self.macros.get(target))
            })
            .cloned();
        if user_macro.is_none() {
            return self.expand_list_children(form, items, depth);
        }
        if self.expansions >= self.options.max_expansions {
            self.diagnostics.push(Diagnostic::error(
                "OSR-M0002",
                format!(
                    "macro expansion exceeded the limit of {} calls",
                    self.options.max_expansions
                ),
                form.span,
            ));
            return error_form("macro expansion limit", form.span);
        }
        self.expansions += 1;
        let expanded = match self.evaluate_macro(
            user_macro.as_ref().expect("macro presence checked"),
            form,
            &items[1..],
        ) {
            Ok(expanded) => Some(expanded),
            Err(error) => {
                self.diagnostics
                    .push(Diagnostic::error(error.code, error.message, error.span));
                Some(error_form("phase-1 evaluation failed", form.span))
            }
        };
        let Some(mut expanded) = expanded else {
            return self.expand_list_children(form, items, depth);
        };
        if form_node_count(&expanded) > DEFAULT_MAX_RESULT_NODES {
            self.diagnostics.push(Diagnostic::error(
                "OSR-M0006",
                format!(
                    "macro expansion result exceeded the limit of {DEFAULT_MAX_RESULT_NODES} forms"
                ),
                form.span,
            ));
            return error_form("macro expansion result limit", form.span);
        }
        expanded.metadata = merge_call_metadata(&form.metadata, &expanded.metadata);
        if let Err(exceeded) = check_metadata_resources(&expanded.metadata, METADATA_TARGET_LIMITS)
        {
            self.diagnostics.push(Diagnostic::error(
                "OSR-M0009",
                format!(
                    "metadata for one syntax target exceeds the {} limit of {} (found {})",
                    exceeded.resource, exceeded.limit, exceeded.actual
                ),
                form.span,
            ));
            return error_form("macro expansion metadata limit", form.span);
        }
        expanded.span = form.span;
        expanded.datum_span = form.datum_span;
        let mut origin = self.active_origins.clone();
        origin.push(form.span);
        self.traces.push(ExpansionTrace {
            macro_name: short_name.to_owned(),
            macro_binding_id: user_macro
                .as_ref()
                .and_then(|definition| definition.macro_binding_id.clone())
                .expect("every collected macro has a stable binding id"),
            call_span: form.span,
            expansion_span: expanded.span,
            depth,
            origin,
        });

        if self.options.once {
            expanded
        } else {
            self.active_origins.push(form.span);
            let recursively_expanded = self.expand_form(&expanded, depth + 1);
            self.active_origins.pop();
            recursively_expanded
        }
    }

    fn expand_list_children(&mut self, form: &Form, items: &[Form], depth: usize) -> Form {
        Self::with_kind(
            form,
            FormKind::List(
                items
                    .iter()
                    .map(|item| self.expand_form(item, depth))
                    .collect(),
            ),
        )
    }

    fn evaluate_macro(
        &mut self,
        definition: &FunctionDef,
        call: &Form,
        arguments: &[Form],
    ) -> Result<Form, EvalError> {
        let values = arguments
            .iter()
            .cloned()
            .map(Value::Data)
            .collect::<Vec<_>>();
        let mut budget = EvalBudget::default();
        let mut environment = Environment::new();
        let previous_namespace = std::mem::replace(
            &mut self.active_phase_namespace,
            definition.namespace.clone(),
        );
        let result = (|| {
            bind_parameters(
                &mut BindContext {
                    expander: self,
                    environment: &mut environment,
                    budget: &mut budget,
                    span: call.span,
                    depth: 0,
                },
                &definition.params,
                &values,
                true,
            )?;
            environment.insert("&form".to_owned(), Value::Data(call.clone()));
            self.eval_body(
                &definition.body,
                &mut environment,
                &mut budget,
                0,
                definition.span,
            )
        })();
        self.active_phase_namespace = previous_namespace;
        result?.into_data(call.span)
    }

    fn eval(
        &mut self,
        form: &Form,
        environment: &mut Environment,
        budget: &mut EvalBudget,
        depth: usize,
    ) -> Result<Value, EvalError> {
        tick_budget(budget, depth, form.span)?;
        match &form.kind {
            FormKind::Symbol(name) => {
                if let Some(value) = environment.get(&name.canonical) {
                    return Ok(value.clone());
                }
                if let Some(namespace) = &self.active_phase_namespace {
                    let scoped = scoped_phase_name(namespace, &name.canonical);
                    if self.phase_functions.contains_key(&scoped) {
                        return Ok(Value::Callable(Callable::User(scoped)));
                    }
                }
                if self.phase_functions.contains_key(&name.canonical) {
                    return Ok(Value::Callable(Callable::User(name.canonical.clone())));
                }
                if let Some(name) = builtin_name(&name.canonical) {
                    return Ok(Value::Callable(Callable::Builtin(name)));
                }
                Err(EvalError::evaluation(
                    format!("unbound phase-1 name `{}`", name.spelling),
                    form.span,
                ))
            }
            FormKind::List(items) => self.eval_list(form, items, environment, budget, depth + 1),
            FormKind::Vector(items) => {
                let items = self.eval_collection(items, environment, budget, depth + 1)?;
                Ok(Value::Data(Self::with_kind(form, FormKind::Vector(items))))
            }
            FormKind::Map(items) => {
                let items = self.eval_collection(items, environment, budget, depth + 1)?;
                Ok(Value::Data(Self::with_kind(form, FormKind::Map(items))))
            }
            FormKind::Set(items) => {
                let items = self.eval_collection(items, environment, budget, depth + 1)?;
                Ok(Value::Data(Self::with_kind(form, FormKind::Set(items))))
            }
            FormKind::ReaderMacro {
                macro_kind: ReaderMacroKind::Quote,
                form: quoted,
            } => Ok(Value::Data((**quoted).clone())),
            FormKind::ReaderMacro {
                macro_kind: ReaderMacroKind::SyntaxQuote,
                form: template,
            } => {
                let mut generated = BTreeMap::new();
                self.syntax_quote(template, environment, budget, depth + 1, &mut generated)
                    .map(Value::Data)
            }
            FormKind::ReaderMacro {
                macro_kind: ReaderMacroKind::Unquote | ReaderMacroKind::UnquoteSplicing,
                ..
            } => Err(EvalError::evaluation(
                "unquote is only valid inside syntax quote",
                form.span,
            )),
            FormKind::Error(message) => Err(EvalError::evaluation(
                format!("cannot evaluate recovered syntax error: {message}"),
                form.span,
            )),
            _ => Ok(Value::Data(form.clone())),
        }
    }

    fn eval_collection(
        &mut self,
        items: &[Form],
        environment: &mut Environment,
        budget: &mut EvalBudget,
        depth: usize,
    ) -> Result<Vec<Form>, EvalError> {
        let mut evaluated = Vec::with_capacity(items.len());
        for item in items {
            evaluated.push(
                self.eval(item, environment, budget, depth)?
                    .into_data(item.span)?,
            );
        }
        Ok(evaluated)
    }

    fn eval_list(
        &mut self,
        form: &Form,
        items: &[Form],
        environment: &mut Environment,
        budget: &mut EvalBudget,
        depth: usize,
    ) -> Result<Value, EvalError> {
        let Some(head) = items.first() else {
            return Ok(Value::Data(form.clone()));
        };
        match symbol_canonical(head) {
            Some("quote") => {
                require_form_arity(items, 2, "quote", form.span)?;
                return Ok(Value::Data(items[1].clone()));
            }
            Some("if") => {
                if !(3..=4).contains(&items.len()) {
                    return Err(EvalError::evaluation(
                        "`if` expects a condition, then branch, and optional else branch",
                        form.span,
                    ));
                }
                let condition = self.eval(&items[1], environment, budget, depth)?;
                if is_truthy(&condition) {
                    return self.eval(&items[2], environment, budget, depth);
                }
                return match items.get(3) {
                    Some(alternative) => self.eval(alternative, environment, budget, depth),
                    None => Ok(Value::Data(none(form.span))),
                };
            }
            Some("do") => {
                return self.eval_body(&items[1..], environment, budget, depth, form.span);
            }
            Some("let") => {
                return self.eval_let(form, items, environment, budget, depth);
            }
            Some("fn") => {
                if items.len() < 3 {
                    return Err(EvalError::evaluation(
                        "`fn` expects a parameter vector and body",
                        form.span,
                    ));
                }
                let params = parse_parameters(&items[1])
                    .map_err(|message| EvalError::evaluation(message, items[1].span))?;
                return Ok(Value::Callable(Callable::Lambda(Rc::new(Lambda {
                    params,
                    body: items[2..].to_vec(),
                    closure: environment.clone(),
                    namespace: self.active_phase_namespace.clone(),
                }))));
            }
            Some("and") => {
                let mut value = Value::Data(boolean(true, form.span));
                for expression in &items[1..] {
                    value = self.eval(expression, environment, budget, depth)?;
                    if !is_truthy(&value) {
                        break;
                    }
                }
                return Ok(value);
            }
            Some("or") => {
                for expression in &items[1..] {
                    let value = self.eval(expression, environment, budget, depth)?;
                    if is_truthy(&value) {
                        return Ok(value);
                    }
                }
                return Ok(Value::Data(none(form.span)));
            }
            Some("cond") => {
                if items.len() % 2 == 0 {
                    return Err(EvalError::evaluation(
                        "`cond` expects condition/result pairs",
                        form.span,
                    ));
                }
                for clause in items[1..].chunks_exact(2) {
                    let matches =
                        matches!(
                            &clause[0].kind,
                            FormKind::Keyword(name) if name.canonical == ":else"
                        ) || is_truthy(&self.eval(&clause[0], environment, budget, depth)?);
                    if matches {
                        return self.eval(&clause[1], environment, budget, depth);
                    }
                }
                return Ok(Value::Data(none(form.span)));
            }
            _ => {}
        }

        let callable = match self.eval(head, environment, budget, depth)? {
            Value::Callable(callable) => callable,
            Value::Data(_) | Value::Reduced(_) => {
                return Err(EvalError::evaluation(
                    "the first item in a phase-1 call must be a function",
                    head.span,
                ));
            }
        };
        let mut arguments = Vec::with_capacity(items.len().saturating_sub(1));
        for argument in &items[1..] {
            arguments.push(self.eval(argument, environment, budget, depth)?);
        }
        self.invoke_callable(callable, arguments, form.span, budget, depth)
    }

    fn eval_let(
        &mut self,
        form: &Form,
        items: &[Form],
        environment: &mut Environment,
        budget: &mut EvalBudget,
        depth: usize,
    ) -> Result<Value, EvalError> {
        if items.len() < 3 {
            return Err(EvalError::evaluation(
                "`let` expects a binding vector and body",
                form.span,
            ));
        }
        let FormKind::Vector(bindings) = &items[1].kind else {
            return Err(EvalError::evaluation(
                "`let` bindings must be a vector",
                items[1].span,
            ));
        };
        if bindings.len() % 2 != 0 {
            return Err(EvalError::evaluation(
                "`let` bindings must contain pattern/value pairs",
                items[1].span,
            ));
        }
        let mut local = environment.clone();
        for binding in bindings.chunks_exact(2) {
            let pattern = parse_pattern(&binding[0])
                .map_err(|message| EvalError::evaluation(message, binding[0].span))?;
            let value = self.eval(&binding[1], &mut local, budget, depth)?;
            bind_pattern(
                &mut BindContext {
                    expander: self,
                    environment: &mut local,
                    budget,
                    span: binding[0].span,
                    depth: depth + 1,
                },
                &pattern,
                value,
            )?;
        }
        self.eval_body(&items[2..], &mut local, budget, depth, form.span)
    }

    fn eval_body(
        &mut self,
        body: &[Form],
        environment: &mut Environment,
        budget: &mut EvalBudget,
        depth: usize,
        fallback_span: Span,
    ) -> Result<Value, EvalError> {
        let mut result = Value::Data(none(fallback_span));
        for expression in body {
            result = self.eval(expression, environment, budget, depth)?;
        }
        Ok(result)
    }

    fn invoke_callable(
        &mut self,
        callable: Callable,
        arguments: Vec<Value>,
        span: Span,
        budget: &mut EvalBudget,
        depth: usize,
    ) -> Result<Value, EvalError> {
        tick_budget(budget, depth, span)?;
        match callable {
            Callable::Builtin(name) => {
                self.invoke_builtin(name, arguments, span, budget, depth + 1)
            }
            Callable::User(name) => {
                let definition = self.phase_functions.get(&name).cloned().ok_or_else(|| {
                    EvalError::evaluation(format!("unknown phase-1 function `{name}`"), span)
                })?;
                let mut environment = Environment::new();
                let previous_namespace = std::mem::replace(
                    &mut self.active_phase_namespace,
                    definition.namespace.clone(),
                );
                let result = (|| {
                    bind_parameters(
                        &mut BindContext {
                            expander: self,
                            environment: &mut environment,
                            budget,
                            span,
                            depth: depth + 1,
                        },
                        &definition.params,
                        &arguments,
                        false,
                    )?;
                    self.eval_body(
                        &definition.body,
                        &mut environment,
                        budget,
                        depth + 1,
                        definition.span,
                    )
                })();
                self.active_phase_namespace = previous_namespace;
                result
            }
            Callable::Lambda(lambda) => {
                let mut environment = lambda.closure.clone();
                let previous_namespace =
                    std::mem::replace(&mut self.active_phase_namespace, lambda.namespace.clone());
                let result = (|| {
                    bind_parameters(
                        &mut BindContext {
                            expander: self,
                            environment: &mut environment,
                            budget,
                            span,
                            depth: depth + 1,
                        },
                        &lambda.params,
                        &arguments,
                        false,
                    )?;
                    self.eval_body(&lambda.body, &mut environment, budget, depth + 1, span)
                })();
                self.active_phase_namespace = previous_namespace;
                result
            }
        }
    }

    fn invoke_builtin(
        &mut self,
        name: &'static str,
        mut arguments: Vec<Value>,
        span: Span,
        budget: &mut EvalBudget,
        depth: usize,
    ) -> Result<Value, EvalError> {
        match name {
            "identity" => {
                require_value_arity(&arguments, 1, name, span)?;
                Ok(arguments.remove(0))
            }
            "reduced" => {
                require_value_arity(&arguments, 1, name, span)?;
                Ok(Value::Reduced(Box::new(arguments.remove(0))))
            }
            "reduced?" => {
                require_value_arity(&arguments, 1, name, span)?;
                Ok(Value::Data(boolean(
                    matches!(arguments.first(), Some(Value::Reduced(_))),
                    span,
                )))
            }
            "unreduced" => {
                require_value_arity(&arguments, 1, name, span)?;
                Ok(match arguments.remove(0) {
                    Value::Reduced(value) => *value,
                    value => value,
                })
            }
            "list" => Ok(Value::Data(list(values_into_forms(arguments, span)?, span))),
            "vector" => Ok(Value::Data(vector(
                values_into_forms(arguments, span)?,
                span,
            ))),
            "hash-map" => {
                let items = values_into_forms(arguments, span)?;
                if items.len() % 2 != 0 {
                    return Err(EvalError::evaluation(
                        "`hash-map` expects key/value pairs",
                        span,
                    ));
                }
                Ok(Value::Data(Form::new(FormKind::Map(items), span)))
            }
            "hash-set" => Ok(Value::Data(Form::new(
                FormKind::Set(unique_forms(values_into_forms(arguments, span)?)),
                span,
            ))),
            "not" => {
                require_value_arity(&arguments, 1, name, span)?;
                Ok(Value::Data(boolean(!is_truthy(&arguments[0]), span)))
            }
            "=" | "not=" => {
                let forms = values_into_forms(arguments, span)?;
                let equal = forms
                    .windows(2)
                    .all(|pair| crate::syntax::datum_eq(&pair[0], &pair[1]));
                Ok(Value::Data(boolean(
                    if name == "=" { equal } else { !equal },
                    span,
                )))
            }
            "+" | "-" | "*" | "/" | "inc" | "dec" => {
                let forms = values_into_forms(arguments, span)?;
                numeric_builtin(name, &forms, span).map(Value::Data)
            }
            "<" | "<=" | ">" | ">=" => {
                let forms = values_into_forms(arguments, span)?;
                compare_builtin(name, &forms, span).map(Value::Data)
            }
            "cons" => {
                require_value_arity(&arguments, 2, name, span)?;
                let mut forms = sequence_items(
                    &arguments.pop().expect("arity checked").into_data(span)?,
                    span,
                )?;
                forms.insert(0, arguments.pop().expect("arity checked").into_data(span)?);
                Ok(Value::Data(list(forms, span)))
            }
            "concat" => {
                let mut result = Vec::new();
                for argument in arguments {
                    result.extend(sequence_items(&argument.into_data(span)?, span)?);
                }
                Ok(Value::Data(list(result, span)))
            }
            "conj" => {
                if arguments.is_empty() {
                    return Err(EvalError::evaluation(
                        "`conj` expects a collection and zero or more values",
                        span,
                    ));
                }
                let collection = arguments.remove(0).into_data(span)?;
                let values = values_into_forms(arguments, span)?;
                conj(collection, values, span).map(Value::Data)
            }
            "first" | "rest" | "next" => {
                require_value_arity(&arguments, 1, name, span)?;
                let collection = arguments.remove(0).into_data(span)?;
                let items = sequence_items(&collection, span)?;
                let result = match name {
                    "first" => items.first().cloned().unwrap_or_else(|| none(span)),
                    "rest" => list(items.into_iter().skip(1).collect(), span),
                    "next" if items.len() <= 1 => none(span),
                    "next" => list(items.into_iter().skip(1).collect(), span),
                    _ => unreachable!(),
                };
                Ok(Value::Data(result))
            }
            "nth" => {
                if !(2..=3).contains(&arguments.len()) {
                    return Err(EvalError::evaluation(
                        "`nth` expects a collection, index, and optional default",
                        span,
                    ));
                }
                let collection = arguments.remove(0).into_data(span)?;
                let index = form_to_usize(&arguments.remove(0).into_data(span)?, span)?;
                let default = arguments
                    .pop()
                    .map(|value| value.into_data(span))
                    .transpose()?;
                sequence_items(&collection, span)?
                    .get(index)
                    .cloned()
                    .or(default)
                    .map(Value::Data)
                    .ok_or_else(|| EvalError::evaluation("`nth` index is out of bounds", span))
            }
            "count" => {
                require_value_arity(&arguments, 1, name, span)?;
                let form = arguments.remove(0).into_data(span)?;
                let count = collection_count(&form, span)?;
                Ok(Value::Data(integer(count, span)))
            }
            "empty?" => {
                require_value_arity(&arguments, 1, name, span)?;
                let form = arguments.remove(0).into_data(span)?;
                Ok(Value::Data(boolean(
                    collection_count(&form, span)? == 0,
                    span,
                )))
            }
            "seq" => {
                require_value_arity(&arguments, 1, name, span)?;
                let form = arguments.remove(0).into_data(span)?;
                let items = sequence_items(&form, span)?;
                Ok(Value::Data(if items.is_empty() {
                    none(span)
                } else {
                    list(items, span)
                }))
            }
            "get" => {
                if !(2..=3).contains(&arguments.len()) {
                    return Err(EvalError::evaluation(
                        "`get` expects a collection, key, and optional default",
                        span,
                    ));
                }
                let collection = arguments.remove(0).into_data(span)?;
                let key = arguments.remove(0).into_data(span)?;
                let default = arguments
                    .pop()
                    .map(|value| value.into_data(span))
                    .transpose()?
                    .unwrap_or_else(|| none(span));
                Ok(Value::Data(
                    get_from_collection(&collection, &key).unwrap_or(default),
                ))
            }
            "contains?" => {
                require_value_arity(&arguments, 2, name, span)?;
                let collection = arguments.remove(0).into_data(span)?;
                let key = arguments.remove(0).into_data(span)?;
                Ok(Value::Data(boolean(
                    collection_contains(&collection, &key),
                    span,
                )))
            }
            "assoc" => {
                if arguments.len() < 3 || arguments.len() % 2 == 0 {
                    return Err(EvalError::evaluation(
                        "`assoc` expects a map and key/value pairs",
                        span,
                    ));
                }
                let map = arguments.remove(0).into_data(span)?;
                assoc_form(map, values_into_forms(arguments, span)?, span).map(Value::Data)
            }
            "dissoc" => {
                if arguments.is_empty() {
                    return Err(EvalError::evaluation(
                        "`dissoc` expects a map and zero or more keys",
                        span,
                    ));
                }
                let map = arguments.remove(0).into_data(span)?;
                dissoc_form(map, &values_into_forms(arguments, span)?, span).map(Value::Data)
            }
            "keys" | "vals" => {
                require_value_arity(&arguments, 1, name, span)?;
                let map = arguments.remove(0).into_data(span)?;
                let FormKind::Map(items) = map.kind else {
                    return Err(EvalError::evaluation(
                        format!("`{name}` expects a map"),
                        span,
                    ));
                };
                let offset = usize::from(name == "vals");
                Ok(Value::Data(list(
                    items.into_iter().skip(offset).step_by(2).collect(),
                    span,
                )))
            }
            "meta" => {
                require_value_arity(&arguments, 1, name, span)?;
                let target = arguments.remove(0).into_data(span)?;
                Ok(Value::Data(metadata_map(&target)))
            }
            "with-meta" => {
                require_value_arity(&arguments, 2, name, span)?;
                let target = arguments.remove(0).into_data(span)?;
                let metadata = arguments.remove(0).into_data(span)?;
                with_metadata(target, &metadata, span).map(Value::Data)
            }
            "vary-meta" => {
                if arguments.len() < 2 {
                    return Err(EvalError::evaluation(
                        "`vary-meta` expects syntax, a function, and optional arguments",
                        span,
                    ));
                }
                let target = arguments.remove(0).into_data(span)?;
                let callable = value_callable(arguments.remove(0), span)?;
                let mut call_arguments = vec![Value::Data(metadata_map(&target))];
                call_arguments.extend(arguments);
                let metadata = self
                    .invoke_callable(callable, call_arguments, span, budget, depth + 1)?
                    .into_data(span)?;
                with_metadata(target, &metadata, span).map(Value::Data)
            }
            "gensym" => {
                if arguments.len() > 1 {
                    return Err(EvalError::evaluation(
                        "`gensym` accepts at most one prefix",
                        span,
                    ));
                }
                let prefix = arguments
                    .pop()
                    .map(|value| value.into_data(span))
                    .transpose()?
                    .map(|form| form_name_or_string(&form, span))
                    .transpose()?
                    .unwrap_or_else(|| "G__".to_owned());
                Ok(Value::Data(self.generated_symbol(&prefix, span)))
            }
            "syntax-error" => {
                if arguments.is_empty() || arguments.len() > 2 {
                    return Err(EvalError::evaluation(
                        "`syntax-error` expects a message, optionally preceded by syntax",
                        span,
                    ));
                }
                let (error_span, message_value) = if arguments.len() == 2 {
                    let target = arguments.remove(0).into_data(span)?;
                    (target.span, arguments.remove(0).into_data(span)?)
                } else {
                    (span, arguments.remove(0).into_data(span)?)
                };
                let message = form_to_string(&message_value, span)?;
                Err(EvalError::new("OSR-M0007", message, error_span))
            }
            "symbol" | "keyword" => {
                require_value_arity(&arguments, 1, name, span)?;
                let spelling = form_to_string(&arguments.remove(0).into_data(span)?, span)?;
                let spelling = if name == "keyword" && !spelling.starts_with(':') {
                    format!(":{spelling}")
                } else {
                    spelling
                };
                Ok(Value::Data(named_form(name == "keyword", &spelling, span)))
            }
            "name" | "namespace" => {
                require_value_arity(&arguments, 1, name, span)?;
                let form = arguments.remove(0).into_data(span)?;
                let full = form_name_or_string(&form, span)?;
                let trimmed = full.strip_prefix(':').unwrap_or(&full);
                let value = if name == "name" {
                    trimmed.rsplit('/').next().unwrap_or(trimmed).to_owned()
                } else {
                    trimmed
                        .rsplit_once('/')
                        .map(|(namespace, _)| namespace.to_owned())
                        .unwrap_or_default()
                };
                Ok(Value::Data(string(&value, span)))
            }
            "str" => {
                let forms = values_into_forms(arguments, span)?;
                let mut value = String::new();
                for form in forms {
                    value.push_str(&display_form(&form));
                }
                Ok(Value::Data(string(&value, span)))
            }
            "nil?" | "some?" | "symbol?" | "keyword?" | "list?" | "vector?" | "map?" | "set?"
            | "sequential?" => {
                require_value_arity(&arguments, 1, name, span)?;
                let form = arguments.remove(0).into_data(span)?;
                let matches = match name {
                    "nil?" => matches!(form.kind, FormKind::None),
                    "some?" => !matches!(form.kind, FormKind::None),
                    "symbol?" => matches!(form.kind, FormKind::Symbol(_)),
                    "keyword?" => matches!(form.kind, FormKind::Keyword(_)),
                    "list?" => matches!(form.kind, FormKind::List(_)),
                    "vector?" => matches!(form.kind, FormKind::Vector(_)),
                    "map?" => matches!(form.kind, FormKind::Map(_)),
                    "set?" => matches!(form.kind, FormKind::Set(_)),
                    "sequential?" => {
                        matches!(form.kind, FormKind::List(_) | FormKind::Vector(_))
                    }
                    _ => unreachable!(),
                };
                Ok(Value::Data(boolean(matches, span)))
            }
            "apply" => {
                if arguments.len() < 2 {
                    return Err(EvalError::evaluation(
                        "`apply` expects a function and an argument sequence",
                        span,
                    ));
                }
                let callable = value_callable(arguments.remove(0), span)?;
                let tail = arguments.pop().expect("length checked").into_data(span)?;
                arguments.extend(sequence_items(&tail, span)?.into_iter().map(Value::Data));
                self.invoke_callable(callable, arguments, span, budget, depth + 1)
            }
            "map" | "mapv" => {
                if arguments.len() < 2 {
                    return Err(EvalError::evaluation(
                        format!("`{name}` expects a function and collections"),
                        span,
                    ));
                }
                let callable = value_callable(arguments.remove(0), span)?;
                let collections = arguments
                    .into_iter()
                    .map(|value| value.into_data(span))
                    .map(|result| result.and_then(|form| sequence_items(&form, span)))
                    .collect::<Result<Vec<_>, _>>()?;
                let length = collections.iter().map(Vec::len).min().unwrap_or(0);
                let mut mapped = Vec::with_capacity(length);
                for index in 0..length {
                    let call_arguments = collections
                        .iter()
                        .map(|collection| Value::Data(collection[index].clone()))
                        .collect();
                    mapped.push(
                        self.invoke_callable(
                            callable.clone(),
                            call_arguments,
                            span,
                            budget,
                            depth + 1,
                        )?
                        .into_data(span)?,
                    );
                }
                Ok(Value::Data(if name == "mapv" {
                    vector(mapped, span)
                } else {
                    list(mapped, span)
                }))
            }
            "mapcat" | "mapcatv" => {
                if arguments.len() != 2 {
                    return Err(EvalError::evaluation(
                        format!("`{name}` expects a function and one collection"),
                        span,
                    ));
                }
                let callable = value_callable(arguments.remove(0), span)?;
                let collection = arguments.remove(0).into_data(span)?;
                let mut flattened = Vec::new();
                for item in sequence_items(&collection, span)? {
                    let result = self
                        .invoke_callable(
                            callable.clone(),
                            vec![Value::Data(item)],
                            span,
                            budget,
                            depth + 1,
                        )?
                        .into_data(span)?;
                    flattened.extend(sequence_items(&result, span)?);
                }
                Ok(Value::Data(if name == "mapcatv" {
                    vector(flattened, span)
                } else {
                    list(flattened, span)
                }))
            }
            "filter" | "filterv" => {
                if arguments.len() != 2 {
                    return Err(EvalError::evaluation(
                        format!("`{name}` expects a predicate and one collection"),
                        span,
                    ));
                }
                let callable = value_callable(arguments.remove(0), span)?;
                let collection = arguments.remove(0).into_data(span)?;
                let mut selected = Vec::new();
                for item in sequence_items(&collection, span)? {
                    let result = self.invoke_callable(
                        callable.clone(),
                        vec![Value::Data(item.clone())],
                        span,
                        budget,
                        depth + 1,
                    )?;
                    let predicate = result.into_data(span)?;
                    if !matches!(predicate.kind, FormKind::None | FormKind::Bool(false)) {
                        selected.push(item);
                    }
                }
                Ok(Value::Data(if name == "filterv" {
                    vector(selected, span)
                } else {
                    list(selected, span)
                }))
            }
            "reduce" | "fold" => {
                let valid = if name == "fold" {
                    arguments.len() == 3
                } else {
                    (2..=3).contains(&arguments.len())
                };
                if !valid {
                    return Err(EvalError::evaluation(
                        if name == "fold" {
                            "`fold` expects a function, initial value, and collection"
                        } else {
                            "`reduce` expects a function, optional initial value, and collection"
                        },
                        span,
                    ));
                }
                let callable = value_callable(arguments.remove(0), span)?;
                let collection = arguments.pop().expect("length checked").into_data(span)?;
                let mut items = sequence_items(&collection, span)?.into_iter();
                let mut accumulator = match arguments.pop() {
                    Some(initial) => initial,
                    None => Value::Data(items.next().ok_or_else(|| {
                        EvalError::evaluation("`reduce` without an initial value needs data", span)
                    })?),
                };
                for item in items {
                    let next = self.invoke_callable(
                        callable.clone(),
                        vec![accumulator, Value::Data(item)],
                        span,
                        budget,
                        depth + 1,
                    )?;
                    match next {
                        Value::Reduced(value) => {
                            accumulator = *value;
                            break;
                        }
                        value => accumulator = value,
                    }
                }
                Ok(accumulator)
            }
            _ => Err(EvalError::evaluation(
                format!("unsupported phase-1 builtin `{name}`"),
                span,
            )),
        }
    }

    fn syntax_quote(
        &mut self,
        form: &Form,
        environment: &mut Environment,
        budget: &mut EvalBudget,
        depth: usize,
        generated: &mut BTreeMap<String, Form>,
    ) -> Result<Form, EvalError> {
        tick_budget(budget, depth, form.span)?;
        match &form.kind {
            FormKind::ReaderMacro {
                macro_kind: ReaderMacroKind::Unquote,
                form: expression,
            } => self
                .eval(expression, environment, budget, depth + 1)?
                .into_data(form.span),
            FormKind::ReaderMacro {
                macro_kind: ReaderMacroKind::UnquoteSplicing,
                ..
            } => Err(EvalError::evaluation(
                "unquote-splicing is only valid inside a syntax-quoted collection",
                form.span,
            )),
            FormKind::List(items) => self
                .syntax_quote_collection(items, environment, budget, depth + 1, generated)
                .map(|items| Self::with_kind(form, FormKind::List(items))),
            FormKind::Vector(items) => self
                .syntax_quote_collection(items, environment, budget, depth + 1, generated)
                .map(|items| Self::with_kind(form, FormKind::Vector(items))),
            FormKind::Map(items) => {
                let items =
                    self.syntax_quote_collection(items, environment, budget, depth + 1, generated)?;
                if items.len() % 2 != 0 {
                    return Err(EvalError::evaluation(
                        "syntax-quoted map contains an odd number of forms after splicing",
                        form.span,
                    ));
                }
                Ok(Self::with_kind(form, FormKind::Map(items)))
            }
            FormKind::Set(items) => self
                .syntax_quote_collection(items, environment, budget, depth + 1, generated)
                .map(|items| Self::with_kind(form, FormKind::Set(items))),
            FormKind::Symbol(name) if name.canonical.ends_with('#') => {
                if let Some(existing) = generated.get(&name.canonical) {
                    return Ok(existing.clone());
                }
                let hint = name.canonical.trim_end_matches('#');
                let generated_symbol = self.generated_symbol(hint, form.span);
                generated.insert(name.canonical.clone(), generated_symbol.clone());
                Ok(generated_symbol)
            }
            FormKind::Symbol(name) => {
                let Some(namespace) = &self.active_phase_namespace else {
                    return Ok(form.clone());
                };
                let Some(canonical) = self
                    .definition_names
                    .get(namespace)
                    .and_then(|names| names.get(&name.canonical))
                else {
                    return Ok(form.clone());
                };
                Ok(Self::with_kind(
                    form,
                    FormKind::Symbol(Name {
                        spelling: format!("{namespace}/{canonical}"),
                        canonical: format!("{namespace}/{canonical}"),
                    }),
                ))
            }
            // Quote and nested syntax quote introduce their own unquote boundary.
            FormKind::ReaderMacro {
                macro_kind: ReaderMacroKind::Quote | ReaderMacroKind::SyntaxQuote,
                ..
            } => Ok(form.clone()),
            _ => Ok(form.clone()),
        }
    }

    fn syntax_quote_collection(
        &mut self,
        items: &[Form],
        environment: &mut Environment,
        budget: &mut EvalBudget,
        depth: usize,
        generated: &mut BTreeMap<String, Form>,
    ) -> Result<Vec<Form>, EvalError> {
        let mut quoted = Vec::new();
        for item in items {
            if let FormKind::ReaderMacro {
                macro_kind: ReaderMacroKind::UnquoteSplicing,
                form: expression,
            } = &item.kind
            {
                let value = self
                    .eval(expression, environment, budget, depth + 1)?
                    .into_data(item.span)?;
                quoted.extend(sequence_items(&value, item.span)?);
            } else {
                quoted.push(self.syntax_quote(item, environment, budget, depth, generated)?);
            }
        }
        Ok(quoted)
    }

    fn generated_symbol(&mut self, hint: &str, span: Span) -> Form {
        let id = self.next_generated_name;
        self.next_generated_name += 1;
        let spelling = format!("{hint}__osr_g{id}");
        Form::new(
            FormKind::Symbol(Name {
                spelling,
                // Reader-created names always canonicalize their spelling.  A
                // separate NUL-prefixed identity therefore cannot collide with
                // a caller binding that merely has the same visible spelling.
                canonical: format!("\0osr-gensym:{id}:{hint}"),
            }),
            span,
        )
    }

    fn with_kind(original: &Form, kind: FormKind) -> Form {
        Form {
            span: original.span,
            datum_span: original.datum_span,
            metadata: original.metadata.clone(),
            kind,
        }
    }
}

#[derive(Clone, Copy)]
enum PhaseDeclarationKind {
    Macro,
    Function,
}

fn parse_phase_declaration(form: &Form, kind: PhaseDeclarationKind) -> Result<FunctionDef, String> {
    let FormKind::List(items) = &form.kind else {
        return Err("phase-1 declaration must be a list".to_owned());
    };
    let declaration = match kind {
        PhaseDeclarationKind::Macro => "defmacro",
        PhaseDeclarationKind::Function => "defn-for-syntax",
    };
    let name = items
        .get(1)
        .and_then(symbol_canonical)
        .ok_or_else(|| format!("`{declaration}` requires a symbol name"))?
        .to_owned();
    let mut index = 2;
    if matches!(
        items.get(index).map(|item| &item.kind),
        Some(FormKind::String(_))
    ) {
        index += 1;
    }
    let parameter_form = items
        .get(index)
        .ok_or_else(|| format!("`{declaration}` requires a parameter vector"))?;
    let params = parse_parameters(parameter_form)?;
    index += 1;
    if items
        .get(index)
        .and_then(symbol_canonical)
        .is_some_and(|name| name == "->")
    {
        if items.get(index + 1).is_none() {
            return Err(format!(
                "`{declaration}` return annotation is missing a type"
            ));
        }
        index += 2;
    }
    let body = items[index..].to_vec();
    if body.is_empty() {
        return Err(format!("`{declaration}` requires a body"));
    }
    Ok(FunctionDef {
        source_name: name.clone(),
        name,
        macro_binding_id: None,
        namespace: None,
        imported: false,
        params,
        body,
        span: form.span,
    })
}

fn scoped_phase_name(namespace: &str, name: &str) -> String {
    format!("{namespace}/{name}")
}

fn parse_parameters(form: &Form) -> Result<Parameters, String> {
    let FormKind::Vector(items) = &form.kind else {
        return Err("phase-1 parameters must be a vector".to_owned());
    };
    let mut fixed = Vec::new();
    let mut rest = None;
    let mut index = 0;
    while index < items.len() {
        if symbol_canonical(&items[index]) == Some("&") {
            if rest.is_some() || index + 2 != items.len() {
                return Err("`&` must precede the final variadic parameter".to_owned());
            }
            rest = Some(Box::new(parse_pattern(&items[index + 1])?));
            index += 2;
            continue;
        }
        fixed.push(parse_pattern(&items[index])?);
        index += 1;
    }
    Ok(Parameters { fixed, rest })
}

fn parse_pattern(form: &Form) -> Result<Pattern, String> {
    match &form.kind {
        FormKind::Symbol(name) if name.canonical == "_" => Ok(Pattern::Ignore),
        FormKind::Symbol(name) if name.canonical != "&" => {
            Ok(Pattern::Bind(name.canonical.clone()))
        }
        FormKind::Vector(_) => parse_parameters(form).map(Pattern::Vector),
        FormKind::Map(items) => parse_map_pattern(items).map(Pattern::Map),
        _ => Err(
            "phase-1 parameters support symbols, vector destructuring, and map destructuring"
                .to_owned(),
        ),
    }
}

fn parse_map_pattern(items: &[Form]) -> Result<MapPattern, String> {
    if items.len() % 2 != 0 {
        return Err("phase-1 map destructuring requires key/value pairs".to_owned());
    }

    let mut entries = Vec::new();
    let mut defaults = BTreeMap::new();
    let mut whole = None;
    let mut seen_options = BTreeSet::new();

    for pair in items.chunks_exact(2) {
        let option = match &pair[0].kind {
            FormKind::Keyword(name) => Some(name.canonical.as_str()),
            _ => None,
        };
        match option {
            Some(":keys" | ":strs" | ":syms") => {
                let option = option.expect("matched above");
                if !seen_options.insert(option.to_owned()) {
                    return Err(format!("duplicate `{option}` in phase-1 map destructuring"));
                }
                let FormKind::Vector(names) = &pair[1].kind else {
                    return Err(format!("`{option}` in map destructuring must be a vector"));
                };
                for name_form in names {
                    let FormKind::Symbol(name) = &name_form.kind else {
                        return Err(format!(
                            "`{option}` entries in map destructuring must be symbols"
                        ));
                    };
                    let local = name
                        .canonical
                        .rsplit('/')
                        .next()
                        .unwrap_or(&name.canonical)
                        .to_owned();
                    let lookup = match option {
                        ":keys" => {
                            named_form(true, &format!(":{}", name.canonical), name_form.span)
                        }
                        ":strs" => string(&name.canonical, name_form.span),
                        ":syms" => named_form(false, &name.canonical, name_form.span),
                        _ => unreachable!(),
                    };
                    entries.push(MapPatternEntry {
                        binding: Pattern::Bind(local),
                        lookup,
                    });
                }
            }
            Some(":or") => {
                if !seen_options.insert(":or".to_owned()) {
                    return Err("duplicate `:or` in phase-1 map destructuring".to_owned());
                }
                let FormKind::Map(values) = &pair[1].kind else {
                    return Err("`:or` in map destructuring must be a map".to_owned());
                };
                if values.len() % 2 != 0 {
                    return Err("`:or` defaults must contain key/value pairs".to_owned());
                }
                for default in values.chunks_exact(2) {
                    let Some(name) = symbol_canonical(&default[0]) else {
                        return Err("`:or` default keys must be binding symbols".to_owned());
                    };
                    if defaults
                        .insert(name.to_owned(), default[1].clone())
                        .is_some()
                    {
                        return Err(format!(
                            "duplicate default for `{name}` in map destructuring"
                        ));
                    }
                }
            }
            Some(":as") => {
                if !seen_options.insert(":as".to_owned()) {
                    return Err("duplicate `:as` in phase-1 map destructuring".to_owned());
                }
                whole = Some(Box::new(parse_pattern(&pair[1])?));
            }
            _ => entries.push(MapPatternEntry {
                binding: parse_pattern(&pair[0])?,
                lookup: pair[1].clone(),
            }),
        }
    }

    let bound_names = entries
        .iter()
        .flat_map(|entry| pattern_binding_names(&entry.binding))
        .chain(
            whole
                .iter()
                .flat_map(|pattern| pattern_binding_names(pattern)),
        )
        .collect::<BTreeSet<_>>();
    if let Some(unknown) = defaults.keys().find(|name| !bound_names.contains(*name)) {
        return Err(format!(
            "`:or` provides a default for unknown destructured binding `{unknown}`"
        ));
    }

    Ok(MapPattern {
        entries,
        defaults,
        whole,
    })
}

fn pattern_binding_names(pattern: &Pattern) -> Vec<String> {
    match pattern {
        Pattern::Bind(name) => vec![name.clone()],
        Pattern::Ignore => Vec::new(),
        Pattern::Vector(parameters) => parameters
            .fixed
            .iter()
            .flat_map(pattern_binding_names)
            .chain(
                parameters
                    .rest
                    .iter()
                    .flat_map(|pattern| pattern_binding_names(pattern)),
            )
            .collect(),
        Pattern::Map(pattern) => pattern
            .entries
            .iter()
            .flat_map(|entry| pattern_binding_names(&entry.binding))
            .chain(
                pattern
                    .whole
                    .iter()
                    .flat_map(|pattern| pattern_binding_names(pattern)),
            )
            .collect(),
    }
}

fn builtin_name(name: &str) -> Option<&'static str> {
    let short = name.rsplit('/').next().unwrap_or(name);
    match short {
        "identity" => Some("identity"),
        "reduced" => Some("reduced"),
        "reduced?" => Some("reduced?"),
        "unreduced" => Some("unreduced"),
        "list" => Some("list"),
        "vector" | "vec" => Some("vector"),
        "hash-map" => Some("hash-map"),
        "hash-set" | "set" => Some("hash-set"),
        "not" => Some("not"),
        "=" => Some("="),
        "not=" => Some("not="),
        "+" => Some("+"),
        "-" => Some("-"),
        "*" => Some("*"),
        "/" => Some("/"),
        "inc" => Some("inc"),
        "dec" => Some("dec"),
        "<" => Some("<"),
        "<=" => Some("<="),
        ">" => Some(">"),
        ">=" => Some(">="),
        "cons" => Some("cons"),
        "concat" => Some("concat"),
        "conj" => Some("conj"),
        "first" => Some("first"),
        "rest" => Some("rest"),
        "next" => Some("next"),
        "nth" => Some("nth"),
        "count" => Some("count"),
        "empty?" => Some("empty?"),
        "seq" => Some("seq"),
        "get" => Some("get"),
        "contains?" => Some("contains?"),
        "assoc" => Some("assoc"),
        "dissoc" => Some("dissoc"),
        "keys" => Some("keys"),
        "vals" => Some("vals"),
        "meta" => Some("meta"),
        "with-meta" => Some("with-meta"),
        "vary-meta" => Some("vary-meta"),
        "gensym" => Some("gensym"),
        "syntax-error" => Some("syntax-error"),
        "symbol" => Some("symbol"),
        "keyword" => Some("keyword"),
        "name" => Some("name"),
        "namespace" => Some("namespace"),
        "str" => Some("str"),
        "nil?" => Some("nil?"),
        "some?" => Some("some?"),
        "symbol?" => Some("symbol?"),
        "keyword?" => Some("keyword?"),
        "list?" => Some("list?"),
        "vector?" => Some("vector?"),
        "map?" => Some("map?"),
        "set?" => Some("set?"),
        "sequential?" => Some("sequential?"),
        "apply" => Some("apply"),
        "map" => Some("map"),
        "mapv" => Some("mapv"),
        "mapcat" => Some("mapcat"),
        "mapcatv" => Some("mapcatv"),
        "filter" => Some("filter"),
        "filterv" => Some("filterv"),
        "reduce" => Some("reduce"),
        "fold" => Some("fold"),
        _ => None,
    }
}

fn tick_budget(budget: &mut EvalBudget, depth: usize, span: Span) -> Result<(), EvalError> {
    if depth > DEFAULT_MAX_EVAL_DEPTH {
        return Err(EvalError::new(
            "OSR-M0005",
            format!(
                "phase-1 evaluation exceeded the recursion depth limit of {DEFAULT_MAX_EVAL_DEPTH}"
            ),
            span,
        ));
    }
    budget.steps = budget.steps.saturating_add(1);
    if budget.steps > DEFAULT_MAX_EVAL_STEPS {
        return Err(EvalError::new(
            "OSR-M0005",
            format!("phase-1 evaluation exceeded the step limit of {DEFAULT_MAX_EVAL_STEPS}"),
            span,
        ));
    }
    Ok(())
}

fn require_form_arity(
    items: &[Form],
    expected: usize,
    name: &str,
    span: Span,
) -> Result<(), EvalError> {
    if items.len() == expected {
        Ok(())
    } else {
        Err(EvalError::evaluation(
            format!(
                "`{name}` expects {} argument(s)",
                expected.saturating_sub(1)
            ),
            span,
        ))
    }
}

fn require_value_arity(
    arguments: &[Value],
    expected: usize,
    name: &str,
    span: Span,
) -> Result<(), EvalError> {
    if arguments.len() == expected {
        Ok(())
    } else {
        Err(EvalError::evaluation(
            format!("`{name}` expects {expected} argument(s)"),
            span,
        ))
    }
}

struct BindContext<'expander, 'environment, 'budget> {
    expander: &'expander mut Expander,
    environment: &'environment mut Environment,
    budget: &'budget mut EvalBudget,
    span: Span,
    depth: usize,
}

fn bind_parameters(
    context: &mut BindContext<'_, '_, '_>,
    parameters: &Parameters,
    arguments: &[Value],
    macro_call: bool,
) -> Result<(), EvalError> {
    let valid = arguments.len() >= parameters.fixed.len()
        && (parameters.rest.is_some() || arguments.len() == parameters.fixed.len());
    if !valid {
        let expectation = if parameters.rest.is_some() {
            format!("at least {}", parameters.fixed.len())
        } else {
            parameters.fixed.len().to_string()
        };
        return Err(EvalError::new(
            if macro_call { "OSR-M0001" } else { "OSR-M0004" },
            format!(
                "phase-1 call expects {expectation} argument(s), received {}",
                arguments.len()
            ),
            context.span,
        ));
    }
    for (pattern, value) in parameters.fixed.iter().zip(arguments) {
        bind_pattern(context, pattern, value.clone())?;
    }
    if let Some(rest) = &parameters.rest {
        let remaining = arguments[parameters.fixed.len()..]
            .iter()
            .cloned()
            .map(|value| value.into_data(context.span))
            .collect::<Result<Vec<_>, _>>()?;
        bind_pattern(context, rest, Value::Data(list(remaining, context.span)))?;
    }
    Ok(())
}

fn bind_pattern(
    context: &mut BindContext<'_, '_, '_>,
    pattern: &Pattern,
    value: Value,
) -> Result<(), EvalError> {
    match pattern {
        Pattern::Bind(name) => {
            context.environment.insert(name.clone(), value);
            Ok(())
        }
        Pattern::Ignore => Ok(()),
        Pattern::Vector(parameters) => {
            let data = value.into_data(context.span)?;
            let items = sequence_items(&data, context.span)?;
            for (index, pattern) in parameters.fixed.iter().enumerate() {
                let item = items
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| none(context.span));
                bind_pattern(context, pattern, Value::Data(item))?;
            }
            if let Some(rest) = &parameters.rest {
                let remaining = items.into_iter().skip(parameters.fixed.len()).collect();
                bind_pattern(context, rest, Value::Data(list(remaining, context.span)))?;
            }
            Ok(())
        }
        Pattern::Map(pattern) => {
            let data = value.into_data(context.span)?;
            let entries = match &data.kind {
                FormKind::None => &[][..],
                FormKind::Map(entries) => entries.as_slice(),
                _ => {
                    return Err(EvalError::evaluation(
                        "map destructuring requires a phase-1 map or none",
                        context.span,
                    ));
                }
            };

            if let Some(whole) = &pattern.whole {
                bind_pattern(context, whole, Value::Data(data.clone()))?;
            }

            for entry in &pattern.entries {
                let found = entries
                    .chunks_exact(2)
                    .find(|pair| crate::syntax::datum_eq(&pair[0], &entry.lookup))
                    .map(|pair| pair[1].clone());
                let selected = if let Some(found) = found {
                    found
                } else if let Pattern::Bind(name) = &entry.binding {
                    match pattern.defaults.get(name) {
                        Some(default) => context
                            .expander
                            .eval(
                                default,
                                context.environment,
                                context.budget,
                                context.depth + 1,
                            )?
                            .into_data(default.span)?,
                        None => none(context.span),
                    }
                } else {
                    none(context.span)
                };
                bind_pattern(context, &entry.binding, Value::Data(selected))?;
            }
            Ok(())
        }
    }
}

fn is_truthy(value: &Value) -> bool {
    !matches!(
        value,
        Value::Data(Form {
            kind: FormKind::None | FormKind::Bool(false),
            ..
        })
    )
}

fn values_into_forms(arguments: Vec<Value>, span: Span) -> Result<Vec<Form>, EvalError> {
    arguments
        .into_iter()
        .map(|value| value.into_data(span))
        .collect()
}

fn value_callable(value: Value, span: Span) -> Result<Callable, EvalError> {
    match value {
        Value::Callable(callable) => Ok(callable),
        Value::Data(_) | Value::Reduced(_) => {
            Err(EvalError::evaluation("expected a phase-1 function", span))
        }
    }
}

fn sequence_items(form: &Form, span: Span) -> Result<Vec<Form>, EvalError> {
    match &form.kind {
        FormKind::None => Ok(Vec::new()),
        FormKind::List(items)
        | FormKind::Vector(items)
        | FormKind::Map(items)
        | FormKind::Set(items) => Ok(items.clone()),
        _ => Err(EvalError::evaluation("expected a phase-1 collection", span)),
    }
}

fn collection_count(form: &Form, span: Span) -> Result<usize, EvalError> {
    match &form.kind {
        FormKind::None => Ok(0),
        FormKind::String(value) => Ok(value.chars().count()),
        FormKind::List(items) | FormKind::Vector(items) | FormKind::Set(items) => Ok(items.len()),
        FormKind::Map(items) => Ok(items.len() / 2),
        _ => Err(EvalError::evaluation("expected a phase-1 collection", span)),
    }
}

fn unique_forms(forms: Vec<Form>) -> Vec<Form> {
    let mut unique = Vec::new();
    for form in forms {
        if !unique
            .iter()
            .any(|existing| crate::syntax::datum_eq(existing, &form))
        {
            unique.push(form);
        }
    }
    unique
}

fn conj(mut collection: Form, values: Vec<Form>, span: Span) -> Result<Form, EvalError> {
    match &mut collection.kind {
        FormKind::List(items) => {
            for value in values {
                items.insert(0, value);
            }
        }
        FormKind::Vector(items) => items.extend(values),
        FormKind::Set(items) => {
            items.extend(values);
            *items = unique_forms(std::mem::take(items));
        }
        FormKind::Map(items) => {
            for value in values {
                match value.kind {
                    FormKind::Vector(pair) if pair.len() == 2 => {
                        assoc_items(items, pair[0].clone(), pair[1].clone());
                    }
                    FormKind::Map(entries) if entries.len() % 2 == 0 => {
                        for pair in entries.chunks_exact(2) {
                            assoc_items(items, pair[0].clone(), pair[1].clone());
                        }
                    }
                    _ => {
                        return Err(EvalError::evaluation(
                            "conjoining into a map requires [key value] pairs or maps",
                            span,
                        ));
                    }
                }
            }
        }
        FormKind::None => return Ok(list(values.into_iter().rev().collect(), span)),
        _ => return Err(EvalError::evaluation("`conj` expects a collection", span)),
    }
    collection.span = span;
    collection.datum_span = span;
    Ok(collection)
}

fn assoc_form(mut map: Form, pairs: Vec<Form>, span: Span) -> Result<Form, EvalError> {
    let FormKind::Map(items) = &mut map.kind else {
        return Err(EvalError::evaluation("`assoc` expects a map", span));
    };
    for pair in pairs.chunks_exact(2) {
        assoc_items(items, pair[0].clone(), pair[1].clone());
    }
    map.span = span;
    map.datum_span = span;
    Ok(map)
}

fn assoc_items(items: &mut Vec<Form>, key: Form, value: Form) {
    if let Some(index) = items
        .chunks_exact(2)
        .position(|pair| crate::syntax::datum_eq(&pair[0], &key))
    {
        items[index * 2] = key;
        items[index * 2 + 1] = value;
    } else {
        items.push(key);
        items.push(value);
    }
}

fn dissoc_form(mut map: Form, keys: &[Form], span: Span) -> Result<Form, EvalError> {
    let FormKind::Map(items) = &mut map.kind else {
        return Err(EvalError::evaluation("`dissoc` expects a map", span));
    };
    let mut retained = Vec::new();
    for pair in items.chunks_exact(2) {
        if !keys
            .iter()
            .any(|key| crate::syntax::datum_eq(key, &pair[0]))
        {
            retained.extend_from_slice(pair);
        }
    }
    *items = retained;
    map.span = span;
    map.datum_span = span;
    Ok(map)
}

fn get_from_collection(collection: &Form, key: &Form) -> Option<Form> {
    match &collection.kind {
        FormKind::Map(items) => items
            .chunks_exact(2)
            .find(|pair| crate::syntax::datum_eq(&pair[0], key))
            .map(|pair| pair[1].clone()),
        FormKind::Vector(items) | FormKind::List(items) => form_to_usize(key, key.span)
            .ok()
            .and_then(|index| items.get(index).cloned()),
        _ => None,
    }
}

fn collection_contains(collection: &Form, key: &Form) -> bool {
    match &collection.kind {
        FormKind::Map(items) => items
            .chunks_exact(2)
            .any(|pair| crate::syntax::datum_eq(&pair[0], key)),
        FormKind::Set(items) => items.iter().any(|item| crate::syntax::datum_eq(item, key)),
        FormKind::Vector(items) | FormKind::List(items) => {
            form_to_usize(key, key.span).is_ok_and(|index| index < items.len())
        }
        _ => false,
    }
}

fn metadata_map(form: &Form) -> Form {
    let items = form
        .metadata
        .iter()
        .flat_map(|entry| [entry.key.clone(), entry.value.clone()])
        .collect();
    Form::new(FormKind::Map(items), form.span)
}

fn with_metadata(mut target: Form, metadata: &Form, span: Span) -> Result<Form, EvalError> {
    if !target.supports_metadata() {
        return Err(EvalError::evaluation(
            "metadata can only be attached to syntax forms that support metadata",
            span,
        ));
    }
    let normalized = match &metadata.kind {
        FormKind::None => Vec::new(),
        FormKind::Map(items) if items.len() % 2 == 0 => {
            let entries = items.len() / 2;
            if entries > METADATA_TARGET_LIMITS.max_entries {
                return Err(EvalError::new(
                    "OSR-M0009",
                    format!(
                        "metadata for one syntax target exceeds the entry count limit of {} (found {entries})",
                        METADATA_TARGET_LIMITS.max_entries
                    ),
                    span,
                ));
            }
            items
                .chunks_exact(2)
                .map(|pair| MetadataEntry {
                    key: pair[0].clone(),
                    value: pair[1].clone(),
                })
                .collect()
        }
        _ => {
            return Err(EvalError::evaluation(
                "metadata must be a map or none",
                span,
            ));
        }
    };
    if normalized.iter().any(|entry| {
        !metadata_datum_is_serializable(&entry.key) || !metadata_datum_is_serializable(&entry.value)
    }) {
        return Err(EvalError::evaluation(
            "metadata must contain only serializable phase-1 data",
            span,
        ));
    }
    if let Err(exceeded) = check_metadata_resources(&normalized, METADATA_TARGET_LIMITS) {
        return Err(EvalError::new(
            "OSR-M0009",
            format!(
                "metadata for one syntax target exceeds the {} limit of {} (found {})",
                exceeded.resource, exceeded.limit, exceeded.actual
            ),
            span,
        ));
    }
    target.metadata = normalized;
    Ok(target)
}

#[derive(Clone, Copy)]
enum Number {
    Integer(i128),
    Float(f64),
}

fn parse_number(form: &Form, span: Span) -> Result<Number, EvalError> {
    match &form.kind {
        FormKind::Integer(value) => value
            .parse::<i128>()
            .map(Number::Integer)
            .map_err(|_| EvalError::evaluation("integer is outside the phase-1 range", span)),
        FormKind::Float(value) => value
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .map(Number::Float)
            .ok_or_else(|| EvalError::evaluation("invalid finite phase-1 float", span)),
        _ => Err(EvalError::evaluation("expected a number", span)),
    }
}

fn numeric_builtin(name: &str, forms: &[Form], span: Span) -> Result<Form, EvalError> {
    let numbers = forms
        .iter()
        .map(|form| parse_number(form, span))
        .collect::<Result<Vec<_>, _>>()?;
    if matches!(name, "inc" | "dec") && numbers.len() != 1 {
        return Err(EvalError::evaluation(
            format!("`{name}` expects one argument"),
            span,
        ));
    }
    if name == "/" {
        if numbers.is_empty() {
            return Err(EvalError::evaluation(
                "`/` expects at least one argument",
                span,
            ));
        }
        let mut values = numbers.iter().map(|number| match number {
            Number::Integer(value) => *value as f64,
            Number::Float(value) => *value,
        });
        let first = values.next().expect("checked above");
        let mut result = if numbers.len() == 1 {
            1.0 / first
        } else {
            first
        };
        if numbers.len() > 1 {
            for value in values {
                result /= value;
            }
        }
        return finite_float(result, span);
    }
    let has_float = numbers
        .iter()
        .any(|number| matches!(number, Number::Float(_)));
    if has_float {
        let values = numbers
            .iter()
            .map(|number| match number {
                Number::Integer(value) => *value as f64,
                Number::Float(value) => *value,
            })
            .collect::<Vec<_>>();
        let result = match name {
            "+" => values.iter().sum(),
            "*" => values.iter().product(),
            "-" if values.len() == 1 => -values[0],
            "-" if !values.is_empty() => values[1..].iter().fold(values[0], |a, b| a - b),
            "inc" => values[0] + 1.0,
            "dec" => values[0] - 1.0,
            _ => return Err(EvalError::evaluation("invalid numeric arity", span)),
        };
        return finite_float(result, span);
    }
    let values = numbers
        .into_iter()
        .map(|number| match number {
            Number::Integer(value) => value,
            Number::Float(_) => unreachable!(),
        })
        .collect::<Vec<_>>();
    let checked = match name {
        "+" => values
            .iter()
            .try_fold(0_i128, |left, right| left.checked_add(*right)),
        "*" => values
            .iter()
            .try_fold(1_i128, |left, right| left.checked_mul(*right)),
        "-" if values.len() == 1 => values[0].checked_neg(),
        "-" if !values.is_empty() => values[1..]
            .iter()
            .try_fold(values[0], |left, right| left.checked_sub(*right)),
        "inc" => values[0].checked_add(1),
        "dec" => values[0].checked_sub(1),
        _ => return Err(EvalError::evaluation("invalid numeric arity", span)),
    };
    checked
        .map(|value| Form::new(FormKind::Integer(value.to_string()), span))
        .ok_or_else(|| EvalError::evaluation("phase-1 integer arithmetic overflow", span))
}

fn compare_builtin(name: &str, forms: &[Form], span: Span) -> Result<Form, EvalError> {
    if forms.len() < 2 {
        return Err(EvalError::evaluation(
            format!("`{name}` expects at least two arguments"),
            span,
        ));
    }
    let values = forms
        .iter()
        .map(|form| parse_number(form, span))
        .map(|result| {
            result.map(|number| match number {
                Number::Integer(value) => value as f64,
                Number::Float(value) => value,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let result = values.windows(2).all(|pair| match name {
        "<" => pair[0] < pair[1],
        "<=" => pair[0] <= pair[1],
        ">" => pair[0] > pair[1],
        ">=" => pair[0] >= pair[1],
        _ => unreachable!(),
    });
    Ok(boolean(result, span))
}

fn finite_float(value: f64, span: Span) -> Result<Form, EvalError> {
    if !value.is_finite() {
        return Err(EvalError::evaluation(
            "phase-1 float arithmetic produced a non-finite value",
            span,
        ));
    }
    let mut spelling = value.to_string();
    if !spelling.contains(['.', 'e', 'E']) {
        spelling.push_str(".0");
    }
    Ok(Form::new(FormKind::Float(spelling), span))
}

fn form_to_usize(form: &Form, span: Span) -> Result<usize, EvalError> {
    let FormKind::Integer(value) = &form.kind else {
        return Err(EvalError::evaluation(
            "expected a non-negative integer",
            span,
        ));
    };
    value
        .parse::<usize>()
        .map_err(|_| EvalError::evaluation("expected a non-negative integer", span))
}

fn form_to_string(form: &Form, span: Span) -> Result<String, EvalError> {
    match &form.kind {
        FormKind::String(value) => Ok(value.clone()),
        _ => Err(EvalError::evaluation("expected a string", span)),
    }
}

fn form_name_or_string(form: &Form, span: Span) -> Result<String, EvalError> {
    match &form.kind {
        FormKind::String(value) => Ok(value.clone()),
        FormKind::Symbol(name) | FormKind::Keyword(name) => Ok(name.spelling.clone()),
        _ => Err(EvalError::evaluation(
            "expected a string, symbol, or keyword",
            span,
        )),
    }
}

fn display_form(form: &Form) -> String {
    match &form.kind {
        FormKind::None => String::new(),
        FormKind::Bool(value) => value.to_string(),
        FormKind::Integer(value) | FormKind::Float(value) | FormKind::String(value) => {
            value.clone()
        }
        FormKind::Symbol(name) | FormKind::Keyword(name) => name.spelling.clone(),
        FormKind::List(items) => display_collection("(", ")", items),
        FormKind::Vector(items) => display_collection("[", "]", items),
        FormKind::Map(items) => display_collection("{", "}", items),
        FormKind::Set(items) => display_collection("#{", "}", items),
        FormKind::ReaderMacro { form, .. } => display_form(form),
        FormKind::Error(message) => format!("#<error:{message}>"),
    }
}

fn display_collection(open: &str, close: &str, items: &[Form]) -> String {
    format!(
        "{open}{}{close}",
        items.iter().map(display_form).collect::<Vec<_>>().join(" ")
    )
}

fn form_node_count(form: &Form) -> usize {
    let metadata = form.metadata.iter().fold(0_usize, |count, entry| {
        count
            .saturating_add(form_node_count(&entry.key))
            .saturating_add(form_node_count(&entry.value))
    });
    let children = match &form.kind {
        FormKind::List(items)
        | FormKind::Vector(items)
        | FormKind::Map(items)
        | FormKind::Set(items) => items.iter().fold(0_usize, |count, item| {
            count.saturating_add(form_node_count(item))
        }),
        FormKind::ReaderMacro { form, .. } => form_node_count(form),
        _ => 0,
    };
    1_usize.saturating_add(metadata).saturating_add(children)
}

fn none(span: Span) -> Form {
    Form::new(FormKind::None, span)
}

fn boolean(value: bool, span: Span) -> Form {
    Form::new(FormKind::Bool(value), span)
}

fn integer(value: usize, span: Span) -> Form {
    Form::new(FormKind::Integer(value.to_string()), span)
}

fn string(value: &str, span: Span) -> Form {
    Form::new(FormKind::String(value.to_owned()), span)
}

fn named_form(keyword: bool, spelling: &str, span: Span) -> Form {
    let name = Name {
        spelling: spelling.to_owned(),
        canonical: spelling.to_owned(),
    };
    Form::new(
        if keyword {
            FormKind::Keyword(name)
        } else {
            FormKind::Symbol(name)
        },
        span,
    )
}

fn is_phase_one_declaration(name: &str) -> bool {
    matches!(name, "defmacro" | "defn-for-syntax")
}

/// Heads whose presence at module level establishes the dependency/phase
/// boundary before runtime macro expansion. They must be authored directly;
/// runtime declarations such as `def`, `defn`, `defstruct`, `extern`, and
/// `static-record` intentionally remain generatable by declaration macros.
fn top_level_boundary_head(form: &Form) -> Option<&str> {
    let FormKind::List(items) = &form.kind else {
        return None;
    };
    let head = items.first().and_then(symbol_canonical)?;
    matches!(
        head,
        "module"
            | "import"
            | "import-for-syntax"
            | "py/import"
            | "export"
            | "alias"
            | "defmacro"
            | "defn-for-syntax"
            | "defstatic-schema"
    )
    .then_some(head)
}

fn generated_top_level_boundary_head(form: &Form) -> Option<&str> {
    if let Some(head) = top_level_boundary_head(form) {
        return Some(head);
    }
    let FormKind::List(items) = &form.kind else {
        return None;
    };
    if items.first().and_then(symbol_canonical) != Some("do") {
        return None;
    }
    items
        .iter()
        .skip(1)
        .find_map(generated_top_level_boundary_head)
}

fn generated_declaration_sequence(form: &Form) -> Option<Vec<Form>> {
    let FormKind::List(items) = &form.kind else {
        return None;
    };
    if items.first().and_then(symbol_canonical) != Some("do") {
        return None;
    }
    let mut declarations = Vec::new();
    for item in items.iter().skip(1) {
        if let Some(nested) = generated_declaration_sequence(item) {
            declarations.extend(nested);
        } else if is_runtime_declaration(item) {
            declarations.push(item.clone());
        } else {
            return None;
        }
    }
    (!declarations.is_empty()).then_some(declarations)
}

fn is_runtime_declaration(form: &Form) -> bool {
    let FormKind::List(items) = &form.kind else {
        return false;
    };
    matches!(
        items.first().and_then(symbol_canonical),
        Some("def" | "defn" | "defstruct" | "extern" | "static-record")
    )
}

fn symbol_canonical(form: &Form) -> Option<&str> {
    match &form.kind {
        FormKind::Symbol(name) => Some(&name.canonical),
        _ => None,
    }
}

fn list(items: Vec<Form>, span: Span) -> Form {
    Form::new(FormKind::List(items), span)
}

fn vector(items: Vec<Form>, span: Span) -> Form {
    Form::new(FormKind::Vector(items), span)
}

fn error_form(message: &str, span: Span) -> Form {
    Form::new(FormKind::Error(message.to_owned()), span)
}

fn merge_call_metadata(
    call: &[crate::syntax::MetadataEntry],
    generated: &[crate::syntax::MetadataEntry],
) -> Vec<crate::syntax::MetadataEntry> {
    let mut metadata = generated.to_vec();
    for entry in call {
        if let Some(existing) = metadata
            .iter_mut()
            .find(|existing| crate::syntax::datum_eq(&existing.key, &entry.key))
        {
            *existing = entry.clone();
        } else {
            metadata.push(entry.clone());
        }
    }
    metadata
}

#[cfg(test)]
mod tests {
    use crate::{
        compiler::{CompileOptions, compile},
        printer::render_document_text,
        project::PythonVersion,
        reader::read,
        syntax::{Form, FormKind, METADATA_TARGET_LIMITS},
    };

    use super::{
        ExpansionOptions, ImportedPhaseModule, expand, expand_with_imported_phase_modules,
    };

    fn expanded(source: &str) -> String {
        let result = expand(&read(source), ExpansionOptions::default());
        assert!(
            result.document.diagnostics.is_empty(),
            "{:?}",
            result.document.diagnostics
        );
        render_document_text(&result.document)
    }

    fn imported_module(
        namespace: &str,
        source: &str,
        macro_names: &[(&str, &str)],
    ) -> ImportedPhaseModule {
        let document = read(source);
        assert!(
            document.diagnostics.is_empty(),
            "{:?}",
            document.diagnostics
        );
        ImportedPhaseModule::new(
            namespace,
            document.forms,
            macro_names
                .iter()
                .map(|(visible, target)| ((*visible).to_owned(), (*target).to_owned()))
                .collect(),
        )
    }

    #[test]
    fn imported_modules_isolate_same_named_helpers_and_macros() {
        let first = imported_module(
            "dep.first",
            "(defn-for-syntax helper [value] `(from-first ~value))\n\
             (defmacro wrap [value] (helper value))",
            &[("first/wrap", "wrap")],
        );
        let second = imported_module(
            "dep.second",
            "(defn-for-syntax helper [value] `(from-second ~value))\n\
             (defmacro wrap [value] (helper value))",
            &[("second/wrap", "wrap"), ("wrap", "wrap")],
        );
        let result = expand_with_imported_phase_modules(
            &read("(first/wrap x) (wrap y)"),
            &[first, second],
            ExpansionOptions::default(),
        );
        assert!(
            result.document.diagnostics.is_empty(),
            "{:?}",
            result.document.diagnostics
        );
        assert_eq!(
            render_document_text(&result.document),
            "(from-first x)\n(from-second y)\n"
        );
        assert_eq!(
            result
                .traces
                .iter()
                .map(|trace| trace.macro_binding_id.as_str())
                .collect::<Vec<_>>(),
            ["dep.first::macro::wrap", "dep.second::macro::wrap"]
        );
    }

    #[test]
    fn local_macro_trace_uses_the_declared_module_binding_id() {
        let result = expand(
            &read("(module local.core) (defmacro wrap [value] value) (wrap 1)"),
            ExpansionOptions::default(),
        );

        assert!(result.document.diagnostics.is_empty());
        assert_eq!(result.traces.len(), 1);
        assert_eq!(result.traces[0].macro_binding_id, "local.core::macro::wrap");
    }

    #[test]
    fn imported_macros_require_an_explicit_visible_name() {
        let module = imported_module(
            "dep.first",
            "(defn-for-syntax helper [value] `(wrapped ~value))\n\
             (defmacro wrap [value] (helper value))",
            &[("first/wrap", "wrap")],
        );
        let result = expand_with_imported_phase_modules(
            &read("(wrap short) (wrong/wrap wrong) (first/wrap right)"),
            &[module],
            ExpansionOptions::default(),
        );
        assert!(
            result.document.diagnostics.is_empty(),
            "{:?}",
            result.document.diagnostics
        );
        assert_eq!(
            render_document_text(&result.document),
            "(wrap short)\n(wrong/wrap wrong)\n(wrapped right)\n"
        );
    }

    #[test]
    fn imported_syntax_quote_resolves_exported_names_at_the_definition_site() {
        let module = imported_module(
            "dep.component",
            "(defmacro declare [name] `(static-record Descriptor ~name {}))",
            &[("component/declare", "declare")],
        )
        .with_definition_names(
            [("Descriptor".to_owned(), "Descriptor".to_owned())]
                .into_iter()
                .collect(),
        );
        let result = expand_with_imported_phase_modules(
            &read("(component/declare normalize)"),
            &[module],
            ExpansionOptions::default(),
        );
        assert!(
            result.document.diagnostics.is_empty(),
            "{:?}",
            result.document.diagnostics
        );
        assert_eq!(
            render_document_text(&result.document),
            "(static-record dep.component/Descriptor normalize {})\n"
        );
    }

    #[test]
    fn duplicate_identical_namespace_loads_merge_explicit_exposures() {
        let source = "(defn-for-syntax helper [value] `(wrapped ~value))\n\
                      (defmacro wrap [value] (helper value))";
        let qualified = imported_module("dep.shared", source, &[("shared/wrap", "wrap")]);
        let referred = imported_module("dep.shared", source, &[("wrap", "wrap")]);
        let result = expand_with_imported_phase_modules(
            &read("(shared/wrap one) (wrap two)"),
            &[qualified, referred],
            ExpansionOptions::default(),
        );
        assert!(
            result.document.diagnostics.is_empty(),
            "{:?}",
            result.document.diagnostics
        );
        assert_eq!(
            render_document_text(&result.document),
            "(wrapped one)\n(wrapped two)\n"
        );
    }

    #[test]
    fn inconsistent_duplicate_namespace_loads_are_rejected() {
        let first = imported_module(
            "dep.shared",
            "(defmacro wrap [value] `(first ~value))",
            &[("shared/wrap", "wrap")],
        );
        let second = imported_module(
            "dep.shared",
            "(defmacro wrap [value] `(second ~value))",
            &[("shared/wrap", "wrap")],
        );
        let result = expand_with_imported_phase_modules(
            &read("(shared/wrap value)"),
            &[first, second],
            ExpansionOptions::default(),
        );
        assert!(
            result
                .document
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "OSR-M0003")
        );
        assert_eq!(
            render_document_text(&result.document),
            "(shared/wrap value)\n"
        );
    }

    #[test]
    fn imported_modules_share_the_existing_expansion_budget() {
        let first = imported_module(
            "dep.first",
            "(defmacro wrap [value] `(first ~value))",
            &[("first/wrap", "wrap")],
        );
        let second = imported_module(
            "dep.second",
            "(defmacro wrap [value] `(second ~value))",
            &[("second/wrap", "wrap")],
        );
        let result = expand_with_imported_phase_modules(
            &read("(first/wrap one) (second/wrap two)"),
            &[first, second],
            ExpansionOptions {
                once: false,
                max_expansions: 1,
            },
        );
        assert!(
            result
                .document
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "OSR-M0002")
        );
    }

    #[test]
    fn macros_cannot_generate_top_level_boundary_declarations() {
        for declaration in [
            "module dep.generated",
            "import dep.generated",
            "import-for-syntax dep.generated",
            "py/import dep.generated",
            "export [value]",
            "alias local target",
            "defmacro generated [] `1",
            "defn-for-syntax generated [] 1",
            "defstatic-schema Generated :schema-id \"generated\" :version 1",
        ] {
            let source = format!("(defmacro emit [] '({declaration}))\n(emit)\n(def value 1)");
            let result = expand(&read(&source), ExpansionOptions::default());
            assert!(
                result
                    .document
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code == "OSR-M0008"),
                "generated declaration should be rejected: {declaration}\n{:?}",
                result.document.diagnostics
            );
            assert!(matches!(
                result.document.forms.get(1).map(|form| &form.kind),
                Some(FormKind::Error(message))
                    if message == "macro-generated top-level declaration"
            ));
            assert!(matches!(
                result.document.forms.get(2).map(|form| &form.kind),
                Some(FormKind::List(items))
                    if items.first().and_then(super::symbol_canonical) == Some("def")
            ));
        }
    }

    #[test]
    fn authored_boundary_declarations_are_not_macro_expanded() {
        let source = "(defmacro module-name [] 'generated)\n(module (module-name))";
        let result = expand(&read(source), ExpansionOptions::default());
        assert!(
            result.document.diagnostics.is_empty(),
            "{:?}",
            result.document.diagnostics
        );
        assert!(result.traces.is_empty());
        assert!(matches!(
            result.document.forms.get(1).map(|form| &form.kind),
            Some(FormKind::List(items))
                if items.first().and_then(super::symbol_canonical) == Some("module")
                    && matches!(items.get(1).map(|form| &form.kind), Some(FormKind::List(_)))
        ));
    }

    #[test]
    fn declaration_macros_can_still_generate_runtime_declarations() {
        let source = "(defmacro emit [] '(def generated 1))\n(emit)";
        let result = expand(&read(source), ExpansionOptions::default());
        assert!(
            result.document.diagnostics.is_empty(),
            "{:?}",
            result.document.diagnostics
        );
        assert!(matches!(
            result.document.forms.get(1).map(|form| &form.kind),
            Some(FormKind::List(items))
                if items.first().and_then(super::symbol_canonical) == Some("def")
        ));
    }

    #[test]
    fn declaration_macros_can_generate_ordered_declaration_sequences() {
        let source = r#"
            (defmacro emit []
              '(do
                 (def generated 1)
                 (do
                   (defn generated-fn [] -> Int generated)
                   (static-record Schema generated {:id "generated"}))))
            (emit)
        "#;
        let result = expand(&read(source), ExpansionOptions::default());
        assert!(
            result.document.diagnostics.is_empty(),
            "{:?}",
            result.document.diagnostics
        );
        assert_eq!(result.document.forms.len(), 4);
        assert_eq!(
            result.document.forms[1..]
                .iter()
                .filter_map(|form| match &form.kind {
                    FormKind::List(items) => items.first().and_then(super::symbol_canonical),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            ["def", "defn", "static-record"]
        );
    }

    #[test]
    fn declaration_sequences_cannot_hide_module_graph_declarations() {
        let source = "(defmacro emit [] '(do (def value 1) (do (import hidden))))\n(emit)";
        let result = expand(&read(source), ExpansionOptions::default());
        assert!(
            result
                .document
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "OSR-M0008")
        );
        assert!(matches!(
            result.document.forms.get(1).map(|form| &form.kind),
            Some(FormKind::Error(message))
                if message == "macro-generated top-level declaration"
        ));
    }

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
        assert!(output.contains(
            "(if (osiris.prelude/truthy* final?) (finish thread__osr_g1) thread__osr_g1)"
        ));
    }

    #[test]
    fn extended_threading_and_object_macros_preserve_single_evaluation() {
        let conditional =
            expanded("(cond->> (load) ready? (map normalize) final? (reduce combine))");
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
            if_some.contains(
                "(let [value (osiris.prelude/present* if-some__osr_g0)] (consume value))"
            )
        );

        let when_some = expanded("(when-some [{:keys [id]} (lookup)] (consume id))");
        assert!(when_some.contains("(osiris.prelude/nil* if-some__osr_g0)"));
        assert!(
            when_some.contains("(let [{:keys [id]} (osiris.prelude/present* if-some__osr_g0)]")
        );
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
            if_let
                .contains("(let [{:keys [value]} (osiris.prelude/present* if-let__osr_g0)] value)")
        );

        let when_let = expanded("(when-let [value (lookup)] (consume value))");
        assert_eq!(when_let.matches("(lookup)").count(), 1);
        assert!(when_let.contains(
            "(let [value (osiris.prelude/present* if-let__osr_g0)] (do (consume value)))"
        ));

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
            when_first.contains(
                "(let [[left right] (nth (osiris.prelude/present* when-first__osr_g0) 0)]"
            )
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

    #[test]
    fn for_expands_multiple_bindings_to_nested_flat_maps() {
        let output = expanded("(for [left lefts right (rights-for left)] (combine left right))");
        assert_eq!(
            output,
            "(osiris.prelude/mapcatv (fn [left] (osiris.prelude/mapv (fn [right] (do (combine left right))) (rights-for left))) lefts)\n"
        );
        assert_eq!(output.matches("lefts").count(), 1);
        assert_eq!(output.matches("(rights-for left)").count(), 1);
    }

    #[test]
    fn for_let_and_when_preserve_clause_order_and_single_evaluation() {
        let output = expanded(
            "(for [item items :let [score (score item)] :when (eligible? score) detail (details item)] (emit item detail score))",
        );
        assert_eq!(output.matches("(score item)").count(), 1);
        assert_eq!(output.matches("(eligible? score)").count(), 1);
        assert_eq!(output.matches("(details item)").count(), 1);
        assert_eq!(output.matches("(emit item detail score)").count(), 1);
        assert!(output.contains("(fn [item] (let [score (score item)]"));
        assert!(output.contains(
            "(if (osiris.prelude/truthy* (eligible? score)) (osiris.prelude/mapv (fn [detail] (do (emit item detail score))) (details item)) [])"
        ));
    }

    #[test]
    fn for_uses_hygienic_temporaries_for_destructured_bindings() {
        let output =
            expanded("(for [[left right] pairs {:keys [value]} rows] (+ left right value))");
        assert!(output.contains("(fn [item__osr_g0]"));
        assert!(output.contains("(let [[left right] item__osr_g0]"));
        assert!(output.contains("(fn [item__osr_g1]"));
        assert!(output.contains("(let [{:keys [value]} item__osr_g1]"));

        let mixed = expanded("(for [group groups {:keys [value]} group] value)");
        assert!(mixed.contains("(fn [group]"));
        assert!(mixed.contains("(fn [item__osr_g0]"));
        assert!(!mixed.contains("item__osr_g1"));
    }

    #[test]
    fn and_and_or_preserve_short_circuit_single_evaluation() {
        let and_output = expanded("(and (first?) (second?) (third?))");
        assert_eq!(and_output.matches("(first?)").count(), 1);
        assert_eq!(and_output.matches("(second?)").count(), 1);
        assert_eq!(and_output.matches("(third?)").count(), 1);
        assert!(and_output.contains("(let [and__osr_g0 (first?)]"));
        assert!(and_output.contains("(if (osiris.prelude/truthy* and__osr_g0)"));

        let or_output = expanded("(or (first?) (second?) (third?))");
        assert_eq!(or_output.matches("(first?)").count(), 1);
        assert_eq!(or_output.matches("(second?)").count(), 1);
        assert_eq!(or_output.matches("(third?)").count(), 1);
        assert!(or_output.contains("(let [or__osr_g0 (first?)]"));
        assert!(or_output.contains("(if (osiris.prelude/truthy* or__osr_g0) or__osr_g0"));
    }

    #[test]
    fn malformed_control_macros_report_macro_diagnostics() {
        for source in [
            "(cond ready?)",
            "(cond :else 1 later? 2)",
            "(for [] x)",
            "(for [x xs y] x)",
            "(for [:when ready? x xs] x)",
            "(for [x xs :while] x)",
            "(for [x xs :when] x)",
            "(for [x xs :let {}] x)",
            "(for [x xs :let [y]] x)",
            "(for [x xs])",
            "(condp = value 1 :one)",
            "(condp = value 1 :>>)",
            "(letfn value body)",
            "(letfn [f (fn [])])",
        ] {
            let result = expand(&read(source), ExpansionOptions::default());
            assert!(
                result
                    .document
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code == "OSR-M0007"),
                "expected macro diagnostic for {source}: {:?}",
                result.document.diagnostics
            );
        }
    }

    #[test]
    fn malformed_for_clauses_have_specific_diagnostics() {
        for (source, expected) in [
            (
                "(for [] value)",
                "for requires at least one pattern/collection pair",
            ),
            (
                "(for [item items detail] value)",
                "for binding pattern requires a collection expression",
            ),
            (
                "(for [item items :when] value)",
                "for :when requires a predicate expression",
            ),
            (
                "(for [item items :let {}] value)",
                "for :let requires a binding vector",
            ),
            (
                "(for [item items :let [score]] value)",
                "for :let requires an even number of binding forms",
            ),
            (
                "(for [item items :let [1 score]] value)",
                "for :let binding pattern must be a symbol, vector, or map",
            ),
            (
                "(for [item items :while] value)",
                "for :while requires a predicate expression",
            ),
            (
                "(for [item items :until ready?] value)",
                "unsupported for modifier :until; expected :let, :when, or :while",
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

    #[test]
    fn malformed_doseq_clauses_have_specific_diagnostics() {
        for (source, expected) in [
            (
                "(doseq [] value)",
                "doseq requires at least one pattern/collection pair",
            ),
            (
                "(doseq [item items :while] value)",
                "doseq :while requires a predicate expression",
            ),
            (
                "(doseq [item items :let [value]] value)",
                "doseq :let requires an even number of binding forms",
            ),
            (
                "(doseq [item items :until ready?] value)",
                "unsupported doseq modifier :until; expected :let, :when, or :while",
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

    #[test]
    fn macro_declarations_keep_syntax_quote_templates_unexpanded() {
        let source = "(defmacro pipeline [x] `(~'-> ~x (f)))";
        assert_eq!(expanded(source), format!("{source}\n"));
    }

    #[test]
    fn malformed_threading_call_is_recoverable() {
        let result = expand(
            &read("(-> value ()) (def okay 1)"),
            ExpansionOptions::default(),
        );
        assert!(
            result
                .document
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "OSR-M0007")
        );
        assert_eq!(result.document.forms.len(), 2);
    }

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
        let source = "(defmacro reject [] (syntax-error &form \"rejected by macro\"))\n(reject)\n(def okay 1)";
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
}
