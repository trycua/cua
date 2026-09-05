#!/usr/bin/env python3
"""Disposable Ubuntu X11 tutorial smoke; never run on a personal desktop.

Tests the checked-out canonical installer and its published component release,
not a source-built Driver. No LLM account or client registration is exercised.
The tutorial's standard profile is overridden with the explicitly authorized
test-only --dangerously-bypass-approvals on this disposable desktop.

The source installer is unchanged. Audited Linux recursive deletion paths are:
its mktemp staging directory, package lock/current links, old release directories,
and the fixed legacy ~/.cua-driver-rs. Fresh task directories and disabled GC
confine the former; an absence precondition makes legacy/local cleanup dormant.
macOS app-bundle deletion branches cannot run here. The shared installer also
stops all named Driver processes/services: require a cold runner before calling
it. HOME is never reassigned. All cleanup here terminates owned processes only;
GitHub discards the VM and task directories after artifact collection.
"""

import base64
import hashlib
import json
import os
from pathlib import Path
import platform
import queue
import re
import signal
import subprocess
import sys
import tempfile
import threading
import time
import tomllib


ROOT = Path(__file__).resolve().parents[2]


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def run(args, *, env=None, timeout=30, check=True):
    result = subprocess.run(args, cwd=ROOT, env=env, text=True,
                            capture_output=True, timeout=timeout)
    if check:
        require(result.returncode == 0,
                f"{args[0]} failed ({result.returncode}): {result.stderr[-2000:]}")
    return result


def save(directory, name, value):
    (directory / name).write_text(
        value if isinstance(value, str) else json.dumps(value, indent=2) + "\n")


def stop(process):
    if process is not None and process.poll() is None:
        # Each process was created with start_new_session=True in this runner.
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=8)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=5)


class MCP:
    """One newline-delimited JSON-RPC transport for the entire GUI loop."""

    def __init__(self, binary, endpoint, env, evidence):
        self.evidence = evidence
        self.sequence = 0
        self.lines = queue.Queue()
        self.log = (evidence / "mcp-stderr.log").open("w")
        self.process = subprocess.Popen(
            [str(binary), "mcp", "--socket", str(endpoint)], cwd=ROOT, env=env,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.log,
            text=True, bufsize=1, start_new_session=True)
        threading.Thread(target=self.read_lines, daemon=True).start()

    def read_lines(self):
        for line in self.process.stdout:
            self.lines.put(line)
        self.lines.put(None)

    def request(self, method, params):
        self.sequence += 1
        request = {"jsonrpc": "2.0", "id": self.sequence,
                   "method": method, "params": params}
        self.process.stdin.write(json.dumps(request) + "\n")
        self.process.stdin.flush()
        deadline = time.monotonic() + 45
        while True:
            try:
                line = self.lines.get(timeout=max(0.01, deadline - time.monotonic()))
            except queue.Empty as error:
                raise RuntimeError(f"MCP {method} response timed out") from error
            require(line is not None, "MCP transport closed before response")
            response = json.loads(line)
            if response.get("id") == self.sequence:
                require("error" not in response, f"MCP error: {response.get('error')}")
                return response["result"]
            require(time.monotonic() < deadline, "MCP response timeout")

    def call(self, name, arguments, label):
        result = self.request("tools/call", {"name": name, "arguments": arguments})
        # Keep real PNGs separately, never huge base64 blobs in semantic evidence.
        images = []
        for index, content in enumerate(result.get("content", [])):
            if content.get("type") == "image":
                require(content.get("mimeType") == "image/png", "Expected a PNG snapshot")
                data = base64.b64decode(content["data"], validate=True)
                require(data.startswith(b"\x89PNG\r\n\x1a\n"), "Invalid PNG evidence")
                image = self.evidence / f"{label}-{index}.png"
                image.write_bytes(data)
                images.append(image)
                content["data"] = f"[saved to {image.name}]"
        save(self.evidence, f"{label}.json", result)
        require(not result.get("isError"), f"{name} reported an error; see {label}.json")
        payload = result.get("structuredContent")
        if payload is None:
            texts = [c["text"] for c in result.get("content", []) if c.get("type") == "text"]
            require(len(texts) == 1, f"{name} omitted structured content")
            payload = json.loads(texts[0])
        return payload, images

    def close(self):
        stop(self.process)
        self.log.close()


def observe(evidence, label):
    """Read actual X state; never use Driver return values as focus proof."""
    root = run(["xprop", "-root", "_NET_ACTIVE_WINDOW", "_NET_CLIENT_LIST_STACKING"]).stdout
    focus = run(["xdotool", "getwindowfocus"]).stdout.strip()
    pointer = run(["xdotool", "getmouselocation", "--shell"]).stdout
    active = re.search(r"_NET_ACTIVE_WINDOW.*?(0x[0-9a-fA-F]+)", root)
    stack_line = next((line for line in root.splitlines()
                       if line.startswith("_NET_CLIENT_LIST_STACKING")), "")
    state = {"active_window": active.group(1) if active else None,
             "keyboard_focus": focus,
             "stacking_bottom_to_top": re.findall(r"0x[0-9a-fA-F]+", stack_line),
             "real_pointer": dict(line.split("=", 1) for line in pointer.splitlines()),
             "raw_root_properties": root}
    save(evidence, f"{label}-external.json", state)
    require(focus.isdigit() and {"X", "Y", "SCREEN"}.issubset(state["real_pointer"]),
            "External focus/pointer observation is incomplete")
    require("_NET_CLIENT_LIST_STACKING" in root, "Window manager did not expose stacking")
    return state


def compare_observations(before, after):
    # Cursor overlay windows may appear. Compare the relative order of the
    # existing application windows, recording the unfiltered stacking as well.
    original = before["stacking_bottom_to_top"]
    return {
        "active_window_unchanged": before["active_window"] == after["active_window"],
        "keyboard_focus_unchanged": before["keyboard_focus"] == after["keyboard_focus"],
        "real_pointer_unchanged": all(before["real_pointer"].get(k) == after["real_pointer"].get(k)
                                      for k in ("X", "Y", "SCREEN")),
        "existing_stacking_unchanged": original == [w for w in after["stacking_bottom_to_top"]
                                                    if w in original],
    }


def button(snapshot, labels):
    matches = [e for e in snapshot.get("elements", [])
               if "button" in e.get("role", "").lower()
               and e.get("label", "").strip().lower() in labels
               and e.get("enabled", True)]
    require(len(matches) == 1, f"Expected one calculator button in {sorted(labels)}, found {len(matches)}")
    require(matches[0].get("element_token"), "Button lacks a fresh snapshot-bound token")
    return matches[0]


def snapshot(mcp, target, label):
    state, images = mcp.call("get_window_state", target, label)
    require(images, f"{label} did not produce an actual snapshot image")
    require(not state.get("degraded") and state.get("elements"),
            f"{label} lacks usable AT-SPI controls")
    return state, images[0]


def displays_42(state):
    values = [str(e.get("value", "")).strip() for e in state["elements"]]
    tree = state.get("tree_markdown", "")
    return "42" in values or bool(re.search(r'\bvalue[=: ]+[\"\']?42(?:[\"\']|\s|$)', tree))


def doctor(binary, env, evidence, label, *, require_window):
    result = run([str(binary), "doctor", "--json"], env=env)
    report = json.loads(result.stdout)
    save(evidence, f"{label}.json", report)
    probes = {p["label"]: p for p in report["probes"]}
    for name in ("display server", "AT-SPI") + (("X11 connection",) if require_window else ()):
        require(probes.get(name, {}).get("status") == "ok", f"doctor readiness failed: {name}")


def main():
    require(sys.platform == "linux" and platform.machine() == "x86_64",
            "Only disposable Linux x86_64 runners are supported")
    require(os.environ.get("GITHUB_ACTIONS") == "true"
            and os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted"
            and os.geteuid() != 0, "Refusing installation outside an unprivileged GitHub-hosted runner")
    temporary = Path(os.environ["RUNNER_TEMP"]).resolve(strict=True)
    evidence = Path(os.environ["CUA_DOCS_EVIDENCE"]).resolve()
    require(evidence.parent == temporary, "Evidence must be directly under RUNNER_TEMP")
    evidence.mkdir(exist_ok=False)
    processes = []
    mcp = None
    try:
        top = Path(run(["git", "rev-parse", "--show-toplevel"]).stdout.strip()).resolve()
        sha = run(["git", "rev-parse", "HEAD"]).stdout.strip()
        dirty = run(["git", "status", "--porcelain"]).stdout
        branch = run(["git", "symbolic-ref", "--short", "HEAD"], check=False).stdout.strip() or "detached HEAD"
        scripts = ROOT / "libs/cua-driver/scripts"
        package = tomllib.loads((ROOT / "libs/cua-driver/rust/Cargo.toml").read_text())
        source_version = package["workspace"]["package"]["version"]
        helper = (scripts / "_install-rust.sh").read_text()
        baked = re.search(r'^CUA_DRIVER_RS_BAKED_VERSION="([0-9.]+)"', helper, re.M).group(1)
        provenance = {"source_path": str(ROOT), "git_top_level": str(top), "branch": branch,
                      "checkout_sha": sha, "dirty_status": dirty, "source_version": source_version,
                      "installer_baked_version": baked, "upstream_comparison": "not requested",
                      "installer_sha256": {name: hashlib.sha256((scripts / name).read_bytes()).hexdigest()
                                           for name in ("install.sh", "_install-rust.sh", "_install-common.sh")},
                      "coverage": "source-tree release installer + published Linux component binary",
                      "permission_override": "test-only unrestricted; tutorial recommends standard"}
        save(evidence, "provenance.json", provenance)
        require(top == ROOT and sha == os.environ["CUA_DOCS_SOURCE_SHA"] and not dirty,
                "Source preflight must match the exact clean checkout SHA")
        require(source_version == baked, "Source and baked release differ; review the intended release before running")
        # Fixed legacy cleanup is outside the overridden package home. Refuse
        # any existing installation, symlink or service rather than sweeping it.
        for relative in (".cua-driver-rs", ".cua-driver", ".local/bin/cua-driver",
                         ".config/systemd/user/cua-driver-rs.service"):
            path = Path.home() / relative
            require(not path.exists() and not path.is_symlink(), f"Cold-install precondition failed: {relative}")
        require(run(["pgrep", "-x", "cua-driver"], check=False).returncode == 1,
                "A Driver process already exists; refusing the installer's process cleanup")
        task = Path(tempfile.mkdtemp(prefix="docs-driver-first-run-", dir=temporary)).resolve()
        require(task.parent == temporary and ROOT not in task.parents, "Invalid isolated install directory")
        env = os.environ.copy()
        for key in ("GH_TOKEN", "GITHUB_TOKEN", "CUA_DRIVER_RS_VERSION", "CUA_DRIVER_VERSION",
                    "CUA_DRIVER_PERMISSION_MODE", "CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS",
                    "WAYLAND_DISPLAY"):
            env.pop(key, None)
        for name, leaf in (("CUA_DRIVER_RS_HOME", "package"), ("CUA_DRIVER_RS_INSTALL_DIR", "bin"),
                           ("TMPDIR", "staging"), ("XDG_DATA_HOME", "data"),
                           ("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache")):
            location = task / leaf
            location.mkdir()
            env[name] = str(location)
        env.update(CUA_DRIVER_RS_KEEP_VERSIONS="0", CUA_DRIVER_RS_TELEMETRY_ENABLED="false",
                   CUA_DRIVER_RS_UPDATE_CHECK="false", CUA_TELEMETRY_ENABLED="false")
        save(evidence, "installer-boundaries.json", {
            "task_directory": str(task), "home_reassigned": False,
            "active_cleanup_targets": [str(task / "staging"), str(task / "package/packages"), str(task / "bin")],
            "legacy_cleanup": "absent, checked before install", "prior_local_cleanup": "fresh empty package home",
            "release_gc": "disabled", "macos_cleanup": "unreachable on Linux",
            "process_cleanup": "no matching Driver process or user service before install"})
        installed = run(["bash", str(scripts / "install.sh"), "--no-modify-path"], env=env,
                        timeout=300, check=False)
        save(evidence, "installer.log", installed.stdout + installed.stderr)
        require(installed.returncode == 0, "Canonical installer failed; see installer.log")
        binary = task / "bin/cua-driver"
        resolved = binary.resolve(strict=True)
        require(resolved.is_relative_to(task / "package/packages/releases"), "Installed binary escaped task package home")
        version = run([str(binary), "--version"], env=env).stdout.strip()
        require(re.search(rf"(?<![\d.]){re.escape(baked)}(?![\d.])", version), "Installed version differs from baked release")
        url = f"https://github.com/trycua/cua/releases/download/cua-driver-rs-v{baked}/cua-driver-rs-{baked}-linux-x86_64-binary.tar.gz"
        require(url in installed.stdout, "Installer did not report the expected exact component asset URL")
        provenance.update(installed_path=str(binary), resolved_binary=str(resolved), installed_version=version,
                          binary_sha256=hashlib.sha256(resolved.read_bytes()).hexdigest(), component_asset_url=url)
        save(evidence, "provenance.json", provenance)
        endpoint = task / "driver.sock"
        cold = run([str(binary), "status", "--socket", str(endpoint)], env=env, check=False)
        save(evidence, "cold-status.log", cold.stdout + cold.stderr)
        require("daemon is not running" in cold.stdout + cold.stderr, "Cold-start daemon precondition failed")
        for name, args in (("openbox", ["openbox"]),
                           ("picom", ["picom", "--backend", "xrender", "--config", "/dev/null"])):
            with (evidence / f"{name}.log").open("w") as log:
                processes.append(subprocess.Popen(args, env=env, stdout=log, stderr=log,
                                                  start_new_session=True))
        time.sleep(2)
        require(all(p.poll() is None for p in processes), "Disposable window manager/compositor failed")
        run(["gsettings", "set", "org.gnome.desktop.interface", "toolkit-accessibility", "true"], env=env)
        with (evidence / "serve.log").open("w") as log:
            daemon = subprocess.Popen([str(binary), "serve", "--socket", str(endpoint),
                                       "--dangerously-bypass-approvals"], env=env,
                                      stdout=log, stderr=log, start_new_session=True)
            processes.append(daemon)
        # serve remains alive in its own process (the tutorial's first terminal).
        # Independent subprocesses below model the second terminal on the same bus.
        for _ in range(30):
            require(daemon.poll() is None, "Foreground serve exited before readiness")
            status = run([str(binary), "status", "--socket", str(endpoint)], env=env, check=False)
            if "daemon is running" in status.stdout:
                break
            time.sleep(1)
        save(evidence, "running-status.log", status.stdout + status.stderr)
        require(status.returncode == 0 and "daemon is running" in status.stdout, "Daemon never became ready")
        doctor(binary, env, evidence, "doctor-before-launch", require_window=False)
        apps = run([str(binary), "call", "list_apps", "--socket", str(endpoint)], env=env)
        save(evidence, "list-apps-before-launch.json", apps.stdout)
        # A new Openbox desktop may have no GUI apps yet. As the tutorial says,
        # launch Calculator and repeat readiness; never accept an empty list.
        mcp = MCP(binary, endpoint, env, evidence)
        save(evidence, "mcp-initialize.json", mcp.request("initialize", {
            "protocolVersion": "2024-11-05", "capabilities": {},
            "clientInfo": {"name": "docs-first-run-smoke", "version": "1"}}))
        mcp.process.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        mcp.process.stdin.flush()
        listing = mcp.request("tools/list", {})
        save(evidence, "mcp-tools.json", listing)
        require({"launch_app", "get_window_state", "click", "get_desktop_state"}.issubset(
            {tool["name"] for tool in listing["tools"]}), "MCP discovery lacks required tools")
        mcp.call("get_desktop_state", {}, "before-launch-desktop")
        before_launch = observe(evidence, "before-launch")
        launch, _ = mcp.call("launch_app", {"name": "galculator"}, "launch-calculator")
        pid = launch.get("pid")
        require(isinstance(pid, int) and pid > 0, "Calculator launch lacks a PID")
        windows = launch.get("windows", [])
        if not windows:
            for attempt in range(10):
                listing, _ = mcp.call("list_windows", {"pid": pid}, f"launch-windows-{attempt}")
                windows = listing.get("windows", []) if isinstance(listing, dict) else listing
                if windows:
                    break
                time.sleep(0.5)
        require(len(windows) == 1, "Expected one calculator window; refusing ambiguous targeting")
        target = {"pid": pid, "window_id": windows[0]["window_id"]}
        state, _ = snapshot(mcp, target, "calculator-initial")
        after_launch = observe(evidence, "after-launch")
        require(hex(target["window_id"]) in after_launch["stacking_bottom_to_top"],
                "External X11 observation did not find the calculator window")
        save(evidence, "launch-external-comparison.json", compare_observations(before_launch, after_launch))
        doctor(binary, env, evidence, "doctor-with-calculator", require_window=True)
        apps = run([str(binary), "call", "list_apps", "--socket", str(endpoint)], env=env)
        save(evidence, "list-apps-with-calculator.json", apps.stdout)
        require("galculator" in apps.stdout.lower() and re.search(rf'"pid"\s*:\s*{pid}\b', apps.stdout),
                "Second-terminal list_apps did not find the running calculator")
        observations = []
        for step, labels in enumerate(({"6"}, {"*", "×", "multiply", "multiplication"}, {"7"}, {"=", "equals", "equal"}), 1):
            # The previous post-action observation is the next action's fresh
            # snapshot. No tokens survive another snapshot of this target.
            element = button(state, labels)
            before = observe(evidence, f"step-{step}-before")
            mcp.call("click", {**target, "element_token": element["element_token"],
                               "delivery_mode": "background"}, f"step-{step}-click")
            state, final_image = snapshot(mcp, target, f"step-{step}-after")
            after = observe(evidence, f"step-{step}-after")
            observations.append(compare_observations(before, after))
        save(evidence, "action-external-comparisons.json", observations)
        # AT-SPI value and independently decoded screenshot must both show 42.
        # Allow the native toolkit to publish the final value after its action
        # callback returns. Refresh observations only; never repeat a click.
        for attempt in range(6):
            if displays_42(state):
                break
            time.sleep(0.5)
            state, final_image = snapshot(mcp, target, f"result-settle-{attempt}")
        ocr = run(["tesseract", str(final_image), "stdout", "--psm", "11"]).stdout
        save(evidence, "final-image-ocr.txt", ocr)
        require(displays_42(state), "Fresh AT-SPI display value did not prove 42")
        require(re.search(r"(?<!\d)42(?!\d)", ocr), "Actual final PNG did not independently show 42 via OCR")
        require(all(all(checks.values()) for checks in observations),
                "A background action changed external focus, pointer or stacking; see comparisons")
        require(daemon.poll() is None, "The original foreground serve process exited")
        save(evidence, "result.json", {"status": "passed", "displayed_result": "42",
             "target": target, "checkout_sha": sha, "installed_version": version,
             "limitations": ["Linux X11/AT-SPI only; not cross-platform certification",
                             "published component binary, not built from checkout SHA",
                             "MCP protocol client, not agent registration or LLM execution",
                             "test-only unrestricted profile; standard profile not tested",
                             "focus/stack/pointer sampled before and after, not continuously",
                             "no occlusion or foreground escalation coverage",
                             "OCR evidence retained for human visual review"]})
        print("PASS: canonical release install, cold serve, readiness, MCP and native Calculator display 42")
    except Exception as error:
        save(evidence, "result.json", {"status": "failed", "error": str(error)})
        raise
    finally:
        if mcp is not None:
            mcp.close()
        for process in reversed(processes):
            stop(process)


if __name__ == "__main__":
    main()
