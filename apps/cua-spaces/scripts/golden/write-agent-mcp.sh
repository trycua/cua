#!/bin/bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# write-agent-mcp.sh — write the agent's MCP registry for this Space, plus the
# specs for any MCP that is hot-loaded (see mcp-lazy-app.py).
#
# The GUI tools come from cua-driver's own MCP for THIS machine (the Space is
# the local machine from the agent's point of view): `cua-driver-local mcp`
# over stdio, talking to the signed CuaDriverLocal.app daemon over its unix
# socket so the TCC grants stay with the bundle. No token: the socket is
# owner-only. (Sandboxes are driven from the host through `cua daemon mcp`.)
/usr/bin/python3 - <<'PY'
import json, os, sys
home = os.path.expanduser("~")
lazy_dir = os.path.join(home, ".cua", "lazy-apps")
os.makedirs(lazy_dir, exist_ok=True)

# --------------------------------------------------------------------------
# Hot-loaded, app-backed MCPs.
# --------------------------------------------------------------------------
# blender-mcp is a client for 127.0.0.1:9876, and that socket is served by an
# addon running on Blender's MODAL EVENT LOOP -- `blender --background` does not
# serve it (measured). So the MCP only works while the GUI application is up.
#
# The previous answer to that was to launch Blender in prepare-space.sh, at
# every boot of every Space. A user who asked for something with nothing to do
# with 3D got Blender on screen. mcp-lazy-app.py breaks the coupling: it
# advertises the tools with Blender CLOSED and starts it on the first tools/call,
# waiting for 9876 to actually answer before forwarding. The desktop stays clean
# until an agent genuinely needs to model something.
#
# Everything platform-specific is in this spec, not in the shim -- the Linux and
# Omarchy ports change `launch` (and nothing else) to their own launcher.
blender_spec = os.path.join(lazy_dir, "blender.json")
with open(blender_spec, "w") as fh:
    json.dump({
        "app": "Blender",
        # The readiness contract is the SOCKET ANSWERING, never "the process
        # exists". A Blender that macOS's Resume launched has no addon server
        # running: pgrep would pass and 9876 would be closed.
        "ready": {"type": "tcp", "host": "127.0.0.1", "port": 9876},
        # -noaudio: CoreAudio initialisation hangs in this VM.
        # NOT --background (see above). NOT --factory-startup: that would
        # discard the userpref that disables the splash and enables the addon.
        # `open`, not the binary: launchservices starts it in its own job, so it
        # outlives the shim that asked for it.
        "launch": ["/usr/bin/open", "-n", "-a", "/Applications/Blender.app",
                   "--args", "-noaudio",
                   "--python", os.path.join(home, ".cua", "blender-mcp-start.py")],
        "ready_timeout_sec": 120,
        "lock_timeout_sec": 180,
        "state_dir": lazy_dir,
        "upstream": {"command": os.path.join(home, ".local/bin/uvx"),
                     "args": ["blender-mcp"]},
        "hint": ("This Space starts Blender on demand: the first call to a "
                 "Blender tool launches the application and waits for its MCP "
                 "socket, which can take up to a minute. That is expected, not "
                 "a hang -- do not retry or fall back to another approach. "
                 "Subsequent calls are immediate."),
    }, fh, indent=2)

cfg = {"mcpServers": {
  "cua-driver": {"command": "/Applications/CuaDriverLocal.app/Contents/MacOS/cua-driver-local",
                 "args": ["mcp", "--socket",
                          f"{home}/Library/Caches/cua-driver-local/cua-driver-local.sock"]},
  # The OFFICIAL Unity CLI, which is what `unity mcp configure <client>` writes.
  # It is a stdio server: no port, and therefore no port file to discover --
  # ~/.unity-mcp/unity-mcp-port.json belongs to the third-party editor package's
  # stdio bridge and does not exist on this path. `unity mcp` with no
  # --project-path locates the running Editor.
  #
  # NOT wrapped by mcp-lazy-app.py, deliberately. Its endpoint is a running
  # Editor with a project open, and opening a project is a GUI act the agent
  # performs as part of the task (sign in, pick the project, clone it). There is
  # no launch argv that could stand that up, and launching Unity Hub on a
  # `unity` tool call would be a lie about what got readied.
  "unity":      {"command": "/usr/local/bin/unity", "args": ["mcp"]},
  "blender":    {"command": "/usr/bin/python3",
                 "args": [f"{home}/.cua/mcp-lazy-app.py", blender_spec]},
  "get-skills": {"command": "/usr/bin/python3",
                 "args": [f"{home}/.cua/demo-kit/get-skills-mcp/server.py"]},
}}
open(os.path.join(home, ".cua", "agent-mcp.json"), "w").write(json.dumps(cfg, indent=2))
PY
