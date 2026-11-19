"""Compatibility with code written against cua-bench 0.2.x.

* :mod:`.legacy_interface`: the cua-computer 0.5 interface surface
  (``session.computer`` / ``session.interface``), over a computer-server or
  a cua-sandbox sandbox.
* :func:`install_computer_alias`: makes ``import computer`` resolve to that
  surface when the retired ``cua-computer`` package is not installed (0.2.x
  pulled it in, and harnesses such as Agents' Last Exam import it directly).
"""

from __future__ import annotations

import importlib.abc
import importlib.util
import os
import sys
from pathlib import Path

_SHIM_DIR = Path(__file__).resolve().parent / "_computer"


class _ComputerAlias(importlib.abc.MetaPathFinder):
    """Last-resort finder for the top-level ``computer`` package.

    It sits at the end of ``sys.meta_path``, so an installed ``cua-computer``
    always wins; only an import that would fail lands here.
    """

    def find_spec(self, fullname, path=None, target=None):  # noqa: D401
        if fullname != "computer":
            return None
        return importlib.util.spec_from_file_location(
            "computer", _SHIM_DIR / "__init__.py", submodule_search_locations=[str(_SHIM_DIR)]
        )


def install_computer_alias() -> bool:
    """Register the ``computer`` fallback (idempotent). Off with ``CUA_BENCH_COMPUTER_ALIAS=0``."""
    if os.environ.get("CUA_BENCH_COMPUTER_ALIAS", "").strip().lower() in (
        "0",
        "false",
        "off",
        "no",
    ):
        return False
    if any(isinstance(finder, _ComputerAlias) for finder in sys.meta_path):
        return True
    sys.meta_path.append(_ComputerAlias())
    return True


class _MovedModuleFinder(importlib.abc.MetaPathFinder, importlib.abc.Loader):
    """``cua_bench.<old>[.x]`` imports ``<new>[.x]`` (a package that moved out)."""

    def __init__(self, old: str, new: str, install_hint: str) -> None:
        self.old, self.new, self.hint = old, new, install_hint

    def find_spec(self, fullname, path=None, target=None):  # noqa: D401
        if fullname != self.old and not fullname.startswith(self.old + "."):
            return None
        return importlib.util.spec_from_loader(fullname, self, is_package=True)

    def create_module(self, spec):
        import importlib as _importlib
        import warnings

        target = self.new + spec.name[len(self.old) :]
        if spec.name == self.old:
            warnings.warn(
                f"{self.old} moved to {self.new} ({self.hint}); import it from there",
                DeprecationWarning,
                stacklevel=3,
            )
        try:
            return _importlib.import_module(target)
        except ModuleNotFoundError as error:
            if error.name and self.new.split(".")[0] == error.name.split(".")[0]:
                raise ModuleNotFoundError(
                    f"{spec.name} moved to {target}: {self.hint}", name=spec.name
                ) from error
            raise

    def exec_module(self, module) -> None:
        return None


def install_moved_modules() -> None:
    """``cua_bench.workers`` / ``cua_bench.trainer`` now live in cua-bench-rl."""
    if any(isinstance(finder, _MovedModuleFinder) for finder in sys.meta_path):
        return
    hint = "pip install cua-bench-rl"
    for old, new in (
        ("cua_bench.workers", "cua_bench_rl.workers"),
        ("cua_bench.trainer", "cua_bench_rl.trainer"),
    ):
        sys.meta_path.insert(0, _MovedModuleFinder(old, new, hint))


__all__ = ["install_computer_alias", "install_moved_modules"]
