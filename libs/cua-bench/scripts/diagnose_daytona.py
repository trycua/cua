"""Diagnostic script to understand why aiohttp /cmd calls fail from subprocess context."""

import asyncio
import os
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

# Load .env
env_file = Path(__file__).parent.parent / ".env"
if env_file.exists():
    for line in env_file.read_text().splitlines():
        line = line.strip()
        if line and not line.startswith("#") and "=" in line:
            k, v = line.split("=", 1)
            os.environ.setdefault(k, v)


async def test_url(url: str, label: str):
    """Test aiohttp POST to /cmd."""
    import aiohttp

    cmd_url = url.rstrip("/") + "/cmd"
    payload = {"command": "run_command", "params": {"command": "echo hello"}}
    headers = {"Content-Type": "application/json"}

    print(f"\n[{label}] POST {cmd_url}")
    try:
        async with aiohttp.ClientSession() as session:
            async with session.post(cmd_url, json=payload, headers=headers, timeout=aiohttp.ClientTimeout(total=30)) as resp:
                body = await resp.text()
                print(f"  status={resp.status} headers={dict(resp.headers)}")
                print(f"  body={body[:200]!r}")
    except Exception as e:
        print(f"  ERROR: {e}")


async def test_status(url: str, label: str):
    """Test urllib GET to /status."""
    status_url = url.rstrip("/") + "/status"
    print(f"\n[{label}] GET {status_url}")
    try:
        with urllib.request.urlopen(status_url, timeout=10) as resp:
            body = resp.read().decode()
            print(f"  status={resp.status} body={body[:100]!r}")
    except Exception as e:
        print(f"  ERROR: {e}")


async def main():
    api_key = os.environ.get("DAYTONA_API_KEY")
    api_url_env = os.environ.get("DAYTONA_API_URL")

    if not api_key:
        print("ERROR: DAYTONA_API_KEY not set")
        sys.exit(1)

    print("Importing Daytona SDK...")
    from daytona_sdk import Daytona, DaytonaConfig
    from daytona_sdk.common.daytona import CreateSandboxFromSnapshotParams
    from daytona_sdk.common.sandbox import Resources

    daytona = Daytona(DaytonaConfig(api_key=api_key, api_url=api_url_env))

    print("Creating sandbox from snapshot nikri/kicad-snorkel:latest...")
    params = CreateSandboxFromSnapshotParams(
        snapshot="nikri/kicad-snorkel:latest",
        env_vars={},
        auto_stop_interval=0,
        resources=Resources(cpu=2, memory=4, disk=10),
    )
    sandbox = daytona.create(params)
    print(f"  Sandbox ID: {sandbox.id}")

    # Bootstrap supervisord
    print("Bootstrapping supervisord...")
    try:
        sandbox.process.exec(
            "nohup /usr/bin/supervisord -c /etc/supervisor/supervisord.conf"
            " > /var/log/supervisor/supervisord.log 2>&1 &",
            timeout=10,
        )
    except Exception as exc:
        print(f"  exec returned (expected for nohup): {exc}")

    # Get signed URL
    signed = sandbox.create_signed_preview_url(8000, expires_in_seconds=14400)
    url = signed.url
    print(f"\nSigned URL: {url!r}")

    # Wait for server to be ready
    print("\nWaiting for cua-computer-server...")
    status_url = url.rstrip("/") + "/status"
    deadline = time.monotonic() + 300
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(status_url, timeout=5) as resp:
                if resp.status == 200:
                    print(f"  Ready! status={resp.status}")
                    break
        except Exception:
            pass
        time.sleep(5)
    else:
        print("  TIMEOUT waiting for server")
        daytona.delete(sandbox)
        sys.exit(1)

    # Test 1: aiohttp from this process
    await test_url(url, "aiohttp-direct")

    # Test 2: urllib from this process
    await test_status(url, "urllib-direct")

    # Test 3: spawn subprocess and test aiohttp from there
    print("\n[subprocess] Testing aiohttp from subprocess...")
    subprocess_code = f"""
import asyncio
import aiohttp
import sys

async def test():
    url = {url!r}
    cmd_url = url.rstrip('/') + '/cmd'
    payload = {{"command": "run_command", "params": {{"command": "echo subprocess_hello"}}}}
    headers = {{"Content-Type": "application/json"}}
    print(f"POST {{cmd_url}}")
    try:
        async with aiohttp.ClientSession() as session:
            async with session.post(cmd_url, json=payload, headers=headers, timeout=aiohttp.ClientTimeout(total=30)) as resp:
                body = await resp.text()
                print(f"status={{resp.status}} body={{body[:200]!r}}")
    except Exception as e:
        print(f"ERROR: {{e}}")

asyncio.run(test())
"""
    result = subprocess.run(
        [sys.executable, "-c", subprocess_code],
        capture_output=True,
        text=True,
        timeout=60,
    )
    print(f"  stdout: {result.stdout!r}")
    print(f"  stderr: {result.stderr[:500]!r}")

    # Test 4: spawn subprocess with start_new_session=True (like task_runner does)
    print("\n[subprocess+new_session] Testing with start_new_session=True...")
    result2 = subprocess.run(
        [sys.executable, "-c", subprocess_code],
        capture_output=True,
        text=True,
        timeout=60,
        start_new_session=True,
    )
    print(f"  stdout: {result2.stdout!r}")
    print(f"  stderr: {result2.stderr[:500]!r}")

    print("\nCleaning up sandbox...")
    try:
        daytona.delete(sandbox)
        print("  Done.")
    except Exception as e:
        print(f"  Cleanup error: {e}")


if __name__ == "__main__":
    asyncio.run(main())
