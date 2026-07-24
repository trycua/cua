from __future__ import annotations

import os
import socket
import subprocess
import sys
from pathlib import Path


def _deny_runtime_operation(*args: object, **kwargs: object) -> None:
    raise RuntimeError("Sphinx autodoc runtime operations are disabled")


class _NoNetworkSocket(socket.socket):
    def connect(self, *args: object, **kwargs: object) -> None:
        _deny_runtime_operation(*args, **kwargs)

    def connect_ex(self, *args: object, **kwargs: object) -> int:
        _deny_runtime_operation(*args, **kwargs)
        return 1


socket.socket = _NoNetworkSocket
socket.create_connection = _deny_runtime_operation
subprocess.Popen = _deny_runtime_operation
os.system = _deny_runtime_operation

ROOT = Path(os.environ.get("CUA_SPHINX_ROOT", Path(__file__).resolve().parents[3]))
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


def _public_config_signature(
    app: object,
    what: str,
    name: str,
    obj: object,
    options: object,
    signature: str | None,
    return_annotation: str | None,
) -> tuple[str, str | None] | None:
    if name == "cua_sandbox.configure":
        return "(*, api_key: str | None = None, base_url: str | None = None)", return_annotation
    return None


def _public_config_docstring(
    app: object,
    what: str,
    name: str,
    obj: object,
    options: object,
    lines: list[str],
) -> None:
    if name == "cua_sandbox.configure":
        lines[:] = ["Configure the SDK API key and cloud endpoint."]
    elif name.startswith("cua_sandbox.Sandbox"):
        lines[:] = [line for line in lines if "fleet" not in line.lower()]


def setup(app: object) -> None:
    app.connect("autodoc-process-signature", _public_config_signature)
    app.connect("autodoc-process-docstring", _public_config_docstring)
