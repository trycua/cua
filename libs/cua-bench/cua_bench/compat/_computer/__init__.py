"""``computer``: the cua-computer 0.5 names, served by cua-bench.

Loaded only when the retired ``cua-computer`` package is not installed (see
``cua_bench.compat.install_computer_alias``). ``Computer`` attaches to a
running computer-server; the VM providers are gone.
"""

from cua_bench.compat.legacy_interface import Computer  # noqa: F401

__version__ = "0.5.17+cua-bench"
