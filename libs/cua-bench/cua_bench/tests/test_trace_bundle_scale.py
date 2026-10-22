"""100,000-event reader acceptance, with a bounded Python allocation budget."""

import json
import time
import tracemalloc

import pytest
from cua_bench.trace_bundle import TraceBundleReader, TraceBundleWriter


@pytest.fixture
def large_bundle(tmp_path):
    with TraceBundleWriter(tmp_path / "bundle") as writer:
        # Bulk fixture construction avoids measuring the producer or dataset library.
        for sequence in range(100_000):
            event = {
                "sequence": sequence,
                "trajectory_id": "large",
                "event_name": "sample",
                "recorded_at": "2026-01-01T00:00:00Z",
                "context": {},
                "data": {"payload": "x" * 1024},
                "images": [],
                "attachments": [],
            }
            writer.journal.write((json.dumps(event) + "\n").encode())
        writer.sequence = 100_000
    return writer.path


def test_large_trace_disk_index(large_bundle):
    started = time.perf_counter()
    tracemalloc.start()
    try:
        with TraceBundleReader(large_bundle) as reader:
            indexed = time.perf_counter()
            assert reader.status()["total"] == 100_000
            page = reader.page(order="observed", selected=99_999)
            assert page["page"] == 999
            assert len(page["events"]) == 100
            assert page["events"][-1]["sequence"] == 99_999
            assert len(json.dumps(page)) < 100_000
            assert len(reader.batch(99_499)["events"]) == 500
            _, peak = tracemalloc.get_traced_memory()
            assert peak < 16 * 1024 * 1024
            print(
                f"100000 events: index={indexed - started:.2f}s; page/batch={time.perf_counter() - indexed:.2f}s; Python peak={peak / 1024**2:.2f} MiB"
            )
    finally:
        tracemalloc.stop()


@pytest.mark.filterwarnings("ignore:datetime.datetime.utcnow:DeprecationWarning")
def test_disk_only_producer_does_not_retain_attachment_registry(tmp_path):
    from cua_bench.tracing import Tracing

    source = tmp_path / "evidence"
    source.write_bytes(b"synthetic")
    with TraceBundleWriter(tmp_path / "bundle") as writer:
        trace = Tracing(None, bundle_writer=writer, retain_rows=False)
        trace.start("long-session")
        # Warm imports and serializer caches before measuring retained growth.
        ref = writer.add_attachment(source)
        trace.record("evidence", {}, attachments=[ref])
        tracemalloc.start()
        try:
            for _ in range(3000):
                ref = writer.add_attachment(source)
                trace.record("evidence", {}, attachments=[ref])
            assert trace._rows == []
            current, peak = tracemalloc.get_traced_memory()
            assert current < 512 * 1024
            assert peak < 1024 * 1024
        finally:
            tracemalloc.stop()
    with TraceBundleReader(writer.path) as reader:
        assert reader.status()["total"] == 3001
        last = reader.event(3000)["attachments"][0]
        with reader.attachment(last["id"])[0] as handle:
            assert handle.read() == b"synthetic"
