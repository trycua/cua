"""Build a WebView APK from a PWA manifest with pwa2apk (host-side, no sandbox)."""

from __future__ import annotations

import logging

from cua_sandbox._paths import cua_home

logger = logging.getLogger(__name__)


async def build_pwa2apk(
    manifest_url: str,
    package_name: str | None = None,
    keystore_path: str | None = None,
    keystore_alias: str = "android",
    keystore_password: str = "android",
) -> tuple:
    """Build a WebView-based APK using pwa2apk (no Chrome dependency)."""
    import shutil
    import subprocess

    node = shutil.which("node")
    if not node:
        raise RuntimeError("node not found on PATH; required for pwa2apk")

    # Find pwa2apk — check common locations
    pwa2apk_cli = None
    for candidate in [
        shutil.which("pwa2apk"),
        # npm global
        *([] if not shutil.which("npm") else []),
    ]:
        if candidate:
            pwa2apk_cli = candidate
            break

    # Fall back to requiring it as a node module
    if not pwa2apk_cli:
        # Try npx
        npx = shutil.which("npx")
        if npx:
            pwa2apk_cli = npx

    # Build via the pwa2apk Node API directly
    import hashlib
    from pathlib import Path

    # Check if pwa2apk is installed globally or locally
    pwa2apk_dir = None
    for p in [
        cua_home() / "pwa2apk",
        Path("/tmp/pwa2apk"),
    ]:
        if (p / "src" / "index.js").exists():
            pwa2apk_dir = p
            break

    if not pwa2apk_dir:
        # Auto-clone pwa2apk
        logger.info("Cloning pwa2apk...")
        pwa2apk_dir = cua_home() / "pwa2apk"
        pwa2apk_dir.mkdir(parents=True, exist_ok=True)
        clone_result = subprocess.run(
            ["git", "clone", "https://github.com/trycua/pwa2apk.git", str(pwa2apk_dir)],
            capture_output=True,
            text=True,
            timeout=60,
        )
        if clone_result.returncode != 0:
            raise RuntimeError(f"Failed to clone pwa2apk: {clone_result.stderr}")

    # Build the args for the CLI
    import os

    cache_key = hashlib.sha256(f"{manifest_url}|{package_name or ''}".encode()).hexdigest()[:12]
    output_apk = cua_home() / "pwa2apk-cache" / f"{cache_key}.apk"
    output_apk.parent.mkdir(parents=True, exist_ok=True)

    cmd = [
        node,
        str(pwa2apk_dir / "src" / "cli.js"),
        manifest_url,
        "--output",
        str(output_apk),
    ]
    if package_name:
        cmd.extend(["--package", package_name])
    if keystore_path:
        cmd.extend(["--keystore", str(keystore_path)])
        cmd.extend(["--keystore-alias", keystore_alias])
        cmd.extend(["--keystore-password", keystore_password])

    env = {**os.environ}
    if "JAVA_HOME" not in env:
        for jdk in [
            # Linux
            "/usr/lib/jvm/java-17-openjdk-amd64",
            "/usr/lib/jvm/java-21-openjdk-amd64",
            # macOS (Homebrew ARM) — prefer @17/@21 over unversioned (may be JDK 25+)
            "/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home",
            "/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home",
            "/opt/homebrew/opt/openjdk/libexec/openjdk.jdk/Contents/Home",
            # macOS (Homebrew Intel)
            "/usr/local/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home",
            "/usr/local/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home",
            "/usr/local/opt/openjdk/libexec/openjdk.jdk/Contents/Home",
        ]:
            if Path(jdk).exists():
                env["JAVA_HOME"] = jdk
                break

    logger.info(f"Building APK with pwa2apk: {manifest_url}")
    result = subprocess.run(
        cmd,
        capture_output=True,
        text=True,
        env=env,
        timeout=300,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"pwa2apk build failed:\nstdout: {result.stdout}\nstderr: {result.stderr}"
        )

    # Extract fingerprint from output
    fingerprint = ""
    for line in result.stdout.splitlines():
        if "SHA-256:" in line:
            fingerprint = line.split("SHA-256:", 1)[1].strip()
            break

    if not output_apk.exists():
        raise RuntimeError(f"pwa2apk did not produce APK at {output_apk}")

    return output_apk, fingerprint
