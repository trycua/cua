"""Stand-in for `claude mcp remove NAME`: deletes the entry by name."""

import json
import sys

config_path, name = sys.argv[1], sys.argv[2]
with open(config_path, encoding="utf-8") as handle:
    config = json.load(handle)
servers = config.get("mcpServers") or {}
if name not in servers:
    raise SystemExit(1)
del servers[name]
with open(config_path, "w", encoding="utf-8") as handle:
    json.dump(config, handle)
