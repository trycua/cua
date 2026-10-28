"""Sandbox references: one scheme for sandboxes and Spaces.

``local:<name>``, ``direct:<host:port>`` and ``relay:<machine-id>``; a bare
name is a local sandbox. A ``cloud:<name>`` ref still parses, and resolving it
raises :class:`~cua_sandbox._ended.CloudEnded`: Cua Cloud has ended. Parsing is
the Rust core's (``cua.parse_sandbox_ref``), so legacy spellings parse the same
way on every surface.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass
from typing import Callable, Optional

from cua_sandbox._ended import CloudEnded
from cua_sandbox._sdk import InvalidArgument, Unsupported

logger = logging.getLogger(__name__)


class AmbiguousSandbox(LookupError):
    """A bare sandbox name matches sandboxes in more than one location.

    ``candidates`` are the qualified refs it matches (``local:box``,
    ``relay:<id>``); pass one of them.
    The native SDK raises ``cua.CuaError.AmbiguousSandbox`` for the same
    case."""

    def __init__(self, name: str, candidates: list[str]):
        self.name = name
        self.candidates = list(candidates)
        super().__init__(
            f"{name!r} names {len(self.candidates)} sandboxes; use one of: "
            + ", ".join(self.candidates)
        )


@dataclass(frozen=True)
class Ref:
    """A parsed ref: ``location`` is ``local``, ``cloud``, ``direct``,
    ``relay`` or ``None`` (a bare name); ``name`` is the part after it."""

    location: Optional[str]
    name: str

    @property
    def id(self) -> str:
        return f"{self.location}:{self.name}" if self.location else self.name


def parse(value: str) -> Ref:
    """Parses a ref or a legacy spelling (a bare name stays bare)."""
    text = (value or "").strip()
    if not text:
        raise InvalidArgument("a sandbox name or ref is required")
    if ":" not in text and "/" not in text:
        return Ref(None, text)
    from cua_sandbox._sdk import native

    try:
        parts = native().parse_sandbox_ref(text)
    except Exception as error:  # noqa: BLE001 - re-raised typed
        raise InvalidArgument(str(error)) from error
    return Ref(parts.location, parts.name)


def qualified(location: str, name: str) -> str:
    """``location:name`` (the id every surface prints)."""
    return f"{location}:{name}"


def _narrowed(ref: Ref, local: Optional[bool]) -> Ref:
    if local is None:
        return ref
    want = "local" if local else "cloud"
    if ref.location is None:
        return Ref(want, ref.name)
    if ref.location != want:
        raise InvalidArgument(f"{ref.id} is a {ref.location} sandbox, not {want} (local={local})")
    return ref


async def resolve(
    name: str, local: Optional[bool], *, is_local: Optional[Callable[[str], bool]] = None
) -> tuple[str, Optional[bool], Optional[str]]:
    """``(plain name, local, url)`` for a sandbox ref or name.

    A qualified ref decides the location (``direct:`` becomes a URL). A bare
    name is the local sandbox when one exists. A ``cloud:`` ref, or
    ``local=False``, raises :class:`~cua_sandbox._ended.CloudEnded`.
    ``relay:`` machines are Spaces (:class:`~cua_sandbox.Unsupported` here).
    """
    ref = _narrowed(parse(name), local)
    if ref.location == "local":
        return ref.name, True, None
    if ref.location == "cloud":
        raise CloudEnded()
    if ref.location == "direct":
        return ref.name, None, f"http://{ref.name}"
    if ref.location == "relay":
        raise Unsupported(
            f"{ref.id}: relay computers are Spaces; open it with cua.embedded().spaces()"
        )
    if is_local is None:
        from cua_sandbox.sandbox import _is_local_sandbox as is_local
    return ref.name, bool(is_local(ref.name)), None
