"""Bridge to the ``cua`` SDK (Rust core, UniFFI binding).

cua-sandbox is a thin wrapper: spacesd access, Fleet sandbox attachment
and the Docker/QEMU/Lume runtimes all go through ``cua`` (``libs/cua``).
This module keeps that dependency in one place and imports it lazily, so
``import cua_sandbox`` stays cheap and the hermetic tests that never touch a
real sandbox do not load the native library.
"""

from __future__ import annotations

import json
import os
import tempfile
import threading
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

#: The port cua-spacesd listens on inside every sandbox image.
SPACESD_PORT = 3211
#: The Fleet service name the spacesd is published under.
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
    ``CuaError.InvalidArgument``), for example ``local=True`` together with
    ``cloud=CloudOptions(...)``."""


class InvalidPlacement(InvalidArgument):
    """A location, kind and runtime that do not go together (the native
    SDK's ``CuaError.InvalidPlacement``), for example ``kind="container"``
    with ``runtime="qemu"``, or ``runtime="kubevirt"`` locally. The message
    lists the valid values."""


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


def fleet_settings() -> Any:
    """``cua.FleetSettings`` built from cua-sandbox's configuration."""
    from cua_sandbox._config import (
        get_client_id,
        get_client_secret,
        get_fleet_base_url,
        get_fleet_token,
        get_token_url,
    )

    token = get_fleet_token()
    return native().FleetSettings(
        base_url=get_fleet_base_url().rstrip("/"),
        token_url=get_token_url(),
        client_id=None if token else get_client_id(),
        client_secret=None if token else get_client_secret(),
        token=token,
    )


_RUNTIMES: dict[tuple, Any] = {}
_RUNTIMES_LOCK = threading.Lock()
_PRIVATE_STATE_DIR: Optional[Path] = None


def runtime(
    *,
    state_dir: Optional[os.PathLike | str] = None,
    fleet: bool = False,
    pool_home: Optional[os.PathLike | str] = None,
) -> Any:
    """An embedded ``cua.Cua`` runtime.

    One runtime per (state dir, Fleet configuration) is cached for the life of
    the process: the runtime owns port forwards, connection pools and the
    managed Fleet claims' heartbeats, and creating one does no I/O.
    ``pool_home`` is where managed Fleet pools keep their name cache and GC
    lock (default ``~/.cua``).
    """
    settings = fleet_settings() if fleet else None
    key = (
        str(state_dir) if state_dir is not None else None,
        str(pool_home) if pool_home is not None else None,
        (
            None
            if settings is None
            else (
                settings.base_url,
                settings.token_url,
                settings.client_id,
                settings.client_secret,
                settings.token,
            )
        ),
    )
    with _RUNTIMES_LOCK:
        existing = _RUNTIMES.get(key)
        if existing is not None:
            return existing
        created = sdk().embedded(
            state_dir=None if state_dir is None else str(state_dir),
            fleet=settings,
            fleet_from_env=False,
            # No token or client credentials: the `cua auth login` session.
            fleet_from_session=fleet and _session_allowed(),
            fleet_pool_home=None if pool_home is None else str(pool_home),
        )
        _RUNTIMES[key] = created
        return created


def _session_allowed() -> bool:
    return os.environ.get("CUA_FLEET_SESSION", "").strip().lower() not in (
        "0",
        "false",
        "off",
        "no",
    )


def local_runtime() -> Any:
    """The runtime local sandboxes use; state lives beside cua-sandbox's own."""
    from cua_sandbox import sandbox_state

    return runtime(state_dir=sandbox_state.state_dir())


def _private_state_dir() -> Path:
    """A private SDK state directory for Fleet attachments and direct URLs.

    The Python layer keeps the user-visible sandbox records in
    ``$CUA_HOME/sandboxes``; the SDK only needs transient records to attach to a
    claim or remember a URL, so it gets its own directory and never rewrites
    the user's files.
    """
    global _PRIVATE_STATE_DIR
    if _PRIVATE_STATE_DIR is None:
        _PRIVATE_STATE_DIR = Path(tempfile.mkdtemp(prefix="cua-sandbox-sdk-"))
    return _PRIVATE_STATE_DIR


async def fleet_sandbox(
    namespace: str,
    claim: str,
    env_token: Optional[str] = None,
    *,
    image_info: Any = None,
) -> Any:
    """A ``cua.Sandbox`` for a bound Fleet claim (pool name == namespace).
    ``env_token`` is the claim's per-claim env token, when it has one;
    ``image_info`` (a ``cua_sandbox.ImageInfo``) what its ``image_info()``
    reports."""
    state_dir = _private_state_dir()
    record = {
        "name": claim,
        "runtime_type": "fleet",
        "pool_name": namespace,
        "status": "running",
        "created_at": datetime.now(timezone.utc).isoformat(),
    }
    if env_token:
        record["env_token"] = env_token
    if image_info is not None:
        import dataclasses

        record["image_info"] = dataclasses.asdict(image_info)
    path = state_dir / f"{claim}.json"
    path.write_text(json.dumps(record))
    if env_token:
        path.chmod(0o600)
    return await runtime(state_dir=state_dir, fleet=True).sandboxes().connect(claim)


async def pool_image_info(pool: str) -> Any:
    """The SDK's ``ImageInfo`` for named Fleet pool ``pool``'s template image
    (``Fleet.pool_image_info``: pinned, cached per pool; the reference without
    a digest when its registry cannot be read), or ``None``. Never raises."""
    try:
        fleet = runtime(state_dir=_private_state_dir(), fleet=True).fleet()
        return await fleet.pool_image_info(pool)
    except Exception as error:  # noqa: BLE001 - informational; never fail a claim
        import logging

        logging.getLogger(__name__).debug("image of pool %s not read: %s", pool, error)
        return None


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
