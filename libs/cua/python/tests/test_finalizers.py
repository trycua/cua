"""Handles alive at interpreter exit must not print finalizer errors.

UniFFI's ``__del__`` freed handles through module globals that CPython has
already cleared at shutdown, so every process ended with
``Exception ignored in: <function Cua.__del__> ... AttributeError``. The
binding is post-processed (libs/cua/scripts/uniffi-python-postprocess.mjs);
libs/python/cua-sandbox/tests/test_sdk_finalizers.py runs a process that
used to print the error.
"""

from __future__ import annotations

import re
from pathlib import Path

PACKAGE = Path(__file__).resolve().parents[1] / "src" / "cua"
NATIVE = PACKAGE / "_native.py"


def test_every_generated_finalizer_is_shutdown_safe():
    source = NATIVE.read_text()
    finalizers = re.findall(r"    def __del__\((.*?)\):\n(.*?)\n\n", source, re.S)
    assert finalizers, "the binding has no object finalizers"
    assert len(finalizers) == source.count("def __del__("), "a finalizer escaped the rewrite"
    for params, body in finalizers:
        assert "_uniffi_finalizing=sys.is_finalizing" in params, params
        assert "_uniffi_free=_UniffiLib." in params, params
        assert "_UniffiLib" not in body, body
        assert "not _uniffi_finalizing()" in body, body

