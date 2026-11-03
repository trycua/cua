"""Smoke test on a real Mac: boot one macOS VM from the bench image through the lume provider, wait until the
cua-driver driver says it's ready (its pinned executable, its pinned tools and the TCC grants), run AgentEnv's
computer conformance probe against it, and delete the VM.

    python smoke/conformance_smoke.py --base agentenv-macos-bench [--level full] [--json report.json] [--keep]

Exits 0 when every check passed, 1 when one failed, 2 when the computer never got ready. Needs lume, and a free VM
slot on this Mac (it refuses to start a third macOS VM).
"""

from __future__ import annotations

import argparse
import asyncio
import json
import secrets
import sys
from pathlib import Path

from agentenv_cua.lume.provider import LumeSandboxProvider
from agentenv_protocol import CONTROL_TOKEN_ENV
from agentenv_protocol.computer.conformance import run_probe

PORT = 18765


async def main(args: argparse.Namespace) -> int:
    provider = LumeSandboxProvider(base=args.base, prefix="agentenv-smoke")
    token = secrets.token_urlsafe(32)
    sandbox = await provider.create_vm(
        exposed_ports=[PORT], env={"ENVIRONMENT_NAME": "cua-driver", CONTROL_TOKEN_ENV: token}
    )
    url = sandbox.tunnel_urls[PORT]
    print(f"booted {sandbox.sandbox_id} at {url}", flush=True)
    try:
        report = await run_probe(
            url, args.level, timeout_s=60, ready_timeout_s=args.ready_timeout, token=token
        )
    except Exception as e:
        print(f"the computer at {url} didn't get ready: {type(e).__name__}: {e}", file=sys.stderr)
        return 2
    finally:
        if not args.keep:
            await sandbox.terminate()
    for check in report.checks:
        print(
            f"{check.status:>14} {check.check} {check.name}{': ' + check.detail if check.detail else ''}"
        )
    counts = report.counts()
    print(f"{report.driver}: {counts}")
    if args.json:
        Path(args.json).write_text(
            json.dumps(
                (
                    report.model_dump(mode="json")
                    if hasattr(report, "model_dump")
                    else report.__dict__
                ),
                indent=1,
                default=str,
            )
        )
    return 1 if counts.get("failed") else 0


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--base", required=True, help="the local lume VM of the bench image")
    parser.add_argument("--level", choices=("basic", "full"), default="full")
    parser.add_argument("--ready-timeout", type=float, default=300)
    parser.add_argument("--json", help="write the probe's report here")
    parser.add_argument("--keep", action="store_true", help="leave the VM running for a look")
    sys.exit(asyncio.run(main(parser.parse_args())))
