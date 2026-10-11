"""Provider locations: contrib platforms (``on="e2b"``, ``on="daytona"``,
``on="modal"``) and your own cloud account (``on="aws"``, ``on="gcp"``,
``on="modal"`` with the ``byoc`` build, connected with ``cua cloud connect``).

Third-party platforms run the same registry images (``Image.from_registry``
or the canonical ``Image.linux()``) through the cua SDK's contrib providers:
the SDK resolves and pins the image, builds and caches the provider's
template or snapshot by digest, exposes cua-spacesd and the declared ports
through the provider's URLs, and deletes the sandbox with the handle. The
providers are opt-in builds of the ``cua`` SDK (``--features contrib``); a
build without one raises :class:`~cua_sandbox.Unsupported` naming the build
to use. Keys come from the provider's environment variable (``E2B_API_KEY``,
``DAYTONA_API_KEY``, ``MODAL_TOKEN_ID``/``MODAL_TOKEN_SECRET``) or
``cua auth provider set <name>``.
"""

from __future__ import annotations

from typing import Any, Optional

from cua_sandbox._sdk import local_runtime, native
from cua_sandbox.image import Image, cloud_registry_image
from cua_sandbox.runtime.base import RuntimeInfo
from cua_sandbox.runtime.native import NativeRuntime

#: The contrib words of a ``cua`` binding that predates ``sandbox_locations``.
_KNOWN = (
    "e2b",
    "daytona",
    "modal",
    "cloudflare",
    "vercel",
    "morph",
    "runloop",
    "fly",
    "blaxel",
    "codesandbox",
    "northflank",
    "aws",
    "gcp",
)


def contrib_locations() -> list[str]:
    """The provider location words the SDK knows, contrib and your own
    clouds (whether or not built)."""
    try:
        words = native().sandbox_locations()
    except (ImportError, AttributeError):
        words = _KNOWN
    return [w for w in words if w not in ("local", "cloud")]


def is_contrib(on: Optional[str]) -> bool:
    """Whether ``on`` names a contrib provider."""
    return bool(on) and on.strip().lower() in contrib_locations()


class ContribRuntime(NativeRuntime):
    """A sandbox on a contrib provider, through the SDK's ``Sandboxes``."""

    provider_kind = "CONTRIB"
    #: The provider takes the token itself (the SDK installs one).
    delivers_guest_env = False

    def __init__(self, provider: str, **kwargs: Any) -> None:
        super().__init__(**kwargs)
        self.on = provider.strip().lower()
        self.runtime_type = self.on

    async def _image_ref(self, image: Image, name: str, **opts: Any) -> str:
        ref = cloud_registry_image(image)
        if ref is None:
            from cua_sandbox._sdk import Unsupported

            raise Unsupported(
                f"on={self.on!r} runs registry images: use Image.from_registry(...) or a "
                "canonical image (Image.linux())"
            )
        if image.kind == "vm":
            return f"vm:{ref}"
        if image.kind == "container":
            return f"container:{ref}"
        return ref

    def _info(self, name: str, handle: Any, image: Optional[Image]) -> RuntimeInfo:
        # Everything is reached through the SDK handle (the provider's port
        # URLs), never a local host:port.
        services = dict(handle.info().services)
        env_port = services.get("env")
        return RuntimeInfo(
            host="",
            api_port=0,
            name=name,
            environment=image.os_type if image else None,
            guest_server_port=int(env_port) if env_port else None,
            native=handle,
            env_ready_timeout=self.env_ready_timeout,
        )

    async def list(self) -> list[dict]:
        n = native()
        rows = []
        for record in await local_runtime().sandboxes().list(n.ProviderKind.CONTRIB):
            if record.runtime_type != self.on:
                continue
            rows.append(
                {
                    "name": record.name,
                    "status": "running",
                    "runtime_type": record.runtime_type,
                    "image": record.image,
                }
            )
        return rows
