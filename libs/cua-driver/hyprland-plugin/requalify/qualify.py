#!/usr/bin/env python3
"""Build and requalify one plugin source against one channel's exact Hyprland.

Runs as root inside a disposable Arch container (see the requalify workflow).
It installs the channel's packages, records their identity and header hashes,
builds the plugin with the production options against those headers, runs the
CTest suite, and then hands a headless Hyprland session to headless.py as an
unprivileged user when a DRM device is present.

The result is CI requalification evidence for one build. It is not native
certification, and nothing here publishes or installs a package elsewhere.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent / "packaging" / "release"))

import channels  # noqa: E402

# The production package's options (packaging/release/profile_verify.py OPTIONS).
BUILD_OPTIONS = {"CUA_HYPRLAND_INPUT": "ON", "CUA_HYPRLAND_TEST_INPUT": "OFF", "CUA_HYPRLAND_INPUT_TRACE": "OFF"}
# make: the CMake API probe test configures a nested project with the default generator.
BUILD_PACKAGES = ("hyprland", "gcc", "cmake", "ninja", "make", "pkgconf", "binutils", "python", "openssl",
                  "libxkbcommon", "xkeyboard-config")
# The headless session's native Wayland clients and font.
SESSION_PACKAGES = ("foot", "ttf-dejavu", "mesa")
USER, UID = "cua", 1000


def sha256_file(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def run(command, log=None, check=True, timeout=1800, **kwargs):
    """Run a command, appending its output to log; return (code, output)."""
    started = time.monotonic()
    result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
                            timeout=timeout, **kwargs)
    if log:
        with open(log, "a") as handle:
            handle.write(f"$ {' '.join(map(str, command))}  ({time.monotonic() - started:.1f}s, exit {result.returncode})\n")
            handle.write(result.stdout)
    if check and result.returncode:
        raise subprocess.CalledProcessError(result.returncode, command, result.stdout)
    return result.returncode, result.stdout


def pacman_conf(channel, with_omarchy):
    lines = ["[options]", "HoldPkg = pacman glibc", "Architecture = auto", "ParallelDownloads = 5",
             "DisableSandbox", "SigLevel = Required DatabaseOptional", "LocalFileSigLevel = Optional", ""]
    for repo, server in channels.CHANNELS[channel]["repos"]:
        if repo == "omarchy" and not with_omarchy:
            continue
        lines += [f"[{repo}]", f"Server = {server}", ""]
    return "\n".join(lines)


def setup_pacman(job, log):
    with_omarchy = bool(job.get("needs_omarchy_repo"))
    Path("/etc/pacman.conf").write_text(pacman_conf(job["channel"], with_omarchy))
    # Drop the bootstrap's Arch databases: pacman's If-Modified-Since would keep
    # a newer Arch database over an older channel mirror's (rc, stable).
    for database in Path("/var/lib/pacman/sync").glob("*"):
        database.unlink()
    run(["pacman-key", "--init"], log)
    run(["pacman-key", "--populate", "archlinux"], log)
    if with_omarchy:
        # Trust bootstrap over HTTPS from the channel's own repository, as a
        # fresh Omarchy install does; this build is evidence, not a package.
        server = dict(channels.CHANNELS[job["channel"]]["repos"])["omarchy"]
        url = channels.repo_url(server, "omarchy")
        database = channels.parse_db(channels.fetch(url + "/omarchy.db"))
        keyring = database["omarchy-keyring"]["filename"]
        run(["pacman", "-U", "--noconfirm", f"{url}/{keyring}"], log)
        run(["pacman-key", "--populate", "omarchy"], log)
    for attempt in range(3):
        # -yy forces fresh databases; -uu lets an older channel downgrade the bootstrap.
        code, _ = run(["pacman", "-Syyuu", "--noconfirm", "--needed", *BUILD_PACKAGES, *SESSION_PACKAGES],
                      log, check=False)
        if code == 0:
            return
        time.sleep(10 * (attempt + 1))
    raise RuntimeError("pacman could not install the channel's packages")


def installed(name):
    return subprocess.check_output(["pacman", "-Q", name], text=True).split()[1]


def measure(job):
    """Identity of the installed compositor, headers, compiler and runtime."""
    import profile_measure
    import profile_verify as verify
    gxx = Path("/usr/bin/g++")
    macros = subprocess.check_output([str(gxx), "-dM", "-E", "-x", "c++", "-"], input="", text=True)
    compiler = re.search(r'^#define __VERSION__ "([^"]+)"$', macros, re.MULTILINE)[1]
    libraries = profile_measure.compositor_libraries()
    packages = {name: installed(name) for name in channels.TRACKED}
    expected = job.get("expected_packages") or {}
    drift = {name: {"detected": version, "installed": packages[name]}
             for name, version in sorted(expected.items()) if packages.get(name) != version}
    return {
        "hyprland": {
            "package_version": packages["hyprland"],
            "header_version": verify.run(str(verify.PKGCONF), "--modversion", "hyprland", extra_env=verify.PKGCONF_ENV),
            "headers_sha256": verify.header_inventory_sha256(),
            "sha256": sha256_file("/usr/bin/Hyprland"),
        },
        "compiler": {"version": compiler, "sha256": sha256_file(gxx)},
        "tracked_packages": packages,
        "runtime": {"basename": libraries["libstdc++.so.6"].name,
                    "sha256": sha256_file(libraries["libstdc++.so.6"]),
                    "packages": profile_measure.owner_versions(set(libraries.values()))},
        "detection_drift": drift,
    }


def build(source, out, header_version):
    build_dir = out / "build"
    log = out / "build.log"
    options = [f"-D{key}={value}" for key, value in BUILD_OPTIONS.items()]
    code, _ = run(["cmake", "-S", str(source), "-B", str(build_dir), "-G", "Ninja", "-DCMAKE_BUILD_TYPE=Release",
                   "-DCMAKE_CXX_COMPILER=/usr/bin/g++", "-DBUILD_TESTING=ON",
                   f"-DCUA_HYPRLAND_EXPECTED_VERSION={header_version}", *options], log, check=False)
    if code:
        return {"status": "fail", "step": "configure", "log": log.name}, None
    code, _ = run(["cmake", "--build", str(build_dir), "--parallel", str(os.cpu_count() or 2)], log, check=False)
    module = build_dir / "cua-hyprland-plugin.so"
    if code or not module.is_file():
        return {"status": "fail", "step": "compile", "log": log.name}, None
    return {"status": "pass", "module_sha256": sha256_file(module), "options": BUILD_OPTIONS, "log": log.name}, module


def ctest(out):
    log = out / "ctest.log"
    code, output = run(["ctest", "--test-dir", str(out / "build"), "--output-on-failure", "--no-tests=error",
                        "--timeout", "120"], log, check=False)
    # CTest omits the failed count when nothing failed: "100% tests passed out of 20".
    summary = re.search(r"(\d+)% tests passed(?:, (\d+) tests? failed)? out of (\d+)", output)
    total = int(summary[3]) if summary else 0
    failed = int(summary[2] or 0) if summary else None
    failures = re.findall(r"^\s*\d+ - (\S+) \(", output, re.MULTILINE)
    return {"status": "pass" if code == 0 and total else "fail", "total": total,
            "failed": failed, "failures": failures, "log": log.name}


def drm_cards():
    return sorted(str(path) for path in Path("/dev/dri").glob("card*")) if Path("/dev/dri").is_dir() else []


def session(module, out):
    """Load and smoke in a headless Hyprland session as an unprivileged user."""
    cards = drm_cards()
    if not cards:
        reason = "no DRM device in the container (vkms unavailable on this runner)"
        return {"status": "unavailable", "reason": reason}, {"status": "unavailable", "reason": reason}
    if subprocess.run(["id", USER], capture_output=True).returncode:
        run(["useradd", "-m", "-u", str(UID), "-G", "video,input", USER])
    runtime = Path(f"/run/user/{UID}")
    runtime.mkdir(parents=True, exist_ok=True)
    shutil.chown(runtime, USER, USER)
    runtime.chmod(0o700)
    for node in Path("/dev/dri").iterdir():
        node.chmod(0o666)
    # The workflow mounts the host's /run/udev read-only, so libudev sees the vkms device's properties.
    session_dir = out / "session"
    session_dir.mkdir(exist_ok=True)
    shutil.chown(session_dir, USER, USER)
    plugin = session_dir / "cua-hyprland-plugin.so"
    shutil.copy2(module, plugin)
    env = {"PATH": "/usr/bin", "HOME": f"/home/{USER}", "XDG_RUNTIME_DIR": str(runtime),
           "LIBSEAT_BACKEND": "noop", "AQ_DRM_DEVICES": cards[0], "LANG": "C.UTF-8"}
    command = ["runuser", "-u", USER, "--", "env", *[f"{k}={v}" for k, v in env.items()],
               sys.executable, str(HERE / "headless.py"), "--module", str(plugin), "--out", str(session_dir)]
    code, _ = run(command, out / "session.log", check=False, timeout=600)
    result_path = session_dir / "result.json"
    if not result_path.is_file():
        failure = {"status": "fail", "reason": f"headless runner exited {code} without a result"}
        return failure, {"status": "unavailable", "reason": "session did not start"}
    result = json.loads(result_path.read_text())
    return result["load"], result["smoke"]


def overall(record):
    build_ok = record["build"]["status"] == "pass" and record.get("ctest", {}).get("status") == "pass"
    if not build_ok or "fail" in (record["load"]["status"], record["smoke"]["status"]):
        return "fail"
    if record["load"]["status"] == "pass" and record["smoke"]["status"] == "pass":
        return "pass"
    return "build-only"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--job", type=Path, required=True, help="one matrix entry (JSON)")
    parser.add_argument("--source", type=Path, required=True, help="plugin source tree to build")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--skip-session", action="store_true")
    args = parser.parse_args()
    job = json.loads(args.job.read_text())
    args.out.mkdir(parents=True, exist_ok=True)
    record = {"schema": 1, "job": job, "started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}
    unavailable = {"status": "unavailable", "reason": "not reached"}
    record.update(build={"status": "fail", "step": "setup"}, load=dict(unavailable), smoke=dict(unavailable))
    try:
        setup_pacman(job, args.out / "pacman.log")
        record["measured"] = measure(job)
        record["build"], module = build(args.source, args.out, record["measured"]["hyprland"]["header_version"])
        if module:
            record["ctest"] = ctest(args.out)
            if args.skip_session:
                record["load"] = record["smoke"] = {"status": "unavailable", "reason": "session skipped"}
            else:
                record["load"], record["smoke"] = session(module, args.out)
    except Exception as error:  # Recorded as a failure of this build, never swallowed.
        record["error"] = f"{type(error).__name__}: {error}"
    record["status"] = overall(record)
    record["finished_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    (args.out / "result.json").write_text(json.dumps(record, indent=2, sort_keys=True) + "\n")
    print(json.dumps({k: record[k] for k in ("status", "build", "load", "smoke")}, indent=2))
    for path in (args.out / "build").glob("**/*"):
        if path.is_file() and path.suffix in (".o", ".a"):
            path.unlink()
    return 0


if __name__ == "__main__":
    sys.exit(main())
