"""Behavior tests for the experimental bundle and real loopback reader."""

import json
from urllib.error import HTTPError
from urllib.parse import urlsplit
from urllib.request import ProxyHandler, Request, build_opener

import pytest
from cua_bench.trace_bundle import TraceBundleReader, TraceBundleWriter
from cua_bench.trace_live_viewer import serve_bundle
from cua_bench.tracing import Tracing
from datasets import load_from_disk
from PIL import Image


def record(writer, name="action", **kwargs):
    trace = Tracing(None, bundle_writer=writer)
    trace.start("trajectory")
    trace.record(name, {"text": '<script>"✓"</script>'}, **kwargs)
    return trace


def get(url, path, *, authenticated=True):
    parts = urlsplit(url)
    headers = {"Authorization": "Bearer " + parts.fragment} if authenticated else {}
    return build_opener(ProxyHandler({})).open(
        Request(f"{parts.scheme}://{parts.netloc}{path}", headers=headers), timeout=10
    )


def test_live_roundtrip_and_legacy_export(tmp_path):
    with TraceBundleWriter(tmp_path / "bundle") as writer:
        trace = record(
            writer,
            context={"clock_domain": "device", "aligned_at": "2026-01-01T01:00:00+01:00"},
            data_images=[Image.new("RGB", (32, 24), "red")],
        )
        reader = TraceBundleReader(writer.path)
        batch = reader.batch()
        assert batch["state"] == "recording"
        assert batch["events"][0]["context"]["aligned_at"] == "2026-01-01T00:00:00.000+00:00"
        trace.start("second")
        trace.record(
            "late", {}, context={"clock_domain": "device", "aligned_at": "2025-12-31T23:59:59Z"}
        )
        assert reader.batch(0)["events"][0]["sequence"] == 1
        trace.save_to_disk(str(tmp_path / "legacy"))
        ds = load_from_disk(str(tmp_path / "legacy"))
        assert set(ds.column_names) == {
            "event_name",
            "data_json",
            "data_images",
            "trajectory_id",
            "timestamp",
        }
        assert list(ds["event_name"]) == ["late"]
        image = batch["events"][0]["images"][0]
        with reader.attachment(image["id"])[0] as handle:
            assert Image.open(handle).size == (32, 24)
    assert reader.batch(1)["state"] == "completed"
    assert reader.batch(1)["final_sequence"] == 1
    with pytest.raises(RuntimeError, match="closed or failed"):
        trace.record("after-close", {})


def test_partial_line_and_corruption(tmp_path):
    writer = TraceBundleWriter(tmp_path / "bundle")
    record(writer)
    reader = TraceBundleReader(writer.path)
    with (writer.path / "events.jsonl").open("ab") as handle:
        handle.write(b'{"sequence":')
    assert reader.batch(0)["events"] == []
    with (writer.path / "events.jsonl").open("ab") as handle:
        handle.write(b"not-json}\n")
    with pytest.raises(ValueError):
        reader.batch(0)
    writer.close(failed=True)


def test_lifecycle_and_generation(tmp_path):
    with pytest.raises(RuntimeError, match="synthetic failure"):
        with TraceBundleWriter(tmp_path / "bundle") as writer:
            record(writer)
            raise RuntimeError("synthetic failure")
    reader = TraceBundleReader(writer.path)
    assert reader.batch()["state"] == "failed"
    writer.close()
    with pytest.raises(FileExistsError):
        TraceBundleWriter(writer.path)
    with pytest.raises(ValueError, match="generation changed"):
        reader.batch(generation="different")
    (writer.path / "events.jsonl").write_bytes(b"")
    with pytest.raises(ValueError, match="truncated"):
        reader.batch()


def test_attachment_integrity_quotas_and_foreign_refs(tmp_path):
    source = tmp_path / "evidence.txt"
    source.write_text("evidence", encoding="utf-8")
    with (
        TraceBundleWriter(tmp_path / "one", max_attachment=10) as one,
        TraceBundleWriter(tmp_path / "two") as two,
    ):
        attachment = one.add_attachment(source, media_type="text/plain")
        with pytest.raises(ValueError, match="does not belong"):
            record(two, attachments=[attachment])
        record(one, attachments=[attachment])
        reader = TraceBundleReader(one.path)
        with reader.attachment(attachment["id"])[0] as handle:
            assert handle.read() == b"evidence"
        (one.path / "artifacts" / attachment["id"]).write_bytes(b"modified")
        with pytest.raises(ValueError, match="digest mismatch"):
            reader.attachment(attachment["id"])
        with pytest.raises(ValueError, match="unknown attachment"):
            reader.attachment("../../secret")
        source.write_text("too many bytes")
        with pytest.raises(ValueError, match="quota exceeded"):
            one.add_attachment(source)
        assert not list((one.path / "artifacts").glob("*.tmp"))


def test_symlink_refusal(tmp_path):
    source = tmp_path / "evidence"
    source.write_text("fixture")
    link = tmp_path / "link"
    try:
        link.symlink_to(source)
    except OSError:
        pytest.skip("host cannot create symlink fixture")
    with TraceBundleWriter(tmp_path / "bundle") as writer:
        with pytest.raises(ValueError, match="non-symlink"):
            writer.add_attachment(link)
        ref = writer.add_attachment(source)
        record(writer, attachments=[ref])
        path = writer.path / "artifacts" / ref["id"]
        path.unlink()
        path.symlink_to(source)
        with pytest.raises(ValueError, match="unavailable"):
            TraceBundleReader(writer.path).attachment(ref["id"])


@pytest.mark.parametrize(
    "context",
    [
        {"aligned_at": "yesterday"},
        {"aligned_at": "2026-01-01T00:00:00"},
        {"aligned_at": "2026-01-01T00:00:00Z"},
    ],
)
def test_invalid_clock_does_not_publish(tmp_path, context):
    with TraceBundleWriter(tmp_path / "bundle") as writer:
        with pytest.raises(ValueError):
            record(writer, context=context)
        assert TraceBundleReader(writer.path).batch()["events"] == []


def test_loopback_auth_cursor_attachment_and_snapshot(tmp_path):
    source = tmp_path / "evidence"
    source.write_bytes(b"\x00binary evidence")
    with TraceBundleWriter(tmp_path / "bundle") as writer:
        ref = writer.add_attachment(source, name='quote".bin')
        record(writer, attachments=[ref])
        with serve_bundle(writer.path, follow=True) as url:
            with get(url, "/", authenticated=False) as response:
                assert b"Cua Bench trace" in response.read()
                assert "script-src 'self'" in response.headers["Content-Security-Policy"]
            with pytest.raises(HTTPError) as error:
                get(url, "/events", authenticated=False)
            assert error.value.code == 403
            with get(url, "/events") as response:
                initial = json.load(response)
            record(writer, "second")
            path = "/events?after=0&generation=" + initial["generation"]
            with get(url, path) as response:
                update = json.load(response)
            assert [event["event_name"] for event in update["events"]] == ["second"]
            with get(url, path) as response:
                assert json.load(response) == update  # Reconnect/replay uses stable identities.
            with get(url, "/attachment/" + ref["id"]) as response:
                assert response.read() == source.read_bytes()
                assert response.headers["Content-Disposition"].startswith("attachment;")
            with pytest.raises(HTTPError) as error:
                get(url, "/events?after=999")
            assert error.value.code == 409
        with serve_bundle(writer.path, follow=False) as url:
            record(writer, "not-in-snapshot")
            with get(url, "/events") as response:
                snapshot = json.load(response)
            assert [e["event_name"] for e in snapshot["events"]] == ["action", "second"]
            assert snapshot["state"] == "snapshot"


def test_legacy_follow_refuses_without_starting_server(tmp_path, capsys):
    from argparse import Namespace

    from cua_bench.cli.commands.trace import cmd_view

    assert cmd_view(Namespace(identifier=str(tmp_path), follow=True)) == 1
    assert "requires a live trace bundle" in capsys.readouterr().out


def test_stable_pages_clock_groups_and_image_navigation(tmp_path):
    with TraceBundleWriter(tmp_path / "bundle") as writer:
        trace = Tracing(None, bundle_writer=writer, retain_rows=False)
        trace.start()
        for i in range(205):
            trace.record(
                str(i),
                {"large": "x" * 2048},
                data_images=[Image.new("RGB", (8, 8))] if i in (0, 102, 204) else [],
                context={"clock_domain": "a" if i % 2 else "b"},
            )
        with TraceBundleReader(writer.path) as reader:
            first = reader.page(order="observed", through=204, selected=102)
            assert 102 in [event["sequence"] for event in first["events"]]
            assert all("data" not in event for event in first["events"])
            trace.record(
                "late", {}, context={"clock_domain": "utc", "aligned_at": "2026-01-01T00:00:00Z"}
            )
            reader.refresh()
            assert reader.page(order="observed", through=204, selected=102) == first
            assert reader.page(order="observed")["events"][0]["event_name"] == "late"
            assert reader.image_neighbor(0, direction=1)["sequence"] == 102
            assert reader.image_neighbor(102, direction=-1)["sequence"] == 0
            assert reader.image_neighbor(204, direction=1) is None
            assert reader.page(page=2)["events"][-1]["sequence"] == 205
        assert trace._rows == []
        with pytest.raises(RuntimeError, match="dataset export requires"):
            _ = trace.dataset


def test_bundle_scope_finalizes_and_restores_recorder(tmp_path):
    trace = Tracing(None)
    with pytest.raises(RuntimeError, match="task failed"):
        with trace.bundle(tmp_path / "failed", retain_rows=False):
            trace.start()
            trace.record("before failure", {})
            raise RuntimeError("task failed")
    assert trace.bundle_writer is None and trace.retain_rows
    with TraceBundleReader(tmp_path / "failed") as reader:
        assert reader.status()["state"] == "failed"
    with trace.bundle(tmp_path / "next"):
        trace.start()
        trace.record("success", {})
    with TraceBundleReader(tmp_path / "next") as reader:
        assert reader.status()["state"] == "completed"


def test_snapshot_bounds_are_inert_and_follow_event_identity(tmp_path):
    with TraceBundleWriter(tmp_path / "bundle") as writer:
        trace = Tracing(None, bundle_writer=writer)
        trace.start()
        trace.record(
            "snapshot",
            {
                "snapshot": {
                    "windows": [
                        {
                            "html": '<html data-window-screen-x="10" data-window-screen-y="20"><body><button aria-label="Save &amp; submit" data-bbox-x="1" data-bbox-y="2" data-bbox-width="30" data-bbox-height="40">Save</button><img src="https://example.invalid/captured" onload="alert(1)"></body></html>'
                        }
                    ]
                }
            },
            [Image.new("RGB", (100, 100))],
        )
        with TraceBundleReader(writer.path) as reader:
            assert reader.event(0)["snapshot_boxes"] == [
                {"x": 11, "y": 22, "width": 30, "height": 40, "label": "Save & submit"}
            ]


def test_failed_journal_write_refuses_future_records(tmp_path, monkeypatch):
    with TraceBundleWriter(tmp_path / "bundle") as writer:

        def fail(*args):
            raise OSError("synthetic disk full")

        with monkeypatch.context() as scoped:
            scoped.setattr(writer.journal, "write", fail)
            with pytest.raises(OSError, match="synthetic disk full"):
                record(writer)
        with pytest.raises(RuntimeError, match="closed or failed"):
            record(writer)
    with TraceBundleReader(writer.path) as reader:
        assert reader.status()["state"] == "failed"


def test_completed_partial_journal_and_unknown_version_refuse(tmp_path):
    with TraceBundleWriter(tmp_path / "bundle") as writer:
        record(writer)
    with (writer.path / "events.jsonl").open("ab") as journal:
        journal.write(b"{")
    with pytest.raises(ValueError, match="completed trace is incomplete"):
        TraceBundleReader(writer.path)
    manifest_path = writer.path / "trace.json"
    manifest = json.loads(manifest_path.read_text())
    manifest["version"] = 999
    manifest_path.write_text(json.dumps(manifest))
    with pytest.raises(ValueError, match="unsupported trace bundle version"):
        TraceBundleReader(writer.path)


def test_page_watermark_and_generation_reset_over_http(tmp_path):
    with TraceBundleWriter(tmp_path / "bundle") as writer:
        record(writer)
        with serve_bundle(writer.path, follow=True) as url:
            with get(url, "/status") as response:
                initial = json.load(response)
            record(writer, "late")
            with get(url, "/page?through=0&order=observed") as response:
                assert [e["event_name"] for e in json.load(response)["events"]] == ["action"]
            with get(url, "/event?through=0&sequence=0") as response:
                assert json.load(response)["event_name"] == "action"
            with pytest.raises(HTTPError) as error:
                get(url, "/event?through=0&sequence=1")
            assert error.value.code == 409
            manifest_path = writer.path / "trace.json"
            manifest = json.loads(manifest_path.read_text())
            manifest["generation"] = "replaced-generation"
            manifest_path.write_text(json.dumps(manifest))
            with pytest.raises(HTTPError) as error:
                get(url, "/status?generation=" + initial["generation"])
            assert error.value.code == 409
            assert json.load(error.value)["error"] == "trace generation changed; reopen the viewer"


def test_live_example_with_non_utf8_parent_locale(tmp_path):
    import os
    import subprocess
    import sys
    from pathlib import Path

    example = Path(__file__).resolve().parents[2] / "examples" / "live_collector.py"
    output = tmp_path / "example"
    subprocess.run(
        [sys.executable, str(example), "--output", str(output), "--delay", "0"],
        env={**os.environ, "PYTHONIOENCODING": "ascii", "PYTHONUTF8": "0"},
        check=True,
        capture_output=True,
        timeout=60,
    )
    with TraceBundleReader(output) as reader:
        assert reader.status()["state"] == "completed"
        assert reader.status()["total"] == 4
        assert reader.event(2)["data"]["payload"]["arguments"]["label"] == 'Demo <form> "✓"'
        assert [e["sequence"] for e in reader.page(order="observed")["events"]] == [0, 3, 2, 1]


@pytest.mark.parametrize("save_pngs", [False, True])
def test_disk_only_export_refuses_even_after_scope(tmp_path, save_pngs):
    trace = Tracing(None)
    trace.start("before")
    trace.record("before", {})
    with trace.bundle(tmp_path / "bundle", retain_rows=False):
        trace.record("disk only", {})
    with pytest.raises(RuntimeError, match="entire trajectory"):
        trace.save_to_disk(str(tmp_path / "export"), save_pngs=save_pngs)
    assert not (tmp_path / "export").exists()
    trace.start("fresh")
    trace.record("new", {})
    assert len(trace.dataset) == 1


@pytest.mark.asyncio
@pytest.mark.parametrize("outcome", ["success", "failure", "cancel"])
async def test_disk_only_episode_lifecycle(tmp_path, outcome, monkeypatch):
    import asyncio
    import io
    import sys
    from types import SimpleNamespace

    from cua_bench.runner.episode import AgentOptions, run_episode

    # Redirected Windows output uses CP1252; logging must not abort lifecycle.
    stream = io.TextIOWrapper(io.BytesIO(), encoding="cp1252", write_through=True)
    monkeypatch.setattr(sys, "stdout", stream)
    trace = Tracing(None)

    async def reset(**kwargs):
        trace.record("setup", {})
        return None, SimpleNamespace(description="fixture")

    async def solve(*args):
        trace.record("action", {})
        assert trace._rows == []
        if outcome == "cancel":
            raise asyncio.CancelledError()
        if outcome == "failure":
            raise RuntimeError("fixture failure")

    env = SimpleNamespace(
        tracing=trace,
        reset=reset,
        session=SimpleNamespace(_cb_no_screen=True),
        solve_task_fn=solve,
        evaluate_task_fn=None,
    )
    call = run_episode(env, 0, AgentOptions(trace_only=True), tmp_path)
    if outcome == "success":
        result = await call
        assert result.extra["trace"] == {
            "format": "bundle",
            "legacy_exports": False,
            "path": "task_0_bundle",
            "state": "completed",
        }
    else:
        with pytest.raises(asyncio.CancelledError if outcome == "cancel" else RuntimeError):
            await call
    assert trace._rows == [] and trace.bundle_writer is None and trace.retain_rows
    assert not (tmp_path / "task_0_trace").exists()
    assert not (tmp_path / "trajectory.json").exists()
    with TraceBundleReader(tmp_path / "task_0_bundle") as reader:
        assert reader.status()["state"] == ("completed" if outcome == "success" else "failed")
        assert reader.event(1)["event_name"] == "action"


@pytest.mark.asyncio
async def test_disk_only_rejects_export_options_before_start(tmp_path):
    from types import SimpleNamespace

    from cua_bench.runner.episode import AgentOptions, run_episode

    with pytest.raises(
        ValueError, match="trace_only cannot be combined with legacy export options"
    ):
        await run_episode(
            SimpleNamespace(), 0, AgentOptions(trace_only=True, save_pngs=True), tmp_path
        )
    assert list(tmp_path.iterdir()) == []


def test_related_evidence_explicit_provenance_and_watermark(tmp_path):
    correlation = {
        "namespace": "fixture",
        "kind": "request",
        "id": "req-1",
        "provenance": "collector-1",
    }
    with TraceBundleWriter(tmp_path / "bundle") as writer:
        trace = Tracing(None, bundle_writer=writer, retain_rows=False)
        trace.start("one")
        trace.record("action", {}, context={"correlations": [correlation]})
        for i in range(105):
            trace.record(f"evidence-{i}", {}, context={"correlations": [correlation]})
        trace.record(
            "unrelated source",
            {},
            context={"correlations": [{**correlation, "provenance": "other"}]},
        )
        trace.record("payload only", {"request_id": "req-1"})
        trace.start("another trajectory")
        trace.record("unrelated trajectory", {}, context={"correlations": [correlation]})
        with TraceBundleReader(writer.path) as reader:
            first = reader.related(0, through=105)
            assert first["total"] == 105 and len(first["events"]) == 100
            assert [e["sequence"] for e in reader.related(0, page=1)["events"]] == list(
                range(101, 106)
            )
            trace.start("one")
            trace.record(
                "late trace artifact",
                {},
                context={
                    "correlations": [correlation],
                    "clock_domain": "fixture",
                    "aligned_at": "2026-01-01T00:00:00Z",
                },
            )
            reader.refresh()
            assert reader.related(0, through=105) == first
            assert (
                reader.related(0, order="observed")["events"][0]["event_name"]
                == "late trace artifact"
            )
        with serve_bundle(writer.path, follow=True) as url:
            with get(url, "/related?sequence=0&through=105&page=1") as response:
                assert len(json.load(response)["events"]) == 5
            with pytest.raises(HTTPError) as error:
                get(url, "/related?sequence=106&through=105")
            assert error.value.code == 409


@pytest.mark.parametrize(
    "correlations", [{}, [dict(id="x")], [dict(namespace="n", kind="k", id="x", provenance="")]]
)
def test_invalid_correlation_refuses_before_append(tmp_path, correlations):
    with TraceBundleWriter(tmp_path / "bundle") as writer:
        with pytest.raises(ValueError, match="correlation"):
            record(writer, context={"correlations": correlations})
        assert writer.sequence == 0


def test_download_ticket_authorization_preview_and_integrity(tmp_path, monkeypatch):
    from cua_bench import trace_live_viewer

    source = tmp_path / "text"
    source.write_bytes(b"<script>inert</script>" + b"x" * 300000)
    with TraceBundleWriter(tmp_path / "bundle") as writer:
        ref = writer.add_attachment(source, media_type="text/plain")
        record(writer, attachments=[ref])
        with serve_bundle(writer.path) as url:
            with pytest.raises(HTTPError) as error:
                get(url, "/download-ticket?id=" + ref["id"], authenticated=False)
            assert error.value.code == 403
            with get(url, "/preview/" + ref["id"]) as response:
                assert len(response.read()) == 256 * 1024
            with get(url, "/download-ticket?id=" + ref["id"]) as response:
                path = json.load(response)["url"]
            with get(url, path, authenticated=False) as response:
                assert response.read() == source.read_bytes()
                assert response.headers["Content-Type"] == "application/octet-stream"
                assert response.headers["X-Content-Type-Options"] == "nosniff"
            with pytest.raises(HTTPError) as error:
                get(url, path, authenticated=False)
            assert error.value.code == 403
            with get(url, "/download-ticket?id=" + ref["id"]) as response:
                path = json.load(response)["url"]
            now = trace_live_viewer.time.monotonic()
            with monkeypatch.context() as scoped:
                scoped.setattr(trace_live_viewer.time, "monotonic", lambda: now + 61)
                with pytest.raises(HTTPError) as error:
                    get(url, path, authenticated=False)
                assert error.value.code == 403
            # Integrity failures never publish content, including through a ticket.
            (writer.path / "artifacts" / ref["id"]).write_bytes(b"y" * source.stat().st_size)
            with get(url, "/download-ticket?id=" + ref["id"]) as response:
                path = json.load(response)["url"]
            with pytest.raises(HTTPError) as error:
                get(url, path, authenticated=False)
            assert error.value.code == 409
            assert json.load(error.value)["error"] == "attachment digest mismatch"


def test_large_download_has_explicit_limits_and_bounded_memory(tmp_path):
    import hashlib
    import tracemalloc

    from cua_bench.trace_bundle import MAX_ATTACHMENT

    source = tmp_path / "large"
    size = MAX_ATTACHMENT + 1
    with source.open("wb") as handle:
        handle.truncate(size)
    tracemalloc.start()
    try:
        with TraceBundleWriter(tmp_path / "bundle", max_attachment=size) as writer:
            ref = writer.add_attachment(source)
            record(writer, attachments=[ref])
            with serve_bundle(writer.path) as url:
                with pytest.raises(HTTPError) as error:
                    get(url, "/download-ticket?id=" + ref["id"])
                assert error.value.code == 409
                assert json.load(error.value)["error"] == "attachment exceeds viewer download limit"
            with serve_bundle(writer.path, max_download=size) as url:
                with get(url, "/download-ticket?id=" + ref["id"]) as response:
                    path = json.load(response)["url"]
                digest = hashlib.sha256()
                count = 0
                with get(url, path, authenticated=False) as response:
                    while chunk := response.read(65536):
                        count += len(chunk)
                        digest.update(chunk)
                assert count == size and digest.hexdigest() == ref["sha256"]
        assert tracemalloc.get_traced_memory()[1] < 16 * 1024**2
    finally:
        tracemalloc.stop()


def test_ticket_admission_and_preview_type_limits(tmp_path):
    source = tmp_path / "image"
    with source.open("wb") as handle:
        handle.truncate(8 * 1024**2 + 1)
    with TraceBundleWriter(tmp_path / "bundle") as writer:
        image = writer.add_attachment(source, media_type="image/png")
        html = writer.add_attachment(source, media_type="text/html")
        record(writer, attachments=[image, html])
        with serve_bundle(writer.path) as url:
            for ref, message in (
                (image, "image exceeds 8 MiB preview limit; download instead"),
                (html, "attachment is download-only"),
            ):
                with pytest.raises(HTTPError) as error:
                    get(url, "/preview/" + ref["id"])
                assert error.value.code == 409
                assert json.load(error.value)["error"] == message
            for _ in range(16):
                with get(url, "/download-ticket?id=" + image["id"]) as response:
                    assert json.load(response)["url"].startswith("/download/")
            with pytest.raises(HTTPError) as error:
                get(url, "/download-ticket?id=" + image["id"])
            assert error.value.code == 409
            assert json.load(error.value)["error"] == "too many pending downloads"


def test_transfer_admission_and_disconnect_cleanup(tmp_path, monkeypatch):
    import threading
    from concurrent.futures import ThreadPoolExecutor

    source = tmp_path / "bytes"
    source.write_bytes(b"evidence")
    release = threading.Event()
    arrived = threading.Barrier(3)
    snapshot = TraceBundleReader.attachment_snapshot

    def hold(self, ref):
        arrived.wait(timeout=10)
        assert release.wait(timeout=10)
        return snapshot(self, ref)

    with TraceBundleWriter(tmp_path / "bundle") as writer:
        ref = writer.add_attachment(source)
        record(writer, attachments=[ref])
        with serve_bundle(writer.path) as url, ThreadPoolExecutor(max_workers=2) as pool:
            monkeypatch.setattr(TraceBundleReader, "attachment_snapshot", hold)
            futures = [pool.submit(get, url, "/attachment/" + ref["id"]) for _ in range(2)]
            try:
                arrived.wait(timeout=10)
                with pytest.raises(HTTPError) as error:
                    get(url, "/attachment/" + ref["id"])
                assert error.value.code == 429
                assert (
                    json.load(error.value)["error"] == "two attachment transfers are already active"
                )
                # Polling stays responsive while transfers prepare snapshots.
                with get(url, "/status") as response:
                    assert json.load(response)["total"] == 1
            finally:
                release.set()
                for future in futures:
                    future.result(timeout=10).close()
            monkeypatch.setattr(TraceBundleReader, "attachment_snapshot", snapshot)
            with get(url, "/attachment/" + ref["id"]) as response:
                assert response.read() == b"evidence"


@pytest.mark.parametrize(
    "options", [{"max_attachment": 4 * 1024**3 + 1}, {"quota": float("inf")}, {"quota": 0}]
)
def test_invalid_storage_limits_refuse_before_creation(tmp_path, options):
    with pytest.raises(ValueError, match="invalid attachment limit or bundle quota"):
        TraceBundleWriter(tmp_path / "bundle", **options)
    assert not (tmp_path / "bundle").exists()


def test_finalization_failure_never_claims_completion(tmp_path, monkeypatch):
    import os

    writer = TraceBundleWriter(tmp_path / "bundle")
    trace = record(writer)

    def fail(fd):
        raise OSError("synthetic fsync failure")

    with monkeypatch.context() as scoped:
        scoped.setattr(os, "fsync", fail)
        with pytest.raises(OSError, match="synthetic fsync failure"):
            writer.close()
    assert writer.closed and writer.journal.closed
    with TraceBundleReader(writer.path) as reader:
        assert reader.status()["state"] == "recording"
        assert reader.status()["total"] == 1
    with pytest.raises(RuntimeError, match="closed or failed"):
        trace.record("later", {})
