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
        result = subprocess.run(["java", "-version"], capture_output=True, timeout=5)
        if result.returncode == 0:
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
        Path(os.environ.get("LOCALAPPDATA", "")) / "Android" / "Sdk",  # Android Studio (Windows)
        Path.home() / "AppData" / "Local" / "Android" / "Sdk",  # Windows fallback
    ]
    for p in common:
        if (p / "emulator").exists():
            return p
    return _SDK_ROOT


def _ensure_sdk() -> Path:
    """Ensure Android SDK command-line tools, emulator, and platform-tools are installed."""
    sdk = _sdk_path()

    _win = _plat.system().lower() == "windows"
    _ext = ".exe" if _win else ""
    _bat = ".bat" if _win else ""
    emulator_bin = sdk / "emulator" / f"emulator{_ext}"
    sdkmanager_bin = sdk / "cmdline-tools" / "latest" / "bin" / f"sdkmanager{_bat}"
    adb_bin = sdk / "platform-tools" / f"adb{_ext}"

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
        elif system == "windows":
            tools_url = f"https://dl.google.com/android/repository/commandlinetools-win-{_CMDLINE_TOOLS_VERSION}_latest.zip"
        else:
            raise RuntimeError(f"Android SDK auto-install not supported on {system}.")

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

    sdkmanager_bin = sdk / "cmdline-tools" / "latest" / "bin" / f"sdkmanager{_bat}"
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
    """Find a binary in the SDK, handling .exe/.bat on Windows."""
    exts = ["", ".exe", ".bat"] if _plat.system().lower() == "windows" else [""]
    dirs = [
        sdk / "emulator",
        sdk / "platform-tools",
        sdk / "cmdline-tools" / "latest" / "bin",
    ]
    for d in dirs:
        for ext in exts:
            c = d / f"{name}{ext}"
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
        grpc_port: Optional[int] = None,
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
        self.grpc_port = grpc_port
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

        # gRPC service: defaults to console_port+3000 if not specified
        resolved_grpc_port = self.grpc_port or (self.adb_port - 1 + 3000)
        cmd += ["-grpc", str(resolved_grpc_port)]

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
            grpc_port=resolved_grpc_port,
        )

        # Install APKs from image layers
        await self.is_ready(info)
        await self._apply_layers(image, adb, env)

        return info

    async def _apply_layers(self, image: Image, adb: str, env: dict) -> None:
        """Apply image layers post-boot (APK installs, shell commands, etc.)."""
        serial = f"emulator-{self.adb_port - 1}"

        # Write image env vars to a persistent file so every subsequent
        # adb shell command (including those from the transport) can source it.
        if image._env:
            import re
            import shlex
            import tempfile

            lines = []
            for k, v in image._env:
                if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", k):
                    raise ValueError(f"Invalid environment variable name: {k!r}")
                lines.append(f"export {k}={shlex.quote(v)}")
            exports = "\n".join(lines) + "\n"
            with tempfile.NamedTemporaryFile(mode="w", suffix=".sh", delete=False) as f:
                f.write(exports)
                tmp_path = f.name
            subprocess.run(
                [adb, "-s", serial, "push", tmp_path, "/data/local/tmp/.cua_env"],
                capture_output=True,
                env=env,
                timeout=10,
            )
            Path(tmp_path).unlink(missing_ok=True)

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
                pkg = layer.get("package_name")
                ks = Path(layer["keystore"]) if layer.get("keystore") else None
                ks_alias = layer.get("keystore_alias", "android")
                ks_pass = layer.get("keystore_password", "android")
                apk_path, fingerprint = await self._build_pwa_apk(
                    manifest_url, pkg, ks, ks_alias, ks_pass
                )
                logger.info(
                    f"PWA APK signing fingerprint (SHA-256): {fingerprint}\n"
                    "  → Set TWA_SHA256_FINGERPRINT env var on your server to this value\n"
                    f"    so /.well-known/assetlinks.json is trusted by Chrome."
                )
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
                # Suppress Chrome first-run wizard so the TWA opens immediately.
                # set-debug-app makes Chrome honour the chrome-command-line file.
                for cmd in [
                    "am set-debug-app --persistent com.android.chrome",
                    "mkdir -p /data/local/tmp && "
                    "echo 'chrome --no-first-run --disable-fre "
                    "--no-default-browser-check' "
                    "> /data/local/tmp/chrome-command-line",
                ]:
                    subprocess.run(
                        [adb, "-s", serial, "shell", cmd],
                        capture_output=True,
                        env=env,
                        timeout=10,
                    )
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
    async def _build_pwa_apk(
        manifest_url: str,
        package_name: Optional[str] = None,
        keystore_path: Optional[Path] = None,
        keystore_alias: str = "android",
        keystore_password: str = "android",
    ) -> tuple[Path, str]:
        """Build a signed APK from a PWA manifest URL using Bubblewrap.

        Auto-installs bubblewrap if not on PATH and auto-configures
        ``~/.bubblewrap/config.json`` from the already-known JDK and SDK paths,
        so no manual interactive setup is required.

        Returns ``(apk_path, sha256_fingerprint)`` where ``sha256_fingerprint``
        is the colon-separated hex digest of the signing certificate — ready to
        embed in the server's ``/.well-known/assetlinks.json``.
        """
        import hashlib
        import json as _json
        from urllib.parse import urlparse

        env = _java_env()
        java_home = env.get("JAVA_HOME", "")
        # Prefer Java 21 for Gradle compatibility (bubblewrap uses Gradle 8.x)
        for _j21 in (
            Path("/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home"),
            Path("/usr/local/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home"),
        ):
            if _j21.exists():
                java_home = str(_j21)
                env["JAVA_HOME"] = java_home
                env["PATH"] = f"{_j21 / 'bin'}:{env.get('PATH', '')}"
                break
        jdk_bin = Path(java_home) / "bin" if java_home else Path(".")
        keytool = str(jdk_bin / "keytool") if (jdk_bin / "keytool").exists() else "keytool"

        # ── 1. Ensure bubblewrap CLI is on PATH ──────────────────────────────
        # Augment PATH with common Homebrew node locations so shutil.which finds them
        # even when the process was launched without a login shell.
        extra_node_paths = ["/opt/homebrew/bin", "/usr/local/bin"]
        augmented_path = os.pathsep.join(extra_node_paths) + os.pathsep + env.get("PATH", "")
        env["PATH"] = augmented_path

        bw = shutil.which("bubblewrap", path=augmented_path)
        if not bw:
            node = shutil.which("node", path=augmented_path)
            npm = shutil.which("npm", path=augmented_path)
            if not node or not npm:
                raise RuntimeError(
                    "node/npm not found on PATH; required for pwa_install.\n"
                    "Install Node.js: https://nodejs.org/"
                )
            logger.info("Installing @bubblewrap/cli …")
            subprocess.run(
                [npm, "install", "-g", "@bubblewrap/cli"],
                check=True,
                capture_output=True,
                timeout=300,
            )
            bw = shutil.which("bubblewrap")
            if not bw:
                raise RuntimeError(
                    "bubblewrap still not found after npm install -g @bubblewrap/cli.\n"
                    "Ensure npm global bin is on PATH."
                )

        # ── 2. Auto-configure ~/.bubblewrap/config.json ──────────────────────
        # bubblewrap expects jdkPath to be the .jdk bundle root on macOS
        # (it appends Contents/Home internally).  Gradle also requires Java ≤ 21.
        def _bubblewrap_jdk_path() -> str:
            """Return the .jdk bundle path suitable for bubblewrap's jdkPath.

            Prefers openjdk@21 (Gradle-compatible) over newer versions.
            On macOS returns the .jdk bundle root; elsewhere the JDK home.
            """
            candidates = [
                # Prefer Java 21 — Gradle 8.x supports up to Java 21
                Path("/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk"),
                Path("/usr/local/opt/openjdk@21/libexec/openjdk.jdk"),
                Path("/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk"),
                # Fall back to whatever _java_env() found
                Path(java_home).parent.parent if java_home.endswith("Contents/Home") else None,
                Path(java_home) if java_home else None,
            ]
            for c in candidates:
                if c and c.exists():
                    return str(c)
            return java_home  # last resort

        bw_config = Path.home() / ".bubblewrap" / "config.json"
        bw_config.parent.mkdir(parents=True, exist_ok=True)
        if not bw_config.exists() or not _json.loads(bw_config.read_text()).get("jdkPath"):
            sdk = _sdk_path()
            cfg = {
                "jdkPath": _bubblewrap_jdk_path(),
                "androidSdkPath": str(sdk),
            }
            bw_config.write_text(_json.dumps(cfg, indent=2))
            logger.info(f"Wrote ~/.bubblewrap/config.json: jdkPath={cfg['jdkPath']}")
        else:
            cfg = _json.loads(bw_config.read_text())
            # Patch androidSdkPath if it no longer exists
            if not Path(cfg.get("androidSdkPath", "")).exists():
                cfg["androidSdkPath"] = str(_sdk_path())
                bw_config.write_text(_json.dumps(cfg, indent=2))

        # ── 2b. Ensure build-tools + tools stub (both required by bubblewrap) ─
        sdk = Path(cfg["androidSdkPath"])
        sdkmanager = sdk / "cmdline-tools" / "latest" / "bin" / "sdkmanager"
        if sdkmanager.exists() and not any(
            (sdk / "build-tools").iterdir() if (sdk / "build-tools").exists() else []
        ):
            logger.info("Installing Android build-tools (required by bubblewrap) …")
            subprocess.run(
                [str(sdkmanager), f"--sdk_root={sdk}", "build-tools;34.0.0"],
                input=b"y\n",
                capture_output=True,
                timeout=300,
                env=env,
            )
        # bubblewrap validates SDK by checking for tools/ or bin/ at sdk root
        (sdk / "tools").mkdir(exist_ok=True)

        # ── 3. Determine package ID and cache directory ──────────────────────
        if not package_name:
            import re

            host = urlparse(manifest_url).hostname or ""
            parts = [p for p in reversed(host.split(".")) if p]
            # Sanitize each part: replace non-alphanumeric/underscore chars with _,
            # prefix with _ if starts with a digit (invalid Java identifier start).
            sanitized = []
            for part in parts:
                part = re.sub(r"[^a-zA-Z0-9_]", "_", part)
                if part and part[0].isdigit():
                    part = "_" + part
                if part:
                    sanitized.append(part)
            package_name = ".".join(sanitized) if sanitized else "com.cua.pwa"

        cache_key = hashlib.sha256(f"{manifest_url}|{package_name}".encode()).hexdigest()[:12]
        cache_dir = Path.home() / ".cua" / "cua-sandbox" / "pwa-cache" / cache_key
        fingerprint_file = cache_dir / "sha256.fingerprint"
        signed_apk = cache_dir / "app-release-signed.apk"

        if signed_apk.exists() and fingerprint_file.exists():
            fingerprint = fingerprint_file.read_text().strip()
            logger.info(f"Using cached PWA APK: {signed_apk}  fingerprint: {fingerprint}")
            return signed_apk, fingerprint

        cache_dir.mkdir(parents=True, exist_ok=True)

        # ── 4. Resolve keystore ──────────────────────────────────────────────
        if keystore_path:
            keystore = Path(keystore_path)
            if not keystore.exists():
                raise RuntimeError(f"Keystore not found: {keystore}")
        else:
            keystore = cache_dir / "android.keystore"
            if not keystore.exists():
                logger.info("Generating signing keystore …")
                subprocess.run(
                    [
                        keytool,
                        "-genkeypair",
                        "-v",
                        "-keystore",
                        str(keystore),
                        "-alias",
                        keystore_alias,
                        "-keyalg",
                        "RSA",
                        "-keysize",
                        "2048",
                        "-validity",
                        "10000",
                        "-storepass",
                        keystore_password,
                        "-keypass",
                        keystore_password,
                        "-dname",
                        "CN=CUA PWA, OU=Dev, O=CUA, L=SF, S=CA, C=US",
                    ],
                    check=True,
                    capture_output=True,
                    timeout=30,
                )

        # ── 5. Extract SHA-256 fingerprint from the keystore ─────────────────
        fp_result = subprocess.run(
            [
                keytool,
                "-list",
                "-v",
                "-keystore",
                str(keystore),
                "-alias",
                keystore_alias,
                "-storepass",
                keystore_password,
            ],
            capture_output=True,
            text=True,
            timeout=15,
        )
        fingerprint = ""
        for line in fp_result.stdout.splitlines():
            if "SHA256:" in line:
                fingerprint = line.split("SHA256:", 1)[1].strip()
                break
        if not fingerprint:
            raise RuntimeError(
                f"Could not extract SHA-256 fingerprint from keystore.\nkeytool output:\n{fp_result.stdout}"
            )
        fingerprint_file.write_text(fingerprint)
        logger.info(f"Keystore SHA-256 fingerprint: {fingerprint}")

        # ── 6. Generate twa-manifest.json via _bw_init.js ────────────────────
        bw_init_js = Path(__file__).parent / "_bw_init.js"
        node = shutil.which("node", path=augmented_path)
        if not node:
            raise RuntimeError("node not found on PATH; required for pwa_install")

        logger.info(f"Generating twa-manifest.json for {manifest_url} …")
        init_result = subprocess.run(
            [
                node,
                str(bw_init_js),
                manifest_url,
                str(cache_dir),
                package_name,
                str(keystore),
                keystore_alias,
                keystore_password,
            ],
            capture_output=True,
            text=True,
            env=env,
            timeout=60,
        )
        if init_result.returncode != 0:
            raise RuntimeError(f"_bw_init.js failed for {manifest_url}:\n{init_result.stderr}")

        # ── 7. bubblewrap update → Android project ───────────────────────────
        logger.info("Running bubblewrap update (generates Android project) …")
        update_result = subprocess.run(
            [bw, "update", "--skipPwaValidation"],
            capture_output=True,
            text=True,
            cwd=str(cache_dir),
            env=env,
            timeout=300,
        )
        if update_result.returncode != 0:
            raise RuntimeError(f"bubblewrap update failed:\n{update_result.stderr}")

        # ── 8. bubblewrap build → signed APK ─────────────────────────────────
        # JAVA_HOME for gradle must point to Contents/Home, not the .jdk bundle.
        jdk_bundle = Path(cfg.get("jdkPath", java_home))
        contents_home = jdk_bundle / "Contents" / "Home"
        gradle_java_home = str(contents_home) if contents_home.exists() else str(jdk_bundle)
        bw_env = {
            **env,
            "JAVA_HOME": gradle_java_home,
            "BUBBLEWRAP_KEYSTORE_PASSWORD": keystore_password,
            "BUBBLEWRAP_KEY_PASSWORD": keystore_password,
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

        apks = list(cache_dir.rglob("*.apk"))
        if not apks:
            raise RuntimeError(
                f"bubblewrap build succeeded but no .apk found in {cache_dir}\n"
                f"stdout: {build_result.stdout}"
            )
        logger.info(f"PWA APK built: {apks[0]}")
        return apks[0], fingerprint

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

    async def list(self) -> list[dict]:
        """List known Android emulator sandboxes from state files, checking if alive."""
        import subprocess

        from cua_sandbox import sandbox_state

        states = [s for s in sandbox_state.list_all() if s.get("runtime_type") == "androidemulator"]
        result = []
        for s in states:
            name = s["name"]
            status = s.get("status", "unknown")
            if status == "running":
                try:
                    alive = (
                        subprocess.run(
                            ["pgrep", "-f", f"qemu.*-avd {name}"],
                            capture_output=True,
                        ).returncode
                        == 0
                    )
                    if not alive:
                        status = "stopped"
                        sandbox_state.update(name, status="stopped")
                except FileNotFoundError:
                    pass
            result.append(
                {
                    "name": name,
                    "status": status,
                    "runtime_type": "androidemulator",
                    "host": s.get("host"),
                    "api_port": s.get("api_port"),
                    "os_type": s.get("os_type"),
                }
            )
        return result

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
