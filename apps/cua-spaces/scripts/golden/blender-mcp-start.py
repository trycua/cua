# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# blender-mcp-start.py — start the blender-mcp addon's TCP socket server
# (127.0.0.1:9876) in a GUI Blender, with no clicking.
#
# Passed to Blender as `--python`, not `--python-expr`: the expression form has
# to survive shell quoting through a LaunchAgent and an ssh hop, and a mangled
# expression fails SILENTLY (Blender starts, nothing listens, and the log is
# empty). A file has no quoting to get wrong.
#
# bpy.ops.blendermcp.start_server() is the exact operator the addon's N-panel
# "Connect to MCP server" button calls. It is fired from a timer because at
# --python time the addon's operators may not be registered yet and the window
# manager does not exist; the timer retries until it takes.
import os
import sys

import addon_utils
import bpy

PORT = 9876
MODULE = "blender_mcp"
_tries = 0


def _ensure_addon():
    """Enable the addon in THIS process.

    The saved userpref lists blender_mcp as enabled and a `--background`
    Blender does load it — but a GUI Blender started from a LaunchAgent came up
    with `bpy.ops.blendermcp` absent ("operator ... could not be found", once a
    second, forever). Rather than depend on the preference being honoured,
    enable it explicitly and report what happened.
    """
    # NOT `hasattr(bpy.ops, "blendermcp")`: bpy.ops resolves submodules lazily
    # and hasattr is True for any name at all, so that check silently passes
    # while the operator does not exist. addon_utils.check() reads the real
    # (default_enabled, enabled_now) pair.
    if addon_utils.check(MODULE)[1]:
        return True
    # Make the addon importable even if Blender's script paths do not include
    # the user addons directory (they will not if HOME is wrong for the
    # process, which is exactly how a mis-launched GUI Blender fails: it logs
    # `Add-on not loaded: "blender_mcp", cause: No module named 'blender_mcp'`
    # once a second and never listens).
    addons = os.path.expanduser(
        "~/Library/Application Support/Blender/4.5/scripts/addons"
    )
    if addons not in sys.path and os.path.isdir(addons):
        sys.path.append(addons)
    try:
        # addon_utils.enable() returns the module, or None on failure — it does
        # not raise. Checking the return value is the difference between
        # knowing it worked and printing a success line while it did not.
        if addon_utils.enable(MODULE, default_set=False, persistent=True) is None:
            print("BLENDER_MCP_ADDON_ENABLE_RETURNED_NONE", flush=True)
            return False
        print("BLENDER_MCP_ADDON_ENABLED_IN_PROCESS", flush=True)
    except Exception as exc:
        print("BLENDER_MCP_ADDON_ENABLE_FAILED:", exc, flush=True)
        return False
    return addon_utils.check(MODULE)[1]


def _start():
    global _tries
    _tries += 1
    _ensure_addon()
    try:
        bpy.ops.blendermcp.start_server()
        print("BLENDER_MCP_SERVER_STARTED after %d attempt(s)" % _tries, flush=True)
        return None
    except Exception as exc:  # operator not registered yet, or no context
        if _tries > 60:
            print("BLENDER_MCP_SERVER_GAVE_UP:", exc, flush=True)
            return None
        print("BLENDER_MCP_SERVER_RETRY:", exc, flush=True)
        return 1.0


bpy.app.timers.register(_start, first_interval=1.0)
