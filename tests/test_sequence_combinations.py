import unittest

from runtime_loader import prelude


class SequenceCombinationTests(unittest.TestCase):
    def test_dedupe_is_lazy_replayable_and_uses_clojure_equality(self):
        events = []

        def source():
            events.append("source")
            yield from ([1], [1], False, False, 0, 0, False)

        values = prelude.dedupe(source())
        self.assertEqual(events, [])
        self.assertEqual(list(values), [[1], False, 0, False])
        self.assertEqual(list(values), [[1], False, 0, False])
        self.assertEqual(events, ["source"])
        self.assertEqual(list(prelude.dedupe(None)), [])

    def test_partition_supports_overlap_gaps_and_final_padding(self):
        self.assertEqual(
            list(prelude.partition(3, (0, 1, 2, 3, 4))),
            [(0, 1, 2)],
        )
        self.assertEqual(
            list(prelude.partition(3, 2, range(6))),
            [(0, 1, 2), (2, 3, 4)],
        )
        self.assertEqual(
            list(prelude.partition(2, 3, range(7))),
            [(0, 1), (3, 4)],
        )
        self.assertEqual(
            list(prelude.partition(3, 2, (9, 8), (0, 1, 2, 3))),
            [(0, 1, 2), (2, 3, 9)],
        )
        self.assertEqual(
            list(prelude.partition(4, 4, (9,), (1, 2))),
            [(1, 2, 9)],
        )
        self.assertEqual(list(prelude.partition(2, None)), [])

    def test_partition_all_retains_each_incomplete_trailing_window(self):
        self.assertEqual(
            list(prelude.partition_all(3, (0, 1, 2, 3, 4))),
            [(0, 1, 2), (3, 4)],
        )
        self.assertEqual(
            list(prelude.partition_all(3, 1, (0, 1, 2))),
            [(0, 1, 2), (1, 2), (2,)],
        )
        self.assertEqual(
            list(prelude.partition_all(3, 4, range(6))),
            [(0, 1, 2), (4, 5)],
        )

    def test_partition_realizes_only_the_requested_prefix_and_replays_it(self):
        events = []

        def source():
            for value in range(5):
                events.append(value)
                yield value

        windows = prelude.partition_all(2, source())
        self.assertEqual(events, [])
        iterator = iter(windows)
        self.assertEqual(next(iterator), (0, 1))
        self.assertEqual(events, [0, 1])
        self.assertEqual(next(iter(windows)), (0, 1))
        self.assertEqual(events, [0, 1])
        self.assertEqual(list(iterator), [(2, 3), (4,)])
        self.assertEqual(list(windows), [(0, 1), (2, 3), (4,)])
        self.assertEqual(events, [0, 1, 2, 3, 4])

    def test_partition_by_calls_the_key_once_per_value_without_eager_input(self):
        calls = []

        def key(value):
            calls.append(value)
            return value

        groups = prelude.partition_by(key, (False, False, 0, 0, False))
        self.assertEqual(calls, [])
        self.assertEqual(
            [list(group) for group in groups],
            [[False, False], [0, 0], [False]],
        )
        self.assertEqual(calls, [False, False, 0, 0, False])
        self.assertEqual(
            [list(group) for group in groups],
            [[False, False], [0, 0], [False]],
        )
        self.assertEqual(calls, [False, False, 0, 0, False])
        self.assertEqual(list(prelude.partition_by(key, None)), [])

    def test_partition_by_returns_an_infinite_first_group_without_materializing_it(self):
        events = []

        def source():
            value = 0
            while True:
                events.append(value)
                yield value
                value += 1

        groups = prelude.partition_by(lambda _: "same", source())
        self.assertEqual(events, [])
        first_group = next(iter(groups))
        self.assertEqual(events, [0])
        self.assertEqual(list(prelude.take(4, first_group)), [0, 1, 2, 3])
        self.assertEqual(events, [0, 1, 2, 3])

    def test_interleave_and_interpose_cover_shortest_and_empty_inputs(self):
        self.assertEqual(
            list(prelude.interleave((1, 2, 3), ("a", "b"), (True, False, True))),
            [1, "a", True, 2, "b", False],
        )
        self.assertEqual(list(prelude.interleave((1, 2), None)), [])
        self.assertEqual(list(prelude.interpose("|", (1, 2, 3))), [1, "|", 2, "|", 3])
        self.assertEqual(list(prelude.interpose("|", (1,))), [1])
        self.assertEqual(list(prelude.interpose("|", None)), [])

    def test_take_last_is_deferred_and_drop_last_streams_with_bounded_buffer(self):
        events = []

        def source():
            for value in range(5):
                events.append(value)
                yield value

        tail = prelude.take_last(2, source())
        self.assertEqual(events, [])
        self.assertEqual(list(tail), [3, 4])
        self.assertEqual(list(tail), [3, 4])
        self.assertEqual(events, [0, 1, 2, 3, 4])

        infinite_prefix = prelude.take(
            4,
            prelude.drop_last(2, prelude.iterate(lambda value: value + 1, 0)),
        )
        self.assertEqual(list(infinite_prefix), [0, 1, 2, 3])
        self.assertEqual(list(prelude.drop_last((1, 2, 3))), [1, 2])
        self.assertEqual(list(prelude.drop_last(0, (1, 2))), [1, 2])
        self.assertEqual(list(prelude.take_last(2, None)), [])
        self.assertEqual(list(prelude.drop_last(2, None)), [])

    def test_counts_steps_and_arity_are_validated(self):
        for function, arguments in (
            (prelude.partition, (0, (1, 2))),
            (prelude.partition, (2, 0, (1, 2))),
            (prelude.partition_all, (-1, (1, 2))),
            (prelude.take_last, (-1, (1, 2))),
            (prelude.drop_last, (-1, (1, 2))),
        ):
            with self.subTest(function=function.__name__, arguments=arguments):
                with self.assertRaises(ValueError):
                    function(*arguments)
        with self.assertRaises(TypeError):
            prelude.interleave((1, 2))
        with self.assertRaises(TypeError):
            prelude.partition_by(1, (1, 2))

    def test_nth_distinguishes_omitted_and_explicit_nil_fallbacks(self):
        with self.assertRaises(IndexError):
            prelude.nth((1,), 4)
        with self.assertRaises(IndexError):
            prelude.nth(None, 0)
        with self.assertRaises(IndexError):
            prelude.nth((1,), -1)
        self.assertIsNone(prelude.nth((1,), 4, None))
        self.assertEqual(prelude.nth((1,), -1, "missing"), "missing")


if __name__ == "__main__":
    unittest.main()
