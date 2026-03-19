"""Cold start benchmark for cloud sandboxes.

Measures provisioning time for each OS type across 2 attempts and
exports results as GitHub Actions env vars for Slack notification.
"""

import asyncio
import json
import os
import time
import urllib.request

from cua_sandbox import Image, sandbox

OS_CONFIGS = [
    ("linux", Image.linux()),
    ("android", Image.android("14")),
]

ATTEMPTS = 2


async def measure_cold_start(image, timeout: float = 120) -> tuple[float | None, str | None]:
    """Create an ephemeral sandbox and return (seconds, error)."""
    t0 = time.time()
    try:
        async with sandbox(image=image) as sb:
            elapsed = time.time() - t0
            # Quick health check
            screenshot = await sb.screenshot()
            if len(screenshot) < 500:
                return elapsed, "screenshot too small"
            return elapsed, None
    except Exception as e:
        return time.time() - t0, str(e)[:80]


def color_for_time(seconds: float | None) -> str:
    if seconds is None:
        return "🔴"
    if seconds < 15:
        return "🟢"
    if seconds < 60:
        return "🟡"
    return "🔴"


def fmt(seconds: float | None, error: str | None) -> str:
    if error:
        return f"❌ {error}"
    return f"{seconds:.1f}s"


async def main():
    results: dict[str, list[tuple[float | None, str | None]]] = {}

    for os_name, image in OS_CONFIGS:
        results[os_name] = []
        for attempt in range(ATTEMPTS):
            print(f"[{os_name}] attempt {attempt + 1}/{ATTEMPTS}...")
            elapsed, error = await measure_cold_start(image)
            results[os_name].append((elapsed, error))
            if error:
                print(f"  ❌ {elapsed:.1f}s - {error}")
            else:
                print(f"  ✅ {elapsed:.1f}s")

    # Build Slack message as a table
    lines = ["*☁️ Cloud Sandbox Cold Start Benchmark*\n"]
    lines.append("```")
    lines.append(f"{'OS':<12} {'Attempt 1':<20} {'Attempt 2':<20}")
    lines.append(f"{'─'*12} {'─'*20} {'─'*20}")

    worst_color = "#36a64f"  # green
    for os_name in [c[0] for c in OS_CONFIGS]:
        cols = []
        for elapsed, error in results[os_name]:
            icon = color_for_time(elapsed if not error else None)
            cols.append(f"{icon} {fmt(elapsed, error)}")
            # Track worst color
            if error:
                worst_color = "#dc3545"
            elif elapsed and elapsed >= 60 and worst_color != "#dc3545":
                worst_color = "#ffa500"
            elif elapsed and elapsed >= 15 and worst_color == "#36a64f":
                worst_color = "#ffa500"
        lines.append(f"{os_name:<12} {cols[0]:<20} {cols[1]:<20}")

    lines.append("```")

    msg = "\n".join(lines)
    print(f"\n{msg}")

    # Export to GitHub Actions
    gh_env = os.environ.get("GITHUB_ENV")
    if gh_env:
        with open(gh_env, "a") as f:
            f.write(f"SLACK_MESSAGE<<EOF\n{msg}\nEOF\n")
            f.write(f"SLACK_COLOR={worst_color}\n")

    # Send directly via webhook (local runs or CI with SLACK_WEBHOOK set)
    webhook_url = os.environ.get("SLACK_WEBHOOK")
    if webhook_url and not gh_env:
        payload = json.dumps(
            {"text": msg, "attachments": [{"color": worst_color, "text": ""}]}
        ).encode()
        req = urllib.request.Request(
            webhook_url, data=payload, headers={"Content-Type": "application/json"}
        )
        with urllib.request.urlopen(req) as resp:
            print(f"\nSlack webhook sent: {resp.status}")


if __name__ == "__main__":
    asyncio.run(main())
