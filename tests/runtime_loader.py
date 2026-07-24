"""Load compiler-owned Python support templates for direct unit tests."""

import importlib.util
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TEMPLATES = ROOT / "src" / "backend" / "python" / "runtime_templates"
SPEC = importlib.util.spec_from_file_location(
    "osiris_runtime_templates",
    TEMPLATES / "__init__.py",
    submodule_search_locations=[str(TEMPLATES)],
)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("could not load Osiris runtime templates")
prelude = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = prelude
SPEC.loader.exec_module(prelude)

__all__ = ["prelude"]
