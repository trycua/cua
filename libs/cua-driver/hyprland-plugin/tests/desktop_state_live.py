"""Native desktop-state cancellation proof in an explicitly disposable guest.

Run each --case once with --mode control, then --mode action --control RESULT.
Both runs require already selected native fixtures and finalized Driver video.
The plan extends input_lifecycle_live's plan with background_address; move_to
or resize_to must choose the exact geometry for those cases. --source-sha names
the independently verified candidate. This is focused evidence, not the desktop
matrix or a physical-hardware claim.

The action run emits GRANT_REQUIRED and waits for grant-initial.json, then
grant-recovery.json in its exclusive evidence directory. Only the external host
operator signs grants (capabilities 10). For destroy it first emits
REPLACEMENT_REQUIRED and waits for recovery-plan.json containing target
{pid,window_id}, bounds, and journal for an explicitly created replacement.
No fixture is automatically relaunched and no signing key enters this process.
Raw journals, traces, grants, wire logs, and video are private evidence.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path
import re
import select
import subprocess
import time
from types import SimpleNamespace

from desktop_state_evidence import agent_cleared, cancellation, compare_control, primary_effect, destroyed_resources_pruned, recovery_input, CASES
from driver_input_live import MCP, journal, state, wait_for, wm
from input_lifecycle_live import validate_plan
from input_transport_test import connect, exchange, refused
from lifecycle_evidence import primary_wire_events
from primary_trace import Trace
from realapp_proof import cleanup_all


def save(path, value):
    path.write_text(json.dumps(value, indent=2))


def run(args):
    from desktop_faults import FaultController

    args.evidence.mkdir(parents=True, exist_ok=False)
    result = {"result": "failed", "case": args.case, "mode": args.mode,
              "full_desktop_matrix": False, "physical_hardware": False}
    client = observer = operator = controller = grab = trace = executor = None
    recording = trace_active = False
    bg_offset = fg_offset = wire_offset = None
    label = "desktop-state-" + args.evidence.name
    try:
        plan = json.loads(args.plan.read_text())
        validate_plan(plan)
        assert re.fullmatch(r"[0-9a-f]{40}", args.source_sha), "require exact verified candidate SHA"
        assert args.module.is_file(), "require exact loaded module artifact"
        save(args.evidence / "plan.json", plan)
        target, foreground = plan["background"], plan["foreground"]
        result["identity"] = {"case": args.case, "source_sha": args.source_sha,
            "module_sha256": hashlib.sha256(args.module.read_bytes()).hexdigest(),
            "harness_sha256": {name: hashlib.sha256(Path(__file__).with_name(name).read_bytes()).hexdigest()
                               for name in ("desktop_state_live.py", "desktop_state_evidence.py", "desktop_faults.py",
                                            "driver_input_live.py", "input_lifecycle_live.py", "lifecycle_evidence.py",
                                            "primary_trace.py", "input_transport_test.py", "realapp_proof.py")},
            "foreground": foreground, "foreground_bounds": plan["foreground_bounds"],
            "background_bounds": plan["background_bounds"],
            "foreground_point": plan["foreground_point"],
            "move_to": plan.get("move_to"), "resize_to": plan.get("resize_to")}
        control = None
        if args.mode == "action":
            assert args.control, "action requires a separately retained fault-only control"
            control = json.loads(args.control.read_text())
            assert control.get("identity") == result["identity"], "control candidate/fixture mismatch"
            assert control.get("result") == "passed" and control.get("mode") == "control"
            result["control_sha256"] = hashlib.sha256(args.control.read_bytes()).hexdigest()

        def new_client(name):
            directory = args.evidence / name
            directory.mkdir()
            return MCP(SimpleNamespace(evidence=directory, driver=args.driver,
                                       driver_socket=args.driver_socket))

        def snapshot(mcp, selection):
            response = mcp.tool("get_window_state", {**selection, "max_elements": 80})
            assert not response.get("isError"), response
            data = response["structuredContent"]
            assert data.get("screenshot_width", 0) > 0 and data.get("screenshot_height", 0) > 0
            return data

        def status():
            return json.loads(subprocess.check_output(["hyprctl", "-j", "cua:status"], timeout=2))

        def probe_pending(selection, suffix):
            assert not (args.evidence / ("grant-" + suffix + ".json")).exists(), "grant predates fresh request"
            snapshot(client, selection)
            response = client.tool("press_key", {**selection, "session": label,
                                   "delivery_mode": "background", "key": "Escape"})
            snapshot(observer, selection)
            pending = response.get("structuredContent", {})
            assert response.get("isError") is True and pending.get("reason") == "pending_operator_approval", response
            save(args.evidence / ("request-" + suffix + ".json"), pending)
            return pending

        def receive_grant(pending, suffix):
            path = args.evidence / ("grant-" + suffix + ".json")
            print(json.dumps({"event": "GRANT_REQUIRED", "phase": suffix,
                              "request": str(args.evidence / ("request-" + suffix + ".json"))}), flush=True)
            wait_for(path.exists, 35)
            grant = json.loads(path.read_text())
            assert all(grant[key] == pending[key] for key in ("epoch", "challenge", "target"))
            assert grant["capabilities"] == 10, "only drag and recovery-key authority permitted"
            assert grant["expires_unix_ms"] / 1000 - time.time() > 5, "insufficient remaining lease"
            return grant

        observer, client = new_client("observer"), new_client("agent")
        initial_status = status()
        assert initial_status["experiment"]["lease_active"] is False, "another experiment owns input authority"
        save(args.evidence / "initial-status.json", initial_status)
        if args.case == "destroy":
            assert isinstance(plan.get("pre_target_resources"), list), "destruction requires pre-target resource baseline"
        bg, fg = snapshot(client, target), snapshot(observer, foreground)
        assert bg["window_bounds"] == plan["background_bounds"], "stale background geometry"
        assert fg["window_bounds"] == plan["foreground_bounds"], "stale foreground geometry"
        for point in (plan["from"], plan["to"]):
            assert point[0] < bg["screenshot_width"] and point[1] < bg["screenshot_height"]
        controller = FaultController(compositor_pid=args.compositor_pid, instance=args.instance,
            target={"pid": target["pid"], "address": plan["background_address"]},
            disposable=args.disposable, evidence=args.evidence / "fault", lock_fixture=args.lock_fixture,
            move_to=plan.get("move_to"), resize_to=plan.get("resize_to"), hold_seconds=12,
            compositor_exe=args.compositor_exe)
        save(args.evidence / "compositor-before.json", controller.snapshot())
        response = observer.tool("start_recording", {"output_dir": str(args.evidence / "recording"),
                                                   "record_video": True})
        # Stop even if startup reports an incomplete recording.
        recording = not response.get("isError")
        assert recording and response.get("structuredContent", {}).get("video_active") is True, "video not active"
        result["video_active"] = True
        if args.mode == "action":
            unapproved_offset = len(journal(args.background_journal))
            pending = probe_pending(target, "initial")
            assert not any(row["kind"] in ("button-press", "button-release", "key-press", "key-release", "scroll")
                           for row in journal(args.background_journal)[unapproved_offset:]), "initial unapproved input delivered"
            grant = receive_grant(pending, "initial")
            lane = pending["lane"]
            assert lane in (0, 1), "unknown experimental lane"
            input_path = args.input_directory / ("cua-input-test.sock" if lane == 0 else "cua-input-test-2.sock")
            operator, _ = connect(input_path)
            assert exchange(operator, grant["packet"])["ok"] is True

        desktop = observer.tool("get_desktop_state", {})["structuredContent"]
        point, box = plan["foreground_point"], fg["window_bounds"]
        grab = subprocess.Popen([str(args.primary_grab), str(box["x"] + point[0]),
            str(box["y"] + point[1]), str(desktop["screen_width"]), str(desktop["screen_height"]),
            "60000"], stdout=subprocess.PIPE, text=True)
        wait_for(lambda: select.select([grab.stdout], [], [], 0)[0])
        assert grab.stdout.readline().strip() == "HELD"
        wait_for(lambda: state(args.foreground_journal)["held"])
        snapshot(observer, foreground)
        before_wm = wm()
        assert before_wm["pid"] == foreground["pid"]
        assert "wl_pointer.button" in primary_wire_events(args.foreground_wire.read_text()), "wire oracle inactive"
        before_primary = state(args.foreground_journal)
        bg_offset = len(journal(args.background_journal))
        fg_offset = len(journal(args.foreground_journal))
        wire_offset = args.foreground_wire.stat().st_size
        trace = Trace(args.input_directory / "cua-input-test.sock")
        trace.exchange("TRACE_START")
        trace_active = True
        future = None
        if args.mode == "action":
            snapshot(client, target)
            executor = ThreadPoolExecutor(max_workers=1)
            def drag():
                response = client.tool("drag", {**target, "session": label, "delivery_mode": "background",
                    "from_x": plan["from"][0], "from_y": plan["from"][1],
                    "to_x": plan["to"][0], "to_y": plan["to"][1], "duration_ms": 2000})
                return response, time.monotonic_ns()
            future = executor.submit(drag)
            press = wait_for(lambda: next((row for row in journal(args.background_journal)[bg_offset:]
                                          if row["kind"] == "button-press"), None), 5)
            assert time.monotonic_ns() - press["time"] < 900_000_000, "fault injection too late"

        fault = controller.apply(args.case)
        save(args.evidence / "fault-observed.json", fault)
        result["fault"] = fault
        if args.case == "destroy":
            def pruned():
                current = status()
                try:
                    proof = destroyed_resources_pruned(plan["pre_target_resources"], initial_status, current)
                except AssertionError:
                    return None
                save(args.evidence / "pruned-status.json", current)
                return proof
            result.update(wait_for(pruned, .5))
        if args.mode == "action":
            def cleared():
                sample = status()
                stamp = time.monotonic_ns()
                save(args.evidence / "agent-status-last.json", {"time": stamp, "status": sample})
                try:
                    agent_cleared(sample, lane)
                except AssertionError:
                    return None
                return sample, stamp
            cleaned_status, status_ns = wait_for(cleared, 2)
            response, response_ns = future.result(timeout=5)
            save(args.evidence / "action-response.json", {"time": response_ns, "response": response})
            if args.case != "destroy":
                wait_for(lambda: any(row["kind"] == "button-release"
                                    for row in journal(args.background_journal)[bg_offset:]), 2)
            result.update(cancellation(args.case, journal(args.background_journal)[bg_offset:],
                fault_ns=fault["fault_ns"], observed_ns=fault["observed_ns"],
                response_ns=response_ns, response=response, status_ns=status_ns, status=cleaned_status,
                lane=lane, target_destroyed=fault["after"]["target"] is None))

        # Give the independently logged GTK state one heartbeat after the fault.
        # This observation window is identical for control and action and is
        # not the cancellation latency oracle.
        wait_for(lambda: state(args.foreground_journal)["time"] > fault["observed_ns"] + 250_000_000, 2)
        after_primary, after_wm = state(args.foreground_journal), wm()
        trace.exchange("TRACE_STOP")
        trace_active = False
        captured_trace = trace.collect()
        save(args.evidence / "primary-trace.json", captured_trace)
        with args.foreground_wire.open("rb") as stream:
            stream.seek(wire_offset)
            wire = stream.read().decode("utf-8")
        (args.evidence / "foreground-wire.log").write_text(wire)
        save(args.evidence / "foreground-journal.json", journal(args.foreground_journal)[fg_offset:])
        save(args.evidence / "background-journal.json", journal(args.background_journal)[bg_offset:])
        save(args.evidence / "primary-state.json", {"before": before_primary, "after": after_primary,
                                                    "wm_before": before_wm, "wm_after": after_wm})
        result["primary_effect"] = primary_effect(before_primary, after_primary, wire,
                                                  captured_trace, before_wm, after_wm)
        if args.mode == "control":
            assert not any(row["kind"] in ("button-press", "key-press")
                           for row in journal(args.background_journal)[bg_offset:]), "control received agent input"
            assert not any(row[2].startswith("agent_") for row in captured_trace["events"]), "control had active agent"
        if control:
            result.update(compare_control(control, result["primary_effect"], result["identity"]))
        save(args.evidence / "rollback.json", controller.rollback())
        snapshot(observer, foreground)
        if args.case != "destroy":
            snapshot(observer, target)
        else:
            windows = observer.tool("list_windows", {"pid": target["pid"]})
            assert not windows.get("isError"), windows
            assert not any(row.get("window_id") == target["window_id"]
                           for row in windows["structuredContent"]["windows"]), "destroyed window remains mapped"
        # End the owned primary adversary before another external approval wait.
        grab.terminate()
        grab.wait(timeout=5)
        wait_for(lambda: not state(args.foreground_journal)["held"])

        if args.mode == "action":
            stale = exchange(operator, grant["packet"])
            save(args.evidence / "stale-grant-response.json", stale)
            refused(stale, "invalid_grant", "stale_target")
            result["old_grant_refused"] = True
            recovery_target, recovery_journal = target, args.background_journal
            if args.case == "destroy":
                replacement = args.evidence / "recovery-plan.json"
                assert not replacement.exists(), "replacement must be selected after destruction"
                print(json.dumps({"event": "REPLACEMENT_REQUIRED", "path": str(replacement)}), flush=True)
                wait_for(replacement.exists, 35)
                selected = json.loads(replacement.read_text())
                recovery_target, recovery_journal = selected["target"], Path(selected["journal"])
                assert set(recovery_target) == {"pid", "window_id"}
                assert recovery_target["pid"] not in (target["pid"], foreground["pid"])
                assert snapshot(client, recovery_target)["window_bounds"] == selected["bounds"]
            recovery_offset = len(journal(recovery_journal))
            fresh = probe_pending(recovery_target, "recovery")
            assert not any(row["kind"] in ("key-press", "button-press")
                           for row in journal(recovery_journal)[recovery_offset:]), "unapproved recovery input delivered"
            result["fresh_approval_required"] = True
            fresh_grant = receive_grant(fresh, "recovery")
            assert fresh_grant["packet"] != grant["packet"], "recovery reused revoked approval"
            assert exchange(operator, fresh_grant["packet"])["ok"] is True
            snapshot(client, recovery_target)
            recovery_offset = len(journal(recovery_journal))
            recovery_primary = state(args.foreground_journal)
            recovery_wm = wm()
            recovery_wire_offset = args.foreground_wire.stat().st_size
            response = client.tool("press_key", {**recovery_target, "session": label,
                                   "delivery_mode": "background", "key": "Escape"})
            snapshot(observer, recovery_target)
            assert not response.get("isError") and response["structuredContent"].get("route") == "synthetic_events", response
            received = wait_for(lambda: next((row for row in journal(recovery_journal)[recovery_offset:]
                if row["kind"] == "key-release" and row.get("key") == "Escape"), None), 3)
            recovery_status = status()
            save(args.evidence / "recovery-status-before-cleanup.json", recovery_status)
            result.update(recovery_input(journal(recovery_journal)[recovery_offset:], recovery_status, lane))
            snapshot(observer, foreground)
            wait_for(lambda: state(args.foreground_journal)["time"] > received["time"], 2)
            for key in ("clicks", "keys", "held", "scroll", "motion"):
                assert state(args.foreground_journal)[key] == recovery_primary[key], "recovery reached foreground"
            assert wm() == recovery_wm, "recovery changed primary compositor state"
            with args.foreground_wire.open("rb") as stream:
                stream.seek(recovery_wire_offset)
                recovery_wire = stream.read().decode("utf-8")
            (args.evidence / "recovery-foreground-wire.log").write_text(recovery_wire)
            assert not primary_wire_events(recovery_wire), "recovery leaked primary input"
            save(args.evidence / "recovery-journal.json", journal(recovery_journal)[recovery_offset:])
            result["freshly_approved_recovery_received"] = bool(received)
        result["result"] = "passed"
    except BaseException as error:
        result.update(error_type=type(error).__name__, error=str(error))
        raise
    finally:
        def retain_failed_oracles():
            for path, offset, name in ((args.background_journal, bg_offset, "background-journal-final.json"),
                                       (args.foreground_journal, fg_offset, "foreground-journal-final.json")):
                if offset is not None:
                    save(args.evidence / name, journal(path)[offset:])
            if wire_offset is not None:
                with args.foreground_wire.open("rb") as stream:
                    stream.seek(wire_offset)
                    (args.evidence / "foreground-wire-final.log").write_bytes(stream.read())
        def revoke():
            if operator:
                try:
                    assert exchange(operator, "CANCEL")["ok"] is True
                    save(args.evidence / "final-agent-status.json", status())
                    agent_cleared(status(), lane)
                finally:
                    operator.close()
        def release_primary():
            if grab and grab.poll() is None:
                grab.terminate()
                grab.wait(timeout=5)
        def finish_trace():
            if trace:
                try:
                    if trace_active:
                        trace.exchange("TRACE_STOP")
                        save(args.evidence / "failed-primary-trace.json", trace.collect())
                finally:
                    trace.close()
        def finish_video():
            if recording:
                response = observer.tool("stop_recording", {})
                assert not response.get("isError"), response
                video = response.get("structuredContent", {}).get("last_video_path")
                assert video and Path(video).is_file() and Path(video).stat().st_size > 0, "missing finalized video"
                probe = json.loads(subprocess.check_output(["ffprobe", "-v", "error", "-show_streams",
                    "-show_format", "-of", "json", video], timeout=10))
                assert any(row.get("codec_type") == "video" for row in probe["streams"])
                assert float(probe["format"]["duration"]) > 0, "empty finalized video"
                result.update(video_finalized=True, video_file=str(Path(video).relative_to(args.evidence)))
                save(args.evidence / "video-probe.json", probe)
        operations = [("cancel_agent", revoke), ("finish_trace", finish_trace),
                      ("retain_oracles", retain_failed_oracles)]
        if controller:
            operations.append(("owned_fault_rollback", lambda: save(args.evidence / "cleanup-rollback.json", controller.rollback())))
            operations.append(("close_fault_controller", controller.close))
        operations.append(("release_primary", release_primary))
        operations.append(("finalize_video", finish_video))
        if executor:
            operations.append(("join_action", lambda: executor.shutdown(wait=True, cancel_futures=True)))
        if client:
            operations.append(("close_agent", client.close))
        if observer:
            operations.append(("close_observer", observer.close))
        errors = cleanup_all(operations)
        result["cleanup_errors"] = errors
        if errors or not result.get("video_finalized"):
            result["result"] = "failed"
        save(args.evidence / "result.json", result)
        print(json.dumps(result), flush=True)
        if errors:
            raise AssertionError("desktop-state cleanup failed: " + str(errors))
    return result


if __name__ == "__main__":
    if not __debug__:
        raise RuntimeError("assertions must remain enabled")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--case", choices=CASES, required=True)
    parser.add_argument("--mode", choices=("control", "action"), required=True)
    for name in ("plan", "evidence", "driver", "driver-socket", "input-directory", "module",
                 "primary-grab", "background-journal", "foreground-journal", "foreground-wire"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--control", type=Path)
    parser.add_argument("--lock-fixture", type=Path)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--compositor-pid", type=int, required=True)
    parser.add_argument("--compositor-exe", type=Path, required=True)
    parser.add_argument("--instance", required=True)
    parser.add_argument("--disposable", action="store_true")
    run(parser.parse_args())
