"""KiCad EDA app definition."""

from __future__ import annotations

import asyncio
from pathlib import Path
from typing import TYPE_CHECKING, Any

from .registry import App, install, launch

if TYPE_CHECKING:
    from .registry import BoundApp


class KiCad(App):
    """KiCad Electronic Design Automation suite.

    Install options:
        - with_shortcut: Create desktop shortcut (default True)

    Launch options:
        - project_path: Path to .kicad_pro file to open (optional)

    Custom methods:
        - read_netlist(netlist_path): Read a .net file from the environment
        - get_components(netlist_path): Parse component list from a .net file
        - compare_netlist(candidate_path, reference_path): Structural comparison
          of candidate (remote path) against reference (local path on agent)
    """

    name = "kicad"
    description = "KiCad EDA suite for schematic capture and PCB layout"

    # =========================================================================
    # Linux
    # =========================================================================

    @install("linux")
    async def install_linux(
        self: "BoundApp",
        *,
        with_shortcut: bool = True,
    ) -> None:
        """KiCad 9 is pre-installed in the image; just create the desktop shortcut."""
        if with_shortcut:
            await self.session.run_command(
                "ln -sf /usr/bin/kicad ~/Desktop/KiCad || true",
                check=False,
            )

    @launch("linux")
    async def launch_linux(
        self: "BoundApp",
        *,
        project_path: str | None = None,
    ) -> None:
        """Launch KiCad on Linux."""
        cmd = "kicad"
        if project_path:
            cmd += f" '{project_path}'"
        # Use setsid to put KiCad in its own session so the shell exits immediately
        # and run_command returns rather than waiting for the GUI process to finish.
        # Catch any connection-level errors: KiCad may have started even if the
        # REST/WebSocket response was not received cleanly.
        try:
            await self.session.run_command(
                f"setsid {cmd} >/dev/null 2>&1 </dev/null &", check=False
            )
        except Exception:
            pass
        await asyncio.sleep(5)

    # =========================================================================
    # Custom methods (platform-agnostic, called via session.apps.kicad.*)
    # =========================================================================

    async def read_netlist(self: "BoundApp", *, netlist_path: str) -> str:
        """Read a KiCad .net file from the environment. Returns '' if missing."""
        try:
            return await self.session.read_file(netlist_path)
        except Exception:
            return ""

    async def get_components(
        self: "BoundApp", *, netlist_path: str
    ) -> list[dict[str, Any]]:
        """Parse and return component list from a .net file on the environment.

        Each component dict has keys: ref, value, lib, part.
        """
        from cua_bench.netlist_compare import parse_kicad_netlist

        content = await self.read_netlist(netlist_path=netlist_path)
        return parse_kicad_netlist(content)["components"]

    async def compare_netlist(
        self: "BoundApp",
        *,
        candidate_path: str,
        reference_path: str,
    ) -> float:
        """Structurally compare the candidate netlist (on environment) against
        a reference netlist (local path on the agent container).

        Returns a score in [0.0, 1.0].
        """
        from cua_bench.netlist_compare import compare_kicad_netlists

        candidate = await self.read_netlist(netlist_path=candidate_path)
        reference = Path(reference_path).read_text(encoding="utf-8", errors="replace")
        return compare_kicad_netlists(candidate, reference)
