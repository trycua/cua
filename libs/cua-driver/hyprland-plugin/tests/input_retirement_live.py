"""Opt-in irreversible seat retirement in an explicitly disposable desktop.

Requires the existing two GTK fixtures, their journals and WAYLAND_DEBUG logs,
a reviewed input_lifecycle_live.py plan, an enabled matching experiment, and
an already running unrestricted Driver service. The operator signs only the
public grant-request.json (capabilities 10) outside the guest, then transfers
grant.json. No private key, automatic grant, install, or compositor restart.

Both module paths must already exist and have different canonical paths and
inodes; the replacement may be an independently staged copy of the same build.
Unload permanently retires seats for this desktop. The runner intentionally
leaves the module unloaded and the lifetime marker intact, even on failure.
Restart the disposable desktop before any further input test. This focused
native gate does not prove fresh-desktop recovery or the full desktop matrix.
Raw evidence is private and can contain paths, input, and public grants.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import os
from pathlib import Path
import re
import select
import stat
import subprocess
import sys
import time
from types import SimpleNamespace

from driver_input_live import MCP, journal, state, wait_for, wm
from input_lifecycle_live import validate_plan
from input_transport_test import connect, exchange
from lifecycle_evidence import held_release, primary_wire_events, unchanged_primary
from live_discovery import connect as discovery_connect
from realapp_proof import cleanup_all
from retirement_evidence import active_experiment, bound_seats, replacement_refusal, retired_seats, service_owns_socket, socket_closed


def ctl(*arguments):
    return subprocess.check_output(["hyprctl", *arguments], text=True, timeout=5).strip()


def status():
    value = json.loads(ctl("-j", "cua:status"))
    if not isinstance(value, dict):
        raise AssertionError("plugin status is not an object")
    return value


def process_identity(pid):
    directory = Path(f"/proc/{pid}")
    if directory.stat().st_uid != os.getuid():
        raise ValueError("selected process does not belong to test user")
    fields = (directory / "stat").read_text().rsplit(") ", 1)[1].split()
    if fields[0] in ("Z", "X"):
        raise AssertionError("selected process is not alive")
    return (str((directory / "exe").resolve(strict=True)), fields[19])


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def preflight(args):
    if not __debug__ or sys.platform != "linux" or not args.disposable_session_restart_required:
        raise ValueError("requires Linux, assertions, and explicit disposable-session-restart-required opt-in")
    if args.compositor_pid <= 0:
        raise ValueError("select the exact disposable compositor PID")
    instance = os.environ.get("HYPRLAND_INSTANCE_SIGNATURE", "")
    if not re.fullmatch(r"[a-zA-Z0-9_.-]+", instance) or instance in (".", ".."):
        raise ValueError("missing safe compositor instance signature")
    matches = [row for row in json.loads(ctl("-j", "instances")) if row.get("instance") == instance]
    if len(matches) != 1 or matches[0].get("pid") != args.compositor_pid:
        raise ValueError("ambient hyprctl instance is not the selected disposable compositor")
    runtime = Path(os.environ["XDG_RUNTIME_DIR"])
    expected = runtime / "hypr" / instance
    for directory in (runtime, runtime / "hypr", expected):
        metadata = directory.lstat()
        if (not directory.is_absolute() or not stat.S_ISDIR(metadata.st_mode)
                or metadata.st_uid != os.getuid() or metadata.st_mode & 0o077):
            raise ValueError("require private same-user runtime directories without symlinks")
    if args.input_directory != expected:
        raise ValueError("input directory must match the exact ambient compositor instance")
    initial = status()
    active_experiment(initial)
    compositor = process_identity(args.compositor_pid)
    if Path(compositor[0]).name != "Hyprland":
        raise ValueError("selected process is not Hyprland")
    for path in (args.module, args.replacement_module, args.driver):
        if not path.is_absolute() or path.is_symlink() or not path.is_file():
            raise ValueError("require explicit absolute regular artifact paths")
    if args.module.resolve() == args.replacement_module.resolve() or args.module.samefile(args.replacement_module):
        raise ValueError("replacement requires a different canonical path and inode")
    mapped = {row.split(maxsplit=5)[5] for row in Path(f"/proc/{args.compositor_pid}/maps").read_text().splitlines()
              if len(row.split(maxsplit=5)) == 6}
    if str(args.module.resolve()) not in mapped or str(args.replacement_module.resolve()) in mapped:
        raise ValueError("initial module must be mapped; replacement must not already be mapped")
    artifacts = {name: digest(getattr(args, name)) for name in ("module", "replacement_module", "driver")}
    for name, actual in artifacts.items():
        if actual != getattr(args, name + "_sha256"):
            raise ValueError("artifact digest does not match selected candidate: " + name)
    root = Path(__file__).resolve().parents[4]
    head = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()
    if head != args.source_sha:
        raise ValueError("harness checkout differs from selected source SHA")
    dirty = subprocess.check_output(["git", "-C", str(root), "status", "--porcelain", "--untracked-files=normal",
                                    "--", "libs/cua-driver"], text=True)
    if dirty:
        raise ValueError("commit the candidate Driver/plugin/harness changes before native certification")
    plan = json.loads(args.plan.read_text())
    validate_plan(plan)
    service = process_identity(args.driver_service_pid)
    if service[0] != str(args.driver.resolve()):
        raise ValueError("Driver service executable does not match the selected artifact")
    socket_metadata = args.driver_socket.lstat()
    if (not args.driver_socket.is_absolute() or not stat.S_ISSOCK(socket_metadata.st_mode)
            or socket_metadata.st_uid != os.getuid()):
        raise ValueError("Driver socket must be an absolute same-user socket without a symlink")
    service_owns_socket(Path("/proc/net/unix").read_text(),
        [os.readlink(path) for path in Path(f"/proc/{args.driver_service_pid}/fd").iterdir()], args.driver_socket)
    service_environment = dict(item.split(b"=", 1) for item in
        Path(f"/proc/{args.driver_service_pid}/environ").read_bytes().split(b"\0") if b"=" in item)
    if (service_environment.get(b"HYPRLAND_INSTANCE_SIGNATURE") != instance.encode()
            or service_environment.get(b"CUA_DRIVER_EXPERIMENTAL_HYPRLAND_INPUT") != b"1"):
        raise ValueError("Driver service must enable input in this exact compositor instance")
    identities = {args.compositor_pid: compositor, args.driver_service_pid: service}
    seats = {}
    for actor in ("background", "foreground"):
        pid = plan[actor]["pid"]
        identities[pid] = process_identity(pid)
        command = Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0")
        if not any(item.endswith(b"/isolated-input/main.py") for item in command):
            raise ValueError("selected client must be the disposable isolated-input GTK fixture")
        log = getattr(args, actor + "_journal")
        heartbeat = state(log)
        if heartbeat["held"] or not 0 <= time.monotonic_ns() - heartbeat["time"] <= 2_000_000_000:
            raise ValueError("fixture must have a fresh idle heartbeat")
        seats[actor] = bound_seats(getattr(args, actor + "_wire").read_text())
    marker = expected / "cua-input-seat-lifetime"
    metadata = marker.lstat()
    if not stat.S_ISDIR(metadata.st_mode) or stat.S_IMODE(metadata.st_mode) != 0o700 or metadata.st_uid != os.getuid():
        raise ValueError("missing private seat-lifetime marker")
    paths = [expected / name for name in ("cua-input-test.sock", "cua-input-test-2.sock")]
    discovery = Path(initial["transport"]["socket"])
    if discovery.parent != expected:
        raise ValueError("discovery socket belongs to another instance")
    return initial, plan, identities, seats, paths + [discovery], (metadata.st_dev, metadata.st_ino), artifacts


def run(args):
    initial, plan, identities, seats, paths, marker_identity, artifacts = preflight(args)
    args.evidence.mkdir(parents=True, exist_ok=False)
    result = {"result": "failed", "native_run": True, "source_sha": args.source_sha,
              "artifact_sha256": artifacts, "full_desktop_matrix": False,
              "fresh_desktop_recovery_tested": False, "desktop_restart_required": True}
    clients, sockets = [], []
    grab = executor = None
    loaded = args.module
    unload_attempted = False
    marker = args.input_directory / "cua-input-seat-lifetime"

    def snapshot(client, actor):
        response = client.tool("get_window_state", {**plan[actor], "max_elements": 80})
        assert not response.get("isError"), response
        value = response["structuredContent"]
        assert value.get("screenshot_width", 0) > 0 and value.get("screenshot_height", 0) > 0
        assert value["window_bounds"] == plan[actor + "_bounds"], "fixture geometry changed"
        return value

    def alive():
        stamp = time.monotonic_ns()
        for pid, identity in identities.items():
            assert process_identity(pid) == identity, "process exited or was replaced"
        assert json.loads(ctl("-j", "version")), "compositor did not answer"
        for actor in ("background", "foreground"):
            wait_for(lambda: state(getattr(args, actor + "_journal"))["time"] > stamp, 2)
            snapshot(observer, actor)

    def no_transports():
        assert all(not os.path.lexists(path) for path in paths), "retired transport path remains"
        metadata = marker.lstat()
        assert (metadata.st_dev, metadata.st_ino) == marker_identity, "lifetime marker changed"

    try:
        (args.evidence / "preflight.json").write_text(json.dumps({"status": initial, "identities": identities,
            "version": json.loads(ctl("-j", "version")), "artifact_sha256": artifacts}, indent=2))
        for name in ("agent", "observer"):
            directory = args.evidence / name
            directory.mkdir()
            clients.append(MCP(SimpleNamespace(evidence=directory, driver=args.driver, driver_socket=args.driver_socket)))
        agent, observer = clients
        bg = snapshot(agent, "background")
        snapshot(observer, "foreground")
        for point in (plan["from"], plan["to"]):
            assert point[0] < bg["screenshot_width"] and point[1] < bg["screenshot_height"]
        arguments = {**plan["background"], "session": "retirement-" + args.evidence.name, "delivery_mode": "background"}
        response = agent.tool("press_key", {**arguments, "key": "Escape"})
        snapshot(observer, "background")
        pending = response.get("structuredContent", {})
        assert response.get("isError") and pending.get("reason") == "pending_operator_approval", response
        (args.evidence / "grant-request.json").write_text(json.dumps(pending))
        print("GRANT_REQUIRED capabilities=10", flush=True)
        wait_for((args.evidence / "grant.json").exists, 120)
        grant = json.loads((args.evidence / "grant.json").read_text())
        assert all(grant[key] == pending[key] for key in ("epoch", "challenge", "target"))
        assert grant["capabilities"] == 10 and grant["packet"].split()[4] == "10"
        assert int(grant["packet"].split()[3]) == grant["expires_unix_ms"]
        assert 15_000 < grant["expires_unix_ms"] - time.time_ns() // 1_000_000 <= 60_000, "need lease time for setup and fault"
        for path in paths[:2]:
            connection, _ = connect(path)
            sockets.append(connection)
        sockets.append(discovery_connect(initial))
        assert type(pending["lane"]) is int and pending["lane"] in (0, 1)
        operator = sockets[pending["lane"]]
        assert exchange(operator, grant["packet"])["ok"]
        desktop = observer.tool("get_desktop_state", {})["structuredContent"]
        box, point = plan["foreground_bounds"], plan["foreground_point"]
        grab = subprocess.Popen([str(args.primary_grab), str(box["x"] + point[0]), str(box["y"] + point[1]),
            str(desktop["screen_width"]), str(desktop["screen_height"]), "60000"], stdout=subprocess.PIPE, text=True)
        wait_for(lambda: select.select([grab.stdout], [], [], 0)[0])
        assert grab.stdout.readline().strip() == "HELD"
        wait_for(lambda: state(args.foreground_journal)["held"])
        snapshot(observer, "foreground")
        primary, before = wm(), state(args.foreground_journal)
        assert primary["pid"] == plan["foreground"]["pid"]
        assert "wl_pointer.button" in primary_wire_events(args.foreground_wire.read_text())
        snapshot(agent, "background")
        assert digest(args.module) == artifacts["module"], "loaded artifact changed after preflight"
        offsets = {actor: getattr(args, actor + "_wire").stat().st_size for actor in seats}
        offset = len(journal(args.background_journal))
        executor = ThreadPoolExecutor(max_workers=1)
        future = executor.submit(agent.tool, "drag", {**arguments, "from_x": plan["from"][0],
            "from_y": plan["from"][1], "to_x": plan["to"][0], "to_y": plan["to"][1], "duration_ms": 2000})
        press = wait_for(lambda: next((row for row in journal(args.background_journal)[offset:]
                                      if row["kind"] == "button-press"), None), 5)
        fault_ns = time.monotonic_ns()
        assert 0 <= fault_ns - press["time"] < 900_000_000, "fault too late to rule out natural completion"
        unload_attempted = True
        assert ctl("plugin", "unload", str(loaded)) == "ok"
        loaded = None
        wait_for(lambda: any(row["kind"] == "button-release" for row in journal(args.background_journal)[offset:]), 3)
        rows = journal(args.background_journal)[offset:]
        (args.evidence / "release-evidence.json").write_text(json.dumps({"fault_ns": fault_ns, "rows": rows}))
        result.update(held_release(rows, fault_ns=fault_ns))
        response = future.result(timeout=5)
        assert response.get("isError") and response.get("structuredContent", {}).get("reason") == "plugin_shutdown", response
        for connection in sockets:
            socket_closed(connection)
        result["old_connections_closed"] = len(sockets)
        no_transports()
        alive()
        assert not state(args.background_journal)["held"]

        def verify_retirement():
            alive()
            assert grab.poll() is None and wm() == primary, "primary grab or compositor state changed"
            for actor in seats:
                with getattr(args, actor + "_wire").open("rb") as stream:
                    stream.seek(offsets[actor])
                    wire = stream.read().decode("utf-8")
                result[actor + "_retirement"] = retired_seats(seats[actor], wire)
                if actor == "foreground":
                    result.update(unchanged_primary(before, state(args.foreground_journal), wire))
            no_transports()

        verify_retirement()
        result["replacement_attempts"] = []
        for label, module in (("same_path", args.module), ("different_path", args.replacement_module)):
            assert digest(module) == artifacts["module" if label == "same_path" else "replacement_module"]
            loaded = module  # Cleanup also covers ambiguous load outcomes.
            assert ctl("plugin", "load", str(module)) == "ok", "replacement registration failed before runtime refusal"
            assert ctl("reload") == "ok"
            refusal = status()  # Absent/invalid status is a failure, never refusal proof.
            result["replacement_attempts"].append({"case": label, **replacement_refusal(refusal)})
            (args.evidence / (label + "-status.json")).write_text(json.dumps(refusal, indent=2))
            verify_retirement()
            assert ctl("plugin", "unload", str(module)) == "ok"
            loaded = None
            verify_retirement()
        result.update(result="passed", same_client_processes_alive=True, compositor_alive=True)
    except Exception as error:
        result.update(error_type=type(error).__name__, error=str(error))
        raise
    finally:
        def stop_input():
            if not unload_attempted:
                for connection in sockets[:2]:
                    exchange(connection, "STOP")

        def release_primary():
            if grab and grab.poll() is None:
                grab.terminate()
                grab.wait(timeout=5)
                wait_for(lambda: not state(args.foreground_journal)["held"], 3)

        def unload_remaining():
            assert ctl("plugin", "unload", str(loaded)) == "ok", "cleanup unload was not acknowledged"

        operations = [("stop_input", stop_input)]
        if unload_attempted and loaded:
            operations.append(("unload_remaining_module", unload_remaining))
        operations += [("primary_release", release_primary)]
        operations += [("socket_close", connection.close) for connection in sockets]
        if executor:
            operations.append(("action_thread", lambda: executor.shutdown(wait=True, cancel_futures=True)))
        operations += [("mcp_close", client.close) for client in clients]
        errors = cleanup_all(operations)
        result["cleanup_errors"] = errors
        if errors:
            result["result"] = "failed"
        (args.evidence / "result.json").write_text(json.dumps(result, indent=2))
        print(json.dumps(result), flush=True)
        if errors:
            raise AssertionError("retirement harness cleanup failed")
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--disposable-session-restart-required", action="store_true")
    parser.add_argument("--compositor-pid", type=int, required=True)
    parser.add_argument("--driver-service-pid", type=int, required=True)
    parser.add_argument("--source-sha", required=True)
    for name in ("plan", "evidence", "driver", "driver-socket", "input-directory", "primary-grab",
                 "background-journal", "foreground-journal", "background-wire", "foreground-wire",
                 "module", "replacement-module"):
        parser.add_argument("--" + name, type=Path, required=True)
    for name in ("module", "replacement-module", "driver"):
        parser.add_argument("--" + name + "-sha256", required=True)
    run(parser.parse_args())
