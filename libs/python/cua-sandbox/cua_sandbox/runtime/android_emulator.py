"""Android Emulator runtime — uses the official Android SDK emulator.

Follows the docker-android pattern:
  1. Auto-installs Android command-line tools + SDK if needed
  2. Creates an AVD (Android Virtual Device)
  3. Launches the emulator with `-no-boot-anim -no-window -no-snapshot`
  4. Waits for `sys.boot_completed == 1` via ADB
  5. Supports APK installation via ADB

On Apple Silicon Macs, uses ARM64 system images with HVF acceleration.
On x86_64 hosts, uses x86_64 system images with KVM/HAXM.
"""

from __future__ import annotations

import asyncio
import logging
import os
import platform as _plat
import shutil
import socket
import subprocess
from pathlib import Path
from typing import Optional

from cua_sandbox.image import Image
from cua_sandbox.runtime.base import Runtime, RuntimeInfo

logger = logging.getLogger(__name__)

# ── SDK paths ────────────────────────────────────────────────────────────────

_SDK_ROOT = Path.home() / ".cua" / "android-sdk"
_AVD_HOME = Path.home() / ".cua" / "android-avd"
_CMDLINE_TOOLS_VERSION = "11076708"  # Latest as of 2024


def _java_env() -> dict:
    """Return env dict with JAVA_HOME set if needed."""
    env = os.environ.copy()
    if env.get("JAVA_HOME"):
        return env
    # Try Homebrew OpenJDK (macOS)
    brew_jdk = Path("/opt/homebrew/opt/openjdk/libexec/openjdk.jdk/Contents/Home")
    if not brew_jdk.exists():
        # Intel Mac
        brew_jdk = Path("/usr/local/opt/openjdk/libexec/openjdk.jdk/Contents/Home")
    if brew_jdk.exists():
        env["JAVA_HOME"] = str(brew_jdk)
        env["PATH"] = f"{brew_jdk / 'bin'}:{env.get('PATH', '')}"
        return env
    # Check if java works without JAVA_HOME
    try:
        subprocess.run(["java", "-version"], capture_output=True, timeout=5)
        return env
    except (subprocess.SubprocessError, FileNotFoundError):
        pass
    raise RuntimeError("Java not found. Install via: brew install openjdk")


def _sdk_path() -> Path:
    """Return the Android SDK root, preferring existing installations."""
    for env in ("ANDROID_HOME", "ANDROID_SDK_ROOT"):
        val = os.environ.get(env)
        if val and Path(val).exists():
            return Path(val)
    # Check common install locations
    common = [
        Path.home() / "Library" / "Android" / "sdk",  # Android Studio (macOS)
        Path("/opt/android"),  # docker-android
        Path.home() / "Android" / "Sdk",  # Linux
    ]
    for p in common:
        if (p / "emulator").exists():
            return p
    return _SDK_ROOT


def _ensure_sdk() -> Path:
    """Ensure Android SDK command-line tools, emulator, and platform-tools are installed."""
    sdk = _sdk_path()

    emulator_bin = sdk / "emulator" / "emulator"
    sdkmanager_bin = sdk / "cmdline-tools" / "latest" / "bin" / "sdkmanager"
    adb_bin = sdk / "platform-tools" / "adb"

    if emulator_bin.exists() and adb_bin.exists():
        return sdk

    logger.info(f"Android SDK not found. Installing to {sdk} ...")
    sdk.mkdir(parents=True, exist_ok=True)

    # Download command-line tools
    if not sdkmanager_bin.exists():
        system = _plat.system().lower()
        if system == "darwin":
            tools_url = f"https://dl.google.com/android/repository/commandlinetools-mac-{_CMDLINE_TOOLS_VERSION}_latest.zip"
        elif system == "linux":
            tools_url = f"https://dl.google.com/android/repository/commandlinetools-linux-{_CMDLINE_TOOLS_VERSION}_latest.zip"
        else:
            raise RuntimeError(
                "Android SDK auto-install not supported on Windows. Install Android Studio manually."
            )

        import urllib.request
        import zipfile

        zip_path = sdk / "cmdline-tools.zip"
        logger.info("Downloading Android command-line tools ...")
        urllib.request.urlretrieve(tools_url, str(zip_path))
        with zipfile.ZipFile(zip_path) as zf:
            zf.extractall(sdk / "cmdline-tools")
        # Move to expected path
        extracted = sdk / "cmdline-tools" / "cmdline-tools"
        dest = sdk / "cmdline-tools" / "latest"
        if extracted.exists() and not dest.exists():
            extracted.rename(dest)
        zip_path.unlink()
        # Ensure scripts are executable
        for f in (sdk / "cmdline-tools" / "latest" / "bin").iterdir():
            f.chmod(0o755)

    sdkmanager_bin = sdk / "cmdline-tools" / "latest" / "bin" / "sdkmanager"
    if not sdkmanager_bin.exists():
        raise RuntimeError(f"sdkmanager not found at {sdkmanager_bin}")

    env = _java_env()

    # Accept licenses
    logger.info("Accepting Android SDK licenses ...")
    subprocess.run(
        [str(sdkmanager_bin), f"--sdk_root={sdk}", "--licenses"],
        input=b"y\ny\ny\ny\ny\ny\ny\ny\n",
        capture_output=True,
        timeout=120,
        env=env,
    )

    # Install emulator, platform-tools, and system image
    arch = "arm64-v8a" if _plat.machine() in ("arm64", "aarch64") else "x86_64"
    api_level = "34"
    img_type = "google_apis"
    package = f"system-images;android-{api_level};{img_type};{arch}"
    platform = f"platforms;android-{api_level}"

    logger.info(f"Installing Android SDK packages: {package} ...")
    result = subprocess.run(
        [
            str(sdkmanager_bin),
            f"--sdk_root={sdk}",
            "--install",
            package,
            platform,
            "platform-tools",
            "emulator",
        ],
        capture_output=True,
        text=True,
        timeout=600,
        env=env,
    )
    if result.returncode != 0:
        raise RuntimeError(f"sdkmanager install failed: {result.stderr}")

    logger.info("Android SDK installation complete.")
    return sdk


def _find_free_emulator_port() -> int:
    """Return a free odd adb_port (console_port = adb_port - 1, must be even).

    Scans even console ports 5554–5682; returns the first pair where both
    console_port and adb_port are not in use (TCP bind check).
    """
    for console_port in range(5554, 5684, 2):  # 5554, 5556, ..., 5682
        adb_port = console_port + 1
        free = True
        for port in (console_port, adb_port):
            try:
                with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
                    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
                    s.bind(("127.0.0.1", port))
            except OSError:
                free = False
                break
        if free:
            return adb_port
    raise RuntimeError("No free Android emulator port pair found in range 5554–5682")


def _find_bin(sdk: Path, name: str) -> str:
    """Find a binary in the SDK."""
    candidates = [
        sdk / "emulator" / name,
        sdk / "platform-tools" / name,
        sdk / "cmdline-tools" / "latest" / "bin" / name,
    ]
    for c in candidates:
        if c.exists():
            return str(c)
    # Fall back to PATH
    found = shutil.which(name)
    if found:
        return found
    raise FileNotFoundError(f"{name} not found in SDK at {sdk}")


class AndroidEmulatorRuntime(Runtime):
    """Android emulator runtime using the official Android SDK.

    Creates an AVD and launches the Android emulator, following the
    docker-android pattern. Uses ADB for boot detection, APK install,
    and shell commands.
    """

    def __init__(
        self,
        *,
        api_level: int = 34,
        img_type: str = "google_apis",
        device_id: str = "pixel",
        memory_mb: int = 4096,
        cpu_count: int = 4,
        adb_port: Optional[int] = None,
        no_boot_anim: bool = True,
        gpu_mode: str = "swiftshader_indirect",
        headless: bool = True,
    ):
        self.api_level = api_level
        self.img_type = img_type
        self.device_id = device_id
        self.memory_mb = memory_mb
        self.cpu_count = cpu_count
        self.adb_port = adb_port
        self.no_boot_anim = no_boot_anim
        self.gpu_mode = gpu_mode
        self.headless = headless
        self._proc: Optional[subprocess.Popen] = None
        self._avd_name: Optional[str] = None
        self._sdk: Optional[Path] = None

    async def start(self, image: Image, name: str, **opts) -> RuntimeInfo:
        self._sdk = _ensure_sdk()
        self._avd_name = name

        # Resolve port lazily so callers don't need to pick one
        if self.adb_port is None:
            self.adb_port = _find_free_emulator_port()
            logger.info(
                f"Auto-selected emulator port: console={self.adb_port - 1}, adb={self.adb_port}"
            )

        arch = "arm64-v8a" if _plat.machine() in ("arm64", "aarch64") else "x86_64"
        package = f"system-images;android-{self.api_level};{self.img_type};{arch}"
        abi = f"{self.img_type}/{arch}"

        avdmanager = _find_bin(self._sdk, "avdmanager")
        emulator = _find_bin(self._sdk, "emulator")
        adb = _find_bin(self._sdk, "adb")

        # Set AVD home
        _AVD_HOME.mkdir(parents=True, exist_ok=True)
        env = _java_env()
        env["ANDROID_SDK_ROOT"] = str(self._sdk)
        env["ANDROID_AVD_HOME"] = str(_AVD_HOME)
        env["ANDROID_EMULATOR_WAIT_TIME_BEFORE_KILL"] = "10"

        # Create AVD if it doesn't exist
        avd_dir = _AVD_HOME / f"{name}.avd"
        if not avd_dir.exists():
            logger.info(f"Creating AVD '{name}' with {package} ...")
            result = subprocess.run(
                [
                    avdmanager,
                    "create",
                    "avd",
                    "--force",
                    "--name",
                    name,
                    "--abi",
                    abi,
                    "--package",
                    package,
                    "--device",
                    self.device_id,
                ],
                input="no\n",
                capture_output=True,
                text=True,
                env=env,
                timeout=60,
            )
            if result.returncode != 0:
                raise RuntimeError(f"AVD creation failed: {result.stderr}\n{result.stdout}")
            logger.info(f"AVD '{name}' created.")

        # Start ADB server
        subprocess.run([adb, "start-server"], capture_output=True, env=env)

        # Launch emulator
        cmd = [
            emulator,
            "-avd",
            name,
            "-gpu",
            self.gpu_mode,
            "-memory",
            str(self.memory_mb),
            "-cores",
            str(self.cpu_count),
            *(["-no-window"] if self.headless else []),
            "-no-snapshot",
            "-skip-adb-auth",
            "-port",
            str(self.adb_port - 1),  # console port = adb_port - 1
        ]
        if self.no_boot_anim:
            cmd.append("-no-boot-anim")

        logger.info(f"Starting Android emulator: {' '.join(cmd)}")
        self._proc = subprocess.Popen(
            cmd,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            env=env,
        )

        # Brief check that it didn't crash immediately
        await asyncio.sleep(3)
        if self._proc.poll() is not None:
            stderr = self._proc.stderr.read().decode() if self._proc.stderr else ""
            raise RuntimeError(f"Emulator crashed on launch: {stderr}")

        info = RuntimeInfo(
            host="localhost",
            api_port=self.adb_port,
            name=name,
            environment="android",
        )

        # Install APKs from image layers
        await self.is_ready(info)
        await self._apply_layers(image, adb, env)

        return info

    async def _apply_layers(self, image: Image, adb: str, env: dict) -> None:
        """Apply image layers post-boot (APK installs, shell commands, etc.)."""
        serial = f"emulator-{self.adb_port - 1}"
        for layer in image._layers:
            lt = layer["type"]
            if lt == "apk_install":
                for apk in layer["packages"]:
                    # Download URL-based APKs
                    local_apk = await self._resolve_apk(apk)
                    logger.info(f"Installing APK: {local_apk}")
                    result = subprocess.run(
                        [adb, "-s", serial, "install", "-r", str(local_apk)],
                        capture_output=True,
                        text=True,
                        env=env,
                        timeout=120,
                    )
                    if result.returncode != 0:
                        logger.warning(f"APK install failed: {result.stderr}")
                    else:
                        logger.info(f"APK installed: {result.stdout.strip()}")
            elif lt == "pwa_install":
                manifest_url = layer["manifest_url"]
                apk_path = await self._build_pwa_apk(manifest_url)
                if apk_path:
                    logger.info(f"Installing PWA APK: {apk_path}")
                    result = subprocess.run(
                        [adb, "-s", serial, "install", "-r", str(apk_path)],
                        capture_output=True,
                        text=True,
                        env=env,
                        timeout=120,
                    )
                    if result.returncode != 0:
                        logger.warning(f"PWA APK install failed: {result.stderr}")
                    else:
                        logger.info(f"PWA APK installed: {result.stdout.strip()}")
            elif lt == "run":
                cmd = layer["command"]
                logger.info(f"Running: {cmd}")
                subprocess.run(
                    [adb, "-s", serial, "shell", cmd],
                    capture_output=True,
                    text=True,
                    env=env,
                    timeout=60,
                )
            elif lt == "pip_install":
                # pip install via termux or similar — skip for now
                logger.warning("pip_install not supported on Android emulator, skipping")

    @staticmethod
    async def _build_pwa_apk(manifest_url: str) -> Optional[Path]:
        """Build a signed debug APK from a PWA manifest URL using Bubblewrap.

        Requires bubblewrap to be installed and initialised (run ``bubblewrap``
        once interactively so it can download the JDK/Android SDK and write
        ``~/.bubblewrap/config.json``).  If that config is missing or bubblewrap
        is not on PATH this method raises a RuntimeError with actionable
        instructions rather than silently skipping.
        """
        import json as _json

        bw = shutil.which("bubblewrap")
        if not bw:
            raise RuntimeError(
                "bubblewrap not found on PATH.\n"
                "Install it with:  npm install -g @bubblewrap/cli\n"
                "Then run it once interactively so it can set up the JDK and Android SDK:\n"
                "  mkdir /tmp/bw-init && cd /tmp/bw-init && bubblewrap init --manifest https://example.com/manifest.json"
            )

        bw_config = Path.home() / ".bubblewrap" / "config.json"
        if not bw_config.exists():
            raise RuntimeError(
                "Bubblewrap config not found at ~/.bubblewrap/config.json.\n"
                "Run bubblewrap interactively once to complete setup:\n"
                "  mkdir /tmp/bw-init && cd /tmp/bw-init && bubblewrap init --manifest https://example.com/manifest.json"
            )
        cfg = _json.loads(bw_config.read_text())
        if not cfg.get("jdkPath") or not cfg.get("androidSdkPath"):
            raise RuntimeError(
                "Bubblewrap config is incomplete (missing jdkPath or androidSdkPath).\n"
                "Re-run bubblewrap interactively to finish setup."
            )

        # Use a stable cache dir keyed on the manifest URL so we don't rebuild
        # every time.
        import hashlib

        url_hash = hashlib.sha256(manifest_url.encode()).hexdigest()[:12]
        cache_dir = Path.home() / ".cua" / "cua-sandbox" / "pwa-cache" / url_hash
        signed_apk = cache_dir / "app-release-signed.apk"
        if signed_apk.exists():
            logger.info(f"Using cached PWA APK: {signed_apk}")
            return signed_apk

        cache_dir.mkdir(parents=True, exist_ok=True)

        # Pre-create the debug keystore so bubblewrap build doesn't prompt.
        keystore = cache_dir / "android.keystore"
        if not keystore.exists():
            jdk_bin = Path(cfg["jdkPath"]) / "bin"
            # On macOS the JDK layout has bin under Contents/Home
            if not (jdk_bin / "keytool").exists():
                jdk_bin = Path(cfg["jdkPath"]) / "Contents" / "Home" / "bin"
            keytool = str(jdk_bin / "keytool") if (jdk_bin / "keytool").exists() else "keytool"
            subprocess.run(
                [
                    keytool,
                    "-genkeypair",
                    "-v",
                    "-keystore",
                    str(keystore),
                    "-alias",
                    "android",
                    "-keyalg",
                    "RSA",
                    "-keysize",
                    "2048",
                    "-validity",
                    "10000",
                    "-storepass",
                    "android",
                    "-keypass",
                    "android",
                    "-dname",
                    "CN=CUA PWA, OU=Dev, O=CUA, L=SF, S=CA, C=US",
                ],
                check=True,
                capture_output=True,
                timeout=30,
            )

        # Step 1: generate twa-manifest.json non-interactively via a small
        # Node.js helper that calls @bubblewrap/core directly.
        from urllib.parse import urlparse

        host = urlparse(manifest_url).hostname or ""
        parts = host.split(".")
        parts.reverse()
        package_id = ".".join(p for p in parts if p) or "com.cua.pwa"

        bw_init_js = Path(__file__).parent / "_bw_init.js"
        node = shutil.which("node")
        if not node:
            raise RuntimeError("node not found on PATH; required for pwa_install")

        logger.info(f"Generating twa-manifest.json for {manifest_url} …")
        init_result = subprocess.run(
            [node, str(bw_init_js), manifest_url, str(cache_dir), package_id],
            capture_output=True,
            text=True,
            timeout=60,
        )
        if init_result.returncode != 0:
            raise RuntimeError(f"_bw_init.js failed for {manifest_url}:\n{init_result.stderr}")

        # Step 2: bubblewrap update generates the Android project from twa-manifest.json.
        logger.info("Running bubblewrap update (generates Android project) …")
        update_result = subprocess.run(
            [bw, "update", "--skipPwaValidation"],
            capture_output=True,
            text=True,
            cwd=str(cache_dir),
            timeout=300,
        )
        if update_result.returncode != 0:
            raise RuntimeError(f"bubblewrap update failed:\n{update_result.stderr}")

        # Step 3: build the APK — pass keystore passwords via env vars so
        # bubblewrap doesn't open an interactive readline prompt.
        jdk_path = cfg["jdkPath"]
        bw_env = {
            **os.environ,
            "JAVA_HOME": jdk_path,
            "BUBBLEWRAP_KEYSTORE_PASSWORD": "android",
            "BUBBLEWRAP_KEY_PASSWORD": "android",
        }
        logger.info("Running bubblewrap build …")
        build_result = subprocess.run(
            [bw, "build"],
            capture_output=True,
            text=True,
            cwd=str(cache_dir),
            env=bw_env,
            timeout=600,
        )
        if build_result.returncode != 0:
            raise RuntimeError(f"bubblewrap build failed:\n{build_result.stderr}")

        # Bubblewrap outputs app-release-signed.apk
        apks = list(cache_dir.rglob("*.apk"))
        if not apks:
            raise RuntimeError(
                f"bubblewrap build succeeded but no .apk found in {cache_dir}\n"
                f"stdout: {build_result.stdout}"
            )
        logger.info(f"PWA APK built: {apks[0]}")
        return apks[0]

    @staticmethod
    async def _resolve_apk(apk_path: str) -> Path:
        """If apk_path is a URL, download it. Otherwise return as-is."""
        if apk_path.startswith(("http://", "https://")):
            import hashlib
            import urllib.request

            cache = Path.home() / ".cua" / "cua-sandbox" / "apk-cache"
            cache.mkdir(parents=True, exist_ok=True)
            filename = apk_path.rsplit("/", 1)[-1].split("?")[0]
            url_hash = hashlib.sha256(apk_path.encode()).hexdigest()[:8]
            local = cache / f"{url_hash}_{filename}"
            if not local.exists():
                logger.info(f"Downloading APK: {apk_path}")
                urllib.request.urlretrieve(apk_path, str(local))
            return local
        return Path(apk_path)

    async def is_ready(self, info: RuntimeInfo, timeout: float = 300) -> bool:
        """Wait for `sys.boot_completed == 1` via ADB, like docker-android."""
        sdk = self._sdk or _sdk_path()
        adb = _find_bin(sdk, "adb")
        serial = f"emulator-{self.adb_port - 1}"
        env = os.environ.copy()
        env["ANDROID_SDK_ROOT"] = str(sdk)

        logger.info(f"Waiting for Android boot (timeout={timeout}s) ...")

        # First wait for ADB device
        deadline = asyncio.get_event_loop().time() + timeout
        while asyncio.get_event_loop().time() < deadline:
            result = subprocess.run(
                [adb, "-s", serial, "shell", "getprop", "sys.boot_completed"],
                capture_output=True,
                text=True,
                env=env,
                timeout=10,
            )
            value = result.stdout.strip()
            if value == "1":
                logger.info(f"Android emulator {info.name} boot completed.")

                # Disable animations for faster interaction (docker-android pattern)
                for prop in [
                    "settings put global window_animation_scale 0.0",
                    "settings put global transition_animation_scale 0.0",
                    "settings put global animator_duration_scale 0.0",
                ]:
                    subprocess.run(
                        [adb, "-s", serial, "shell", prop],
                        capture_output=True,
                        env=env,
                    )

                return True
            await asyncio.sleep(5)

        raise TimeoutError(f"Android emulator {info.name} did not boot within {timeout}s")

    async def stop(self, name: str) -> None:
        if self._proc and self._proc.poll() is None:
            self._proc.terminate()
            try:
                self._proc.wait(timeout=15)
            except subprocess.TimeoutExpired:
                self._proc.kill()
            self._proc = None

        # Also try ADB kill
        try:
            sdk = self._sdk or _sdk_path()
            adb = _find_bin(sdk, "adb")
            subprocess.run(
                [adb, "-s", f"emulator-{self.adb_port - 1}", "emu", "kill"],
                capture_output=True,
                timeout=10,
            )
        except Exception:
            pass

    async def take_screenshot(self) -> bytes:
        """Take a screenshot via ADB screencap."""
        sdk = self._sdk or _sdk_path()
        adb = _find_bin(sdk, "adb")
        serial = f"emulator-{self.adb_port - 1}"

        # screencap to file, then pull
        subprocess.run(
            [adb, "-s", serial, "shell", "screencap", "-p", "/sdcard/screenshot.png"],
            capture_output=True,
            timeout=15,
        )
        result = subprocess.run(
            [adb, "-s", serial, "exec-out", "cat", "/sdcard/screenshot.png"],
            capture_output=True,
            timeout=15,
        )
        if result.returncode == 0 and result.stdout[:4] == b"\x89PNG":
            return result.stdout
        raise RuntimeError("Failed to take screenshot via ADB")
