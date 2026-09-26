#!/usr/bin/env python3
"""Qualify one built candidate on an explicit emulator, then on an explicit physical phone.

Every selected device is admitted before anything is installed. The same APKs and host Driver run
the existing harnesses on the emulator first; the physical phase runs only after that passes. The
receipt names devices by role only. Serials, screenshots and raw logs stay in the evidence directory.
"""
import argparse
import hashlib
import json
import pathlib
import re
import shlex
import subprocess
import sys
import time

ROOT = pathlib.Path(__file__).resolve().parents[1]
PACKAGES = {"runtime": "ai.cua.driver.runtime", "fixture-target": "ai.cua.fixture.notes", "demo": "ai.cua.android.demo"}
SYNTHETIC = ("ai.cua.fixture.notes", "ai.cua.android.demo")
RUNTIME_API = "37"
LEASE_CLEANUP_SECONDS = 75  # 60-second runtime lease, 1-second expiry timer, and display teardown.
PROPS = ("ro.kernel.qemu", "ro.boot.qemu", "ro.build.version.sdk", "ro.build.version.release",
         "ro.product.model", "ro.product.cpu.abi")
# Checks the harness deliberately does not run on a physical phone, recorded rather than skipped.
PHYSICAL_NOT_RUN = [
    {"check": "lifecycle-smoke.py", "reason": "expands and reads the phone's notification shade"},
    {"check": "human IME concurrency", "reason": "display-0 typing is synthetic ADB text"},
    {"check": "fold posture or rotation change during a session", "reason": "not exercised"},
]


def device_class(props):
    """Emulator images report qemu; physical phones do not."""
    return "emulator" if "1" in (props.get("ro.kernel.qemu"), props.get("ro.boot.qemu")) else "physical"


def admission(role, serial, props, online):
    """Reasons this device cannot fill the role, decided before any mutation."""
    if online.get(serial) != "device":
        return ["not_online:" + (online.get(serial) or "absent")]
    return [] if device_class(props) == role else ["device_class_is_" + device_class(props)]


def unsupported(props):
    sdk = props.get("ro.build.version.sdk")
    return [] if sdk == RUNTIME_API else ["api_level_" + str(sdk) + "_runtime_requires_" + RUNTIME_API]


def physical_gate(emulator_status):
    return None if emulator_status == "pass" else "emulator_phase_" + emulator_status


def redact(text, serials):
    for role, serial in serials.items():
        text = text.replace(serial, "<" + role + ">")
    return text


def synthetic_tasks_off_main_display(stack):
    """Task IDs of synthetic packages that `am stack list` places on any display other than 0."""
    display, found = None, []
    for line in stack.splitlines():
        root = re.match(r"RootTask id=\d+ .*\bdisplayId=(\d+)", line.strip())
        if root:
            display = int(root.group(1))
            continue
        task = re.match(r"taskId=(\d+): ([\w.]+)/", line.strip())
        if task and display not in (None, 0) and task.group(2) in SYNTHETIC:
            found.append(int(task.group(1)))
    return found


def sha256(path):
    return hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()


def refuse(message):
    print(message, file=sys.stderr)
    sys.exit(2)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--emulator", required=True, help="adb serial of the emulator; always runs first")
    p.add_argument("--physical", help="adb serial of the physical phone; runs only after the emulator passes")
    p.add_argument("--driver", type=pathlib.Path, required=True)
    p.add_argument("--evidence-dir", type=pathlib.Path, required=True)
    p.add_argument("--runs", type=int, default=2)
    p.add_argument("--lifecycle", action="store_true", help="also run lifecycle-smoke.py on the emulator")
    args = p.parse_args()
    evidence = args.evidence_dir.resolve()
    repo = subprocess.run(["git", "-C", str(ROOT), "rev-parse", "--show-toplevel"], capture_output=True, text=True)
    if repo.returncode == 0 and evidence.is_relative_to(pathlib.Path(repo.stdout.strip())) and subprocess.run(
            ["git", "-C", str(ROOT), "check-ignore", "-q", str(evidence)]).returncode != 0:
        refuse("Evidence contains screenshots and device identifiers; choose a directory outside the repository")
    serials = {"emulator": args.emulator}
    if args.physical:
        serials["physical"] = args.physical
    if args.physical == args.emulator:
        refuse("The emulator and physical phases need two different devices")
    listed = subprocess.run(["adb", "devices"], capture_output=True, text=True, timeout=15, check=True).stdout
    online = dict(line.split("\t", 1) for line in listed.splitlines()[1:] if "\t" in line)

    def shell(serial, *command, check=True):
        return subprocess.run(["adb", "-s", serial, "shell", shlex.join(map(str, command))],
                              capture_output=True, text=True, timeout=30, check=check).stdout

    props = {}
    for role, serial in serials.items():
        props[role] = {k: shell(serial, "getprop", k).strip() for k in PROPS} if online.get(serial) == "device" else {}
        refused = admission(role, serial, props[role], online)
        if refused:
            refuse(role + " device refused before any mutation: " + ", ".join(refused))
    apks = {module: ROOT / module / "build/outputs/apk/debug" / (module + "-debug.apk") for module in PACKAGES}
    def identity():
        return {module: sha256(apk) for module, apk in apks.items()}, sha256(args.driver)

    candidate = {"source_revision": subprocess.run(["git", "-C", str(ROOT), "rev-parse", "HEAD"],
                                                   capture_output=True, text=True).stdout.strip() or None,
                 "source_dirty": bool(subprocess.run(["git", "-C", str(ROOT), "status", "--porcelain", "--", "."],
                                                     capture_output=True, text=True).stdout.strip()),
                 "apk_sha256": identity()[0], "driver_sha256": identity()[1]}
    evidence.mkdir(parents=True, exist_ok=False)
    receipt = {"schema": "cua.android.qualification.v0", "candidate": candidate, "phases": {}}

    def environment(serial, role):
        displays = shell(serial, "dumpsys", "SurfaceFlinger", "--display-id", check=False)
        state = re.search(r"name='([A-Z_]+)'", shell(serial, "cmd", "device_state", "state", check=False))
        return {"device_class": role, "model": props[role]["ro.product.model"],
                "android_release": props[role]["ro.build.version.release"],
                "api_level": props[role]["ro.build.version.sdk"], "abi": props[role]["ro.product.cpu.abi"],
                "physical_displays": len(re.findall(r"^Display \d+ \(HWC display", displays, re.M)),
                "device_state": state.group(1) if state else "unavailable"}

    def run_phase(role, serial):
        out = evidence / role
        out.mkdir()
        phase = {"steps": {}, "status": "fail"}

        def step(name, command, timeout):
            log = out / (name + ".log")
            with log.open("w") as handle:
                code = subprocess.run(command, stdout=handle, stderr=subprocess.STDOUT, timeout=timeout).returncode
            phase["steps"][name] = "pass" if code == 0 else "fail"
            if code:
                tail = log.read_text(errors="replace").strip().splitlines()[-12:]
                phase["failure"] = {"step": name, "exit_code": code, "log": str(log.relative_to(evidence)),
                                    "tail": [redact(line, serials) for line in tail]}
            return code == 0

        try:
            phase["environment"] = environment(serial, role)
            if identity() != (candidate["apk_sha256"], candidate["driver_sha256"]):
                phase["failure"] = {"step": "candidate", "reason": "artifacts_changed_since_admission"}
                return phase
            ok = step("deploy", [sys.executable, str(ROOT / "scripts/deploy.py"), "--device", serial], 300)
            if ok:
                installed = {}
                for module, package in PACKAGES.items():
                    path = shell(serial, "pm", "path", package).strip().splitlines()[0].removeprefix("package:")
                    installed[module] = shell(serial, "sha256sum", path).split()[0]
                phase["installed_identity"] = "match" if installed == candidate["apk_sha256"] else "mismatch"
                ok = phase["installed_identity"] == "match"
            smoke = [sys.executable, str(ROOT / "scripts/smoke.py"), "--device", serial,
                     "--driver", str(args.driver), "--evidence-dir", str(out / "smoke"), "--runs", str(args.runs)]
            ok = ok and step("smoke", smoke, 900)
            if role == "emulator" and args.lifecycle:
                ok = ok and step("lifecycle", [sys.executable, str(ROOT / "scripts/lifecycle-smoke.py"), "--device", serial,
                                               "--driver", str(args.driver), "--evidence-dir", str(out / "lifecycle")], 900)
            if role == "physical":
                phase["not_run"] = PHYSICAL_NOT_RUN
            if ok:
                caps = json.loads(subprocess.run([str(args.driver), "--device", serial, "capabilities"],
                                                 capture_output=True, text=True, timeout=30).stdout)["data"]["capabilities"]
                phase["driver_unsupported"] = sorted(c["capability"] for c in caps if c["support"] == "unsupported")
                phase["driver_unverified"] = sorted(c["capability"] for c in caps
                                                    if c.get("qualification", {}).get("status") == "unverified")
            phase["status"] = "pass" if ok else "fail"
        except Exception as error:  # Keep the original failure beside the cleanup result.
            phase["failure"] = {"step": "harness", "error": redact(repr(error), serials)}
        finally:
            for package in SYNTHETIC:
                shell(serial, "am", "force-stop", package, check=False)

            def residue():
                return {"cua_virtual_displays": shell(serial, "dumpsys", "display", check=False)
                        .count('DisplayDeviceInfo{"Cua agent"'),
                        "synthetic_tasks_off_display_0": synthetic_tasks_off_main_display(
                            shell(serial, "am", "stack", "list", check=False))}

            # A session whose controller just died is released by the runtime lease, not by the harness.
            start, left = time.monotonic(), residue()
            while any(left.values()) and time.monotonic() - start < LEASE_CLEANUP_SECONDS:
                time.sleep(3)
                left = residue()
            waited = round(time.monotonic() - start)
            phase["cleanup"] = dict(left, waited_s=waited, result="residue" if any(left.values())
                                    else "clean" if waited == 0 else "clean_after_lease_expiry")
            if phase["status"] == "pass" and phase["cleanup"]["result"] != "clean":
                phase["status"] = "fail"
        return phase

    for role, serial in serials.items():
        blocked = physical_gate(receipt["phases"]["emulator"]["status"]) if role == "physical" else None
        if blocked:
            receipt["phases"][role] = {"status": "blocked", "reason": blocked}
        elif unsupported(props[role]):
            receipt["phases"][role] = {"status": "unsupported", "reasons": unsupported(props[role]),
                                       "environment": environment(serial, role)}
        else:
            receipt["phases"][role] = run_phase(role, serial)
        (evidence / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
        print(role + ": " + receipt["phases"][role]["status"], flush=True)
    if "physical" not in serials:
        receipt["phases"]["physical"] = {"status": "not_selected"}
        (evidence / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    passed = all(phase["status"] == "pass" for role, phase in receipt["phases"].items() if role in serials)
    print("Receipt: " + str(evidence / "receipt.json"))
    sys.exit(0 if passed else 1)


if __name__ == "__main__":
    main()
