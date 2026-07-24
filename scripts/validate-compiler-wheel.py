#!/usr/bin/env python3
"""Validate the deployment boundary of an osiris-lang compiler wheel."""

from __future__ import annotations

import sys
from pathlib import Path
from zipfile import ZipFile


REPOSITORY_ROOT = Path(__file__).resolve().parents[1]
STANDARD_LIBRARY_FILES = {
    "stdlib/README.md",
    "stdlib/pyproject.toml",
    "stdlib/osiris.jsonc",
    "stdlib/uv.lock",
    *{
        path.relative_to(REPOSITORY_ROOT).as_posix()
        for path in (REPOSITORY_ROOT / "stdlib" / "src").rglob("*.osr")
    },
}


def validate(path: Path) -> None:
    with ZipFile(path) as archive:
        names = set(archive.namelist())

    failures: list[str] = []
    if not any(name == "osiris_build/__init__.py" for name in names):
        failures.append("missing osiris_build backend package")
    if not any(
        ".data/scripts/" in name and name.rsplit("/", 1)[-1] in {"osr", "osr.exe"}
        for name in names
    ):
        failures.append("missing native osr script")
    if any(name.startswith("osiris/") for name in names):
        failures.append("contains forbidden shared osiris runtime package")
    if any("/__osiris_runtime__/" in name for name in names):
        failures.append("contains generated distribution-private runtime support")
    missing_standard_files = sorted(STANDARD_LIBRARY_FILES - names)
    if missing_standard_files:
        failures.append(
            "missing source-distributed standard library files: "
            + ", ".join(missing_standard_files)
        )
    if failures:
        raise SystemExit(f"{path}: " + "; ".join(failures))


def main(arguments: list[str]) -> None:
    if not arguments:
        raise SystemExit("usage: validate-compiler-wheel.py WHEEL...")
    for argument in arguments:
        validate(Path(argument))


if __name__ == "__main__":
    main(sys.argv[1:])
