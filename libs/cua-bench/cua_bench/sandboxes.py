"""The one place cua-bench talks to cua-sandbox about sandbox lifecycle.

``open_sandbox`` yields a connected ``cua_sandbox.Sandbox`` for an
:class:`~cua_bench.targets.EnvSpec` on a :class:`~cua_bench.targets.Target`
and releases it on exit (including cancellation, e.g. Ctrl-C):

* ``--on local``: ``Sandbox.ephemeral(image, on="local")``. The SDK runs a
  container under gVisor (``runsc``) or a VM under QEMU / Lume.
* ``--on cloud`` (Cua Cloud) and ``--image pool:<name>`` end with
  :class:`~cua_bench.targets.CloudEnded`: Cua Cloud has ended. They are
  refused when the target is resolved; this module refuses a hand-built
  ``Target(on="cloud")`` too.
* ``--on e2b`` (``daytona``, ``modal``, ...): ``Sandbox.ephemeral(image,
  on="e2b")``; the SDK's contrib provider runs the same registry image on
  that platform. cua-bench has no provider-specific code: the location word
  goes to the SDK as is.
* ``--on aws`` (``gcp``, ``hetzner``, ``modal``): your own cloud account, connected with
  ``cua cloud connect``; the SDK starts one VM (or Hetzner server, or Modal sandbox) per
  sandbox, joined to the cua.ai relay, and deletes it with the handle.

The lifecycle uses only public cua-sandbox API (``Image``, ``Sandbox.ephemeral``).
"""

from __future__ import annotations

import asyncio
import logging
import re
from contextlib import asynccontextmanager
from dataclasses import dataclass
from typing import Any, AsyncIterator, Callable, Optional

from .targets import EnvSpec, Target

logger = logging.getLogger(__name__)

#: Sandbox boot deadline in seconds (a first start can pull an image).
TIME_TO_START_S = 900
#: Sandbox release deadline in seconds: past it the batch proceeds and the
#: sandbox may leak.
TIME_TO_RELEASE_S = 300


@dataclass
class Progress:
    """A lifecycle event for the progress display."""

    stage: str  # waiting | ready | release
    message: str
    elapsed: float = 0.0


ProgressFn = Callable[[Progress], Any]


def _sdk() -> tuple[Any, Any]:
    from cua_sandbox import Image, Sandbox

    return Image, Sandbox


def build_image(spec: EnvSpec, image_cls: Any = None) -> Any:
    """The cua-sandbox Image for a spec (registry refs resolve the same everywhere)."""
    if image_cls is None:
        image_cls, _ = _sdk()
    kind = spec.kind  # "container" | "vm"
    if spec.image:
        image = image_cls.from_registry(spec.image, os_type=spec.os_type, kind=kind)
        for port in getattr(spec, "ports", ()) or ():
            image = image.expose(port)
        return image
    factory = {
        "linux": image_cls.linux,
        "windows": image_cls.windows,
        "macos": image_cls.macos,
        "android": image_cls.android,
    }[spec.os_type]
    if spec.os_version:
        return factory(version=spec.os_version, kind=kind)
    return factory(kind=kind)


def index_runtime(ref: str, os_type: str = "linux") -> Optional[str]:
    """What a registry image offers: ``container`` (a rootfs), ``vm`` (only a
    containerDisk / Lume variant) or ``None`` when it cannot be read.

    Uses the SDK's one native resolver (``cua_sandbox.image.resolve_image_kind``)
    and never raises: planning falls back to the OS default.
    """
    try:
        from cua_sandbox import Image
        from cua_sandbox.image import resolve_image_kind

        resolved = resolve_image_kind(Image.from_registry(ref, os_type=os_type))
    except Exception as error:  # noqa: BLE001 - offline / unauthenticated / no SDK
        logger.debug("could not read the variant index of %s: %s", ref, error)
        return None
    kind = getattr(resolved, "kind", None)
    return kind if kind in ("container", "vm") else None


def cached_index_runtime() -> Callable[[str, str], Optional[str]]:
    """``index_runtime`` resolved once per (ref, os) for a batch."""
    cache: dict[tuple[str, str], Optional[str]] = {}

    def resolve(ref: str, os_type: str = "linux") -> Optional[str]:
        key = (ref, os_type)
        if key not in cache:
            cache[key] = index_runtime(ref, os_type)
        return cache[key]

    return resolve


def image_facts(sandbox: Any, spec: EnvSpec) -> dict:
    """What ran, for results: kind, runtime, image_ref, image_variant, image_digest, arch.

    ``Sandbox.image_info`` (cua-sandbox) reports the pinned digest the SDK
    resolved; without it the requested ref is recorded and the digest is
    ``None``.
    """
    info = None
    try:
        info = getattr(sandbox, "image_info", None)
    except Exception:  # noqa: BLE001 - informational only
        info = None
    facts = {
        "kind": spec.kind,
        "runtime": spec.runtime,
        "image_ref": spec.image,
        "image_variant": spec.image_variant,
        "image_digest": None,
        "arch": None,
    }
    if info is not None:
        facts["image_ref"] = getattr(info, "reference", None) or spec.image
        facts["image_variant"] = getattr(info, "variant", None) or spec.image_variant
        facts["image_digest"] = getattr(info, "pinned_ref", None) or None
        facts["arch"] = getattr(info, "arch", None)
    return facts


def ephemeral_kwargs(spec: EnvSpec, target: Target) -> dict:
    """Keyword arguments for ``Sandbox.ephemeral`` on ``target``.

    ``cpu``/``memory_mb`` size the sandbox (the SDK sizes local containers and
    VMs with them). Nothing is published for the display: images with cua-spacesd serve
    the viewer on the ``env`` service every sandbox declares, so
    ``Sandbox.get_display_url()`` returns a viewer link.
    """
    kwargs: dict[str, Any] = {
        "on": target.on,
        "time_to_start": TIME_TO_START_S,
        "telemetry_enabled": False,
    }
    if spec.runtime is not None:
        kwargs["runtime"] = spec.runtime
    if target.cpu is not None:
        kwargs["cpu"] = target.cpu
    if target.memory_mb is not None:
        kwargs["memory_mb"] = target.memory_mb
    if spec.server_port is not None:
        kwargs["server_port"] = spec.server_port
    return kwargs


@asynccontextmanager
async def open_sandbox(
    spec: EnvSpec,
    target: Target,
    *,
    on_progress: Optional[ProgressFn] = None,
) -> AsyncIterator[Any]:
    """Yield a connected sandbox for ``spec``; release it on exit."""
    image_cls, sandbox_cls = _sdk()

    if target.on == "cloud":
        # Cua Cloud has ended: `--on cloud` starts nothing.
        from cua_bench.targets import CloudEnded

        raise CloudEnded()
    image = build_image(spec, image_cls)
    kwargs = ephemeral_kwargs(spec, target)
    ephemeral = sandbox_cls.ephemeral(image, **kwargs)
    sb = await ephemeral.__aenter__()
    try:
        yield sb
    except BaseException as exc:
        await _release(ephemeral, type(exc), exc, exc.__traceback__)
        raise
    else:
        released = await _release(ephemeral, None, None, None)
        if released and on_progress is not None:
            on_progress(Progress(stage="release", message="sandbox released"))


async def _release(ephemeral: Any, *exc_info: Any) -> bool:
    """Exit ``ephemeral`` under the release bound. A wedged release logs a
    warning and returns False so the batch proceeds (the sandbox may leak);
    any other failure propagates."""
    try:
        async with asyncio.timeout(TIME_TO_RELEASE_S) as deadline:
            await ephemeral.__aexit__(*exc_info)
    except TimeoutError:
        if not deadline.expired():
            raise
        logger.warning(
            "sandbox release did not finish within %ss; the sandbox may leak",
            TIME_TO_RELEASE_S,
        )
        return False
    return True


# ── Errors worth explaining ────────────────────────────────────────────────

_PULL_PATTERNS = re.compile(
    r"ErrImagePull|ImagePullBackOff|manifest unknown|pull access denied|"
    r"repository does not exist|not found: manifest|no such image|"
    r"failed to (?:pull|resolve)|unauthorized: authentication required|"
    r"image .* not found|No such image",
    re.IGNORECASE,
)


def explain_error(error: BaseException, spec: Optional[EnvSpec], target: Target) -> str:
    """A one-paragraph, actionable message for common sandbox failures."""
    text = f"{type(error).__name__}: {error}"
    image = spec.image if spec is not None and spec.image else None
    if _PULL_PATTERNS.search(text):
        ref = image or "the task image"
        registry = ref.split("/", 1)[0] if "/" in ref else "docker.io"
        return (
            f"{ref} is not available locally and could not be pulled from {registry}. "
            f"Build it (docker build -t {ref} ...) or log in to the registry "
            "(docker login), then retry.\n  "
            f"({text})"
        )
    return text
