"""Creator agent — generates install/launch/test scripts for one app x OS.

The agent gets NO Bash access. All execution goes through sandbox MCP tools.
submit_result requires a screenshot that is verified by haiku vision — if the
app isn't visibly open, the agent is told to keep going.
The orchestrator force-deletes any leftover sandboxes after the agent finishes.
"""

from __future__ import annotations

import base64
import json
import logging
import os
from pathlib import Path
from typing import Any

from claude_agent_sdk import ClaudeAgentOptions, create_sdk_mcp_server, query

from .sandbox_tools import make_sandbox_tools

logger = logging.getLogger(__name__)


# Error-masking patterns: `|| true`, `|| :`, `|| echo ""`, `|| echo ''`.
# A script that swallows failures this way cannot be trusted to report errors.
_ERROR_MASK_RE = __import__("re").compile(
    r'\|\|\s*(?:true\b|:(?:\s|$|[;)&#])|echo\s+(?:""|\'\')\s*(?:$|[;)&#]))'
)


def _lint_error_masking(script: str) -> list[tuple[int, str]]:
    """Return (lineno, line) pairs that mask shell errors.

    Respects `# lint-ignore: error-masking` on the same or previous line.
    """
    violations: list[tuple[int, str]] = []
    prev = ""
    for i, line in enumerate(script.splitlines(), start=1):
        if line.lstrip().startswith("#"):
            prev = line
            continue
        if _ERROR_MASK_RE.search(line):
            if "# lint-ignore: error-masking" in line or "# lint-ignore: error-masking" in prev:
                prev = line
                continue
            violations.append((i, line.rstrip()))
        prev = line
    return violations


def _lint_fail_fast(script: str) -> list[str]:
    """Return list of missing fail-fast directives from the script header."""
    head = "\n".join(script.splitlines()[:20])
    missing = []
    if "set -e" not in head and "set -eu" not in head and "set -euo" not in head:
        missing.append("`set -e` (exit on any command failure)")
    if "set -u" not in head and "set -eu" not in head and "set -euo" not in head:
        missing.append("`set -u` (fail on undefined variables)")
    if "pipefail" not in head:
        missing.append("`set -o pipefail` (surface errors in pipelines)")
    return missing


CREATOR_SYSTEM_PROMPT = """\
You are an app installer agent. Your job is to create working install and launch
scripts for a specific application on a specific OS.

You have NO direct shell access. All commands must run inside a sandbox VM via
the sandbox tools. This is critical — never try to run commands outside the sandbox.

TOOLS AVAILABLE:
- create_sandbox(os) — spin up a VM, returns a name
- sandbox_run(name, command) — run shell command inside the VM
- sandbox_write(name, path, content) — write a file into the VM
- sandbox_read(name, path) — read a file from the VM
- sandbox_screenshot(name) — screenshot the VM display
- delete_sandbox(name) — tear down the VM
- submit_result(result) — submit your final deliverables
- WebSearch / WebFetch — research install instructions

WORKFLOW:
1. WebSearch for official install instructions for the app on the target OS.
2. Call create_sandbox(os) to get a VM.
3. Write your install script into the sandbox via sandbox_write.
4. Run it via sandbox_run and check the output.
5. If it fails, debug: read logs, adjust the script, retry.
6. Once install works, write a launch script.
7. RUN THE LAUNCH SCRIPT via sandbox_run to actually start the application.
8. Wait a few seconds (sandbox_run "sleep 5") for the app to start.
9. Take a screenshot with sandbox_screenshot.
10. Write an extract_metadata.sh (or .ps1 for Windows) script and run it in the sandbox.
    This script MUST output a JSON object to stdout with these fields:
    {
      "binary_path": "/usr/bin/godot3",         // absolute path to main binary
      "binary_name": "godot3",                   // binary/command name
      "display_name": "Godot Engine",            // human-readable app name
      "desktop_entry": "/usr/share/applications/godot3.desktop",  // .desktop file path (Linux) or null
      "icon_paths": ["/usr/share/icons/..."],    // list of icon file paths found
      "version": "3.2.3"                         // version string from --version or similar
    }
    The script should:
    - Find the installed binary using `which`, `command -v`, or known install paths
    - Extract the version via --version, -V, or similar flags
    - Find .desktop files (Linux), .app bundles (macOS), or Start Menu shortcuts (Windows)
    - Find icon files (.png, .svg, .ico) from standard icon dirs, .desktop files, or .app bundles
    - Output ONLY the JSON object to stdout (no other text)
    After running it, read the JSON output and include it in submit_result as "metadata".
11. Call submit_result with ALL deliverables. submit_result checks:
    - Screenshot shows the app visibly open (verified by AI vision)
    - extract_metadata script ran and produced valid JSON with binary_path + display_name
    If any check fails it returns "CRITERIA NOT MET" — fix the issue and resubmit.
12. Once submit_result returns OK, call delete_sandbox to clean up.

EARLY EXIT CONDITIONS — call submit_result immediately without creating a sandbox:

1. NO PUBLIC DOWNLOAD: If the software requires requesting a quote, contacting
   sales, enterprise agreement, or paid subscription with no trial — submit with:
   install_exit_code: -1, download_available: false, notes: "<reason>"

2. LIBRARY/SDK: If app_type is "library" or the software is a development
   dependency (pip/npm/cargo package, SDK, framework) — submit with:
   install_exit_code: -1, app_type_detected: "library", notes: "Development library, not end-user software"

3. WEBAPP ONLY: If the software is purely browser-based with no installable
   client — submit with:
   install_exit_code: -1, app_type_detected: "webapp", notes: "Browser-only webapp"

Only proceed with sandbox creation for standalone GUI apps, CLI tools, and
server software that can actually be installed and run.

RULES:
- **FAIL FAST.** Every bash script MUST start with `set -euo pipefail` so
  any unchecked failure, undefined variable, or broken pipe aborts
  immediately. Don't catch or swallow errors just to keep going.
- **NO ERROR MASKING.** Never append `|| true`, `|| :`, `|| echo ""` to a
  command. The linter rejects these. If a command is genuinely optional,
  write a real `if` guard (`if command -v foo; then foo ...; fi`) and log
  the decision.
- **IDEMPOTENT.** Safe to run twice against the same VM without breakage.
  Check-before-act patterns: `[ -f /path ] || wget ...`, `dpkg -s pkg ||
  apt-get install -y pkg`. Do NOT simulate idempotency by ignoring errors
  on the second run.
- Prefer package managers (apt, brew, choco, winget) over direct downloads.
- Handle dependencies.
- The install script should be self-contained (no user interaction).
- The strict verifier re-runs the submitted install.sh end-to-end in a
  FRESH sandbox. If your script only works because of state the
  interactive session left behind, it will be rejected.
- Linux VMs run a full Ubuntu Desktop with XFCE. There is already a display
  server running. Do NOT install or use xvfb — just run GUI apps directly.
- YOU MUST LAUNCH THE APP before taking a screenshot. The screenshot must show
  the application window visible on screen. An OS desktop or login screen is NOT
  sufficient — the app itself must be open.

IMPORTANT — macOS VMs often show an Apple Account sign-in dialog on first boot.
You MUST dismiss it before launching your app. Run these commands right after
create_sandbox:
  sandbox_run(name, 'osascript -e "tell application \"System Events\" to key code 53"')
  sandbox_run(name, 'sleep 2')
  sandbox_run(name, 'osascript -e "tell application \"System Events\" to key code 53"')
This sends Escape to close any modal dialogs. Repeat if needed. You can also try:
  sandbox_run(name, 'killall "Setup Assistant" 2>/dev/null; true')
  sandbox_run(name, 'osascript -e "tell application \"System Events\" to click button \"Skip\" of window 1 of process \"UserNotificationCenter\"" 2>/dev/null; true')
"""


def _save_transcript(session_id: str | None, logs_dir: Path, app_id: str, target_os: str) -> None:
    """Copy the Claude Code session JSONL to logs/ and convert to HTML via claude-code-transcripts."""
    if not session_id:
        logger.warning("No session ID captured for %s/%s — skipping transcript", app_id, target_os)
        return

    import glob
    import shutil
    import subprocess

    # Find the session JSONL in ~/.claude/projects/
    home = Path.home()
    pattern = str(home / ".claude" / "projects" / "**" / f"{session_id}.jsonl")
    matches = glob.glob(pattern, recursive=True)

    if not matches:
        logger.warning("Session JSONL not found for %s (%s/%s)", session_id, app_id, target_os)
        return

    src = Path(matches[0])
    dst = logs_dir / "install-session.jsonl"
    shutil.copy2(src, dst)
    logger.info("Copied session JSONL: %s → %s", src, dst)

    # Convert to HTML
    try:
        subprocess.run(
            [
                "claude-code-transcripts",
                "json",
                str(dst),
                "-o",
                str(logs_dir / "install-transcript"),
            ],
            capture_output=True,
            text=True,
            timeout=60,
        )
        logger.info("Generated HTML transcript for %s/%s", app_id, target_os)
    except (subprocess.SubprocessError, FileNotFoundError) as e:
        logger.warning("Failed to generate HTML transcript: %s", e)


async def _verify_screenshot(
    screenshot_path: Path, app_name: str, is_cli: bool = False
) -> tuple[bool, str]:
    """Use Bedrock haiku vision to check if the app is visible in the screenshot.

    Returns (passed, reason).
    """
    import boto3

    if not screenshot_path.exists():
        return False, "Screenshot file not found"

    img_bytes = screenshot_path.read_bytes()
    img_b64 = base64.b64encode(img_bytes).decode("utf-8")
    media_type = "image/jpeg" if screenshot_path.suffix == ".jpg" else "image/png"

    region = os.environ.get("AWS_REGION", "us-east-1")
    client = boto3.client("bedrock-runtime", region_name=region)

    if is_cli:
        verification_text = (
            f'Is the CLI tool "{app_name}" visibly running or showing output in this screenshot? '
            f"Look for a terminal window showing the tool's output, version info, help text, "
            f"or a running process. A terminal with relevant command output counts as a PASS. "
            f"An empty desktop, login screen, or unrelated terminal does NOT count.\n"
            f"Reply with exactly one line:\n"
            f"PASS: <brief description of what you see>\n"
            f"or\n"
            f"FAIL: <brief description of what you see instead>"
        )
    else:
        verification_text = (
            f'Is the application "{app_name}" visibly open and running in this screenshot? '
            f"Look for the app window, splash screen, or main UI — NOT just an OS desktop, "
            f"login screen, or terminal. Reply with exactly one line:\n"
            f"PASS: <brief description of what you see>\n"
            f"or\n"
            f"FAIL: <brief description of what you see instead>"
        )

    body = {
        "anthropic_version": "bedrock-2023-05-31",
        "max_tokens": 256,
        "messages": [
            {
                "role": "user",
                "content": [
                    {
                        "type": "image",
                        "source": {"type": "base64", "media_type": media_type, "data": img_b64},
                    },
                    {"type": "text", "text": verification_text},
                ],
            }
        ],
    }

    try:
        resp = client.invoke_model(
            modelId="us.anthropic.claude-haiku-4-5-20251001-v1:0",
            contentType="application/json",
            accept="application/json",
            body=json.dumps(body),
        )
        result = json.loads(resp["body"].read())
        text = result.get("content", [{}])[0].get("text", "").strip()
        logger.info("Screenshot verification for %s: %s", app_name, text)

        if text.upper().startswith("PASS"):
            return True, text
        return False, text
    except Exception as e:
        logger.error("Screenshot verification failed: %s", e)
        return False, f"Verification error: {e}"


async def _verify_metadata_script(script: str, app_name: str) -> tuple[bool, str]:
    """Use Bedrock haiku to check that the script actually inspects install artifacts
    rather than hardcoding values like the app name or binary path.

    Returns (passed, reason).
    """
    import boto3

    region = os.environ.get("AWS_REGION", "us-east-1")
    client = boto3.client("bedrock-runtime", region_name=region)

    body = {
        "anthropic_version": "bedrock-2023-05-31",
        "max_tokens": 256,
        "messages": [
            {
                "role": "user",
                "content": (
                    f'Review this metadata extraction script for the app "{app_name}". '
                    f"The script is supposed to dynamically discover the binary path, display name, "
                    f"version, icons, and desktop entries by inspecting the actual installed artifacts "
                    f"(e.g. using `which`, `dpkg -L`, `rpm -ql`, reading .desktop files, parsing "
                    f"package manager output, checking /usr/share/applications, etc.).\n\n"
                    f"Does the script ACTUALLY inspect installed artifacts to extract metadata, "
                    f"or does it just hardcode/echo static values?\n\n"
                    f"Script:\n```\n{script}\n```\n\n"
                    f"Reply with exactly one line:\n"
                    f"PASS: <brief description of how it inspects artifacts>\n"
                    f"or\n"
                    f"FAIL: <what is hardcoded and how it should dynamically discover it instead>"
                ),
            }
        ],
    }

    try:
        resp = client.invoke_model(
            modelId="us.anthropic.claude-haiku-4-5-20251001-v1:0",
            contentType="application/json",
            accept="application/json",
            body=json.dumps(body),
        )
        result = json.loads(resp["body"].read())
        text = result.get("content", [{}])[0].get("text", "").strip()
        logger.info("Metadata script verification for %s: %s", app_name, text)

        if text.upper().startswith("PASS"):
            return True, text
        return False, text
    except Exception as e:
        logger.error("Metadata script verification failed: %s", e)
        return False, f"Verification error: {e}"


def _make_submit_tool(
    result_holder: list,
    output_dir: Path | None,
    app_name: str,
    sandbox_registry: dict[str, Any] | None = None,
    target_os: str = "linux",
    strict_install_verify: bool = True,
):
    """MCP tool that verifies the screenshot before accepting the submission."""
    from claude_agent_sdk import tool

    @tool(
        "submit_result",
        "Submit the final deliverables. Checks: (1) screenshot shows app open, "
        "(2) extract_metadata produced valid JSON with binary_path and display_name. "
        "If any check fails, returns CRITERIA NOT MET — fix and resubmit.",
        {
            "type": "object",
            "properties": {
                "result": {
                    "type": "object",
                    "properties": {
                        "app_id": {"type": "string"},
                        "os": {"type": "string"},
                        "sandbox_name": {
                            "type": "string",
                            "description": "Name of the sandbox to verify screenshot from",
                        },
                        "install_script": {
                            "type": "string",
                            "description": "Contents of the install script",
                        },
                        "launch_script": {
                            "type": "string",
                            "description": "Contents of the launch script",
                        },
                        "extract_metadata_script": {
                            "type": "string",
                            "description": "Contents of the extract_metadata.sh/.ps1 script",
                        },
                        "metadata": {
                            "type": "object",
                            "description": "JSON output from extract_metadata script with binary_path, display_name, icon_paths, version",
                        },
                        "install_exit_code": {"type": "integer"},
                        "install_stdout": {"type": "string"},
                        "verification_command": {"type": "string"},
                        "download_available": {
                            "type": "boolean",
                            "description": "False if no public download exists (quote/sales required)",
                        },
                        "app_type_detected": {
                            "type": "string",
                            "description": "Set to 'library' or 'webapp' for early exit",
                        },
                        "is_cli": {
                            "type": "boolean",
                            "description": "True if this is a CLI/TUI tool (terminal screenshot accepted)",
                        },
                        "notes": {"type": "string"},
                    },
                    "required": ["app_id", "os", "install_exit_code"],
                },
            },
            "required": ["result"],
        },
    )
    async def submit_result(args: dict[str, Any]) -> dict[str, Any]:
        result = args.get("result", {})
        sandbox_name = result.get("sandbox_name", "")

        # ── Early exit: no public download, library, or webapp ──
        if result.get("download_available") is False or result.get("install_exit_code") == -1:
            detected_type = result.get("app_type_detected", "")
            reason = result.get("notes", "no public download")
            result["verification_passed"] = False
            if detected_type in ("library", "webapp"):
                result["download_available"] = None  # not applicable
            else:
                result["download_available"] = False
            result_holder.append(result)
            logger.info(
                "Early exit for %s/%s: %s — %s",
                result.get("app_id"),
                result.get("os"),
                detected_type or "no-download",
                reason,
            )
            return {
                "content": [
                    {
                        "type": "text",
                        "text": (
                            f"OK: recorded for {result.get('app_id')} on {result.get('os')}. "
                            f"Type: {detected_type or 'no-download'}. Reason: {reason}"
                        ),
                    }
                ]
            }

        if not sandbox_name:
            return {"content": [{"type": "text", "text": "ERROR: sandbox_name is required."}]}

        failures = []
        is_cli = result.get("is_cli", False)

        # ── Check 1: screenshot exists and shows the app ──
        screenshot_path = None
        if output_dir:
            for ext in (".jpg", ".png"):
                p = output_dir / f"{sandbox_name}{ext}"
                if p.exists():
                    screenshot_path = p
                    break
        if not screenshot_path or not screenshot_path.exists():
            failures.append("No screenshot found. Call sandbox_screenshot first.")
        else:
            passed, reason = await _verify_screenshot(screenshot_path, app_name, is_cli=is_cli)
            if not passed:
                failures.append(f"Screenshot: {reason}")
            else:
                result["screenshot_verification"] = reason

        # ── Check 2: metadata extraction produced valid results ──
        metadata = result.get("metadata")
        if not metadata or not isinstance(metadata, dict):
            failures.append(
                "metadata is missing. Run your extract_metadata script and include its JSON output."
            )
        else:
            if not metadata.get("binary_path"):
                failures.append(
                    "metadata.binary_path is missing — the script must find the installed binary path."
                )
            if not metadata.get("display_name"):
                failures.append(
                    "metadata.display_name is missing — the script must extract the app's display name."
                )

        # ── Check 3: install_script must parse (bash -n) and run end-to-end in
        # a clean sandbox. Catches cases where the agent debugged interactively
        # under a different filename then submitted a subtly different text.
        install_script = result.get("install_script", "")
        # Static lints — cheap, always run, catch error-masking & missing
        # fail-fast pragmas before we burn a fresh sandbox on a broken script.
        if install_script and target_os != "windows":
            masks = _lint_error_masking(install_script)
            if masks:
                rendered = "; ".join(f"line {ln}: {txt.strip()}" for ln, txt in masks[:5])
                failures.append(
                    f"install_script ERROR-MASKING: the install must FAIL FAST on any "
                    f'error — strip all `|| true`, `|| :`, `|| echo ""` etc. '
                    f"If a command is genuinely optional, use a real `if` guard and "
                    f"log the decision. Violations: {rendered}"
                )
            missing_ff = _lint_fail_fast(install_script)
            if missing_ff:
                failures.append(
                    "install_script missing fail-fast pragmas near the top: "
                    + ", ".join(missing_ff)
                    + ". Add `set -euo pipefail` to the "
                    "header so unchecked failures and undefined vars abort the script."
                )
        if (
            strict_install_verify
            and install_script
            and target_os != "windows"
            and sandbox_registry is not None
        ):
            sb = sandbox_registry.get(sandbox_name)
            if sb is not None:
                await sb.files.write_text("/tmp/_verify_install.sh", install_script)
                r_syntax = await sb.shell.run("bash -n /tmp/_verify_install.sh")
                if r_syntax.returncode != 0:
                    failures.append(
                        f"install_script syntax error (bash -n): {(r_syntax.stderr or '')[:500]}"
                    )
                else:
                    # Full end-to-end re-run in a fresh sandbox to catch idempotency
                    # regressions that the agent's iterative debugging masked.
                    from cua_sandbox import Image as _Image
                    from cua_sandbox import Sandbox as _Sandbox

                    img_map = {
                        "linux": _Image.linux,
                        "macos": _Image.macos,
                        "android": _Image.android,
                    }
                    img = img_map[target_os]()
                    verify_sb = await _Sandbox.create(
                        img, local=True, name=f"verify-{sandbox_name}"
                    )
                    try:
                        await verify_sb.files.write_text("/tmp/install.sh", install_script)
                        await verify_sb.shell.run("chmod +x /tmp/install.sh")
                        pty = await verify_sb.shell.run(
                            "bash -c 'bash /tmp/install.sh > /tmp/install.log 2>&1; "
                            "echo $? > /tmp/install.exit'",
                            background=True,
                        )
                        import asyncio as _aio

                        done = False
                        for _ in range(180):  # up to 30 min
                            await _aio.sleep(10)
                            r = await verify_sb.shell.run(
                                "cat /tmp/install.exit 2>/dev/null; echo DONE"
                            )
                            lines = (r.stdout or "").strip().splitlines()
                            if len(lines) >= 2 and lines[0].isdigit():
                                rc = int(lines[0])
                                if rc != 0:
                                    tail = await verify_sb.shell.run("tail -n 30 /tmp/install.log")
                                    failures.append(
                                        f"install_script failed in fresh sandbox "
                                        f"(exit {rc}): {(tail.stdout or '')[-500:]}"
                                    )
                                done = True
                                break
                        if not done:
                            failures.append(
                                "install_script timed out in fresh-sandbox verify (30 min)"
                            )
                    finally:
                        try:
                            await verify_sb.destroy()
                        except Exception as _e:
                            logger.warning("verify sandbox cleanup error: %s", _e)

        # ── Check 4: extract_metadata_script was provided and actually inspects artifacts ──
        extract_script = result.get("extract_metadata_script", "")
        if not extract_script:
            failures.append(
                "extract_metadata_script is missing. Write and include the metadata extraction script."
            )
        else:
            # Verify the script actually inspects artifacts rather than hardcoding values
            script_ok, script_reason = await _verify_metadata_script(extract_script, app_name)
            if not script_ok:
                failures.append(f"extract_metadata_script: {script_reason}")

        if failures:
            checklist = "\n".join(f"  - {f}" for f in failures)
            return {
                "content": [
                    {
                        "type": "text",
                        "text": (
                            f"CRITERIA NOT MET:\n{checklist}\n\n"
                            f"Fix the issues above, then call submit_result again."
                        ),
                    }
                ]
            }

        # All checks passed — extract icons from sandbox before it gets deleted
        icon_paths = metadata.get("icon_paths", []) if metadata else []
        sb = (sandbox_registry or {}).get(sandbox_name)
        extracted_icons = []
        if sb and icon_paths and output_dir:
            for icon_path in icon_paths:
                try:
                    # Get icon dimensions and read file from sandbox
                    r = await sb.shell.run(
                        f"file {icon_path} 2>/dev/null | grep -oP '\\d+ x \\d+' | head -1"
                    )
                    size = "unknown"
                    if r.returncode == 0 and r.stdout.strip():
                        dims = r.stdout.strip().split(" x ")
                        if len(dims) == 2:
                            size = dims[0].strip()
                    # Fallback: try to get size from path (e.g. /usr/share/icons/hicolor/256x256/...)
                    if size == "unknown":
                        import re

                        m = re.search(r"(\d+)x\d+", icon_path)
                        if m:
                            size = m.group(1)
                    r = await sb.shell.run(f"base64 {icon_path} 2>/dev/null")
                    if r.returncode == 0 and r.stdout.strip():
                        icon_bytes = base64.b64decode(r.stdout.strip())
                        ext = Path(icon_path).suffix or ".png"
                        local_path = output_dir / f"install-icon-{size}x{size}{ext}"
                        local_path.write_bytes(icon_bytes)
                        extracted_icons.append(str(local_path))
                        logger.info("Extracted icon: %s → %s", icon_path, local_path)
                except Exception as e:
                    logger.warning("Failed to extract icon %s: %s", icon_path, e)

        result["verification_passed"] = True
        result["extracted_icons"] = extracted_icons
        result_holder.append(result)
        return {
            "content": [
                {
                    "type": "text",
                    "text": (
                        f"OK: result accepted for {result.get('app_id')} on {result.get('os')}.\n"
                        f"Screenshot: {result.get('screenshot_verification', 'verified')}\n"
                        f"Metadata: binary={metadata.get('binary_path')}, name={metadata.get('display_name')}, "
                        f"version={metadata.get('version')}, icons={len(extracted_icons)} extracted"
                    ),
                }
            ]
        }

    return submit_result


async def create_app_installer(
    app_entry: dict,
    target_os: str,
    apps_dir: Path,
    *,
    model: str = "haiku",
    strict_install_verify: bool = True,
) -> dict | None:
    """Run a creator agent for one app x OS pair.

    Returns the result dict, or None on failure. Always cleans up sandboxes.
    """
    app_name = app_entry.get("name", "unknown")
    app_id = app_entry.get("id", app_name.lower().replace(" ", "-"))

    # All output for this app×OS goes in one directory
    app_os_dir = apps_dir / app_id / target_os
    logs_dir = app_os_dir / "logs"
    app_os_dir.mkdir(parents=True, exist_ok=True)
    logs_dir.mkdir(parents=True, exist_ok=True)

    # Per-agent isolated logger (not root — avoids cross-agent log pollution)
    agent_logger = logging.getLogger(f"creator.{app_id}.{target_os}")
    agent_logger.setLevel(logging.INFO)
    agent_log_path = logs_dir / "install-agent.log"
    file_handler = logging.FileHandler(str(agent_log_path), mode="w", encoding="utf-8")
    file_handler.setFormatter(
        logging.Formatter("%(asctime)s [%(levelname)s] %(name)s: %(message)s")
    )
    agent_logger.addHandler(file_handler)

    # Per-agent sandbox registry — no cross-agent interference
    my_sandboxes: dict[str, Any] = {}

    # Build MCP tools — screenshots + logs go into logs_dir
    sandbox_tools = make_sandbox_tools(evidence_dir=logs_dir, sandbox_registry=my_sandboxes)
    result_holder: list = []
    submit_tool = _make_submit_tool(
        result_holder,
        logs_dir,
        app_name,
        sandbox_registry=my_sandboxes,
        target_os=target_os,
        strict_install_verify=strict_install_verify,
    )

    all_tools = sandbox_tools + [submit_tool]
    server = create_sdk_mcp_server(name="sandbox", version="1.0.0", tools=all_tools)

    # Tool names for allowed_tools — NO Bash
    tool_names = [
        "WebSearch",
        "WebFetch",
        "mcp__sandbox__create_sandbox",
        "mcp__sandbox__sandbox_run",
        "mcp__sandbox__sandbox_write",
        "mcp__sandbox__sandbox_read",
        "mcp__sandbox__sandbox_screenshot",
        "mcp__sandbox__delete_sandbox",
        "mcp__sandbox__submit_result",
    ]

    prompt = (
        f"Create install and launch scripts for: {app_name}\n"
        f"Target OS: {target_os}\n"
        f"App metadata: {json.dumps(app_entry, default=str)}\n\n"
        f"Research the official install method, create scripts, test them in a sandbox, "
        f"LAUNCH the app, take a screenshot showing it running, and submit your result.\n"
        f"The submit_result tool will verify the screenshot — if the app isn't visibly open "
        f"it will reject and you must keep trying. Remember to delete the sandbox when done."
    )

    # Suppress noisy httpx request logging from sandbox HTTP calls
    logging.getLogger("httpx").setLevel(logging.WARNING)

    session_id = None
    try:
        async for msg in query(
            prompt=prompt,
            options=ClaudeAgentOptions(
                model=model,
                allowed_tools=tool_names,
                permission_mode="dontAsk",
                system_prompt=CREATOR_SYSTEM_PROMPT,
                mcp_servers={"sandbox": server},
            ),
        ):
            # Capture session ID for transcript export
            if hasattr(msg, "session_id") and msg.session_id:
                session_id = msg.session_id
    except Exception as e:
        logger.error("Creator agent failed for %s/%s: %s", app_id, target_os, e)
    finally:
        # Force cleanup any sandboxes this agent created (or forgot to delete)
        for sb_name in list(my_sandboxes):
            sb = my_sandboxes.pop(sb_name, None)
            if sb:
                try:
                    await sb.destroy()
                    logger.info("Force-cleaned sandbox %s for %s/%s", sb_name, app_id, target_os)
                except Exception as e:
                    logger.warning("Cleanup error for %s: %s", sb_name, e)

    # Copy session JSONL and convert to HTML transcript
    _save_transcript(session_id, logs_dir, app_id, target_os)

    # Remove per-agent file handler
    agent_logger.removeHandler(file_handler)
    file_handler.close()

    if result_holder:
        result = result_holder[0]

        # Early exit — no download available, just save the result
        if result.get("download_available") is False:
            app_os_dir.mkdir(parents=True, exist_ok=True)
            (app_os_dir / "result.json").write_text(
                json.dumps(result, indent=2, default=str, ensure_ascii=True), encoding="utf-8"
            )
            app_json = apps_dir / app_id / "app.json"
            if not app_json.exists():
                app_json.write_text(json.dumps(app_entry, indent=2, default=str), encoding="utf-8")
            logger.info(
                "Recorded %s/%s as not downloadable: %s", app_id, target_os, result.get("notes")
            )
            return result

        # Write scripts to disk
        install_script = result.get("install_script", "")
        launch_script = result.get("launch_script", "")

        ext = ".ps1" if target_os == "windows" else ".sh"
        if install_script:
            (app_os_dir / f"install{ext}").write_text(install_script, encoding="utf-8")
        if launch_script:
            (app_os_dir / f"launch{ext}").write_text(launch_script, encoding="utf-8")

        # Write extract_metadata script
        extract_script = result.get("extract_metadata_script", "")
        if extract_script:
            (app_os_dir / f"extract_metadata{ext}").write_text(extract_script, encoding="utf-8")

        # Write extracted metadata
        metadata = result.get("metadata")
        if metadata:
            (app_os_dir / "metadata.json").write_text(
                json.dumps(metadata, indent=2, default=str, ensure_ascii=True), encoding="utf-8"
            )

        # Write full result to logs
        (logs_dir / "install-result.json").write_text(
            json.dumps(result, indent=2, default=str, ensure_ascii=True), encoding="utf-8"
        )

        # Write app.json at the app level (shared across OS targets)
        app_json = apps_dir / app_id / "app.json"
        if not app_json.exists():
            app_json.write_text(json.dumps(app_entry, indent=2, default=str), encoding="utf-8")

        logger.info(
            "Created installer for %s/%s (verified=%s, screenshot=%s)",
            app_id,
            target_os,
            result.get("verification_passed"),
            result.get("screenshot_verification", "n/a"),
        )
        return result

    logger.warning("No result from creator agent for %s/%s", app_id, target_os)
    return None


async def create_installer_from_entry(
    entry: dict,
    target_os: str,
    apps_dir: Path,
    *,
    model: str = "haiku",
    strict_install_verify: bool = True,
) -> dict | None:
    """Convenience wrapper — create installer from a catalog entry dict."""
    return await create_app_installer(
        entry,
        target_os,
        apps_dir,
        model=model,
        strict_install_verify=strict_install_verify,
    )
