import threading
import unittest

from runtime_loader import prelude


class LazySequenceEdgeTests(unittest.TestCase):
    def test_none_thunk_is_an_empty_memoized_sequence(self):
        calls = []

        def produce():
            calls.append(1)
            return None

        sequence = prelude.lazy_seq(produce)
        self.assertFalse(sequence)
        self.assertEqual(list(sequence), [])
        self.assertEqual(calls, [1])

    def test_false_first_element_does_not_make_sequence_false(self):
        sequence = prelude.lazy_seq(lambda: (False, 1))
        self.assertTrue(sequence)
        self.assertEqual(list(sequence), [False, 1])

    def test_source_exception_is_memoized(self):
        calls = []

        def fail():
            calls.append("run")
            raise ValueError("lazy failure")

        sequence = prelude.lazy_seq(fail)
        for _ in range(2):
            with self.assertRaisesRegex(ValueError, "lazy failure"):
                list(sequence)
        self.assertEqual(calls, ["run"])

    def test_concurrent_consumers_realize_the_thunk_once(self):
        started = threading.Event()
        release = threading.Event()
        calls = []

        def produce():
            calls.append("run")
            started.set()
            self.assertTrue(release.wait(2))
            return (1, 2, 3)

        sequence = prelude.lazy_seq(produce)
        results = []

        def consume():
            results.append(list(sequence))

        first = threading.Thread(target=consume)
        second = threading.Thread(target=consume)
        first.start()
        self.assertTrue(started.wait(2))
        second.start()
        release.set()
        first.join(2)
        second.join(2)
        self.assertFalse(first.is_alive())
        self.assertFalse(second.is_alive())
        self.assertEqual(results, [[1, 2, 3], [1, 2, 3]])
        self.assertEqual(calls, ["run"])


if __name__ == "__main__":
    unittest.main()
