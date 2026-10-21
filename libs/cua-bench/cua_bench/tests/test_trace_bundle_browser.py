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

from .test_trace_bundle_scale import large_bundle as _large_bundle

large_bundle = _large_bundle


@pytest.mark.skipif(
    os.environ.get("CUA_BENCH_BROWSER_TESTS") != "1", reason="opt-in browser acceptance"
)
def test_live_browser(tmp_path):
    source = tmp_path / "evidence.txt"
    source.write_bytes("<script>window.capturedScriptRan=true</script>\n✓".encode("utf-8"))
    with TraceBundleWriter(tmp_path / "bundle") as writer:
        trace = Tracing(None, bundle_writer=writer)
        trace.start("browser-fixture")
        ref = writer.add_attachment(source, media_type="text/plain")
        trace.record(
            "response",
            {
                "status": 200,
                "snapshot": {
                    "windows": [
                        {
                            "html": '<html data-window-screen-x="2"><button aria-label="Submit" data-bbox-x="1" data-bbox-y="2" data-bbox-width="10" data-bbox-height="10">Submit</button></html>'
                        }
                    ]
                },
            },
            context={"clock_domain": "fixture", "aligned_at": "2026-01-01T00:00:03Z"},
            data_images=[Image.new("RGB", (32, 24), "navy")],
            attachments=[ref],
        )
        trace.record(
            "request",
            {},
            context={"clock_domain": "fixture", "aligned_at": "2026-01-01T00:00:02Z"},
            data_images=[Image.new("RGB", (16, 16), "green")],
        )
        with serve_bundle(writer.path, follow=True) as url:
            script = Path(__file__).parent / "browser" / "live_viewer.cjs"
            process = subprocess.Popen(
                [os.environ.get("CUA_BENCH_NODE", "node"), str(script), url],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                encoding="utf-8",
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
                        trace.record(
                            "equal other clock",
                            {},
                            context={"clock_domain": "other", "aligned_at": "2026-01-01T00:00:03Z"},
                        )
                        trace.record("unaligned b", {}, context={"clock_domain": "b"})
                        trace.record("unaligned a", {}, context={"clock_domain": "a"})
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


@pytest.mark.skipif(
    os.environ.get("CUA_BENCH_BROWSER_TESTS") != "1", reason="opt-in browser acceptance"
)
def test_large_browser(large_bundle):
    with serve_bundle(large_bundle) as url:
        result = subprocess.run(
            [
                os.environ.get("CUA_BENCH_NODE", "node"),
                str(Path(__file__).parent / "browser" / "live_viewer.cjs"),
                url,
                "large",
            ],
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=60,
        )
        assert result.returncode == 0, result.stdout + result.stderr
        assert "passed" in result.stdout
        print(result.stdout)
