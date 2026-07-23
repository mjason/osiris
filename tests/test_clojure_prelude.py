import threading
import time
import unittest

from runtime_loader import prelude


class ClojurePreludeTests(unittest.TestCase):
    def test_lazy_sequence_combinators_are_replayable(self):
        calls = []

        def step(value):
            calls.append(value)
            return value + 1

        values = prelude.take(4, prelude.iterate(step, 0))
        self.assertEqual(list(values), [0, 1, 2, 3])
        self.assertEqual(list(values), [0, 1, 2, 3])
        self.assertEqual(calls, [0, 1, 2])

    def test_sequence_helpers_follow_nil_and_clojure_truthiness(self):
        self.assertEqual(list(prelude.cons(1, None)), [1])
        self.assertEqual(list(prelude.concat((1, 2), None, (3,))), [1, 2, 3])
        self.assertIsNone(prelude.first(None))
        self.assertEqual(prelude.rest((1, 2, 3)), [2, 3])
        self.assertIsNone(prelude.next((1,)))
        self.assertEqual(list(prelude.next((1, 2, 3))), [2, 3])
        self.assertEqual(prelude.nth((1,), 9, "missing"), "missing")
        self.assertIsNone(prelude.nth((1,), 9, None))
        with self.assertRaisesRegex(IndexError, "out of range"):
            prelude.nth((1,), 9)
        with self.assertRaisesRegex(IndexError, "out of range"):
            prelude.nth((1,), -1)
        self.assertEqual(prelude.nth(prelude.iterate(lambda value: value + 1, 0), 5), 5)
        self.assertEqual(list(prelude.take_while(lambda value: value < 3, (1, 2, 3, 4))), [1, 2])
        self.assertEqual(list(prelude.drop_while(lambda value: value < 3, (1, 2, 3, 4))), [3, 4])
        self.assertEqual(list(prelude.remove(lambda value: value, (None, False, 0, 1))), [None, False])
        self.assertEqual(list(prelude.keep(lambda value: value, (None, False, 0, 1))), [False, 0, 1])

    def test_reductions_and_short_circuit_predicates(self):
        self.assertEqual(list(prelude.reductions(lambda acc, value: acc + value, 0, (1, 2, 3))), [0, 1, 3, 6])
        self.assertEqual(prelude.some(lambda value: value if value > 1 else None, (1, 2, 3)), 2)
        self.assertTrue(prelude.every_q(lambda value: value, (0, "", ())))
        self.assertFalse(prelude.not_any_q(lambda value: value == 2, (1, 2, 3)))
        self.assertIsNone(prelude.doall(None))
        self.assertIsNone(prelude.dorun(None))
        values = prelude.iterate(lambda value: value + 1, 0)
        self.assertIs(prelude.doall(3, values), values)
        self.assertIsNone(prelude.dorun(3, values))

    def test_future_promise_timeout_and_locking(self):
        future = prelude.future_call(lambda: 42)
        self.assertEqual(prelude.deref(future), 42)
        self.assertTrue(prelude.future_done(future))

        promise = prelude.promise()
        self.assertEqual(prelude.deref(promise, 0, "timeout"), "timeout")
        delivered = prelude.deliver(promise, 7)
        self.assertIs(delivered, promise)
        self.assertEqual(prelude.deref(promise), 7)
        self.assertEqual(prelude.deref(promise, 0, "ignored"), 7)

        lock = prelude.lock()
        self.assertEqual(prelude.locking(lock, lambda: "inside"), "inside")
        self.assertEqual(prelude.locking(lock, lambda: "again"), "again")

    def test_locking_releases_after_exception(self):
        lock = prelude.lock()
        with self.assertRaisesRegex(ValueError, "boom"):
            prelude.locking(lock, lambda: (_ for _ in ()).throw(ValueError("boom")))
        self.assertTrue(lock.acquire(timeout=0.1))
        lock.release()

    def test_locking_closes_context_manager_on_success_and_failure(self):
        events = []

        class Guard:
            def __enter__(self):
                events.append("enter")
                return self

            def __exit__(self, exc_type, exc, traceback):
                events.append(("exit", exc_type.__name__ if exc_type else None))
                return False

        guard = Guard()
        self.assertEqual(prelude.locking(guard, lambda: 1), 1)
        with self.assertRaisesRegex(ValueError, "boom"):
            prelude.locking(guard, lambda: (_ for _ in ()).throw(ValueError("boom")))
        self.assertEqual(events, ["enter", ("exit", None), "enter", ("exit", "ValueError")])


if __name__ == "__main__":
    unittest.main()
