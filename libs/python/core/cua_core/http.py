"""Shared HTTP utilities for CUA SDK requests."""

from functools import lru_cache

CUA_CLIENT_VERSION_HEADER = "X-Cua-Client-Version"

# CUA packages whose versions are included in the header value.
_CUA_PACKAGES = ("cua-agent", "cua-computer", "cua-core")


@lru_cache(maxsize=1)
def _build_version_string() -> str:
    """Return a composite version string like ``agent:0.4.0 computer:0.1.0 core:0.1.8``."""
    from importlib.metadata import PackageNotFoundError, version

    parts: list[str] = []
    for pkg in _CUA_PACKAGES:
        try:
            short = pkg.removeprefix("cua-")
            parts.append(f"{short}:{version(pkg)}")
        except PackageNotFoundError:
            continue
    return " ".join(parts)


def cua_version_headers() -> dict[str, str]:
    """Return headers dict containing the CUA client version header.

    Only installed CUA packages are included.  If none are found the dict is
    empty so it is always safe to unpack with ``**cua_version_headers()``.
    """
    value = _build_version_string()
    if not value:
        return {}
    return {CUA_CLIENT_VERSION_HEADER: value}
