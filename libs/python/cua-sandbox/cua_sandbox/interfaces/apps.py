"""Apps interface — install and launch apps from the cua-sandbox-apps catalog."""

from __future__ import annotations

from cua_sandbox.interfaces.shell import CommandResult
from cua_sandbox.transport.base import Transport


class Apps:
    """Install and launch cataloged applications inside the sandbox."""

    def __init__(self, transport: Transport, os_type: str):
        self._t = transport
        self._os_type = os_type

    async def install(self, app_id: str) -> CommandResult:
        """Install an app by its catalog ID (e.g. ``"unity"``, ``"godot-engine"``)."""
        from cua_sandbox.builder.executor import _find_app_install_script

        script = _find_app_install_script(app_id, self._os_type)
        if script is None:
            return CommandResult(
                stdout="",
                returncode=1,
                stderr=f"No install script for '{app_id}' on {self._os_type}. "
                f"Install cua-sandbox-apps or run 'cua-sandbox-apps generate'.",
            )
        result = await self._t.send(
            "run_command", command=f"bash -c {_sh_quote(script)}", timeout=900
        )
        if isinstance(result, dict):
            return CommandResult(
                stdout=result.get("stdout", ""),
                stderr=result.get("stderr", ""),
                returncode=result.get("return_code", result.get("returncode", -1)),
            )
        return CommandResult(
            stdout=getattr(result, "stdout", ""),
            stderr=getattr(result, "stderr", ""),
            returncode=getattr(result, "returncode", -1),
        )

    async def launch(self, app_id: str) -> CommandResult:
        """Launch a previously-installed app by its catalog ID."""
        from cua_sandbox.builder.executor import _find_app_launch_script

        script = _find_app_launch_script(app_id, self._os_type)
        if script is None:
            return CommandResult(
                stdout="",
                returncode=1,
                stderr=f"No launch script for '{app_id}' on {self._os_type}.",
            )
        result = await self._t.pty_create(command=f"bash -c {_sh_quote(script)}")
        pid = result.get("pid") if isinstance(result, dict) else None
        return CommandResult(stdout=str(pid or ""), stderr="", returncode=0)


def _sh_quote(s: str) -> str:
    return "'" + s.replace("'", "'\\''") + "'"
