import importlib.util
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
PRELUDE_PATH = ROOT / "src" / "osiris" / "prelude.py"


def _load_prelude():
    spec = importlib.util.spec_from_file_location("osiris_test_control_prelude", PRELUDE_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("could not load the Osiris runtime prelude")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


prelude = _load_prelude()


class ControlRuntimeTests(unittest.TestCase):
    def test_binding_control_helpers_use_clojure_truthiness(self):
        self.assertFalse(prelude.truthy(None))
        self.assertFalse(prelude.truthy(False))
        self.assertTrue(prelude.truthy(0))
        self.assertTrue(prelude.truthy(""))
        self.assertTrue(prelude.truthy(()))
        self.assertTrue(prelude.is_nil(None))
        self.assertFalse(prelude.is_nil(False))

    def test_structural_first_helpers_do_not_consume_or_inspect_the_first_item(self):
        self.assertFalse(prelude.nonempty(None))
        self.assertFalse(prelude.nonempty(()))
        self.assertTrue(prelude.nonempty((None,)))
        self.assertIsNone(prelude.present(None))

    def test_collection_stop_token_breaks_mapcat_and_doseq(self):
        def mapped(value):
            if value == 3:
                return prelude.for_stop()
            return (value, value + 10)

        self.assertEqual(prelude.mapcatv(mapped, (1, 2, 3, 4)), (1, 11, 2, 12))

        visited = []

        def visit(value):
            if value == 3:
                return prelude.for_stop()
            visited.append(value)
            return None

        self.assertIsNone(prelude.doseq(visit, (1, 2, 3, 4)))
        self.assertEqual(visited, [1, 2])

    def test_filter_uses_clojure_truthiness_at_dynamic_boundaries(self):
        predicate = lambda value: value
        values = (None, False, 0, "", (), [], 1)
        self.assertEqual(prelude.filter(predicate, values), [0, "", (), [], 1])
        self.assertEqual(prelude.filterv(predicate, values), (0, "", (), [], 1))

    def test_reduced_protocol_stops_reduce_and_fold(self):
        visited = []

        def total_until_three(accumulator, value):
            visited.append(value)
            if value == 3:
                return prelude.reduced(accumulator)
            return accumulator + value

        self.assertEqual(prelude.reduce(total_until_three, 0, (1, 2, 3, 4)), 3)
        self.assertEqual(visited, [1, 2, 3])

        visited.clear()
        self.assertEqual(prelude.fold(total_until_three, 10, (1, 2, 3, 4)), 13)
        self.assertEqual(visited, [1, 2, 3])

    def test_reduced_helpers_remove_exactly_one_marker(self):
        marker = prelude.reduced(7)
        nested = prelude.reduced(marker)

        self.assertTrue(prelude.reduced_p(marker))
        self.assertFalse(prelude.reduced_p(7))
        self.assertEqual(prelude.unreduced(marker), 7)
        self.assertIs(prelude.unreduced(nested), marker)
        self.assertEqual(prelude.unreduced(7), 7)

    def test_reduce_without_initial_value_honors_reduced(self):
        visited = []

        def stop(accumulator, value):
            visited.append(value)
            return prelude.reduced(accumulator)

        self.assertEqual(prelude.reduce(stop, (1, 2, 3)), 1)
        self.assertEqual(visited, [2])

        with self.assertRaises(TypeError):
            prelude.reduce(stop, ())

    def test_loop_recur_uses_constant_stack_protocol(self):
        def step(value, total):
            if value == 0:
                return total
            return prelude.recur(value - 1, total + value)

        self.assertEqual(prelude.loop(step, 10_000, 0), 50_005_000)

    def test_loop_recur_rejects_wrong_state_arity(self):
        with self.assertRaises(TypeError):
            prelude.loop(lambda value: prelude.recur(value, value), 1)

    def test_trampoline_bounces_without_recursive_calls(self):
        def step(value, total):
            if value == 0:
                return total
            return lambda: step(value - 1, total + value)

        self.assertEqual(prelude.trampoline(step, 10_000, 0), 50_005_000)

    def test_lazy_seq_realizes_once(self):
        calls = []

        def produce():
            calls.append(1)
            return (1, 2, 3)

        sequence = prelude.lazy_seq(produce)
        self.assertEqual(list(sequence), [1, 2, 3])
        self.assertEqual(list(sequence), [1, 2, 3])
        self.assertEqual(len(calls), 1)

    def test_lazy_seq_realizes_generator_on_demand_and_replays_cache(self):
        events = []

        def produce():
            events.append("thunk")
            for value in range(3):
                events.append(value)
                yield value

        sequence = prelude.lazy_seq(produce)
        iterator = iter(sequence)
        self.assertEqual(next(iterator), 0)
        self.assertEqual(events, ["thunk", 0])
        self.assertEqual(list(iterator), [1, 2])
        self.assertEqual(events, ["thunk", 0, 1, 2])

    def test_delay_is_lazy_memoized_and_reports_realization(self):
        calls = []

        def produce():
            calls.append("run")
            return 42

        value = prelude.delay(produce)
        self.assertFalse(prelude.realized(value))
        self.assertEqual(prelude.force(value), 42)
        self.assertTrue(prelude.realized(value))
        self.assertEqual(prelude.force(value), 42)
        self.assertEqual(calls, ["run"])

    def test_delay_caches_exceptions_without_rerunning_the_thunk(self):
        calls = []

        def fail():
            calls.append("run")
            raise ValueError("boom")

        value = prelude.delay(fail)
        for _ in range(2):
            with self.assertRaisesRegex(ValueError, "boom"):
                prelude.force(value)
        self.assertTrue(prelude.realized(value))
        self.assertEqual(calls, ["run"])


if __name__ == "__main__":
    unittest.main()
