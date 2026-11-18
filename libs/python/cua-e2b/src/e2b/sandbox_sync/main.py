"""Sandbox: the synchronous e2b API, run on a private event-loop thread over
the asynchronous implementation (`AsyncSandbox`)."""

from __future__ import annotations

import logging
from typing import Any, Dict, List, Optional

from e2b.sandbox.sandbox_api import (
    SandboxInfo,
    SandboxMetrics,
    SandboxNetworkUpdate,
    SnapshotInfo,
)
from e2b.sandbox.utils import class_method_variant
from e2b.sandbox_async.main import AsyncSandbox
from e2b.sandbox_sync._loop import Loop
from e2b.sandbox_sync.commands.command import Commands
from e2b.sandbox_sync.commands.command_handle import CommandHandle
from e2b.sandbox_sync.commands.pty import Pty
from e2b.sandbox_sync.filesystem.filesystem import Filesystem
from e2b.sandbox_sync.filesystem.watch_handle import WatchHandle
from e2b.sandbox_sync.git import Git
from e2b.sandbox_sync.paginator import SandboxPaginator, SnapshotPaginator
from e2b.sandbox_sync.sandbox_api import SandboxApi

_Loop = Loop  # name the template and volume modules import
_run = Loop.run


class Sandbox(SandboxApi):
    """The synchronous e2b `Sandbox`, backed by a Cua Space of kind `vm`.

    Create it with `Sandbox.create(...)`; the asynchronous implementation runs
    on a background thread (output callbacks you pass to `commands.run` run
    there)."""

    def __init__(self, inner: AsyncSandbox):
        self._s = inner
        self._commands = Commands(inner.commands)
        self._files = Filesystem(inner.files)
        self._pty = Pty(inner.pty)
        self._git = Git(self._commands)

    # -- the identity SandboxBase holds, taken from the async sandbox ----------

    connection_config = property(lambda self: self._s.connection_config)
    sandbox_id = property(lambda self: self._s.sandbox_id)
    sandbox_domain = property(lambda self: self._s.sandbox_domain)
    envd_api_url = property(lambda self: self._s.envd_api_url)
    envd_direct_url = property(lambda self: self._s.envd_direct_url)
    traffic_access_token = property(lambda self: self._s.traffic_access_token)
    metadata = property(lambda self: self._s.metadata)

    @property
    def _mcp_token(self) -> Optional[str]:
        return self._s._mcp_token

    @property
    def commands(self) -> Commands:
        return self._commands

    @property
    def files(self) -> Filesystem:
        return self._files

    @property
    def pty(self) -> Pty:
        return self._pty

    @property
    def git(self) -> Git:
        return self._git

    # -- lifecycle ------------------------------------------------------------

    @classmethod
    def create(
        cls,
        template: Optional[str] = None,
        timeout: Optional[int] = None,
        metadata: Optional[Dict[str, str]] = None,
        envs: Optional[Dict[str, str]] = None,
        secure: Optional[bool] = None,
        allow_internet_access: Optional[bool] = None,
        mcp=None,
        network=None,
        iam=None,
        lifecycle=None,
        volume_mounts=None,
        logger: Optional[logging.Logger] = None,
        **opts,
    ) -> "Sandbox":
        """Creates a sandbox (see `AsyncSandbox.create`)."""
        return cls(
            _run(
                AsyncSandbox.create(
                    template=template,
                    timeout=timeout,
                    metadata=metadata,
                    envs=envs,
                    secure=secure,
                    allow_internet_access=allow_internet_access,
                    mcp=mcp,
                    network=network,
                    iam=iam,
                    lifecycle=lifecycle,
                    volume_mounts=volume_mounts,
                    logger=logger,
                    **opts,
                )
            )
        )

    @classmethod
    def _cls_connect(
        cls, sandbox_id: str, timeout: Optional[int] = None, *, on_resume: str = "restore", **opts
    ) -> "Sandbox":
        return cls(
            _run(AsyncSandbox._cls_connect(sandbox_id, timeout, on_resume=on_resume, **opts))
        )

    @class_method_variant("_cls_connect")
    def connect(
        self, timeout: Optional[int] = None, *, on_resume: str = "restore", **opts
    ) -> "Sandbox":
        """Connects to the sandbox again, resuming it if it was paused."""
        _run(self._s.connect(timeout=timeout, on_resume=on_resume, **opts))
        return self

    @classmethod
    def _cls_kill(cls, sandbox_id: str, **opts) -> bool:
        return _run(AsyncSandbox._cls_kill(sandbox_id, **opts))

    @class_method_variant("_cls_kill")
    def kill(self, **opts) -> bool:
        """Deletes the Space. `False` if it was already gone."""
        return _run(self._s.kill(**opts))

    @classmethod
    def _cls_set_timeout(cls, sandbox_id: str, timeout: int, **opts) -> None:
        return _run(AsyncSandbox._cls_set_timeout(sandbox_id, timeout, **opts))

    @class_method_variant("_cls_set_timeout")
    def set_timeout(self, timeout: int, **opts) -> None:
        """Sets the lifetime to `timeout` seconds from now."""
        return _run(self._s.set_timeout(timeout, **opts))

    def is_running(self, request_timeout: Optional[float] = None) -> bool:
        return _run(self._s.is_running(request_timeout))

    @classmethod
    def _cls_update_network(cls, sandbox_id: str, network: SandboxNetworkUpdate, **opts) -> None:
        return _run(AsyncSandbox._cls_update_network(sandbox_id, network, **opts))

    @class_method_variant("_cls_update_network")
    def update_network(self, network: SandboxNetworkUpdate, **opts) -> None:
        """Replaces the sandbox's egress rules (see `AsyncSandbox.update_network`)."""
        return _run(self._s.update_network(network, **opts))

    # -- info -------------------------------------------------------------------

    @classmethod
    def _cls_get_info(cls, sandbox_id: str, **opts) -> SandboxInfo:
        return _run(AsyncSandbox._cls_get_info(sandbox_id, **opts))

    @class_method_variant("_cls_get_info")
    def get_info(self, **opts) -> SandboxInfo:
        return _run(self._s.get_info(**opts))

    @classmethod
    def _cls_get_metrics(
        cls, sandbox_id: str, start=None, end=None, **opts
    ) -> List[SandboxMetrics]:
        return _run(AsyncSandbox._cls_get_metrics(sandbox_id, start, end, **opts))

    @class_method_variant("_cls_get_metrics")
    def get_metrics(self, start=None, end=None, **opts) -> List[SandboxMetrics]:
        return _run(self._s.get_metrics(start, end, **opts))

    def get_host(self, port: int) -> str:
        """A `host:port` that reaches guest `port` (plain TCP: `http://`)."""
        return self._s.get_host(port)

    def download_url(self, path, user=None, use_signature_expiration=None) -> str:
        return self._s.download_url(path, user, use_signature_expiration)

    def upload_url(self, path, user=None, use_signature_expiration=None) -> str:
        return self._s.upload_url(path, user, use_signature_expiration)

    def get_mcp_url(self) -> str:
        return self._s.get_mcp_url()

    def get_mcp_token(self) -> Optional[str]:
        return _run(self._s.get_mcp_token())

    # -- pause, snapshots, fork ------------------------------------------------------

    @classmethod
    def _cls_pause(cls, sandbox_id: str, keep_memory: Optional[bool] = None, *, mode=None, **opts):
        return _run(AsyncSandbox._cls_pause(sandbox_id, keep_memory, mode=mode, **opts))

    @class_method_variant("_cls_pause")
    def pause(self, keep_memory: Optional[bool] = None, *, mode: Optional[str] = None, **opts):
        """Suspends the VM; `connect()` resumes it."""
        return _run(self._s.pause(keep_memory, mode=mode, **opts))

    beta_pause = pause

    @classmethod
    def _cls_create_snapshot(
        cls, sandbox_id: str, name: Optional[str] = None, *, mode: Optional[str] = None, **opts
    ) -> SnapshotInfo:
        return _run(AsyncSandbox._cls_create_snapshot(sandbox_id, name, mode=mode, **opts))

    @class_method_variant("_cls_create_snapshot")
    def create_snapshot(
        self, name: Optional[str] = None, *, mode: Optional[str] = None, **opts
    ) -> SnapshotInfo:
        return _run(self._s.create_snapshot(name, mode=mode, **opts))

    @classmethod
    def _cls_list_snapshots(
        cls, sandbox_id=None, limit=None, next_token=None, name=None, **opts
    ) -> SnapshotPaginator:
        return SnapshotPaginator(sandbox_id, limit, next_token, name, **opts)

    @class_method_variant("_cls_list_snapshots")
    def list_snapshots(self, limit=None, next_token=None, name=None, **opts) -> SnapshotPaginator:
        return SnapshotPaginator(self.sandbox_id, limit, next_token, name, **opts)

    @classmethod
    def delete_snapshot(cls, snapshot_id: str, **opts) -> bool:
        """Deletes a snapshot (sandboxes made from it keep running)."""
        return _run(AsyncSandbox.delete_snapshot(snapshot_id, **opts))

    @classmethod
    def _cls_fork_sandbox(
        cls, sandbox_id: str, timeout=None, count=None, logger=None, **opts
    ) -> list:
        return [
            cls(r) if isinstance(r, AsyncSandbox) else r
            for r in _run(
                AsyncSandbox._cls_fork_sandbox(sandbox_id, timeout, count, logger, **opts)
            )
        ]

    @class_method_variant("_cls_fork_sandbox")
    def fork(self, timeout=None, count=None, **opts) -> list:
        """`count` running copies of this sandbox (disk and memory)."""
        return [
            type(self)(r) if isinstance(r, AsyncSandbox) else r
            for r in _run(self._s.fork(timeout=timeout, count=count, **opts))
        ]

    def __enter__(self):
        return self

    def __exit__(self, *exc: Any) -> None:
        self.kill()

    def __repr__(self) -> str:
        return f"Sandbox(sandbox_id={self.sandbox_id!r})"


__all__ = [
    "CommandHandle",
    "Sandbox",
    "SandboxPaginator",
    "SnapshotPaginator",
    "WatchHandle",
]
