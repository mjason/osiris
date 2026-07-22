import importlib.util
import sys
import threading
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
PRELUDE_PATH = ROOT / "src" / "osiris" / "prelude.py"
spec = importlib.util.spec_from_file_location("osiris_test_dynamic_binding", PRELUDE_PATH)
if spec is None or spec.loader is None:
    raise RuntimeError("could not load the Osiris runtime prelude")
prelude = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = prelude
spec.loader.exec_module(prelude)


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
