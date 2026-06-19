"""Periodic Windows **cua-driver** smoke test on a Cua cloud VM.

Sibling of ``test_windows_cloud_vm.py`` (which only checks that a bare Windows
cloud VM boots). This one exercises the **cua-driver built from source on
``main``** end-to-end, the same way the Linux periodic test exercises the SDK:

    1. provision an ephemeral Windows cloud VM   Sandbox.ephemeral(Image.windows("11"))
    2. push the freshly-built cua-driver onto it  sb.files.upload(...)
    3. start the driver daemon                    cua-driver serve  (detached)
    4. drive Windows Calculator to compute 2 + 2  launch_app -> Invoke the UWP
       buttons by UIA element_index -> read the result from the UIA tree
    5. screenshot the result + write result.json  (the workflow uploads the
       screenshot to S3 and posts the outcome to am.cua.ai)

Why drive **inline** instead of replaying a recorded trajectory?
``cua-driver`` recording is currently **macOS-only** (the Windows port returns
"not yet supported"), so a Windows trajectory can't be recorded and replayed.
And UWP apps (Win11 Calculator) **ignore PostMessage clicks**, so buttons are
invoked by UIA ``element_index`` (the documented reliable path) rather than by
pixel clicks or keystrokes. See
``libs/cua-driver/rust/Skills/cua-driver/WINDOWS.md``.

Env (all optional except CUA_API_KEY):
    CUA_API_KEY      cloud provisioning key (CI reuses PERIODIC_TEST_CUA_API_KEY)
    DRIVER_EXE       host path to the built cua-driver.exe      (default driver-bin/cua-driver.exe)
    DRIVER_UIA_EXE   host path to the built cua-driver-uia.exe  (default driver-bin/cua-driver-uia.exe)
    SCREENSHOT_OUT   where to write the result screenshot        (default ./screenshot.png)
    RESULT_JSON      where to write the result summary           (default ./result.json)
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import re
import time
from pathlib import Path

import pytest

from cua_sandbox import Image, Sandbox

pytestmark = pytest.mark.asyncio

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)s %(name)s: %(message)s",
)
logger = logging.getLogger("test_windows_cloud_driver")

CALC_AUMID = "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"
GUEST_DIR = r"C:\cua"
GUEST_DRIVER = r"C:\cua\cua-driver.exe"
GUEST_UIA = r"C:\cua\cua-driver-uia.exe"
# UIA accessible names of the Win11 Calculator buttons we Invoke to enter 2 + 2 =.
# Keyboard input is unreliable on UWP, so we click the buttons by element_index.
CALC_BUTTONS = ["Two", "Plus", "Two", "Equals"]


def _env(name: str, default: str | None = None) -> str | None:
    return os.environ.get(name) or default


def _has_cua_api_key() -> bool:
    return bool(os.environ.get("CUA_API_KEY"))


def _driver_paths() -> tuple[Path, Path]:
    return (
        Path(_env("DRIVER_EXE", "driver-bin/cua-driver.exe")),
        Path(_env("DRIVER_UIA_EXE", "driver-bin/cua-driver-uia.exe")),
    )


def _ok(res) -> bool:
    return bool(getattr(res, "success", False))


def _out(res) -> str:
    return getattr(res, "stdout", "") or ""


async def _run(sb, cmd: str):
    logger.info("guest$ %s", cmd)
    res = await sb.shell.run(cmd)
    logger.info(
        "  -> success=%s rc=%s\n     stdout=%s\n     stderr=%s",
        getattr(res, "success", None),
        getattr(res, "returncode", None),
        _out(res)[:800],
        (getattr(res, "stderr", "") or "")[:400],
    )
    return res


def _ps(command: str) -> str:
    """Wrap a PowerShell command (sb.shell.run executes via cmd.exe)."""
    return f'powershell -NoProfile -Command "{command}"'


async def _driver_call(sb, tool: str, args: dict):
    """Invoke ``cua-driver call <tool>`` with JSON piped via stdin.

    Piping the args through a file dodges cmd.exe / PowerShell quote-mangling of
    embedded JSON (see Skills/cua-driver/WINDOWS.md).
    """
    await sb.files.write_text(r"C:\cua\_args.json", json.dumps(args))
    return await _run(
        sb,
        _ps(
            "Get-Content -Raw -LiteralPath 'C:\\cua\\_args.json' | "
            f"& '{GUEST_DRIVER}' call {tool}"
        ),
    )


def _find_calc_window(stdout: str) -> tuple[int | None, int | None]:
    """Parse (pid, window_id) of the Calculator window from list_windows output.

    list_windows prints one line per window:
        - <app> (pid 6004) "Calculator" [window_id: 459672]
    """
    for line in stdout.splitlines():
        low = line.lower()
        if "calculator" in low and "window_id" in low:
            m = re.search(r"pid\s+(\d+).*?window_id:\s*(\d+)", line)
            if m:
                return int(m.group(1)), int(m.group(2))
    return None, None


def _element_index(tree: str, name: str) -> int | None:
    """Find the UIA element_index whose accessible name is exactly ``name``.

    get_window_state renders each node as e.g. ``[12] Button "Two"``.
    """
    m = re.search(rf'\[(\d+)\][^\n"]*"{re.escape(name)}"', tree)
    return int(m.group(1)) if m else None


def _display_value(stdout: str) -> str | None:
    """Read the Calculator result. Win11 Calculator names its display element
    ``Display is <value>`` in the UIA tree (a Calculator string, not a driver one)."""
    m = re.search(r"Display is ([^\"\n]+)", stdout)
    return m.group(1).strip() if m else None


@pytest.mark.skipif(not _has_cua_api_key(), reason="CUA_API_KEY not set")
async def test_windows_cloud_driver():
    exe, uia = _driver_paths()
    if not exe.is_file():
        pytest.skip(f"driver binary not found at {exe} — build it or set DRIVER_EXE")

    screenshot_out = Path(_env("SCREENSHOT_OUT", "screenshot.png"))
    result_json = Path(_env("RESULT_JSON", "result.json"))
    screenshot_out.parent.mkdir(parents=True, exist_ok=True)
    result_json.parent.mkdir(parents=True, exist_ok=True)
    result: dict = {
        "passed": False,
        "launched": False,
        "uia_elements": 0,
        "clicked": False,
        "display": None,
        "error": None,
    }

    t0 = time.monotonic()
    logger.info("Creating ephemeral Windows cloud VM (windows 11, kind=vm)")
    try:
        async with Sandbox.ephemeral(
            Image.windows("11"),
            name=f"cua-driver-win-{os.environ.get('GITHUB_RUN_ID', 'local')}",
        ) as sb:
            logger.info(
                "VM ready: name=%s in %.1fs",
                getattr(sb, "name", "unknown"),
                time.monotonic() - t0,
            )
            try:
                # 1. push the freshly-built driver onto the VM
                await _run(sb, _ps(f"New-Item -ItemType Directory -Force -Path '{GUEST_DIR}' | Out-Null"))
                await sb.files.upload(str(exe), GUEST_DRIVER)
                if uia.is_file():
                    await sb.files.upload(str(uia), GUEST_UIA)

                # 2. the binary runs on Windows
                ver = await _run(sb, f'"{GUEST_DRIVER}" --version')
                assert _ok(ver), f"cua-driver --version failed: {_out(ver)} {getattr(ver, 'stderr', '')}"

                # 3. start the daemon detached in the interactive session, poll status
                await _run(sb, _ps(f"Start-Process -FilePath '{GUEST_DRIVER}' -ArgumentList 'serve' -WindowStyle Hidden"))
                up = False
                for _ in range(30):
                    if _ok(await _run(sb, f'"{GUEST_DRIVER}" status')):
                        up = True
                        break
                    await asyncio.sleep(2)
                assert up, "cua-driver daemon did not come up"
                # diagnostics: session id (must be >=1 for UIA), COM/UIA reachability
                await _run(sb, f'"{GUEST_DRIVER}" doctor')

                # 4. launch the UWP Calculator via the driver (no focus steal)
                la = await _driver_call(sb, "launch_app", {"aumid": CALC_AUMID})
                assert _ok(la), f"launch_app failed: {_out(la)}"
                result["launched"] = True

                # 5. resolve the Calculator window (pid + HWND) via list_windows
                pid = wid = None
                for _ in range(15):
                    lw = await _driver_call(sb, "list_windows", {})
                    pid, wid = _find_calc_window(_out(lw))
                    if pid and wid:
                        break
                    await asyncio.sleep(2)
                assert pid and wid, "Calculator window did not appear in list_windows"

                # 6. snapshot the UIA tree (proves UIA enumeration works) + find buttons
                ws = await _driver_call(sb, "get_window_state", {"pid": pid, "window_id": wid})
                assert _ok(ws), f"get_window_state failed: {_out(ws)}"
                m = re.search(r"elements=(\d+)", _out(ws))
                result["uia_elements"] = int(m.group(1)) if m else 0
                assert result["uia_elements"] > 0, f"empty UIA tree: {_out(ws)[:500]}"
                tree = _out(ws)

                # 7. compute 2 + 2 by Invoking the buttons by element_index (UWP-safe).
                #    Indices come from the single snapshot above; Calculator's button
                #    grid is static, so they stay valid across the clicks.
                for name in CALC_BUTTONS:
                    idx = _element_index(tree, name)
                    assert idx is not None, f'Calculator button "{name}" not found in UIA tree'
                    ck = await _driver_call(sb, "click", {"pid": pid, "window_id": wid, "element_index": idx})
                    assert _ok(ck), f"click {name}[{idx}] failed: {_out(ck)}"
                    await asyncio.sleep(0.3)
                result["clicked"] = True

                # 8. read the result back from the UIA tree
                disp = None
                for _ in range(5):
                    ds = await _driver_call(sb, "get_window_state", {"pid": pid, "window_id": wid, "query": "Display"})
                    disp = _display_value(_out(ds))
                    if disp:
                        break
                    await asyncio.sleep(1)
                result["display"] = disp

                # 9. screenshot for the notification
                png = await sb.screenshot()
                screenshot_out.write_bytes(png)
                assert png[:4] == b"\x89PNG", f"screenshot not PNG: {png[:4]!r}"

                # 10. verify the arithmetic landed
                assert disp is not None, "could not read the Calculator display from the UIA tree"
                assert disp.endswith("4"), f"calculator display was {disp!r}, expected to end with 4"
                result["passed"] = True
            except Exception as e:  # noqa: BLE001
                result["error"] = f"{type(e).__name__}: {e}"
                # best-effort screenshot so the notification still has a picture
                try:
                    png = await sb.screenshot()
                    screenshot_out.write_bytes(png)
                except Exception:  # noqa: BLE001
                    pass
                raise
    finally:
        result_json.write_text(json.dumps(result, indent=2))
        logger.info("result: %s", result)


async def _main():
    await test_windows_cloud_driver()


if __name__ == "__main__":
    asyncio.run(_main())
