"""Config-toggle fault for an explicitly test-owned, sourced Lua file.

The caller must create this exact one-line file in its disposable compositor
config and pass it explicitly. This helper does not modify arbitrary user
configuration or unload a session-lifetime input plugin.
"""
import json
from pathlib import Path
import subprocess

ENABLED = "hl.config({plugin = {cua = {enabled = true}}})\n"
DISABLED = "hl.config({plugin = {cua = {enabled = false}}})\n"


class InputConfigToggle:
    def __init__(self, path):
        self.path = Path(path)
        if self.path.is_symlink() or self.path.read_text() != ENABLED:
            raise ValueError("require the exact test-owned enabled Lua include")
        self.changed = False

    def _set(self, enabled):
        expected = DISABLED if self.changed else ENABLED
        if self.path.is_symlink() or self.path.read_text() != expected:
            raise RuntimeError("test-owned toggle file changed unexpectedly")
        self.path.write_text(ENABLED if enabled else DISABLED)
        self.changed = not enabled
        output = subprocess.check_output(["hyprctl", "reload"], text=True, timeout=5)
        if output.strip() != "ok":
            raise RuntimeError("config reload did not acknowledge toggle")
        status = json.loads(subprocess.check_output(["hyprctl", "-j", "cua:status"], timeout=5))
        if status.get("configured") is not enabled or status.get("experiment", {}).get("transport_ready") is not enabled:
            raise RuntimeError("plugin did not observe the sourced toggle file")

    def disable(self):
        self._set(False)

    def restore(self):
        if self.changed:
            self._set(True)
