"""Bridge to the ``cua`` SDK (Rust core, UniFFI binding).

cua-sandbox is a thin wrapper: spacesd access and the Docker/QEMU/Lume
runtimes all go through ``cua`` (``libs/cua``).
This module keeps that dependency in one place and imports it lazily, so
``import cua_sandbox`` stays cheap and the hermetic tests that never touch a
real sandbox do not load the native library.
"""

from __future__ import annotations

import os
import tempfile
import threading
from pathlib import Path
from typing import Any, Optional

#: The port cua-spacesd listens on inside every sandbox image.
SPACESD_PORT = 3211
#: The service name cua-spacesd is published under.
ENV_SERVICE = "env"


class SpacesdNotAvailable(RuntimeError):
    """The sandbox has no reachable cua-spacesd.

    Sandboxes are daemon-agnostic: creating, listing and deleting them never
    needs a guest agent. The computer interfaces (``screen``, ``mouse``,
    ``shell``, ``files`` ...) need cua-spacesd, or one of the agentless
    transports (QMP, VNC, SSH, ADB) the runtime exposes.
    """


class Unsupported(NotImplementedError):
    """The operation is not supported for this sandbox, image or provider
    (the native SDK's ``CuaError.Unsupported``). A ``NotImplementedError``,
    so older ``except NotImplementedError`` handlers still catch it."""


class InvalidArgument(ValueError):
    """Contradictory or invalid options (the native SDK's
    ``CuaError.InvalidArgument``), for example ``on="local"`` together with
    ``local=False``."""


class InvalidPlacement(InvalidArgument):
    """A location, kind and runtime that do not go together (the native
    SDK's ``CuaError.InvalidPlacement``), for example ``kind="container"``
    with ``runtime="qemu"``. The message lists the valid values."""


def sdk() -> Any:
    """Import and return the ``cua`` SDK module."""
    try:
        import cua  # noqa: PLC0415 - imported lazily on purpose
    # OSError: the package is present but its native library was never built
    # or staged (a source checkout), which ctypes reports while loading it.
    except (ImportError, OSError) as error:  # pragma: no cover - packaging error
        raise ImportError(
            "cua-sandbox needs the `cua` SDK (the native cua-sdk binding). "
            "Install it with `pip install cua` or, in the repo, "
            "`uv pip install -e libs/cua/python` after building cua-sdk."
        ) from error
    native = getattr(cua, "_native", None)
    if native is None or not hasattr(native, "Cua"):
        raise ImportError(
            "the installed `cua` package is not the cua SDK (it has no native "
            "binding); uninstall the old `cua` meta-package (<0.2) and install cua>=0.2"
        )
    return cua


def native() -> Any:
    """The generated binding module (``cua._native``)."""
    return sdk()._native


def is_env_not_available(error: BaseException) -> bool:
    """Whether *error* is the SDK's "no spacesd here" error."""
    try:
        cls = native().CuaError.SpacesdNotAvailable
    except Exception:  # noqa: BLE001 - no SDK means it cannot be that error
        return False
    return isinstance(error, cls)


def is_not_found(error: BaseException) -> bool:
    try:
        cls = native().CuaError.NotFound
    except Exception:  # noqa: BLE001
        return False
    return isinstance(error, cls)


_RUNTIMES: dict[Optional[str], Any] = {}
_RUNTIMES_LOCK = threading.Lock()
_PRIVATE_STATE_DIR: Optional[Path] = None


def runtime(*, state_dir: Optional[os.PathLike | str] = None) -> Any:
    """An embedded ``cua.Cua`` runtime.

    One runtime per state dir is cached for the life of the process: the
    runtime owns port forwards and connection pools, and creating one does no
    I/O.
    """
    key = str(state_dir) if state_dir is not None else None
    with _RUNTIMES_LOCK:
        existing = _RUNTIMES.get(key)
        if existing is not None:
            return existing
        created = sdk().embedded(state_dir=None if state_dir is None else str(state_dir))
        _RUNTIMES[key] = created
        return created


def local_runtime() -> Any:
    """The runtime local sandboxes use; state lives beside cua-sandbox's own."""
    from cua_sandbox import sandbox_state

    return runtime(state_dir=sandbox_state.state_dir())


def daemon_runtime() -> Any:
    """The running ``cua daemon`` (it keeps the pools of
    :mod:`cua_sandbox.sandbox_pool`). It shares the state directory
    of :func:`local_runtime`, so a Space it creates connects like any other
    local one. Raises a clear error when no daemon runs."""
    n = native()
    try:
        return n.Cua.connect(None, None)
    except Exception as error:  # noqa: BLE001 - say what to do instead
        raise n.CuaError.Unsupported(
            "pools are kept by `cua daemon`: start it (`cua daemon start`) "
            f"and try again ({error})"
        ) from error


def _private_state_dir() -> Path:
    """A private SDK state directory for direct URLs.

    The Python layer keeps the user-visible sandbox records in
    ``$CUA_HOME/sandboxes``; the SDK only needs transient records to remember
    a URL, so it gets its own directory and never rewrites the user's files.
    """
    global _PRIVATE_STATE_DIR
    if _PRIVATE_STATE_DIR is None:
        _PRIVATE_STATE_DIR = Path(tempfile.mkdtemp(prefix="cua-sandbox-sdk-"))
    return _PRIVATE_STATE_DIR


async def connect_url(url: str, token: Optional[str] = None, name: Optional[str] = None) -> Any:
    """A direct ``cua.Sandbox`` for a spacesd URL (nothing is probed).

    The SDK keys direct handles (and their env connections) by name, and an
    unnamed one is named after its host; a unique name keeps two connections
    to the same host with different tokens from sharing a client.
    """
    import uuid

    unique = name or f"direct-{uuid.uuid4().hex[:12]}"
    return await runtime(state_dir=_private_state_dir()).sandboxes().connect_url(url, token, unique)


def millis(seconds: Optional[float]) -> Optional[int]:
    if seconds is None:
        return None
    return max(1, int(seconds * 1000))
