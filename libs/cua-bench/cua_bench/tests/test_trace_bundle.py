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
