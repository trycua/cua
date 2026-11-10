"""Identification the SDK would send with every request: version and runtime."""

import platform

from e2b._version import package_version

default_headers = {
    "lang": "python",
    "lang_version": platform.python_version(),
    "package_version": package_version,
    "publisher": "cua-e2b",
    "sdk_runtime": "python",
    "system": platform.system(),
}
