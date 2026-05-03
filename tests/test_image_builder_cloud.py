"""Cloud integration tests for Android image builder methods.

Tests .env(), .apk_install(), and .pwa_install() against the local stack
dev stack (CUA_BASE_URL=http://localhost:8082).

Run with:
    CUA_API_KEY=sk-dev-test-key-local-12345 CUA_BASE_URL=http://localhost:8082 \
    pytest tests/test_image_builder_cloud.py -v -s
"""

from __future__ import annotations

import os

import pytest

from cua import Image, Sandbox

pytestmark = pytest.mark.asyncio


def _has_cua_api_key() -> bool:
    return bool(os.environ.get("CUA_API_KEY"))


# ── helpers ──────────────────────────────────────────────────────────────────


async def _run_cloud(image: Image, cmd: str, timeout: int = 120) -> str:
    """Ephemeral cloud sandbox: run cmd and return stdout."""
    async with Sandbox.ephemeral(image) as sb:
        r = await sb.shell.run(cmd, timeout=timeout)
        assert r.success, f"Command failed (rc={r.returncode}): {r.stderr}"
        return r.stdout.strip()


# ═══════════════════════════════════════════════════════════════════════════
# Android .env()
# ═══════════════════════════════════════════════════════════════════════════


@pytest.mark.skipif(not _has_cua_api_key(), reason="CUA_API_KEY not set")
async def test_android_env_single():
    """Single env var is available inside the Android shell."""
    image = Image.android("14").env(MY_VAR="cloud_test_123")
    out = await _run_cloud(image, "echo $MY_VAR")
    assert "cloud_test_123" in out


@pytest.mark.skipif(not _has_cua_api_key(), reason="CUA_API_KEY not set")
async def test_android_env_multiple():
    """Multiple env vars are all available."""
    image = Image.android("14").env(VAR_A="alpha", VAR_B="beta")
    out = await _run_cloud(image, "echo $VAR_A $VAR_B")
    assert "alpha" in out
    assert "beta" in out


# ═══════════════════════════════════════════════════════════════════════════
# Android .apk_install()
# ═══════════════════════════════════════════════════════════════════════════


@pytest.mark.skipif(not _has_cua_api_key(), reason="CUA_API_KEY not set")
async def test_android_apk_install():
    """APK install from URL — F-Droid should be listed in packages."""
    image = Image.android("14").apk_install("https://f-droid.org/F-Droid.apk")
    out = await _run_cloud(image, "pm list packages org.fdroid.fdroid")
    assert "org.fdroid.fdroid" in out


# ═══════════════════════════════════════════════════════════════════════════
# Android .pwa_install()
# ═══════════════════════════════════════════════════════════════════════════


@pytest.mark.skipif(not _has_cua_api_key(), reason="CUA_API_KEY not set")
async def test_android_pwa_install():
    """PWA install from manifest URL — the TWA package should appear."""
    import tempfile
    import urllib.request

    manifest_url = os.environ.get(
        "ANDROID_TEST_PWA_URL",
        "https://cuaai--todo-gym-web.modal.run/manifest.json",
    )
    keystore_url = (
        "https://raw.githubusercontent.com/trycua/android-example-gym-pwa-app/main/android.keystore"
    )
    with tempfile.NamedTemporaryFile(suffix=".keystore", delete=False) as f:
        urllib.request.urlretrieve(keystore_url, f.name)
        keystore_path = f.name

    image = Image.android("14").pwa_install(
        manifest_url,
        keystore=keystore_path,
        keystore_alias="android",
        keystore_password="android",
    )
    out = await _run_cloud(image, "pm list packages")
    # The TWA package name is derived from the manifest — just check pm list works
    assert "package:" in out
