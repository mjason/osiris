import unittest

from runtime_loader import prelude


class SequenceRuntimeTests(unittest.TestCase):
    def test_lazy_combinators_preserve_single_traversal(self):
        events = []

        def source():
            events.append("source")
            yield from (1, 2, 3, 4)

        values = prelude.sequence(source())
        prefix = prelude.take_while(lambda value: value < 3, values)
        self.assertEqual(list(prefix), [1, 2])
        self.assertEqual(list(prefix), [1, 2])
        self.assertEqual(events, ["source"])

    def test_keep_remove_and_indexed_helpers_use_clojure_truthiness(self):
        values = (None, False, 0, 1, 2)
        self.assertEqual(list(prelude.keep(lambda value: value, values)), [False, 0, 1, 2])
        self.assertEqual(list(prelude.remove(lambda value: value, values)), [None, False])
        self.assertEqual(
            list(
                prelude.keep_indexed(
                    lambda index, value: index if prelude.truthy(value) else None,
                    values,
                )
            ),
            [2, 3, 4],
        )
        self.assertEqual(list(prelude.map_indexed(lambda index, value: index + value, (4, 5))), [4, 6])

    def test_infinite_sequence_builders_are_consumable_on_demand(self):
        self.assertEqual(list(prelude.take(4, prelude.iterate(lambda value: value + 1, 0))), [0, 1, 2, 3])
        self.assertEqual(list(prelude.take(3, prelude.repeat("x"))), ["x", "x", "x"])
        self.assertEqual(list(prelude.take(3, prelude.repeatedly(lambda: 7))), [7, 7, 7])
        self.assertEqual(list(prelude.take(5, prelude.cycle((1, 2)))), [1, 2, 1, 2, 1])

        events = []

        def infinite_source():
            value = 0
            while True:
                events.append(value)
                yield value
                value += 1

        self.assertEqual(
            list(prelude.take(4, prelude.cycle(infinite_source()))),
            [0, 1, 2, 3],
        )
        self.assertEqual(events, [0, 1, 2, 3])

    def test_repeat_counts_are_strict_integers(self):
        self.assertEqual(list(prelude.repeat(2, "x")), ["x", "x"])
        self.assertEqual(list(prelude.repeat(-2, "x")), [])
        self.assertEqual(list(prelude.repeatedly(2, lambda: "x")), ["x", "x"])
        self.assertEqual(list(prelude.repeatedly(-2, lambda: "x")), [])
        for value in (1.5, "2", True):
            with self.subTest(value=value):
                with self.assertRaises(TypeError):
                    prelude.repeat(value, "x")
                with self.assertRaises(TypeError):
                    prelude.repeatedly(value, lambda: "x")

    def test_sequence_predicates_use_the_osiris_collection_boundaries(self):
        values = prelude.sequence(iter((1, 2)))
        cases = [
            (None, False, False, False),
            ([], True, True, True),
            ((), False, True, True),
            ({"value": 1}, False, True, False),
            ({1, 2}, False, True, False),
            (values, True, True, True),
            ("text", False, False, False),
            (iter((1, 2)), False, False, False),
            (7, False, False, False),
        ]
        for value, expected_seq, expected_coll, expected_sequential in cases:
            with self.subTest(value=value):
                self.assertIs(prelude.seq_q(value), expected_seq)
                self.assertIs(prelude.coll_q(value), expected_coll)
                self.assertIs(prelude.sequential_q(value), expected_sequential)

    def test_distinct_is_lazy_replayable_and_supports_unhashable_values(self):
        events = []

        def source():
            events.append("source")
            yield from ([1], [1], [2], False, 0, False)

        values = prelude.distinct(source())
        self.assertEqual(events, [])
        self.assertEqual(list(prelude.take(4, values)), [[1], [2], False, 0])
        self.assertEqual(events, ["source"])
        self.assertEqual(list(values), [[1], [2], False, 0])
        self.assertEqual(events, ["source"])

    def test_deref_and_sequence_materialization_boundaries(self):
        self.assertEqual(prelude.first((1, 2)), 1)
        self.assertIsNone(prelude.first(()))
        self.assertEqual(prelude.rest((1, 2, 3)), [2, 3])
        self.assertEqual(list(prelude.next((1, 2, 3))), [2, 3])
        self.assertIsNone(prelude.next((1,)))
        self.assertEqual(prelude.nth((1, 2), 5, "fallback"), "fallback")
        self.assertEqual(prelude.count(None), 0)
        self.assertEqual(list(prelude.reductions(lambda acc, value: acc + value, 0, (1, 2, 3))), [0, 1, 3, 6])

    def test_mapcat_and_partition_treat_nil_boundaries_as_empty(self):
        self.assertEqual(
            list(prelude.mapcat(lambda value: None if value == 1 else (value,), (1, 2))),
            [2],
        )
        self.assertEqual(list(prelude.partition(2, 2, (), (1,))), [(1,)])

    def test_collection_combinators_treat_none_as_an_empty_sequence(self):
        calls = []

        def visit(value):
            calls.append(value)

        self.assertEqual(prelude.mapv(lambda value: value + 1, None), ())
        self.assertEqual(list(prelude.map(lambda value: value + 1, (1, 2), None)), [])
        self.assertEqual(prelude.mapcatv(lambda value: (value,), None), ())
        self.assertEqual(prelude.filterv(lambda value: True, None), ())
        self.assertEqual(list(prelude.filter(lambda value: True, None)), [])
        self.assertIsNone(prelude.doseq(visit, None))
        self.assertEqual(calls, [])
        self.assertEqual(prelude.reduce(lambda acc, value: acc + value, 10, None), 10)

    def test_empty_q_handles_nil_sized_and_memoized_iterables(self):
        self.assertTrue(prelude.empty_q(None))
        self.assertTrue(prelude.empty_q(()))
        self.assertFalse(prelude.empty_q((None,)))

        events = []

        def source():
            events.append("source")
            yield 1

        values = prelude.sequence(source())
        self.assertFalse(prelude.empty_q(values))
        self.assertFalse(prelude.empty_q(values))
        self.assertEqual(events, ["source"])

        with self.assertRaises(TypeError):
            prelude.empty_q(7)


if __name__ == "__main__":
    unittest.main()
