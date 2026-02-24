"""KiCad EDA app definition."""

from __future__ import annotations

import asyncio
from typing import TYPE_CHECKING, Any, Dict, List, Optional

from .registry import App, install, launch, uninstall

if TYPE_CHECKING:
    from .registry import BoundApp

_SPICE_TYPE_MAP = {
    "R": "resistor",
    "C": "capacitor",
    "L": "inductor",
    "V": "voltage_source",
    "I": "current_source",
    "D": "diode",
    "Q": "bjt",
    "M": "mosfet",
    "X": "subcircuit",
}


def _stdout(result) -> str:
    """Extract stdout string from a run_command result."""
    return (result.get("stdout", "") if isinstance(result, dict) else str(result)).strip()


def _parse_spice_components(netlist: str) -> List[dict]:
    """Parse SPICE netlist text into a list of component dicts."""
    components = []
    for line in netlist.splitlines():
        line = line.strip()
        if not line or line.startswith(("*", ".", "#")):
            continue
        parts = line.split()
        ref = parts[0]
        ctype = _SPICE_TYPE_MAP.get(ref[0].upper(), "unknown")
        components.append(
            {"ref": ref, "type": ctype, "value": parts[-1], "nodes": parts[1:-1]}
        )
    return components


def _tokenize_sexpr(text: str) -> List[str]:
    """Tokenize s-expression: '(', ')', quoted strings, symbols."""
    tokens: List[str] = []
    i, n = 0, len(text)
    while i < n:
        if text[i].isspace():
            i += 1
            continue
        if text[i] == "(":
            tokens.append("(")
            i += 1
            continue
        if text[i] == ")":
            tokens.append(")")
            i += 1
            continue
        if text[i] == '"':
            j = i + 1
            while j < n and text[j] != '"':
                if text[j] == "\\":
                    j += 2
                    continue
                j += 1
            tokens.append(text[i : j + 1])
            i = j + 1
            continue
        j = i
        while j < n and not text[j].isspace() and text[j] not in "()":
            j += 1
        tokens.append(text[i:j])
        i = j
    return tokens


def _parse_sexpr(tokens: List[str], pos: int) -> tuple[Any, int]:
    """Parse tokens into nested lists. Returns (tree, next_pos)."""
    if pos >= len(tokens):
        return [], pos
    if tokens[pos] != "(":
        return tokens[pos], pos + 1
    pos += 1
    result: List[Any] = []
    while pos < len(tokens) and tokens[pos] != ")":
        elem, pos = _parse_sexpr(tokens, pos)
        result.append(elem)
    return result, (pos + 1) if pos < len(tokens) else pos


def _find_in_tree(tree: Any, name: str) -> Optional[List[Any]]:
    """Find (name ...) in tree."""
    if not isinstance(tree, list) or not tree:
        return None
    if tree[0] == name:
        return tree
    for child in tree:
        found = _find_in_tree(child, name)
        if found is not None:
            return found
    return None


def _get_field(node: List[Any], key: str) -> Optional[str]:
    """From (comp (ref "X") (value "Y") ...) get value for key."""
    for item in node[1:]:
        if isinstance(item, list) and len(item) >= 2 and item[0] == key:
            v = item[1]
            if isinstance(v, str) and v.startswith('"') and v.endswith('"'):
                return v[1:-1].replace('\\"', '"')
            return str(v) if v is not None else None
    return None


def _parse_kicad_netlist_components(netlist: str) -> List[dict]:
    """Parse KiCad .net (s-expression) netlist into a list of component dicts.

    Returns list of dicts with keys ref, type, value, nodes (nodes from nets).
    """
    tokens = _tokenize_sexpr(netlist)
    if not tokens or tokens[0] != "(":
        return []
    tree, _ = _parse_sexpr(tokens, 0)
    components_node = _find_in_tree(tree, "components")
    if not components_node:
        return []
    comps: List[dict] = []
    for node in components_node[1:]:
        if not isinstance(node, list) or not node or node[0] != "comp":
            continue
        ref = _get_field(node, "ref")
        if ref is None:
            continue
        value = _get_field(node, "value") or ""
        prefix = ref[0].upper() if ref else ""
        ctype = _SPICE_TYPE_MAP.get(prefix, "unknown")
        comps.append({"ref": ref, "type": ctype, "value": value, "nodes": []})
    return comps


class KiCad(App):
    """KiCad - Open source electronics design automation.

    KiCad is a free software suite for schematic capture and PCB layout.

    Install options:
        - with_shortcut: Create desktop shortcut (default True)

    Launch options:
        - project_path: Path to project directory (optional)
    """

    name = "kicad"
    description = "KiCad - Open source electronics design automation"

    # =========================================================================
    # Linux
    # =========================================================================

    @install("linux")
    async def install_linux(
        self: "BoundApp",
        *,
        with_shortcut: bool = True,
    ) -> None:
        """Install KiCad on Linux via PPA."""
        result = await self.session.run_command(
            "sudo add-apt-repository --yes ppa:kicad/kicad-9.0-releases && "
            "sudo apt update && "
            "sudo apt install -y --install-recommends kicad",
            check=False,
        )

        verify = await self.session.run_command(
            "command -v kicad >/dev/null 2>&1 && echo FOUND || echo NOT_FOUND",
            check=False,
        )
        if _stdout(verify) != "FOUND":
            raise RuntimeError(
                f"KiCad install failed: {_stdout(result)}"
            )

        if with_shortcut:
            await self.session.run_command(
                "mkdir -p ~/Desktop ~/.local/share/applications && "
                "printf '%s\n' "
                "'[Desktop Entry]' "
                "'Name=KiCad' "
                "'Comment=Electronic Design Automation Suite' "
                "'Exec=kicad' "
                "'Icon=kicad' "
                "'Type=Application' "
                "'Categories=Development;Electronics;' "
                "'Terminal=true' "  # currently requires terminal otherwise the gui crashes when opening the file explorer, not sure why
                "> ~/Desktop/KiCad.desktop && "
                "chmod +x ~/Desktop/KiCad.desktop && "
                "cp ~/Desktop/KiCad.desktop ~/.local/share/applications/KiCad.desktop",
                check=False,
            )

    @launch("linux")
    async def launch_linux(
        self: "BoundApp",
        *,
        project_path: Optional[str] = None,
    ) -> None:
        """Launch KiCad on Linux."""
        cmd = "kicad"
        if project_path:
            cmd += f" '{project_path}'"
        done, pending = await asyncio.wait(
            [
                asyncio.ensure_future(
                    self.session.run_command(f"{cmd} &", check=False)
                ),
                asyncio.ensure_future(asyncio.sleep(3)),
            ],
            return_when=asyncio.FIRST_COMPLETED,
        )
        for task in pending:
            task.cancel()

    @uninstall("linux")
    async def uninstall_linux(self: "BoundApp", **kwargs) -> None:
        """Uninstall KiCad from Linux."""
        await self.session.run_command(
            "sudo apt remove -y kicad && "
            "rm -f ~/Desktop/KiCad.desktop && "
            "rm -f ~/.local/share/applications/KiCad.desktop",
            check=False,
        )

    # =========================================================================
    # Windows
    # =========================================================================

    @install("windows")
    async def install_windows(
        self: "BoundApp",
        *,
        with_shortcut: bool = True,
    ) -> None:
        """Install KiCad on Windows via winget."""
        await self.session.run_command(
            "winget install -e --id KiCad.KiCad --accept-package-agreements --accept-source-agreements",
            check=False,
        )
        await asyncio.sleep(30)  # Installer may take a while

    @launch("windows")
    async def launch_windows(
        self: "BoundApp",
        *,
        project_path: Optional[str] = None,
    ) -> None:
        """Launch KiCad on Windows."""
        if project_path:
            # Launch KiCad with project path (opens project)
            await self.session.run_command(
                f'start "" "C:\\Program Files\\KiCad\\8.0\\bin\\kicad.exe" "{project_path}"',
                check=False,
            )
        else:
            await self.session.run_command(
                'start "" "C:\\Program Files\\KiCad\\8.0\\bin\\kicad.exe"',
                check=False,
            )
        await asyncio.sleep(5)

    @uninstall("windows")
    async def uninstall_windows(self: "BoundApp", **kwargs) -> None:
        """Uninstall KiCad from Windows."""
        await self.session.run_command(
            "winget uninstall -e --id KiCad.KiCad",
            check=False,
        )

    # =========================================================================
    # macOS
    # =========================================================================

    @install("macos")
    async def install_macos(
        self: "BoundApp",
        *,
        with_shortcut: bool = True,
    ) -> None:
        """Install KiCad on macOS via Homebrew."""
        await self.session.run_command(
            "brew install --cask kicad",
            check=False,
        )

    @launch("macos")
    async def launch_macos(
        self: "BoundApp",
        *,
        project_path: Optional[str] = None,
    ) -> None:
        """Launch KiCad on macOS."""
        cmd = 'open -a "KiCad"'
        if project_path:
            cmd += f' --args "{project_path}"'
        await self.session.run_command(cmd, check=False)
        await asyncio.sleep(3)

    @uninstall("macos")
    async def uninstall_macos(self: "BoundApp", **kwargs) -> None:
        """Uninstall KiCad from macOS."""
        await self.session.run_command("brew uninstall --cask kicad", check=False)

    # =========================================================================
    # Netlist helpers (cross-platform)
    # =========================================================================

    async def export_netlist(
        self: "BoundApp",
        *,
        schematic_path: str,
        output_path: str,
    ) -> str:
        """Export a SPICE netlist from a .kicad_sch using kicad-cli.

        Args:
            schematic_path: Path to the .kicad_sch schematic on the VM.
            output_path: Destination path for the .cir netlist on the VM.

        Returns:
            The output_path on success.

        Raises:
            RuntimeError: If the output file was not created.
        """
        result = await self.session.run_command(
            f'kicad-cli sch export spice -o "{output_path}" "{schematic_path}"',
            check=False,
        )
        verify = await self.session.run_command(
            f'test -f "{output_path}" && echo FOUND || echo NOT_FOUND',
            check=False,
        )
        if _stdout(verify) != "FOUND":
            raise RuntimeError(f"Netlist export failed: {_stdout(result)}")
        return output_path

    async def read_netlist(self: "BoundApp", *, netlist_path: str) -> str:
        """Read a netlist file from the VM (SPICE .cir or KiCad .net).

        Args:
            netlist_path: Path to the netlist file on the VM.

        Returns:
            Raw text content of the netlist.
        """
        result = await self.session.run_command(
            f'cat "{netlist_path}"', check=False,
        )
        return _stdout(result)

    async def get_components(
        self: "BoundApp", *, netlist_path: str
    ) -> List[dict]:
        """Read and parse a netlist into structured components.

        Supports SPICE (.cir) and KiCad (.net) formats. For .net files uses
        s-expression parsing; for others uses SPICE line format.

        Args:
            netlist_path: Path to the .cir or .net file on the VM.

        Returns:
            List of dicts with keys ``ref``, ``type``, ``value``, ``nodes``.
        """
        netlist = await self.read_netlist(netlist_path=netlist_path)
        if netlist_path.rstrip("/").endswith(".net"):
            return _parse_kicad_netlist_components(netlist)
        return _parse_spice_components(netlist)

    async def simulate_operating_point(
        self: "BoundApp",
        *,
        netlist_path: str,
        ground: int = 0,
    ) -> Dict[str, float]:
        """Run a DC operating-point simulation via PySpice.

        Reads the netlist from the VM, then simulates locally.
        Requires the ``spice`` extra (``pip install cua-bench[spice]``).

        Args:
            netlist_path: Path to the .cir file on the VM.
            ground: Node number to use as ground reference.

        Returns:
            Dict mapping node name to its DC voltage.
        """
        from PySpice.Spice.Parser import SpiceParser

        netlist = await self.read_netlist(netlist_path=netlist_path)
        parser = SpiceParser(source=netlist)
        circuit = parser.build_circuit(ground=ground)
        simulator = circuit.simulator(temperature=25, nominal_temperature=25)
        analysis = simulator.operating_point()
        return {str(node): float(node) for node in analysis.nodes.values()}

    async def simulate_transient(
        self: "BoundApp",
        *,
        netlist_path: str,
        step_time_us: float,
        end_time_us: float,
        ground: int = 0,
    ) -> Dict[str, list]:
        """Run a transient simulation via PySpice.

        Reads the netlist from the VM, then simulates locally.
        Requires the ``spice`` extra (``pip install cua-bench[spice]``).

        Args:
            netlist_path: Path to the .cir file on the VM.
            step_time_us: Simulation step time in microseconds.
            end_time_us: Simulation end time in microseconds.
            ground: Node number to use as ground reference.

        Returns:
            Dict with a ``time`` key (list of seconds) and one key per
            circuit node mapping to a list of voltage samples.
        """
        from PySpice.Spice.Parser import SpiceParser
        from PySpice.Unit import u_us

        netlist = await self.read_netlist(netlist_path=netlist_path)
        parser = SpiceParser(source=netlist)
        circuit = parser.build_circuit(ground=ground)
        simulator = circuit.simulator(temperature=25, nominal_temperature=25)
        analysis = simulator.transient(
            step_time=step_time_us @ u_us, end_time=end_time_us @ u_us
        )
        result: Dict[str, list] = {
            "time": [float(t) for t in analysis.time],
        }
        for node in analysis.nodes.values():
            result[str(node)] = [float(v) for v in node]
        return result
