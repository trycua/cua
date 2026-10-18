"""Where cua-sandbox keeps state: ``$CUA_HOME``, else ``~/.cua``.

The same rule as the Rust core (``cua_daemon::cua_home``): a non-empty
``CUA_HOME`` wins, else ``$HOME`` (``%USERPROFILE%`` on Windows) plus
``.cua``. It is read on every call, never cached, so a process (or a test)
that sets ``CUA_HOME`` never touches the real ``~/.cua``.

Module constants such as ``sandbox_state.SANDBOX_STATE_DIR`` keep their
import-time value for compatibility (tests patch them); :func:`patched_or`
returns the patched value when one was set and the live path otherwise.
"""

from __future__ import annotations

import os
from pathlib import Path


def cua_home() -> Path:
    """``$CUA_HOME`` when set and non-empty, else ``~/.cua``."""
    home = os.environ.get("CUA_HOME")
    if home:
        return Path(home)
    base = os.environ.get("HOME") or os.environ.get("USERPROFILE") or "."
    return Path(base) / ".cua"


def _truthy(value: str | None) -> bool:
    return bool(value) and value.strip().lower() not in ("", "0", "false")


def is_test_process() -> bool:
    """``CUA_TEST`` is set, or pytest is running a test."""
    return _truthy(os.environ.get("CUA_TEST")) or bool(os.environ.get("PYTEST_CURRENT_TEST"))


def real_cua_home() -> Path | None:
    """The user's real ``~/.cua`` from the account database, not ``$HOME``."""
    try:
        import pwd

        return Path(pwd.getpwuid(os.getuid()).pw_dir) / ".cua"
    except (ImportError, KeyError, AttributeError):
        profile = os.environ.get("USERPROFILE")
        return Path(profile) / ".cua" if profile else None


def guard_write(path: Path) -> None:
    """Refuses (``PermissionError``) a write under the real ``~/.cua`` from a
    test, the same guard as the Rust core (``cua-home``): a test that forgets
    a temporary ``CUA_HOME`` fails instead of leaking state into the user's
    Spaces app."""
    if not is_test_process():
        return
    real = real_cua_home()
    if real is None:
        return
    target = Path(path).expanduser().absolute()
    try:
        inside = target.resolve().is_relative_to(real.resolve())
    except OSError:
        inside = target.is_relative_to(real)
    if inside or target.is_relative_to(real):
        raise PermissionError(
            f"refusing to write {target} from a test: it is the user's real ~/.cua. "
            "Isolate the test with a temporary CUA_HOME."
        )


def patched_or(current: Path, import_default: Path, *parts: str) -> Path:
    """``current`` when a caller replaced the module constant (it is no
    longer the import-time object), else ``cua_home()/parts`` now."""
    if current is not import_default:
        return current
    return cua_home().joinpath(*parts)
