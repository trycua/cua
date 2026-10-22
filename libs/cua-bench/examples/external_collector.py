"""Record synthetic external events alongside an action, without a sandbox.

From libs/cua-bench, run:
    uv run python examples/external_collector.py --output /tmp/collector-trace
    uv run cb trace view /tmp/collector-trace

The --emit mode is a standalone JSONL producer. The parent replays its output
through Tracing.record; it does not install an OS or network collector.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path


def emit_events() -> None:
    """Emit deliberately out-of-order, fictional collector observations."""
    observations = [
        (
            "external.network.response",
            30,
            {"request_id": "request-1", "status": 200},
        ),
        (
            "external.application.operation",
            20,
            {"operation": "submit", "arguments": {"label": 'Demo <form> "✓"'}},
        ),
        (
            "external.network.request",
            10,
            {"request_id": "request-1", "method": "POST", "url": "https://example.test/submit"},
        ),
    ]
    for sequence, (event_name, timestamp, payload) in enumerate(observations):
        print(
            json.dumps(
                {
                    "event_name": event_name,
                    "data": {
                        "collector": "synthetic-jsonl",
                        "source_sequence": sequence,
                        "source_timestamp_ms": timestamp,
                        "clock_domain": "synthetic-session-relative",
                        "action_id": "submit-1",
                        "synthetic": True,
                        "payload": payload,
                    },
                },
                # ASCII JSON is valid UTF-8 regardless of the producer's locale.
                # json.loads restores the original Unicode payload in the parent.
                ensure_ascii=True,
            ),
            flush=True,
        )


def record_example(output: Path) -> None:
    """Import a finite collector batch through one recorder owner."""
    from cua_bench.tracing import Tracing
    from PIL import Image

    if output.exists():
        raise FileExistsError(f"Choose a new trace directory: {output}")

    collector = subprocess.run(
        [sys.executable, str(Path(__file__).resolve()), "--emit"],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=30,
    )
    tracing = Tracing(env=None)
    tracing.start("external-collector-example")
    tracing.record(
        "action",
        {"action_id": "submit-1", "action": "click", "target": "Submit", "synthetic": True},
        [Image.new("RGB", (160, 90), color="navy")],
    )
    for line in collector.stdout.splitlines():
        event = json.loads(line)
        tracing.record(event["event_name"], event["data"])
    tracing.save_to_disk(str(output))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--emit", action="store_true", help="Write synthetic JSONL to stdout")
    mode.add_argument("--output", type=Path, help="New directory for the combined trace")
    args = parser.parse_args()
    if args.emit:
        emit_events()
    else:
        output = args.output.expanduser().resolve()
        record_example(output)
        print(f"Saved trace to {output}")


if __name__ == "__main__":
    main()
