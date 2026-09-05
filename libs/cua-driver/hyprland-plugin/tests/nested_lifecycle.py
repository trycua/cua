"""Test discovery lifecycle in two owned, disposable nested Hyprland processes.

Requires a matching prebuilt module and an active Wayland parent. Never loads
the module into the parent or edits its configuration. This is not a direct-DRM,
application-delivery, package-installation, or physical-input test.
"""

import argparse
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import live_discovery as live


def command(*args, env=None):
    return subprocess.check_output(args, env=env, text=True, timeout=10).strip()


def wait_for(check, description):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        value = check()
        if value:
            return value
        time.sleep(0.1)
    raise TimeoutError(description)


class NestedCompositor:
    def __init__(self, directory, binary, module):
        self.directory = directory
        self.binary = binary
        self.module = module
        self.process = None
        self.log = None
        self.instance = None
        self.metadata = None
        directory.mkdir()
        self.config = directory / "hyprland.lua"
        self.config.write_text("hl.config({plugin = {cua = {enabled = true}}})\n")

    def ctl(self, *args):
        if not self.instance:
            raise RuntimeError("owned compositor has no instance")
        return command("hyprctl", "-i", self.instance, *args)

    def start(self):
        if self.process is not None:
            raise RuntimeError("compositor already started")
        env = dict(os.environ, AQ_BACKEND="wayland")
        env.pop("HYPRLAND_INSTANCE_SIGNATURE", None)
        self.log = (self.directory / "compositor.log").open("a")
        try:
            self.process = subprocess.Popen(
                [self.binary, "--config", str(self.config)],
                env=env,
                stdout=self.log,
                stderr=subprocess.STDOUT,
            )
        except OSError:
            self.log.close()
            raise

        def ready():
            if self.process.poll() is not None:
                raise RuntimeError(f"nested compositor exited; see {self.directory}")
            instances = json.loads(command("hyprctl", "-j", "instances"))
            matches = [i for i in instances if i["pid"] == self.process.pid]
            if len(matches) == 1:
                self.instance = matches[0]["instance"]
                try:
                    json.loads(self.ctl("-j", "version"))
                    return True
                except (subprocess.CalledProcessError, json.JSONDecodeError):
                    return False
            return False

        wait_for(ready, "owned nested compositor did not become ready")
        assert self.ctl("plugin", "load", str(self.module)) == "ok"
        assert self.ctl("reload") == "ok"
        env = dict(os.environ, HYPRLAND_INSTANCE_SIGNATURE=self.instance)
        result = json.loads(command(sys.executable, str(Path(live.__file__)), env=env))
        assert result["mutation_refusals"] == 6
        assert result["compositor_state_unchanged"] is True
        self.metadata = json.loads(self.ctl("-j", "cua:status"))
        assert self.metadata["transport"]["ready"] and self.metadata["abi"]["match"]
        return self.metadata

    def stop(self):
        if self.process is None:
            return
        process = self.process
        try:
            process.terminate()
            try:
                process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
                raise RuntimeError("nested compositor required forced termination")
            if self.metadata:
                path = Path(self.metadata["transport"]["socket"])
                assert not path.exists(), "clean shutdown left a discovery socket"
        finally:
            self.log.close()
            self.process = None
            self.instance = None
            self.metadata = None


def run(module, directory, binary):
    if not __debug__:
        raise RuntimeError("assertions must be enabled; do not run Python with -O")
    parent = os.environ.get("HYPRLAND_INSTANCE_SIGNATURE")
    if not parent or not os.environ.get("WAYLAND_DISPLAY"):
        raise RuntimeError("run inside a disposable Hyprland Wayland parent session")
    parent_version = json.loads(command("hyprctl", "-i", parent, "-j", "version"))
    first = NestedCompositor(directory / "first", binary, module)
    second = NestedCompositor(directory / "second", binary, module)
    try:
        a_info = first.start()
        b_info = second.start()
        assert a_info["transport"]["socket"] != b_info["transport"]["socket"]
        assert a_info["compositor_epoch"] != b_info["compositor_epoch"]
        with live.connect(a_info) as a, live.connect(b_info) as b:
            assert live.exchange(a, 3, 2) == (4, b"")
            second.stop()
            assert b.recv(1) == b"", "shutdown left old connection open"
            assert live.exchange(a, 3, 3) == (4, b"")
            c_info = second.start()
            assert c_info["transport"]["socket"] != b_info["transport"]["socket"]
            assert c_info["compositor_epoch"] not in (
                a_info["compositor_epoch"],
                b_info["compositor_epoch"],
            )
            with live.connect(c_info) as c:
                assert live.exchange(c, 3, 2) == (4, b"")
            second.stop()
            assert live.exchange(a, 3, 4) == (4, b"")
    finally:
        try:
            second.stop()
        finally:
            first.stop()
    assert json.loads(command("hyprctl", "-i", parent, "-j", "version")) == parent_version
    return {
        "result": "passed",
        "module_sha256": hashlib.sha256(module.read_bytes()).hexdigest(),
        "hyprland_version": parent_version["version"],
        "two_owned_compositors": True,
        "mutation_refusals_per_start": 6,
        "clean_restart": True,
        "old_connection_closed": True,
        "old_socket_removed": True,
        "fresh_socket_and_epoch": True,
        "sibling_connection_survived": True,
        "parent_responsive": True,
        "owned_compositors_stopped": True,
        "application_delivery_tested": False,
        "physical_input_isolation_tested": False,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--module", required=True, type=Path)
    parser.add_argument("--hyprland", default="Hyprland")
    args = parser.parse_args()
    module = args.module.resolve(strict=True)
    directory = Path(tempfile.mkdtemp(prefix="cua-plugin-lifecycle-"))
    print(f"Lifecycle logs: {directory}", file=sys.stderr)
    print(json.dumps(run(module, directory, args.hyprland)))


if __name__ == "__main__":
    main()
