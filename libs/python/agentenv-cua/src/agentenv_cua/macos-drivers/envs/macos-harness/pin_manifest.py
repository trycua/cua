"""Write ``tools.json``: the eleven tools of the pinned macos-harness, as its own tool table says them.

Run it with the harness installed (the driver's venv has it):

    /opt/agentenv/drivers/macos-harness/venv/bin/python pin_manifest.py

It needs no Mac permissions and no helper: it reads ``macos_harness.tools`` and ``macos-harness skill``, and records
each tool's name, description, flags (with what the harness's own ``--help`` says of each) and an input schema, plus the
sha256 of the skill text, so a version bump shows as a reviewable diff."""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
from pathlib import Path

from macos_harness import protocol, tools

OUT = Path(__file__).with_name("tools.json")
#: Flag kinds in the harness's table, and the JSON type of each.
JSON_TYPES = {
    "str": "string",
    "bool": "boolean",
    "int": "integer",
    "num": "number",
    "button": "string",
}
#: What each tool needs: its app, bar the one tool that lists them all.
NO_APP = {"list_apps"}


def main() -> None:
    skill = subprocess.run(
        [sys.executable, "-m", "macos_harness.cli", "skill"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    entries = []
    for name, tool in tools.TOOLS.items():
        properties = {}
        for flag in tool.params:
            kind, help_text = tools.PARAMS[flag]
            properties[flag] = {"type": JSON_TYPES[kind], "description": help_text}
        schema = {"type": "object", "properties": properties, "additionalProperties": False}
        if name not in NO_APP:
            schema["required"] = ["app"]
        entries.append(
            {
                "name": name,
                "description": tool.description,
                "example": tool.example,
                "stdin": tool.stdin,
                "inputSchema": schema,
            }
        )
    OUT.write_text(
        json.dumps(
            {
                "version": protocol.VERSION,
                "skill_sha256": hashlib.sha256(skill.encode()).hexdigest(),
                "tools": sorted(entries, key=lambda e: e["name"]),
            },
            indent=1,
        )
        + "\n"
    )
    print(f"wrote {OUT}: {len(entries)} tools")


if __name__ == "__main__":
    main()
