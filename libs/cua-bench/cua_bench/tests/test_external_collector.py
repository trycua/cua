"""External JSONL survives recording, persistence and the existing viewer."""

from __future__ import annotations

import base64
import json
import os
import subprocess
import sys
from argparse import Namespace
from html.parser import HTMLParser
from io import BytesIO
from pathlib import Path
from urllib.request import ProxyHandler, build_opener

import pytest
from datasets import load_from_disk
from PIL import Image

EXAMPLE = Path(__file__).resolve().parents[2] / "examples" / "external_collector.py"


@pytest.fixture(params=["inherited", "ascii"])
def collector_env(request):
    env = dict(os.environ)
    if request.param == "ascii":
        env.update(LC_ALL="C", PYTHONUTF8="0", PYTHONCOERCECLOCALE="0")
        env.pop("PYTHONIOENCODING", None)
    return env


def test_external_collector_emits_portable_jsonl(collector_env):
    result = subprocess.run(
        [sys.executable, str(EXAMPLE), "--emit"],
        check=True,
        capture_output=True,
        env=collector_env,
        timeout=30,
    )
    events = [json.loads(line) for line in result.stdout.decode("utf-8").splitlines()]
    assert len(events) == 3
    assert events[1]["data"]["payload"]["arguments"]["label"] == 'Demo <form> "✓"'


@pytest.fixture
def recorded_trace(tmp_path, collector_env):
    output = tmp_path / "collector trace"
    subprocess.run(
        [sys.executable, str(EXAMPLE), "--output", str(output)],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
        env=collector_env,
        timeout=60,
    )
    return output


def test_external_collector_round_trip(recorded_trace):
    rows = list(load_from_disk(str(recorded_trace)))
    assert [row["event_name"] for row in rows] == [
        "action",
        "external.network.response",
        "external.application.operation",
        "external.network.request",
    ]
    assert {row["trajectory_id"] for row in rows} == {"external-collector-example"}
    payloads = [json.loads(row["data_json"]) for row in rows]
    assert {payload["action_id"] for payload in payloads} == {"submit-1"}
    # Recorder insertion order is retained; collector timestamps do not sort rows.
    assert [payload["source_timestamp_ms"] for payload in payloads[1:]] == [30, 20, 10]
    assert [payload["source_sequence"] for payload in payloads[1:]] == [0, 1, 2]
    assert payloads[2]["payload"]["arguments"]["label"] == 'Demo <form> "✓"'
    assert payloads[1]["payload"]["request_id"] == payloads[3]["payload"]["request_id"]
    assert rows[0]["data_images"][0].size == (160, 90)
    assert all(row["data_images"] == [] for row in rows[1:])


class ViewerContent(HTMLParser):
    def __init__(self):
        super().__init__()
        self.json_values = []
        self.images = []

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag == "andypf-json-viewer":
            self.json_values.append(json.loads(attrs["data"]))
        if tag == "img" and attrs.get("class") == "trace-img":
            self.images.append(attrs["src"])


def test_external_collector_in_existing_viewer(recorded_trace, monkeypatch):
    from cua_bench.cli.commands import trace

    pages = []

    def read_page(url):
        # Bypass developer proxy settings for the viewer's loopback server.
        with build_opener(ProxyHandler({})).open(url, timeout=10) as response:
            pages.append(response.read().decode("utf-8"))
        return True

    # Fetch the real served HTML without launching a browser or waiting for input.
    monkeypatch.setattr(trace.webbrowser, "open", read_page)
    monkeypatch.setattr("builtins.input", lambda: "")
    assert trace.cmd_view(Namespace(identifier=str(recorded_trace))) == 0
    assert len(pages) == 1
    content = ViewerContent()
    content.feed(pages[0])
    rows = list(load_from_disk(str(recorded_trace)))
    assert content.json_values[::2] == [
        {key: row[key] for key in ("event_name", "timestamp", "trajectory_id")} for row in rows
    ]
    assert content.json_values[1::2] == [json.loads(row["data_json"]) for row in rows]
    assert len(content.images) == 1
    assert content.images[0].startswith("data:image/png;base64,")
    with Image.open(BytesIO(base64.b64decode(content.images[0].split(",", 1)[1]))) as image:
        assert image.size == (160, 90)
