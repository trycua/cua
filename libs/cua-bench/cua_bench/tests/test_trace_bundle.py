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
