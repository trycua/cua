#!/usr/bin/env python3
"""Bake an AndroidWorld AVD with all apps pre-installed.

Called by build-androidworld.sh. Boots a fresh Android emulator, runs
AndroidWorld's emulator_setup=True to install all 20 benchmark apps and
save per-app snapshots, then cleanly shuts down the emulator.

The resulting ~/.cua/android-avd/androidworld.avd is ready to be pushed
as an OCI artifact by build-androidworld.sh.

Environment variables
---------------------
AW_SOURCE_DIR   Path to android_world source checkout (default: /tmp/android_world)
AW_VENV         Path to venv with android_world installed (default: /tmp/aw-venv)
"""
import logging
import os
import subprocess
import sys
import time
from pathlib import Path

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(message)s")
log = logging.getLogger(__name__)

AW_SOURCE_DIR = os.environ.get("AW_SOURCE_DIR", "/tmp/android_world")
sys.path.insert(0, AW_SOURCE_DIR)

SDK = Path.home() / ".cua" / "android-sdk"
AVD_HOME = Path.home() / ".cua" / "android-avd"
AVD_NAME = "androidworld"
CONSOLE_PORT = 5564
GRPC_PORT = CONSOLE_PORT + 3000
ADB = str(SDK / "platform-tools" / "adb")
EMULATOR = str(SDK / "emulator" / "emulator")


def adb(*args, timeout=30):
    return subprocess.run(
        [ADB, "-s", f"emulator-{CONSOLE_PORT}", *args],
        capture_output=True,
        text=True,
        timeout=timeout,
    )


def wait_boot(timeout=300):
    log.info("Waiting for emulator boot...")
    deadline = time.time() + timeout
    while time.time() < deadline:
        if adb("shell", "getprop", "sys.boot_completed", timeout=10).stdout.strip() == "1":
            log.info("Boot complete.")
            return
        time.sleep(5)
    raise RuntimeError("Emulator timed out")


def patch_setup_to_ignore_errors():
    """Replace setup_apps with a version that skips per-app failures."""
    import android_world.env.setup_device.setup as _setup_mod
    from android_world.env import adb_utils
    from android_world.utils import app_snapshot as _snap

    def _resilient_setup_apps(env, app_list=None):
        adb_utils.press_home_button(env.controller)
        adb_utils.set_root_if_needed(env.controller)
        log.info("Installing and setting up applications ...")
        if app_list is None:
            app_list = _setup_mod._APPS
        for app in app_list:
            try:
                _setup_mod.maybe_install_app(app, env)
            except Exception as e:
                log.warning(f"  [SKIP] {app.app_name} install error: {e}")
                continue
            try:
                app.setup(env)
            except Exception as e:
                log.warning(f"  [WARN] {app.app_name} setup error (continuing): {e}")
            try:
                _snap.save_snapshot(app.app_name, env.controller)
            except Exception as e:
                log.warning(f"  [WARN] {app.app_name} snapshot error (continuing): {e}")
        log.info("setup_apps complete.")

    _setup_mod.setup_apps = _resilient_setup_apps


def main():
    import platform as _plat

    arch = "arm64-v8a" if _plat.machine() in ("arm64", "aarch64") else "x86_64"
    api_level = 33
    img_type = "google_apis"
    package = f"system-images;android-{api_level};{img_type};{arch}"
    abi = f"{img_type}/{arch}"
    avdmanager = str(SDK / "cmdline-tools" / "latest" / "bin" / "avdmanager")
    sdkmanager = str(SDK / "cmdline-tools" / "latest" / "bin" / "sdkmanager")

    env = os.environ.copy()
    env["ANDROID_SDK_ROOT"] = str(SDK)
    env["ANDROID_AVD_HOME"] = str(AVD_HOME)
    # macOS: auto-detect openjdk
    for jdk in (
        Path("/opt/homebrew/opt/openjdk/libexec/openjdk.jdk/Contents/Home"),
        Path("/usr/local/opt/openjdk/libexec/openjdk.jdk/Contents/Home"),
    ):
        if jdk.exists():
            env["JAVA_HOME"] = str(jdk)
            env["PATH"] = f"{jdk / 'bin'}:{env.get('PATH', '')}"
            break

    AVD_HOME.mkdir(parents=True, exist_ok=True)
    avd_dir = AVD_HOME / f"{AVD_NAME}.avd"

    if not (SDK / "system-images" / f"android-{api_level}" / img_type / arch).exists():
        log.info(f"Installing system image {package} ...")
        subprocess.run(
            [sdkmanager, f"--sdk_root={SDK}", "--install", package],
            input=b"y\n",
            capture_output=True,
            timeout=600,
            env=env,
        )

    log.info(f"Creating AVD '{AVD_NAME}' ...")
    result = subprocess.run(
        [
            avdmanager,
            "create",
            "avd",
            "--force",
            "--name",
            AVD_NAME,
            "--abi",
            abi,
            "--package",
            package,
            "--device",
            "pixel_6",
        ],
        input="no\n",
        capture_output=True,
        text=True,
        env=env,
        timeout=60,
    )
    if result.returncode != 0:
        log.error(f"avdmanager failed: {result.stderr}")
        sys.exit(1)

    log.info("Starting emulator ...")
    subprocess.run([ADB, "start-server"], capture_output=True, env=env)
    proc = subprocess.Popen(
        [
            EMULATOR,
            "-avd",
            AVD_NAME,
            "-gpu",
            "swiftshader_indirect",
            "-no-window",
            "-no-boot-anim",
            "-no-snapshot",
            "-port",
            str(CONSOLE_PORT),
            "-grpc",
            str(GRPC_PORT),
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        env=env,
    )
    log.info(f"Emulator PID {proc.pid}, console port {CONSOLE_PORT}")

    try:
        wait_boot(timeout=300)
        patch_setup_to_ignore_errors()

        log.info("Running AndroidWorld emulator_setup=True ...")
        from android_world.env import env_launcher

        android_env = env_launcher.load_and_setup_env(
            console_port=CONSOLE_PORT,
            grpc_port=GRPC_PORT,
            emulator_setup=True,
            freeze_datetime=True,
            adb_path=ADB,
        )
        log.info("Setup complete.")
        android_env.close()
    finally:
        log.info("Shutting down emulator ...")
        proc.terminate()
        try:
            proc.wait(timeout=30)
        except subprocess.TimeoutExpired:
            proc.kill()

    size = sum(f.stat().st_size for f in avd_dir.rglob("*") if f.is_file())
    log.info(f"Baked AVD: {avd_dir}  ({size / 1e9:.1f} GB)")


if __name__ == "__main__":
    main()
