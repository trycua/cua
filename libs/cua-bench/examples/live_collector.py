"""Create an experimental live bundle using the synthetic external producer."""

import argparse
import json
import os
import subprocess
import sys
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path
from tempfile import TemporaryDirectory

from cua_bench.trace_bundle import TraceBundleWriter
from cua_bench.tracing import Tracing
from PIL import Image


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--delay", type=float, default=5, help="Seconds between observations")
    args = parser.parse_args()
    if args.delay < 0:
        parser.error("--delay must be nonnegative")
    output = args.output.expanduser().resolve()
    producer = Path(__file__).with_name("external_collector.py")
    result = subprocess.run(
        [sys.executable, str(producer), "--emit"],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
        env={**os.environ, "PYTHONIOENCODING": "utf-8"},
        timeout=30,
    )
    with TraceBundleWriter(output) as writer, TemporaryDirectory() as scratch:
        evidence = Path(scratch) / "collector.txt"
        evidence.write_text(
            "Synthetic collector evidence. No real network or device capture.\n", encoding="utf-8"
        )
        ref = writer.add_attachment(evidence, media_type="text/plain")
        trace = Tracing(None, bundle_writer=writer)
        trace.start("live-collector-example")
        epoch = datetime.now(timezone.utc)
        trace.record(
            "action",
            {"action_id": "submit-1", "synthetic": True},
            [Image.new("RGB", (320, 180), "navy")],
            context={"clock_domain": "synthetic", "aligned_at": epoch.isoformat()},
            attachments=[ref],
        )
        print(f'In another terminal: uv run cb trace view "{output}" --follow', flush=True)
        for line in result.stdout.splitlines():
            time.sleep(args.delay)
            event = json.loads(line)
            data = event["data"]
            trace.record(
                event["event_name"],
                data,
                context={
                    "clock_domain": "synthetic",
                    "source_timestamp_ms": data["source_timestamp_ms"],
                    "aligned_at": (
                        epoch + timedelta(milliseconds=data["source_timestamp_ms"])
                    ).isoformat(),
                    "action_id": "submit-1",
                },
            )
    print("Recording completed. The bundle remains available for review.")


if __name__ == "__main__":
    main()
