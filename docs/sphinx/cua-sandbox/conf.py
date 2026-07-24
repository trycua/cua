from __future__ import annotations

import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "libs/python/cua-sandbox"))
os.environ.setdefault("CUA_SPHINX_AUTODOC", "1")

project = "CUA Sandbox API"
html_title = "CUA Sandbox API"
html_theme = "basic"
html_last_updated_fmt = None
extensions = ["sphinx.ext.autodoc", "sphinx.ext.autosummary", "sphinx.ext.napoleon"]
autosummary_generate = False
autodoc_typehints = "signature"
autodoc_mock_imports = [
    "cua_auto",
    "cua_core",
    "cyclops_sdk",
    "google",
    "grpc",
    "httpx",
    "oras",
    "paramiko",
    "pycdlib",
    "vncdotool",
    "websockets",
]
exclude_patterns = ["_build"]
