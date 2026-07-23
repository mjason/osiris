from importlib.metadata import PackageNotFoundError, version as distribution_version

__all__ = ["version"]

try:
    __version__ = distribution_version("osiris-lang")
except PackageNotFoundError:
    __version__ = "0+unknown"


def version() -> str:
    return __version__
