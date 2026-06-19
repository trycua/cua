"""Periodic Windows **cua-driver** smoke test on a Cua cloud VM.

Sibling of ``test_windows_cloud_vm.py`` (which only checks that a bare Windows
cloud VM boots). This one exercises the **cua-driver built from source on
``main``** end-to-end, the same way the Linux periodic test exercises the SDK:

    1. provision an ephemeral Windows cloud VM   Sandbox.ephemeral(Image.windows("11"))
    2. push the freshly-built cua-driver onto it  sb.files.upload(...)
    3. start the driver daemon                    cua-driver serve  (detached)
    4. replay a PRE-PROVIDED trajectory           cua-driver call replay_trajectory
       that opens Calculator and computes 2 + 2
    5. screenshot the result + write result.json  (the workflow uploads the
       screenshot to S3 and posts the outcome to am.cua.ai)

The trajectory is a *pre-provided* asset committed at
``examples/sandboxes/fixtures/calculator-2plus2/`` (the ``trajectories/`` dir is
gitignored as generated output, so the committed fixture lives under
``fixtures/``). It must use
keyboard/pixel actions — ``element_index`` values are per-session and do NOT
survive replay (see ``libs/cua-driver/rust/Skills/cua-driver/RECORDING.md``).
Until a real recording (with ``session.json``) is committed there, this test
skips the replay rather than failing.

Env (all optional except CUA_API_KEY):
    CUA_API_KEY      cloud provisioning key (CI reuses PERIODIC_TEST_CUA_API_KEY)
    DRIVER_EXE       host path to the built cua-driver.exe      (default driver-bin/cua-driver.exe)
    DRIVER_UIA_EXE   host path to the built cua-driver-uia.exe  (default driver-bin/cua-driver-uia.exe)
    TRAJECTORY_DIR   host path to the committed trajectory dir
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
GUEST_TRAJ = r"C:\cua\traj"


def _env(name: str, default: str | None = None) -> str | None:
    return os.environ.get(name) or default


def _has_cua_api_key() -> bool:
    return bool(os.environ.get("CUA_API_KEY"))


def _driver_paths() -> tuple[Path, Path]:
    return (
        Path(_env("DRIVER_EXE", "driver-bin/cua-driver.exe")),
        Path(_env("DRIVER_UIA_EXE", "driver-bin/cua-driver-uia.exe")),
    )


def _trajectory_dir() -> Path:
    return Path(_env("TRAJECTORY_DIR", "examples/sandboxes/fixtures/calculator-2plus2"))


def _trajectory_ready(traj: Path) -> bool:
    # A real cua-driver trajectory always has a session.json at its root; the
    # README placeholder alone does not count as "provided".
    return traj.is_dir() and (traj / "session.json").is_file()


def _trajectory_files(traj: Path) -> list[Path]:
    return [p for p in traj.rglob("*") if p.is_file() and p.name != "README.md"]


async def _run(sb, cmd: str):
    logger.info("guest$ %s", cmd)
    res = await sb.shell.run(cmd)
    logger.info(
        "  -> success=%s rc=%s stdout=%r stderr=%r",
        getattr(res, "success", None),
        getattr(res, "returncode", None),
        (getattr(res, "stdout", "") or "")[:400],
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


def _parse_json_stdout(stdout: str):
    stdout = stdout or ""
    try:
        return json.loads(stdout)
    except Exception:  # noqa: BLE001 - fall back to a bracket slice
        start, end = stdout.find("{"), stdout.rfind("}")
        if 0 <= start < end:
            try:
                return json.loads(stdout[start : end + 1])
            except Exception:  # noqa: BLE001
                return None
        return None


@pytest.mark.skipif(not _has_cua_api_key(), reason="CUA_API_KEY not set")
async def test_windows_cloud_driver():
    exe, uia = _driver_paths()
    if not exe.is_file():
        pytest.skip(f"driver binary not found at {exe} — build it or set DRIVER_EXE")

    traj = _trajectory_dir()
    if not _trajectory_ready(traj):
        pytest.skip(
            f"no pre-provided trajectory at {traj} (missing session.json) — "
            "commit the recorded trajectory before this test can replay it"
        )
    traj_files = _trajectory_files(traj)

    screenshot_out = Path(_env("SCREENSHOT_OUT", "screenshot.png"))
    result_json = Path(_env("RESULT_JSON", "result.json"))
    screenshot_out.parent.mkdir(parents=True, exist_ok=True)
    result_json.parent.mkdir(parents=True, exist_ok=True)
    result: dict = {"passed": False, "replayed": False, "display": None, "error": None}

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
                # 1. push the freshly-built driver + the pre-provided trajectory
                await _run(sb, _ps(f"New-Item -ItemType Directory -Force -Path '{GUEST_DIR}' | Out-Null"))
                await sb.files.upload(str(exe), GUEST_DRIVER)
                if uia.is_file():
                    await sb.files.upload(str(uia), GUEST_UIA)
                for f in traj_files:
                    rel = str(f.relative_to(traj)).replace("/", "\\")
                    guest = f"{GUEST_TRAJ}\\{rel}"
                    parent = guest.rsplit("\\", 1)[0]
                    await _run(sb, _ps(f"New-Item -ItemType Directory -Force -Path '{parent}' | Out-Null"))
                    await sb.files.upload(str(f), guest)

                # 2. sanity: the binary runs in-guest
                ver = await _run(sb, f'"{GUEST_DRIVER}" --version')
                assert getattr(ver, "success", False), f"cua-driver --version failed: {ver.stderr}"

                # 3. start the daemon detached in the interactive session
                await _run(sb, _ps(f"Start-Process -FilePath '{GUEST_DRIVER}' -ArgumentList 'serve' -WindowStyle Hidden"))
                up = False
                for _ in range(30):
                    s = await _run(sb, f'"{GUEST_DRIVER}" status')
                    if getattr(s, "success", False):
                        up = True
                        break
                    await asyncio.sleep(2)
                assert up, "cua-driver daemon did not come up"

                # 4. replay the pre-provided trajectory (opens Calculator, 2 + 2)
                rep = await _driver_call(sb, "replay_trajectory", {"dir": GUEST_TRAJ, "delay_ms": 500})
                result["replayed"] = bool(getattr(rep, "success", False))
                assert result["replayed"], f"replay_trajectory failed: {rep.stderr or rep.stdout}"

                # 5. best-effort verification of the result via the UIA tree
                try:
                    la = await _driver_call(sb, "launch_app", {"aumid": CALC_AUMID})
                    meta = _parse_json_stdout(getattr(la, "stdout", "")) or {}
                    pid = meta.get("pid")
                    wins = meta.get("windows") or []
                    wid = wins[0].get("window_id") if wins else None
                    if pid and wid:
                        ws = await _driver_call(sb, "get_window_state", {"pid": pid, "window_id": wid})
                        m = re.search(r"Display is ([^\n\"]+)", getattr(ws, "stdout", "") or "")
                        if m:
                            result["display"] = m.group(1).strip()
                except Exception as e:  # noqa: BLE001 - verification is best-effort
                    logger.warning("display verification skipped: %s", e)

                # 6. screenshot for the notification
                png = await sb.screenshot()
                screenshot_out.write_bytes(png)
                assert png[:4] == b"\x89PNG", f"screenshot not PNG: {png[:4]!r}"

                disp = result["display"]
                result["passed"] = result["replayed"] and (disp is None or disp.endswith("4"))
                if disp is not None:
                    assert disp.endswith("4"), f"calculator display was {disp!r}, expected to end with 4"
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
