import unittest

from runtime_loader import prelude


class PackageImportTests(unittest.TestCase):
    def test_compiler_owned_templates_are_self_contained(self):
        self.assertEqual(prelude.mapv(lambda value: value + 1, (1, 2)), (2, 3))

if __name__ == "__main__":
    unittest.main()
