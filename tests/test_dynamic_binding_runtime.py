import threading
import unittest

from runtime_loader import prelude


class DynamicBindingRuntimeTests(unittest.TestCase):
    def test_nested_binding_and_exception_restore_the_previous_context(self):
        key = "demo::value::*value*"

        def nested():
            self.assertEqual(prelude.dynamic_get(key, 1), 2)
            return prelude.binding_values(
                (key,), (3,), lambda: prelude.dynamic_get(key, 1)
            )

        self.assertEqual(prelude.binding_values((key,), (2,), nested), 3)
        self.assertEqual(prelude.dynamic_get(key, 1), 1)

        with self.assertRaisesRegex(ValueError, "boom"):
            prelude.binding_values(
                (key,),
                (4,),
                lambda: (_ for _ in ()).throw(ValueError("boom")),
            )
        self.assertEqual(prelude.dynamic_get(key, 1), 1)

    def test_context_is_isolated_between_threads_and_copied_to_futures(self):
        key = "demo::value::*value*"
        entered = threading.Event()
        release = threading.Event()
        observed = []

        def worker():
            observed.append(prelude.dynamic_get(key, 1))
            entered.set()
            self.assertTrue(release.wait(2))
            observed.append(prelude.dynamic_get(key, 1))

        thread = threading.Thread(target=worker)

        def scoped():
            thread.start()
            self.assertTrue(entered.wait(2))
            future = prelude.future_call(lambda: prelude.dynamic_get(key, 1))
            self.assertEqual(prelude.deref(future), 7)
            release.set()

        prelude.binding_values((key,), (7,), scoped)
        thread.join(2)
        self.assertFalse(thread.is_alive())
        self.assertEqual(observed, [1, 1])
        self.assertEqual(prelude.dynamic_get(key, 1), 1)


if __name__ == "__main__":
    unittest.main()
