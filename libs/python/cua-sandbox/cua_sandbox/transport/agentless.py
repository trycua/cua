"""AgentlessTransport: shell and screen for a local Lume sandbox without cua-spacesd.

The published macOS 15 image carries no cua-spacesd (macOS 26 runs it). For a
local macOS Lume sandbox without it the SDK reaches the guest without it (``Sandbox.guest_sh`` over
``lume ssh``, ``Sandbox.guest_screenshot`` over the VM's VNC endpoint,
``Sandbox.guest_display``), the same code ``cua sb exec`` / ``screenshot``
/ ``view`` use. :class:`EnvTransport` hands over to this transport when the
sandbox has no cua-spacesd. Only ``run_command``, screenshots,
the screen size and the display URL work; everything else needs
cua-spacesd.
"""

from __future__ import annotations

import logging
from typing import Any, Dict, Optional

from cua_sandbox._sdk import millis
from cua_sandbox.transport.base import Transport, convert_screenshot

logger = logging.getLogger(__name__)


def supports_agentless(handle: Any) -> bool:
    """Whether the SDK ``Sandbox`` handle is a local Lume sandbox, which the
    SDK can reach without cua-spacesd. :class:`EnvTransport` uses the
    fallback only once cua-spacesd is known to be absent."""
    try:
        info = handle.info()
        return getattr(info, "location", "") == "local" and getattr(info, "runtime", "") == "lume"
    except Exception as e:  # noqa: BLE001 - an older handle has no fallback
        logger.debug("no agentless fallback: %s", e)
        return False


class AgentlessTransport(Transport):
    """``run_command`` via SSH and screenshots via VNC, through the SDK handle."""

    def __init__(self, handle: Any, environment: Optional[str] = None):
        self._handle = handle
        self._environment = environment or "mac"

    async def connect(self) -> None:
        logger.info("no cua-spacesd; shell via SSH (lume ssh), screen via VNC")

    async def disconnect(self) -> None:
        pass

    async def send(self, action: str, **params: Any) -> Any:
        if action == "run_command":
            timeout = params.get("timeout")
            out = await self._handle.guest_sh(
                str(params["command"]), millis(float(timeout)) if timeout else None
            )
            return {
                "stdout": bytes(out.stdout).decode("utf-8", "replace"),
                "stderr": bytes(out.stderr).decode("utf-8", "replace"),
                "returncode": out.exit.code if out.exit.code is not None else -1,
            }
        raise NotImplementedError(
            f"{action} needs cua-spacesd; this sandbox has none and is driven "
            "via SSH and VNC (shell commands and screenshots only)"
        )

    async def screenshot(self, format: str = "png", quality: int = 95) -> bytes:
        shot = await self._handle.guest_screenshot()
        data = bytes(shot.image)
        if format.lower() in ("jpeg", "jpg"):
            data = convert_screenshot(data, "jpeg", quality)
        return data

    async def get_screen_size(self) -> Dict[str, int]:
        shot = await self._handle.guest_screenshot()
        return {"width": int(shot.width), "height": int(shot.height)}

    async def get_environment(self) -> str:
        return self._environment

    async def get_display_url(self, *, share: bool = False) -> str:
        """The loopback VNC URL with the password masked (``vnc://****@...``).

        The password Lume set for this run is available only explicitly, as
        ``(await handle.guest_display()).url_with_password()``; treat it as
        a secret.
        """
        if share:
            raise NotImplementedError("share=True needs cua-spacesd")
        return (await self._handle.guest_display()).url()
