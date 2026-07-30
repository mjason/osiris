"""Osiris → Python name translation (OEP-0001-R005A/R005B).

This is the backend's single implementation of the identifier mapping, the
counterpart of `language/name/bindings.rs` on the compiler side. The backend
needs it because a wheel is Python territory: every archive path spells module
components the Python way, while a declared module name inside an `.osri` stays
Osiris-spelled. Comparing one against the other without translating first is
how `my-pkg.core` compiled fine and then could never be packaged — the check
in `_interface.py` demanded the two spellings be literally equal.

Transcribed from `python_identifier` / `python_module_identifier`, minus the
`\\0`-prefixed compiler-internal branch, which cannot reach an interface or an
archive path. Classification goes through `unicodedata.category` rather than
`str.isalnum`/`str.isnumeric` because the Rust side classifies by Unicode
category while Python's predicates follow `Numeric_Type` — `"五"` is numeric to
Python and alphabetic to Rust, and a name like `五行` would otherwise translate
differently on the two sides. The `NameTranslationParityTests` vectors hold the
two implementations to agreement (OEP-0001-R005D).
"""

import unicodedata

# The Rust side matches an explicit list rather than asking the host, so a new
# Python version cannot silently change which names gain a trailing `_`.
_PYTHON_KEYWORDS = frozenset(
    (
        "False", "None", "True", "and", "as", "assert", "async", "await",
        "break", "class", "continue", "def", "del", "elif", "else", "except",
        "finally", "for", "from", "global", "if", "import", "in", "is",
        "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try",
        "while", "with", "yield",
    )
)

_LETTER_OR_NUMBER = ("Lu", "Ll", "Lt", "Lm", "Lo", "Nd", "Nl", "No")
_NUMBER = ("Nd", "Nl", "No")


def python_identifier(name: str) -> str:
    result = []
    for character in unicodedata.normalize("NFC", name):
        if character == "-":
            result.append("_")
        elif character == "?":
            result.append("_p")
        elif character == "!":
            result.append("_bang")
        elif character == "_" or unicodedata.category(character) in _LETTER_OR_NUMBER:
            result.append(character)
        else:
            result.append("_u%x_" % ord(character))
    identifier = "".join(result)
    if not identifier:
        identifier = "_osiris_empty"
    if unicodedata.category(identifier[0]) in _NUMBER:
        identifier = "_" + identifier
    if identifier in _PYTHON_KEYWORDS:
        identifier += "_"
    return identifier


def python_module_identifier(module: str) -> str:
    components = module.replace("/", ".").split(".")
    return ".".join(python_identifier(component) for component in components)
