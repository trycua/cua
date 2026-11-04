"""The version cua-e2b identifies itself with (User-Agent, `default_headers`)."""

from importlib import metadata

try:
    package_version = metadata.version("cua-e2b")
except metadata.PackageNotFoundError:  # running from a source tree
    package_version = "0.1.0"
