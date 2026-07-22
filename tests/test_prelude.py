import importlib.util
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
PRELUDE_PATH = ROOT / "src" / "osiris" / "prelude.py"


def _load_prelude():
    spec = importlib.util.spec_from_file_location("osiris_test_prelude", PRELUDE_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("could not load the Osiris runtime prelude")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


prelude = _load_prelude()


class MapvTests(unittest.TestCase):
    def test_maps_one_collection(self):
        self.assertEqual(prelude.mapv(lambda value: value * 2, [1, 2, 3]), (2, 4, 6))

    def test_maps_multiple_collections(self):
        self.assertEqual(
            prelude.mapv(lambda left, right: left + right, [1, 2], [10, 20]),
            (11, 22),
        )

    def test_stops_at_the_shortest_collection(self):
        self.assertEqual(
            prelude.mapv(lambda left, right: left + right, [1, 2, 3], [10]),
            (11,),
        )

    def test_materializes_a_tuple_vector(self):
        result = prelude.mapv(str.upper, (value for value in ["a", "b"]))

        self.assertIsInstance(result, tuple)
        self.assertEqual(result, ("A", "B"))

    def test_propagates_function_exceptions(self):
        class ExpectedError(Exception):
            pass

        expected = ExpectedError("failed while mapping")

        def fail_on_second(value):
            if value == 2:
                raise expected
            return value

        with self.assertRaises(ExpectedError) as context:
            prelude.mapv(fail_on_second, [1, 2, 3])

        self.assertIs(context.exception, expected)

    def test_mapcat_supports_multiple_collections(self):
        self.assertEqual(
            prelude.mapcatv(
                lambda left, right: (left, right, left + right),
                (1, 2),
                (10,),
            ),
            (1, 10, 11),
        )
        self.assertEqual(
            prelude.mapcat(lambda left, right: (left, right), (1, 2), (10,)),
            [1, 10],
        )


class ConcurrencyTests(unittest.TestCase):
    def test_promise_delivers_once_and_supports_timeout_default(self):
        value = prelude.promise()
        self.assertEqual(prelude.deref(value, 0, "missing"), "missing")
        self.assertIs(prelude.deliver(value, 7), value)
        self.assertEqual(prelude.deref(value), 7)
        prelude.deliver(value, 9)
        self.assertEqual(prelude.deref(value), 7)
        self.assertTrue(prelude.realized(value))

    def test_future_executes_and_deref_propagates_result(self):
        value = prelude.future_call(lambda: 41 + 1)
        self.assertEqual(prelude.deref(value), 42)
        self.assertTrue(prelude.future_done(value))
        self.assertTrue(prelude.realized(value))

    def test_future_task_timeout_error_is_not_a_wait_timeout(self):
        def fail():
            raise TimeoutError("task failure")

        value = prelude.future_call(fail)
        with self.assertRaisesRegex(TimeoutError, "task failure"):
            prelude.deref(value, 100, "fallback")

    def test_locking_releases_after_exception(self):
        guard = prelude.lock()
        with self.assertRaisesRegex(RuntimeError, "boom"):
            prelude.locking(guard, lambda: (_ for _ in ()).throw(RuntimeError("boom")))
        self.assertEqual(prelude.locking(guard, lambda: 3), 3)


if __name__ == "__main__":
    unittest.main()
