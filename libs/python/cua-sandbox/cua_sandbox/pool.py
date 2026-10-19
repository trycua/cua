"""Pools and templates: Cua Cloud has ended.

Every call raises :class:`~cua_sandbox._ended.CloudEnded`. The classes stay
importable so existing code keeps importing; the module goes away in the next
minor release.
"""

from __future__ import annotations

from typing import Any

from cua_sandbox._ended import CloudEnded
from cua_sandbox._sdk import ENV_SERVICE
from cua_sandbox.spec import PoolExport, SandboxSpec


class Template:
    """A sandbox template on Cua Cloud, which has ended. Every call raises
    :class:`~cua_sandbox._ended.CloudEnded`."""

    def __init__(self, *args: Any, **kwargs: Any) -> None:
        raise CloudEnded()

    @classmethod
    async def reconcile(cls, request: Any) -> "Template":
        raise CloudEnded()


class Pool:
    """A warm pool on Cua Cloud, which has ended. Every call raises
    :class:`~cua_sandbox._ended.CloudEnded`."""

    def __init__(self, *args: Any, **kwargs: Any) -> None:
        raise CloudEnded()

    @classmethod
    async def reconcile(cls, request: Any) -> "Pool":
        raise CloudEnded()

    @classmethod
    async def get(cls, name: str) -> "Pool":
        raise CloudEnded()

    @classmethod
    async def apply(cls, *args: Any, **kwargs: Any) -> "Pool":
        raise CloudEnded()

    @staticmethod
    async def export(name: str, *, terraform: bool = False) -> "PoolExport | str":
        raise CloudEnded()

    @staticmethod
    async def check(name: str, spec: SandboxSpec) -> None:
        raise CloudEnded()

    @staticmethod
    async def apply_template(name: str, spec: SandboxSpec) -> None:
        raise CloudEnded()

    async def delete(self) -> None:
        raise CloudEnded()

    async def create_claim(
        self,
        *,
        spec: Any = None,
        name: str | None = None,
        ttl_seconds_after_created: int | None = None,
    ) -> Any:
        raise CloudEnded()

    def claim(
        self,
        *,
        spec: Any = None,
        name: str | None = None,
        service: str = ENV_SERVICE,
        time_to_start: float | None = None,
        ttl_seconds_after_created: int | None = None,
        agent_type: str | None = None,
        claim_token: str | None = None,
    ) -> Any:
        raise CloudEnded()
