from __future__ import annotations

import sys
from pathlib import Path

from osiris._core import _run_cli, _run_lsp_stdio, version

__all__ = ["version"]
__version__ = version()


def main() -> None:
    if sys.argv[1:] == ["lsp"]:
        _run_lsp_stdio()
        return
    arguments = list(sys.argv[1:])
    if arguments[:1] == ["compile"]:
        roots = sorted(
            {
                str(Path(entry).resolve())
                for entry in sys.path
                if entry and Path(entry).is_dir()
            }
        )
        for root in roots:
            arguments.extend(["--site-root", root])
    exit_code, stdout, stderr = _run_cli(arguments)
    sys.stdout.write(stdout)
    sys.stderr.write(stderr)
    if exit_code:
        raise SystemExit(exit_code)
