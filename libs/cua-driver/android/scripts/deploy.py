#!/usr/bin/env python3
"""Install the explicit synthetic development packages on one authorized device."""
import argparse
import hashlib
import json
import pathlib
import re
import shlex
import subprocess
import time

ROOT = pathlib.Path(__file__).resolve().parents[1]
REMOTE = "/data/local/tmp/cua-driver"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--device", required=True)
    args = parser.parse_args()
    adb = ["adb", "-s", args.device]

    def shell(*command, check=True):
        return subprocess.run(adb + ["shell", shlex.join(command)], check=check,
                              capture_output=True, text=True, timeout=15)

    assert shell("getprop", "sys.boot_completed").stdout.strip() == "1", "Device not booted"
    old = shell("cat", REMOTE + "/runtime.pid", check=False)
    if old.returncode == 0 and old.stdout.strip().isdigit():
        pid = old.stdout.strip()
        command = shell("cat", "/proc/" + pid + "/cmdline", check=False).stdout
        if "ai.cua.driver.RuntimeMain" in command:
            shell("kill", pid)
            time.sleep(1)
        elif command:
            raise RuntimeError("Recorded PID belongs to another process; refusing to signal it")
    packages = {"runtime": "ai.cua.driver.runtime", "fixture-target": "ai.cua.fixture.notes", "demo": "ai.cua.android.demo"}
    for module, package in packages.items():
        apk = ROOT / module / "build/outputs/apk/debug" / (module + "-debug.apk")
        subprocess.run(adb + ["install", "-r", str(apk)], check=True, timeout=60)
        metadata = json.loads((apk.parent / "output-metadata.json").read_text())["elements"][0]
        installed = shell("dumpsys", "package", package).stdout
        assert re.search(r"versionName=" + re.escape(metadata["versionName"]) + r"\s", installed), "Installed version mismatch"
        print(package + " version " + metadata["versionName"] + " verified", flush=True)
    shell("mkdir", "-p", REMOTE)
    subprocess.run(adb + ["push", str(ROOT / "runtime/build/outputs/apk/debug/runtime-debug.apk"),
                         REMOTE + "/runtime.apk"], check=True, timeout=30)
    subprocess.run(adb + ["push", str(ROOT / "scripts/cua-driver"), REMOTE + "/cua-driver"],
                   check=True, timeout=30)
    digest = hashlib.sha256((ROOT / "runtime/build/outputs/apk/debug/runtime-debug.apk").read_bytes()).hexdigest()
    assert shell("sha256sum", REMOTE + "/runtime.apk").stdout.split()[0] == digest, "Runtime APK digest mismatch"
    shell("chmod", "444", REMOTE + "/runtime.apk")
    shell("chmod", "755", REMOTE + "/cua-driver")
    # Fixed command only; no caller input is interpolated into the remote shell.
    launch = ("CLASSPATH=/data/local/tmp/cua-driver/runtime.apk nohup app_process / "
              "ai.cua.driver.RuntimeMain >/data/local/tmp/cua-driver/runtime.log 2>&1 "
              "</dev/null & echo $! >/data/local/tmp/cua-driver/runtime.pid")
    subprocess.run(adb + ["shell", launch], check=True, timeout=10)
    time.sleep(1)
    started_pid = shell("cat", REMOTE + "/runtime.pid").stdout.strip()
    assert started_pid.isdigit(), "Runtime did not record its PID"
    assert "ai.cua.driver.RuntimeMain" in shell("cat", "/proc/" + started_pid + "/cmdline").stdout, "New runtime exited"
    result = shell(REMOTE + "/cua-driver", "--local", "doctor")
    doctor = json.loads(result.stdout)
    assert doctor["status"] == "ok" and doctor["data"]["runtime_pid"] == int(started_pid), "Doctor reached a different runtime"
    print("Runtime and synthetic fixtures installed; local doctor passed.")


if __name__ == "__main__":
    main()
