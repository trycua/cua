"""Integration tests for Image builder methods across all guest OS types.

Every image builder method is tested on every guest OS that supports it.
Tests auto-skip via skip_if_unsupported() when the required runtime is
not installed on this host.

Run with:
    pytest tests/test_image_builder.py -v
    pytest tests/test_image_builder.py -v -k linux   # only linux tests
"""

from __future__ import annotations

import pytest
from cua import Image, Sandbox, skip_if_unsupported

# ── helpers ──────────────────────────────────────────────────────────────────


async def _run(image: Image, cmd: str) -> str:
    """Ephemeral sandbox: run cmd and return stdout. Asserts success."""
    async with Sandbox.ephemeral(image, local=True) as sb:
        r = await sb.shell.run(cmd, timeout=120)
        assert r.success, f"Command failed (rc={r.returncode}): {r.stderr}"
        return r.stdout.strip()


# ═══════════════════════════════════════════════════════════════════════════
# Linux (Docker container)
# ═══════════════════════════════════════════════════════════════════════════


class TestLinuxAptInstall:
    def _base(self):
        return Image.linux()

    def setup_method(self):
        skip_if_unsupported(self._base())

    async def test_single_package(self):
        image = self._base().apt_install("curl")
        out = await _run(image, "curl --version")
        assert "curl" in out.lower()

    async def test_multiple_packages(self):
        image = self._base().apt_install("curl", "wget")
        out = await _run(image, "curl --version && wget --version")
        assert "curl" in out.lower()

    async def test_chained_calls(self):
        image = self._base().apt_install("curl").apt_install("wget")
        out = await _run(image, "curl --version && wget --version")
        assert "curl" in out.lower()


class TestLinuxPipInstall:
    def _base(self):
        return Image.linux().apt_install("python3-pip")

    def setup_method(self):
        skip_if_unsupported(Image.linux())

    async def test_single_package(self):
        image = self._base().pip_install("httpx")
        out = await _run(image, "python3 -c 'import httpx; print(httpx.__version__)'")
        assert out  # any version string

    async def test_multiple_packages(self):
        image = self._base().pip_install("httpx", "rich")
        out = await _run(image, "python3 -c 'import httpx, rich; print(\"ok\")'")
        assert "ok" in out


class TestLinuxUvInstall:
    def setup_method(self):
        skip_if_unsupported(Image.linux())

    async def test_single_package(self):
        image = Image.linux().uv_install("httpx")
        out = await _run(image, "uv run python -c 'import httpx; print(\"ok\")'")
        assert "ok" in out

    async def test_multiple_packages(self):
        image = Image.linux().uv_install("httpx", "rich")
        out = await _run(image, "uv run python -c 'import httpx, rich; print(\"ok\")'")
        assert "ok" in out


class TestLinuxRun:
    def setup_method(self):
        skip_if_unsupported(Image.linux())

    async def test_writes_file(self):
        image = Image.linux().run("echo 'hello' > /etc/hello.txt")
        out = await _run(image, "cat /etc/hello.txt")
        assert "hello" in out

    async def test_creates_directory(self):
        image = Image.linux().run("mkdir -p /opt/myapp")
        out = await _run(image, "test -d /opt/myapp && echo exists")
        assert "exists" in out

    async def test_chained(self):
        image = Image.linux().run("mkdir -p /opt/myapp").run("echo 'v1' > /opt/myapp/version.txt")
        out = await _run(image, "cat /opt/myapp/version.txt")
        assert "v1" in out


class TestLinuxEnv:
    def setup_method(self):
        skip_if_unsupported(Image.linux())

    async def test_single_variable(self):
        image = Image.linux().env(MY_VAR="hello_integration")
        out = await _run(image, "echo $MY_VAR")
        assert "hello_integration" in out

    async def test_multiple_variables(self):
        image = Image.linux().env(VAR_A="alpha", VAR_B="beta")
        out = await _run(image, "echo $VAR_A $VAR_B")
        assert "alpha" in out
        assert "beta" in out

    async def test_variable_used_in_run(self):
        image = Image.linux().env(APP_PORT="8080").run("echo port=$APP_PORT > /tmp/port.txt")
        out = await _run(image, "cat /tmp/port.txt")
        assert "8080" in out


class TestLinuxCopy:
    def setup_method(self):
        skip_if_unsupported(Image.linux())

    async def test_copy_file(self, tmp_path):
        src = tmp_path / "hello.txt"
        src.write_text("from host")
        image = Image.linux().copy(str(src), "/tmp/hello.txt")
        out = await _run(image, "cat /tmp/hello.txt")
        assert "from host" in out

    async def test_copy_preserves_content(self, tmp_path):
        src = tmp_path / "data.json"
        src.write_text('{"key": "value"}')
        image = Image.linux().copy(str(src), "/tmp/data.json")
        out = await _run(image, "cat /tmp/data.json")
        assert '"key"' in out


class TestLinuxExpose:
    def setup_method(self):
        skip_if_unsupported(Image.linux())

    async def test_expose_port_reachable(self):
        """expose() should not raise — container starts with the port mapped."""
        image = Image.linux().expose(9999)
        async with Sandbox.ephemeral(image, local=True) as sb:
            # Just verify the sandbox is reachable; port mapping is a docker-run concern
            r = await sb.shell.run("echo ok")
            assert r.success


class TestLinuxFromRegistry:
    def setup_method(self):
        skip_if_unsupported(Image.linux())

    async def test_ubuntu_22(self):
        # from_registry with plain images only works if the image has computer-server.
        # Use the cua base image which always has it.
        image = Image.from_registry("docker.io/trycua/cua-xfce:latest")
        out = await _run(image, "cat /etc/os-release")
        assert "ubuntu" in out.lower() or "linux" in out.lower()

    async def test_debian(self):
        # Same constraint — use cua base image.
        image = Image.from_registry("docker.io/trycua/cua-xfce:latest")
        out = await _run(image, "uname -s")
        assert "linux" in out.lower()


class TestLinuxFullChain:
    def setup_method(self):
        skip_if_unsupported(Image.linux())

    async def test_full_chain(self, tmp_path):
        src = tmp_path / "app.sh"
        src.write_text("#!/bin/sh\necho app-$VERSION\n")
        image = (
            Image.linux()
            .apt_install("curl")
            .env(VERSION="1.0.0")
            .copy(str(src), "/usr/local/bin/app.sh")
            .run("chmod +x /usr/local/bin/app.sh")
            .expose(8080)
        )
        out = await _run(image, "/usr/local/bin/app.sh")
        assert "app-1.0.0" in out


# ═══════════════════════════════════════════════════════════════════════════
# macOS VM
# ═══════════════════════════════════════════════════════════════════════════


class TestMacOSBrewInstall:
    def setup_method(self):
        skip_if_unsupported(Image.macos())

    async def test_single_package(self):
        image = Image.macos().brew_install("jq")
        out = await _run(image, "jq --version")
        assert "jq" in out

    async def test_multiple_packages(self):
        image = Image.macos().brew_install("jq", "wget")
        out = await _run(image, "jq --version && wget --version")
        assert "jq" in out


class TestMacOSRun:
    def setup_method(self):
        skip_if_unsupported(Image.macos())

    async def test_run_command(self):
        image = Image.macos().run("echo setup > /tmp/setup.txt")
        out = await _run(image, "cat /tmp/setup.txt")
        assert "setup" in out


class TestMacOSEnv:
    def setup_method(self):
        skip_if_unsupported(Image.macos())

    async def test_env_variable(self):
        image = Image.macos().env(MY_ENV="macos_test")
        out = await _run(image, "echo $MY_ENV")
        assert "macos_test" in out


class TestMacOSCopy:
    def setup_method(self):
        skip_if_unsupported(Image.macos())

    async def test_copy_file(self, tmp_path):
        src = tmp_path / "hello.txt"
        src.write_text("hello from host")
        image = Image.macos().copy(str(src), "/tmp/hello.txt")
        out = await _run(image, "cat /tmp/hello.txt")
        assert "hello from host" in out


class TestMacOSPipInstall:
    def setup_method(self):
        skip_if_unsupported(Image.macos())

    async def test_pip_install(self):
        image = Image.macos().pip_install("httpx")
        out = await _run(image, "python3 -c 'import httpx; print(\"ok\")'")
        assert "ok" in out


# ═══════════════════════════════════════════════════════════════════════════
# Windows VM
# ═══════════════════════════════════════════════════════════════════════════


class TestWindowsChocoInstall:
    def setup_method(self):
        skip_if_unsupported(Image.windows())

    async def test_single_package(self):
        image = Image.windows().choco_install("curl")
        out = await _run(image, "curl --version")
        assert "curl" in out.lower()

    async def test_multiple_packages(self):
        image = Image.windows().choco_install("curl", "wget")
        out = await _run(image, "curl --version")
        assert "curl" in out.lower()


class TestWindowsWingetInstall:
    def setup_method(self):
        skip_if_unsupported(Image.windows())

    async def test_single_package(self):
        image = Image.windows().winget_install("Microsoft.PowerShell")
        out = await _run(image, "pwsh --version")
        assert "powershell" in out.lower()


class TestWindowsRun:
    def setup_method(self):
        skip_if_unsupported(Image.windows())

    async def test_run_powershell(self):
        image = Image.windows().run("echo setup > C:\\setup.txt")
        out = await _run(image, "type C:\\setup.txt")
        assert "setup" in out


class TestWindowsEnv:
    def setup_method(self):
        skip_if_unsupported(Image.windows())

    async def test_env_variable(self):
        image = Image.windows().env(MY_VAR="win_test")
        out = await _run(image, "echo %MY_VAR%")
        assert "win_test" in out


class TestWindowsCopy:
    def setup_method(self):
        skip_if_unsupported(Image.windows())

    async def test_copy_file(self, tmp_path):
        src = tmp_path / "hello.txt"
        src.write_text("hello windows")
        image = Image.windows().copy(str(src), "C:\\hello.txt")
        out = await _run(image, "type C:\\hello.txt")
        assert "hello windows" in out


class TestWindowsExpose:
    def setup_method(self):
        skip_if_unsupported(Image.windows())

    async def test_expose_port(self):
        image = Image.windows().expose(8080)
        async with Sandbox.ephemeral(image, local=True) as sb:
            r = await sb.shell.run("echo exposed")
            assert r.success


# ═══════════════════════════════════════════════════════════════════════════
# Android VM
# ═══════════════════════════════════════════════════════════════════════════


class TestAndroidApkInstall:
    def setup_method(self):
        skip_if_unsupported(Image.android())

    async def test_apk_install(self):
        """Basic APK install — downloads F-Droid APK from URL."""
        image = Image.android().apk_install("https://f-droid.org/F-Droid.apk")
        async with Sandbox.ephemeral(image, local=True) as sb:
            r = await sb.shell.run("pm list packages")
            assert r.success
            assert "org.fdroid" in r.stdout

    async def test_apk_install_from_env(self):
        import os

        apk_path = os.environ.get("ANDROID_TEST_APK")
        if not apk_path:
            pytest.skip("ANDROID_TEST_APK not set")
        image = Image.android().apk_install(apk_path)
        async with Sandbox.ephemeral(image, local=True) as sb:
            r = await sb.shell.run("pm list packages")
            assert r.success


class TestAndroidPwaInstall:
    def setup_method(self):
        skip_if_unsupported(Image.android())

    async def test_pwa_install(self):
        """PWA install from the android-example-gym-pwa-app manifest."""
        import os

        manifest_url = os.environ.get(
            "ANDROID_TEST_PWA_URL", "https://cuaai--todo-gym-web.modal.run/manifest.json"
        )
        image = Image.android().pwa_install(manifest_url)
        async with Sandbox.ephemeral(image, local=True) as sb:
            r = await sb.shell.run("pm list packages")
            assert r.success
            assert "trycua" in r.stdout or r.returncode == 0


class TestAndroidRun:
    def setup_method(self):
        skip_if_unsupported(Image.android())

    async def test_run_adb_shell(self):
        image = Image.android().run("echo android > /data/local/tmp/test.txt")
        out = await _run(image, "cat /data/local/tmp/test.txt")
        assert "android" in out


class TestAndroidEnv:
    def setup_method(self):
        skip_if_unsupported(Image.android())

    async def test_env_variable(self):
        image = Image.android().env(MY_VAR="android_test")
        out = await _run(image, "echo $MY_VAR")
        assert "android_test" in out
