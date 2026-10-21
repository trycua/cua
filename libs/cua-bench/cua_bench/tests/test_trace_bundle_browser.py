"""Opt-in real browser acceptance; no live desktop or collector required.

CUA_BENCH_BROWSER_TESTS=1 runs this with Node and Playwright installed.
CUA_BENCH_PLAYWRIGHT and CUA_BENCH_CHROME can select local installations.
"""

import os
import queue
import subprocess
import threading
from pathlib import Path

import pytest
from cua_bench.trace_bundle import TraceBundleWriter
from cua_bench.trace_live_viewer import serve_bundle
from cua_bench.tracing import Tracing
from PIL import Image


@pytest.mark.skipif(
    os.environ.get("CUA_BENCH_BROWSER_TESTS") != "1", reason="opt-in browser acceptance"
)
def test_live_browser(tmp_path):
    source = tmp_path / "evidence.txt"
    source.write_text("<script>window.capturedScriptRan=true</script>\n✓", encoding="utf-8")
    with TraceBundleWriter(tmp_path / "bundle") as writer:
        trace = Tracing(None, bundle_writer=writer)
        trace.start("browser-fixture")
        ref = writer.add_attachment(source, media_type="text/plain")
        trace.record(
            "response",
            {"status": 200},
            context={"clock_domain": "fixture", "aligned_at": "2026-01-01T00:00:03Z"},
            data_images=[Image.new("RGB", (32, 24), "navy")],
            attachments=[ref],
        )
        trace.record(
            "request", {}, context={"clock_domain": "fixture", "aligned_at": "2026-01-01T00:00:02Z"}
        )
        with serve_bundle(writer.path, follow=True) as url:
            script = Path(__file__).parent / "browser" / "live_viewer.cjs"
            process = subprocess.Popen(
                [os.environ.get("CUA_BENCH_NODE", "node"), str(script), url],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
            )
            lines = queue.Queue()

            def read_output():
                for line in process.stdout:
                    lines.put(line.strip())
                lines.put(None)

            thread = threading.Thread(target=read_output, daemon=True)
            thread.start()
            output = []
            try:
                while True:
                    line = lines.get(timeout=60)
                    if line is None:
                        break
                    output.append(line)
                    if line == "append":
                        trace.record(
                            "late observation",
                            {},
                            context={
                                "clock_domain": "fixture",
                                "aligned_at": "2026-01-01T00:00:01Z",
                            },
                        )
                        process.stdin.write("ok\n")
                        process.stdin.flush()
                    elif line == "finish":
                        writer.close()
                        process.stdin.write("ok\n")
                        process.stdin.flush()
                assert process.wait(timeout=10) == 0, "\n".join(output)
                assert "passed" in output, "\n".join(output)
            finally:
                if process.poll() is None:
                    process.kill()
                process.wait(timeout=10)
                process.stdin.close()
                process.stdout.close()
                thread.join(timeout=5)
