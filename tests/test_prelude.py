import unittest

from runtime_loader import prelude


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

    def test_lazy_map_realizes_left_to_right_and_memoizes_values_and_errors(self):
        calls = []
        expected = RuntimeError("lazy failure")

        def transform(value):
            calls.append(value)
            if value == 3:
                raise expected
            return value * 10

        values = prelude.map(transform, (1, 2, 3, 4))
        self.assertEqual(calls, [])
        iterator = iter(values)
        self.assertEqual(next(iterator), 10)
        self.assertEqual(next(iterator), 20)
        self.assertEqual(calls, [1, 2])
        with self.assertRaises(RuntimeError) as first:
            next(iterator)
        self.assertIs(first.exception, expected)
        with self.assertRaises(RuntimeError) as second:
            list(values)
        self.assertIs(second.exception, expected)
        self.assertEqual(calls, [1, 2, 3])

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
            list(prelude.mapcat(lambda left, right: (left, right), (1, 2), (10,))),
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


class LogicalCollectionTests(unittest.TestCase):
    def test_maps_distinguish_boolean_and_numeric_keys(self):
        counts = prelude.frequencies((False, 0, False, 0, True, 1))

        self.assertEqual(
            list(counts.items()),
            [(False, 2), (0, 2), (True, 1), (1, 1)],
        )
        self.assertEqual(counts[False], 2)
        self.assertEqual(counts[0], 2)
        self.assertEqual(counts, prelude.zipmap((False, 0, True, 1), (2, 2, 1, 1)))
        self.assertTrue(prelude.map_p(counts))

    def test_map_operations_preserve_logical_key_semantics(self):
        value = prelude.assoc(None, False, "false", 0, "zero")

        self.assertEqual(list(value.items()), [(False, "false"), (0, "zero")])
        self.assertEqual(prelude.get(value, False), "false")
        self.assertEqual(prelude.get(value, 0), "zero")
        self.assertEqual(list(prelude.dissoc(value, False).items()), [(0, "zero")])
        self.assertEqual(
            list(prelude.merge(value, prelude.assoc(None, False, "updated")).items()),
            [(False, "updated"), (0, "zero")],
        )

    def test_key_transformations_reject_collisions(self):
        source = prelude.assoc(None, "left", 1, "right", 2)

        cases = (
            lambda: prelude.index_by(lambda _value: "same", (1, 2)),
            lambda: prelude.rename_keys(source, {"left": "same", "right": "same"}),
            lambda: prelude.update_keys(lambda _key: "same", source),
            lambda: prelude.invert(prelude.assoc(None, "left", 1, "right", 1)),
        )
        for operation in cases:
            with self.subTest(operation=operation):
                with self.assertRaisesRegex(ValueError, "produced duplicate key"):
                    operation()


class ReductionTests(unittest.TestCase):
    def test_empty_reduce_invokes_the_zero_arity_callback(self):
        calls = []

        def empty_value():
            calls.append("called")
            return 42

        self.assertEqual(prelude.reduce(empty_value, ()), 42)
        self.assertEqual(calls, ["called"])
        self.assertEqual(list(prelude.reductions(empty_value, ())), [42])
        self.assertEqual(calls, ["called", "called"])

    def test_reduced_stops_only_the_nearest_reduction(self):
        inner_calls = []

        def inner(total, value):
            inner_calls.append(value)
            if value == 2:
                return prelude.reduced(total + value)
            return total + value

        nested = (
            prelude.reduce(inner, 0, (1, 2, 100)),
            prelude.reduce(inner, 0, (3, 4)),
        )
        self.assertEqual(prelude.reduce(lambda total, value: total + value, 0, nested), 10)
        self.assertEqual(inner_calls, [1, 2, 3, 4])


class StandardContractTests(unittest.TestCase):
    def test_finite_sequence_counts_reject_bool_and_non_integers(self):
        for operation in (prelude.take, prelude.drop):
            for invalid in (True, 1.5, "1"):
                with self.subTest(operation=operation.__name__, invalid=invalid):
                    with self.assertRaises(TypeError):
                        operation(invalid, (1, 2, 3))

    def test_range_rejects_bool_non_integers_and_zero_step(self):
        for invalid in (True, 1.5, "1"):
            with self.subTest(invalid=invalid):
                with self.assertRaises(TypeError):
                    prelude.range(invalid)
        with self.assertRaises(ValueError):
            prelude.range(0, 10, 0)
        self.assertEqual(tuple(prelude.range(1, 5, 2)), (1, 3))

    def test_logical_collection_predicates_accept_linked_map_and_set_values(self):
        self.assertTrue(prelude.coll_p(prelude.assoc(None, "key", 1)))
        self.assertTrue(prelude.coll_p({1, 2}))
        self.assertFalse(prelude.coll_p("text"))
        self.assertFalse(prelude.number_p(1 + 2j))

    def test_partition_windows_are_vectors(self):
        windows = list(prelude.partition_all(2, (1, 2, 3)))
        self.assertEqual(windows, [(1, 2), (3,)])
        self.assertTrue(all(isinstance(window, tuple) for window in windows))


if __name__ == "__main__":
    unittest.main()
