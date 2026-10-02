"""Where a new sandbox runs, what kind it is and which engine runs it.

Three independent axes, the same on every cua surface:

* ``on``: ``"local"`` or ``"cloud"`` (``local=True``/``False`` is the same
  switch). Other locations (``direct:<addr>``, registered providers) are
  existing machines or plugins, not something cua-sandbox creates.
* ``kind``: ``"auto"``, ``"container"`` or ``"vm"``.
* ``runtime``: ``"auto"`` or an engine the location offers for the kind:
  locally ``gvisor``/``runc`` (containers) and ``qemu``/``lume`` (VMs); in the
  cloud ``gvisor`` (containers) and ``kubevirt`` (VMs). A legacy
  :class:`~cua_sandbox.runtime.Runtime` object still picks a local adapter.

Unset values come from the user's defaults, in this order: environment
(``CUA_DEFAULT_ON``, ``CUA_DEFAULT_KIND``, ``CUA_DEFAULT_RUNTIME``), then
``$CUA_HOME/config.toml`` (``cua config set default.on cloud``), then the
built-in ``local``/``auto``/``auto``. The native cua SDK owns both the
settings and the validation; this module only asks it.
"""

from __future__ import annotations

import logging
import os
import warnings
from dataclasses import dataclass
from typing import Any, Optional

from cua_sandbox._sdk import InvalidArgument, InvalidPlacement, Unsupported

logger = logging.getLogger(__name__)

#: Locations cua-sandbox creates sandboxes on.
LOCATIONS = ("local", "cloud")
KINDS = ("auto", "container", "vm")
#: Every engine; which ones a location offers is the native SDK's rule.
ENGINES = ("auto", "gvisor", "runc", "qemu", "lume", "kubevirt")
#: The kind an engine runs.
ENGINE_KIND = {
    "gvisor": "container",
    "runc": "container",
    "qemu": "vm",
    "lume": "vm",
    "kubevirt": "vm",
}

_ENV = {"default.on": "CUA_DEFAULT_ON", "default.kind": "CUA_DEFAULT_KIND"}
_ENV["default.runtime"] = "CUA_DEFAULT_RUNTIME"
_BUILTIN = {"default.on": "local", "default.kind": "auto", "default.runtime": "auto"}

_default_notice_shown = False


@dataclass(frozen=True)
class Setting:
    """One user default: its value and where it came from (``env``,
    ``config`` or ``default``; ``origin`` names the variable or file)."""

    value: str
    source: str
    origin: str = ""


def setting(key: str) -> Setting:
    """The effective value of ``key`` (``default.on``, ``default.kind``,
    ``default.runtime``) from ``cua.config_get``."""
    try:
        from cua_sandbox._sdk import native

        entry = native().config_get(key)
        origin = getattr(entry, "_from", None) or getattr(entry, "from_", "") or ""
        return Setting(str(entry.value), str(entry.source), str(origin))
    except ImportError:
        # No native SDK (a packaging error elsewhere): env, else built-in.
        env = _ENV.get(key)
        value = os.environ.get(env, "").strip() if env else ""
        if value:
            return Setting(value, "env", env or "")
        return Setting(_BUILTIN.get(key, ""), "default")


def translate(error: BaseException) -> BaseException:
    """The cua-sandbox exception for a native placement error (the message,
    with the valid values, is kept); anything else unchanged."""
    try:
        from cua_sandbox._sdk import native

        cua_error = native().CuaError
    except Exception:  # noqa: BLE001 - no SDK: nothing to translate
        return error
    if isinstance(error, cua_error.InvalidPlacement):
        return InvalidPlacement(str(error))
    if isinstance(error, cua_error.InvalidArgument):
        return InvalidArgument(str(error))
    return error


def check(on: str, kind: Optional[str], runtime: Optional[str]) -> None:
    """Validates the combination with the native rule
    (``cua.check_placement``); raises :class:`InvalidPlacement` listing the
    valid values."""
    from cua_sandbox._sdk import native

    try:
        native().check_placement(on, kind or "", runtime or "")
    except Exception as error:  # noqa: BLE001 - translated below
        translated = translate(error)
        if translated is error:
            raise
        raise translated from None


def _text(value: Any, what: str) -> Optional[str]:
    """``None``/``""``/``"auto"`` as ``None``; otherwise the lowercased word."""
    if value is None:
        return None
    if not isinstance(value, str):
        raise InvalidArgument(f"{what}= takes a string, got {type(value).__name__}")
    word = value.strip().lower()
    return None if word in ("", "auto") else word


def _on_word(on: Any) -> str:
    if not isinstance(on, str) or not on.strip():
        raise InvalidArgument(f"on= takes 'local' or 'cloud', got {on!r}")
    text = on.strip()
    head, sep, rest = text.partition(":")
    return head.lower() + sep + rest if sep else text.lower()


@dataclass(frozen=True)
class Placement:
    """The resolved location, kind and engine of a new sandbox."""

    on: str
    #: ``explicit`` (on=/local=), ``implied`` (cloud options, a legacy
    #: Runtime object), or the default's source (``env``, ``config``,
    #: ``default``).
    on_source: str
    on_origin: str = ""
    #: ``container``/``vm``, or ``None`` for auto.
    kind: Optional[str] = None
    #: An engine string, or ``None`` for auto.
    runtime: Optional[str] = None
    #: A legacy Runtime object (local adapters), when one was passed.
    legacy_runtime: Any = None

    @property
    def local(self) -> bool:
        return self.on == "local"

    def cloud_default_hint(self) -> Optional[str]:
        """What to say when the cloud came from a user default and there are
        no cloud credentials."""
        if self.on != "cloud" or self.on_source not in ("env", "config"):
            return None
        origin = self.on_origin or ("CUA_DEFAULT_ON" if self.on_source == "env" else "config")
        tail = " and unset CUA_DEFAULT_ON" if self.on_source == "env" else ""
        return (
            f"the default location is cloud ({origin}); run `cua auth login`, or switch "
            f"back with `cua config set default.on local`{tail}"
        )


def _notice() -> None:
    global _default_notice_shown
    if _default_notice_shown:
        return
    _default_notice_shown = True
    quiet = os.environ.get("CUA_QUIET_DEFAULT", "").strip().lower()
    if quiet not in ("1", "true", "yes", "on"):
        # Logged from cua_sandbox.sandbox, where users filter it.
        logging.getLogger("cua_sandbox.sandbox").warning(
            "defaulting to a local sandbox; pass local=False (or on='cloud') for cloud, "
            "or make it the default with `cua config set default.on cloud` "
            "(CUA_QUIET_DEFAULT=1 hides this notice)"
        )


def resolve(
    *,
    on: Optional[str] = None,
    local: Optional[bool] = None,
    kind: Optional[str] = None,
    runtime: Any = None,
    cloud: Any = None,
    cloud_only: Optional[list[str]] = None,
    image_kind: Optional[str] = None,
    stacklevel: int = 4,
) -> Placement:
    """Where, what kind and which engine (see the module docstring).

    Location: ``on=`` or ``local=`` (contradicting each other is
    :class:`InvalidArgument`); else ``cloud=`` implies the cloud; else a
    cloud-only argument keeps the cloud with a ``DeprecationWarning``; else a
    legacy ``Runtime`` object means local; else ``default.on``. The one-time
    "defaulting to a local sandbox" notice appears only when that is the
    built-in default.

    Kind: ``kind=`` > the image's ``kind`` > the engine's kind >
    ``default.kind`` (skipped when it does not fit) > auto.
    Engine: ``runtime=`` > ``default.runtime`` (skipped when it does not
    fit) > auto.
    """
    legacy = runtime if runtime is not None and not isinstance(runtime, str) else None
    engine = _text(runtime, "runtime") if legacy is None else None
    explicit_kind = _text(kind, "kind")

    if on is not None:
        where, source, origin = _on_word(on), "explicit", ""
        if local is not None and (where == "local") != bool(local):
            raise InvalidArgument(
                f"on={on!r} and local={local!r} contradict each other; pass one of them"
            )
        if where == "local" and cloud is not None:
            raise InvalidArgument(
                "on='local' and cloud=CloudOptions(...) contradict each other: cloud options "
                "run the sandbox in the cloud. Drop cloud=, or pass on='cloud'"
            )
    elif local is not None:
        if local and cloud is not None:
            raise InvalidArgument(
                "local=True and cloud=CloudOptions(...) contradict each other: cloud options "
                "run the sandbox in the cloud. Drop cloud=, or pass local=False"
            )
        where, source, origin = ("local" if local else "cloud"), "explicit", ""
    elif cloud is not None:
        where, source, origin = "cloud", "implied", ""
    elif cloud_only:
        warnings.warn(
            f"{', '.join(cloud_only)} only appl{'ies' if len(cloud_only) == 1 else 'y'} in the "
            "cloud, so this sandbox runs in the cloud; sandboxes are local by default now. "
            "Pass local=False to keep this behaviour without the warning",
            DeprecationWarning,
            stacklevel=stacklevel,
        )
        where, source, origin = "cloud", "implied", ""
    elif legacy is not None:
        # A Runtime object is a local adapter (Docker, QEMU, Lume, Tart, ...).
        where, source, origin = "local", "implied", ""
    else:
        default = setting("default.on")
        where, source, origin = _on_word(default.value), default.source, default.origin
        if source == "default" and where == "local":
            _notice()

    if legacy is not None:
        if where != "local":
            if on is not None:
                raise InvalidArgument(
                    f"runtime={type(legacy).__name__}(...) runs the sandbox on this machine; "
                    "drop on='cloud', or pass an engine name (runtime='gvisor'/'kubevirt')"
                )
            # Shipped behaviour (0.8.0): a Runtime object ran locally, even
            # with local=False.
            where, source, origin = "local", "implied", ""
        if explicit_kind is not None:
            check("local", explicit_kind, None)
        return Placement(where, source, origin, explicit_kind or image_kind, None, legacy)

    # Validate what was asked for before defaults fill the rest.
    check(where, explicit_kind, engine)
    if where not in LOCATIONS:
        raise Unsupported(
            f"on={where!r}: cua-sandbox creates local and cloud sandboxes. Connect to an "
            "existing machine with Sandbox.connect(url=...), or use the cua SDK for other "
            "providers"
        )

    # The image's kind (Image.linux(kind="vm")) is as explicit as kind=.
    resolved_kind = explicit_kind or image_kind or (ENGINE_KIND.get(engine) if engine else None)
    if resolved_kind is None:
        default_kind = _text(setting("default.kind").value, "default.kind")
        if default_kind is not None and _fits(where, default_kind, engine):
            resolved_kind = default_kind
    # The image's own kind against the engine (Image.linux(kind="vm") with
    # runtime="gvisor").
    check(where, resolved_kind, engine)
    if engine is None:
        default_engine = _text(setting("default.runtime").value, "default.runtime")
        if default_engine is not None and _fits(where, resolved_kind, default_engine):
            engine = default_engine
            resolved_kind = resolved_kind or ENGINE_KIND.get(engine)
    return Placement(where, source, origin, resolved_kind, engine, None)


def _fits(on: str, kind: Optional[str], runtime: Optional[str]) -> bool:
    try:
        check(on, kind, runtime)
    except InvalidArgument:
        return False
    return True
