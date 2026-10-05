#!/usr/bin/env python3
"""Write the arm B MCP config for Claude Code from OpenAI's shipped plugin file.

The server entry is the `cua_repl` entry of the newest `unified-computer-use` plugin in the Codex plugin cache,
copied unchanged: same command, same arguments, same environment. Two things are different on purpose: the server
is named `codex-cu` (Claude Code reserves the name `computer-use`), and `CUA_REPL_ENABLED_SURFACES` is set to
`computer` so the browser surface is switched off and no browser can be touched.

  make_codex_cu_mcp.py OUT.json [--plugin-root ~/.codex/plugins/cache/openai-bundled/unified-computer-use]
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

DEFAULT_ROOT = (
    Path.home() / ".codex" / "plugins" / "cache" / "openai-bundled" / "unified-computer-use"
)


def version_key(path: Path) -> tuple[int, ...]:
    return tuple(int(part) for part in re.findall(r"\d+", path.name))


def newest_plugin_dir(root: Path) -> Path:
    candidates = [p for p in root.iterdir() if (p / ".mcp.json").is_file()]
    if not candidates:
        raise SystemExit(f"no unified-computer-use plugin with a .mcp.json under {root}")
    return max(candidates, key=version_key)


def build_config(plugin_dir: Path) -> dict:
    shipped = json.loads((plugin_dir / ".mcp.json").read_text("utf-8"))
    entry = shipped["mcpServers"]["cua_repl"]
    env = dict(entry.get("env", {}))
    env["CUA_REPL_ENABLED_SURFACES"] = "computer"
    return {
        "mcpServers": {
            "codex-cu": {
                "type": "stdio",
                "command": entry["command"],
                "args": list(entry.get("args", [])),
                "env": env,
            }
        }
    }


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("out", type=Path)
    ap.add_argument("--plugin-root", type=Path, default=DEFAULT_ROOT)
    args = ap.parse_args()
    plugin_dir = newest_plugin_dir(args.plugin_root.expanduser())
    cfg = build_config(plugin_dir)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(cfg, indent=1) + "\n", "utf-8")
    print(f"wrote {args.out} from plugin {plugin_dir.name}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
