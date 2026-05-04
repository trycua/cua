"""conftest.py — makes cua_driver importable when running examples without install.

Add the SDK src/ directory to sys.path so that ``import cua_driver`` works
when running ``pytest sdk/examples/python/`` directly from the repo root
without ``pip install -e .``.

Also explicitly registers the pytest_cua plugin so that its fixtures
(cua_driver, cua_app, async_cua_driver, async_cua_app) are available even
when the package is not installed via pip (i.e. no entry-point discovery).
"""

import os
import sys

_SDK_SRC = os.path.normpath(
    os.path.join(os.path.dirname(__file__), "..", "..", "python", "src")
)
if _SDK_SRC not in sys.path:
    sys.path.insert(0, _SDK_SRC)

# Explicitly load the pytest_cua plugin fixtures (bypasses entry-point lookup)
pytest_plugins = ["pytest_cua.plugin"]
